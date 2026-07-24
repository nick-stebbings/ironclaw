//! Audio decode + encode for the mux, wasm32-only.
//!
//! Constraints the linked ffmpeg forces on the design:
//!   * **No swresample** (`ffmpeg-wasi` ships no libswresample, not even the
//!     symbols). We cannot change the sample rate, so the encoder is matched to
//!     the source rate: Opus when the MP3 is already 48k-family (Opus's only
//!     rates), Vorbis otherwise. Both are valid WebM audio.
//!   * **No AVAudioFifo bindings.** Intro audio is a few seconds, so we decode
//!     the whole MP3 into per-channel float buffers up front and re-slice into
//!     encoder-sized frames — no streaming FIFO needed.
//!   * The MP3 decoder emits planar float (FLTP), which is exactly what Vorbis
//!     and native Opus want, so there is no sample-format conversion either.
//!
//! We do not hand-interleave: `av_interleaved_write_frame` reorders by DTS, so
//! the caller writes all video packets then all audio packets and the muxer
//! sorts them.

#![cfg(target_arch = "wasm32")]

use std::os::raw::c_int;
use std::ptr;

use ffmpeg_wasi::avcodec::*;

const AVERROR_EAGAIN: c_int = -11;
const AVERROR_EOF_VAL: c_int = -541_478_725;
const FLTP: AVSampleFormat = AVSampleFormat_AV_SAMPLE_FMT_FLTP;
const MP3_ID: AVCodecID = AVCodecID_AV_CODEC_ID_MP3;

/// Opus's supported sample rates. Anything else must use Vorbis (no resampler).
fn opus_ok(rate: c_int) -> bool {
    matches!(rate, 8000 | 12000 | 16000 | 24000 | 48000)
}

/// Fully decoded audio: interleaved-by-channel planar float, plus the shape.
pub struct DecodedAudio {
    /// One Vec<f32> per channel (planar). All the same length.
    pub planes: Vec<Vec<f32>>,
    pub sample_rate: c_int,
    pub channels: c_int,
}

impl DecodedAudio {
    pub fn total_samples(&self) -> usize {
        self.planes.first().map(|p| p.len()).unwrap_or(0)
    }
}

// Guards mirror encode.rs; kept module-local so audio.rs stands alone.
struct CtxGuard(*mut AVCodecContext);
impl Drop for CtxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { avcodec_free_context(&mut self.0) }
        }
    }
}
struct FrameGuard(*mut AVFrame);
impl Drop for FrameGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { av_frame_free(&mut self.0) }
        }
    }
}
struct PktGuard(*mut AVPacket);
impl Drop for PktGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { av_packet_free(&mut self.0) }
        }
    }
}

/// Decode an entire MP3 to planar float. Short intros only — bounded by
/// `MAX_SAMPLES` so a hostile input can't exhaust linear memory.
pub unsafe fn decode_mp3(bytes: &[u8]) -> Result<DecodedAudio, String> {
    // ~15 min stereo at 48k is the hard ceiling; a Loom intro is seconds.
    const MAX_SAMPLES_PER_CH: usize = 48_000 * 60 * 15;

    let codec = avcodec_find_decoder(MP3_ID);
    if codec.is_null() {
        return Err("mp3 decoder not present in this build".into());
    }
    let ctx = avcodec_alloc_context3(codec);
    if ctx.is_null() {
        return Err("avcodec_alloc_context3 (mp3) returned null".into());
    }
    let _ctx = CtxGuard(ctx);
    if avcodec_open2(ctx, codec, ptr::null_mut()) < 0 {
        return Err("could not open mp3 decoder".into());
    }

    let pkt = av_packet_alloc();
    if pkt.is_null() {
        return Err("av_packet_alloc (mp3) returned null".into());
    }
    let _pkt = PktGuard(pkt);
    (*pkt).data = bytes.as_ptr() as *mut u8;
    (*pkt).size = bytes.len() as c_int;

    let frame = av_frame_alloc();
    if frame.is_null() {
        return Err("av_frame_alloc (mp3) returned null".into());
    }
    let _frame = FrameGuard(frame);

    if avcodec_send_packet(ctx, pkt) < 0 {
        return Err("mp3 decode: send_packet failed".into());
    }
    avcodec_send_packet(ctx, ptr::null_mut()); // flush

    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut sample_rate = 0;
    let mut channels = 0;

    loop {
        let r = avcodec_receive_frame(ctx, frame);
        if r == AVERROR_EAGAIN || r == AVERROR_EOF_VAL {
            break;
        }
        if r < 0 {
            return Err(format!("mp3 decode: receive_frame failed ({r})"));
        }
        if (*frame).format != FLTP {
            return Err(format!(
                "mp3 decoder produced sample format {} (expected FLTP {FLTP}); \
                 no resampler is available to convert it",
                (*frame).format
            ));
        }
        let n = (*frame).nb_samples as usize;
        let ch = (*frame).ch_layout.nb_channels as usize;
        if planes.is_empty() {
            planes = vec![Vec::new(); ch];
            sample_rate = (*frame).sample_rate;
            channels = ch as c_int;
        }
        for (c, plane) in planes.iter_mut().enumerate().take(ch) {
            if plane.len() + n > MAX_SAMPLES_PER_CH {
                return Err("audio track exceeds the 15-minute decode ceiling".into());
            }
            let src = (*frame).data[c] as *const f32;
            plane.extend_from_slice(std::slice::from_raw_parts(src, n));
        }
    }

    if planes.is_empty() || sample_rate == 0 {
        return Err("mp3 produced no audio samples".into());
    }
    Ok(DecodedAudio {
        planes,
        sample_rate,
        channels,
    })
}

/// The chosen audio encoder, ready to open, plus what it is for the output JSON.
pub struct AudioPlan {
    pub codec_name: &'static str,
}

/// Pick an audio codec that needs no resampling for this source rate.
pub fn plan_audio(sample_rate: c_int, opus_available: bool, vorbis_available: bool) -> Result<AudioPlan, String> {
    if opus_available && opus_ok(sample_rate) {
        Ok(AudioPlan { codec_name: "opus" })
    } else if vorbis_available {
        // Vorbis takes any rate + FLTP -> the resample-free WebM fallback.
        Ok(AudioPlan { codec_name: "vorbis" })
    } else if opus_available {
        Err(format!(
            "source is {sample_rate} Hz, which Opus cannot encode, and no Vorbis \
             encoder is available to take its place (no resampler in this build)"
        ))
    } else {
        Err("no WebM-compatible audio encoder (opus/vorbis) in this build".into())
    }
}

/// An opened audio encoder context + its stream-facing metadata. The caller
/// owns muxing; this just produces encoded packets.
pub struct AudioEncoder {
    pub ctx: *mut AVCodecContext,
    pub codec_name: &'static str,
    pub frame_size: c_int,
    pub sample_rate: c_int,
    _guard: CtxGuard,
}

pub unsafe fn open_audio_encoder(
    plan: &AudioPlan,
    decoded: &DecodedAudio,
    global_header: bool,
) -> Result<AudioEncoder, String> {
    let name = std::ffi::CString::new(plan.codec_name).unwrap();
    let codec = avcodec_find_encoder_by_name(name.as_ptr());
    if codec.is_null() {
        return Err(format!("audio encoder {} not found", plan.codec_name));
    }
    let ctx = avcodec_alloc_context3(codec);
    if ctx.is_null() {
        return Err("avcodec_alloc_context3 (audio) returned null".into());
    }
    let guard = CtxGuard(ctx);

    (*ctx).sample_fmt = FLTP;
    (*ctx).sample_rate = decoded.sample_rate;
    (*ctx).bit_rate = 96_000;
    (*ctx).time_base = AVRational {
        num: 1,
        den: decoded.sample_rate,
    };
    av_channel_layout_default(&mut (*ctx).ch_layout, decoded.channels);
    if global_header {
        // WebM/MP4 want extradata in the header, not inline.
        (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
    }
    // Native Opus is flagged experimental in ffmpeg; without this it refuses.
    if plan.codec_name == "opus" {
        (*ctx).strict_std_compliance = FF_COMPLIANCE_EXPERIMENTAL;
    }

    if avcodec_open2(ctx, codec, ptr::null_mut()) < 0 {
        return Err(format!("could not open {} encoder", plan.codec_name));
    }
    // Some encoders only publish frame_size after open; Vorbis reports 0 to mean
    // "variable", in which case we pick a sane block.
    let frame_size = if (*ctx).frame_size > 0 {
        (*ctx).frame_size
    } else {
        1024
    };

    Ok(AudioEncoder {
        ctx,
        codec_name: plan.codec_name,
        frame_size,
        sample_rate: decoded.sample_rate,
        _guard: guard,
    })
}

/// Build one FLTP frame of `frame_size` samples starting at `offset`, zero-padded
/// if the tail is short. Returns None past the end.
pub unsafe fn make_frame(
    decoded: &DecodedAudio,
    enc: &AudioEncoder,
    offset: usize,
) -> Result<Option<(*mut AVFrame, FrameGuardPub)>, String> {
    let total = decoded.total_samples();
    if offset >= total {
        return Ok(None);
    }
    let take = (total - offset).min(enc.frame_size as usize);

    let frame = av_frame_alloc();
    if frame.is_null() {
        return Err("av_frame_alloc (audio) returned null".into());
    }
    let guard = FrameGuardPub(frame);
    (*frame).format = FLTP as c_int;
    (*frame).nb_samples = enc.frame_size;
    (*frame).sample_rate = enc.sample_rate;
    av_channel_layout_default(&mut (*frame).ch_layout, decoded.channels);
    if av_frame_get_buffer(frame, 0) < 0 {
        return Err("av_frame_get_buffer (audio) failed".into());
    }
    for (c, plane) in decoded.planes.iter().enumerate() {
        let dst = (*frame).data[c] as *mut f32;
        // copy `take` samples, then zero-pad to frame_size
        ptr::copy_nonoverlapping(plane[offset..offset + take].as_ptr(), dst, take);
        if take < enc.frame_size as usize {
            ptr::write_bytes(dst.add(take), 0, enc.frame_size as usize - take);
        }
    }
    Ok(Some((frame, guard)))
}

/// Public alias so the caller (encode.rs) can hold the frame guard.
pub struct FrameGuardPub(pub *mut AVFrame);
impl Drop for FrameGuardPub {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { av_frame_free(&mut self.0) }
        }
    }
}
