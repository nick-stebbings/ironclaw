//! Video encode + in-memory mux, wasm32-only (needs the linked ffmpeg).
//!
//! Slice 1 scope: a still screenshot → N repeated frames → VP9 (or AV1) in a
//! container, muxed to an in-memory `Vec<u8>` via a custom `AVIOContext` write
//! callback. **No audio track yet** — MP3 decode → Opus encode is the next slice.
//! A video-only WebM is still a valid, playable, uploadable artifact.
//!
//! `ffmpeg-wasi` is a per-module bindgen dump: each of `avcodec` / `avformat` /
//! `swscale` redefines the shared structs, so `avcodec::AVFrame` and
//! `avformat::AVFrame` are *distinct Rust types with identical C layout*. Types
//! and codec calls come from `avcodec`; `avformat`/`swscale` calls are qualified
//! and their struct-pointer arguments cast with `as _` (sound: repr(C), identical
//! layout). Every `av*_alloc` is paired with its free on every return path via
//! the drop guards below.

#![cfg(target_arch = "wasm32")]

use std::os::raw::{c_int, c_void};
use std::ptr;

use ffmpeg_wasi::avcodec::*;
use ffmpeg_wasi::{avformat, swscale};

use crate::audio;
use crate::render::{Compose, Composed, EncoderAvailability};

// errno sentinels ffmpeg returns; both are stable values, hardcoded to avoid the
// macro/const ambiguity across the per-module bindings.
// NOTE: EAGAIN is 6 on wasm32-wasi (not 11 as on Linux), so ffmpeg returns
// AVERROR(EAGAIN) = -6. Using -11 mis-reads the encoder's normal "need more
// input" as fatal and every encode drain dies with -6.
const AVERROR_EAGAIN: c_int = -6; // -EAGAIN (wasi errno)
const AVERROR_EOF_VAL: c_int = -541_478_725; // FFERRTAG('E','O','F',' ')

const PIX_YUV420P: AVPixelFormat = AVPixelFormat_AV_PIX_FMT_YUV420P;
const CODEC_ID_PNG: AVCodecID = AVCodecID_AV_CODEC_ID_PNG;
const CODEC_ID_MJPEG: AVCodecID = AVCodecID_AV_CODEC_ID_MJPEG;
const SWS_BILINEAR_FLAG: c_int = 2;

/// The Vec the muxer writes into, handed to the AVIO callback as opaque userdata.
struct WriteSink {
    buf: Vec<u8>,
}

unsafe extern "C" fn write_cb(opaque: *mut c_void, buf: *const u8, len: c_int) -> c_int {
    if opaque.is_null() || buf.is_null() || len <= 0 {
        return 0;
    }
    let sink = &mut *(opaque as *mut WriteSink);
    sink.buf
        .extend_from_slice(std::slice::from_raw_parts(buf, len as usize));
    len
}

// --- drop guards: free ffmpeg allocations on every return path ---------------

struct FrameGuard(*mut AVFrame);
impl Drop for FrameGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { av_frame_free(&mut self.0) }
        }
    }
}

struct PacketGuard(*mut AVPacket);
impl Drop for PacketGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { av_packet_free(&mut self.0) }
        }
    }
}

struct CodecCtxGuard(*mut AVCodecContext);
impl Drop for CodecCtxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { avcodec_free_context(&mut self.0) }
        }
    }
}

struct SwsGuard(*mut swscale::SwsContext);
impl Drop for SwsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { swscale::sws_freeContext(self.0) }
        }
    }
}

/// Frees the AVFormatContext (and, through it, the AVIOContext + its buffer),
/// then reclaims the boxed sink. Runs on every exit after the format context
/// exists, so no early `?` leaks it.
struct MuxGuard {
    fmt: *mut avformat::AVFormatContext,
    sink: *mut WriteSink,
}
impl Drop for MuxGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt.is_null() {
                avformat::avformat_free_context(self.fmt);
            }
            if !self.sink.is_null() {
                drop(Box::from_raw(self.sink));
            }
        }
    }
}

fn ffmpeg_err(context: &str, code: c_int) -> String {
    format!("{context} (ffmpeg error {code})")
}

/// Decode a PNG/JPEG screenshot to a single frame in its native pixel format.
unsafe fn decode_image(bytes: &[u8]) -> Result<(*mut AVFrame, FrameGuard), String> {
    for codec_id in [CODEC_ID_PNG, CODEC_ID_MJPEG] {
        let codec = avcodec_find_decoder(codec_id);
        if codec.is_null() {
            continue;
        }
        let ctx = avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return Err("avcodec_alloc_context3 (image decoder) returned null".into());
        }
        let ctx_guard = CodecCtxGuard(ctx);
        if avcodec_open2(ctx, codec, ptr::null_mut()) < 0 {
            continue;
        }

        let pkt = av_packet_alloc();
        if pkt.is_null() {
            return Err("av_packet_alloc returned null".into());
        }
        let _pkt_guard = PacketGuard(pkt);
        (*pkt).data = bytes.as_ptr() as *mut u8;
        (*pkt).size = bytes.len() as c_int;

        if avcodec_send_packet(ctx, pkt) < 0 {
            continue;
        }
        let frame = av_frame_alloc();
        if frame.is_null() {
            return Err("av_frame_alloc returned null".into());
        }
        let frame_guard = FrameGuard(frame);
        if avcodec_receive_frame(ctx, frame) == 0 {
            drop(ctx_guard); // frame owns its own buffers; ctx can go now
            return Ok((frame, frame_guard));
        }
    }
    Err("could not decode screenshot as PNG or JPEG".into())
}

pub fn encode(input: &Compose<'_>) -> Result<Composed, String> {
    unsafe { encode_inner(input) }
}

unsafe fn encode_inner(input: &Compose<'_>) -> Result<Composed, String> {
    let width = input.width as c_int;
    let height = input.height as c_int;
    let fps = input.fps.max(1) as c_int;
    // Audio-length sync lands with the audio slice; slice 1 takes explicit
    // duration or a sane default.
    let duration = input.duration_sec.unwrap_or(10.0).clamp(1.0, 120.0);
    let n_frames = (duration * fps as f64).round().max(1.0) as i64;

    let (src, _src_guard) = decode_image(input.screenshot)?;

    // Decode and plan audio before choosing the video/container pair. A 44.1
    // kHz MP3 cannot use Opus without resampling, so it must select AAC/MP4.
    let avail = EncoderAvailability::probe();
    let audio: Option<(audio::DecodedAudio, audio::AudioPlan)> = if input.audio_mp3.is_empty() {
        None
    } else {
        let decoded = audio::decode_mp3(input.audio_mp3)?;
        let plan = audio::plan_audio(decoded.sample_rate, avail.opus, avail.aac, avail.vorbis)?;
        Some((decoded, plan))
    };
    let triple = match audio.as_ref() {
        Some((_, plan)) => avail.choose_for_audio(plan.codec_name)?,
        None => avail.choose()?,
    };

    // --- video encoder ---
    let venc_name = std::ffi::CString::new(triple.video).unwrap();
    let venc = avcodec_find_encoder_by_name(venc_name.as_ptr());
    if venc.is_null() {
        return Err(format!(
            "video encoder {} not found in this build",
            triple.video
        ));
    }
    let vctx = avcodec_alloc_context3(venc);
    if vctx.is_null() {
        return Err("avcodec_alloc_context3 (video) returned null".into());
    }
    let _vctx_guard = CodecCtxGuard(vctx);
    (*vctx).width = width;
    (*vctx).height = height;
    (*vctx).pix_fmt = PIX_YUV420P;
    (*vctx).time_base = AVRational { num: 1, den: fps };
    (*vctx).framerate = AVRational { num: fps, den: 1 };
    (*vctx).gop_size = fps; // one keyframe per second
    (*vctx).bit_rate = (width as i64 * height as i64) / 2; // a still needs little
    (*vctx).thread_count = 1; // wasm has no threads
    if triple.container == "mp4" {
        // MP4 stores codec setup in the container header. Without this flag,
        // the muxer cannot initialize AV1 for fragmented, non-seekable output.
        (*vctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
    }

    if avcodec_open2(vctx, venc, ptr::null_mut()) < 0 {
        return Err(format!("could not open {} encoder", triple.video));
    }

    // --- audio: decode the MP3 and open a rate-matched encoder (no resampler) ---
    // A container needs all streams declared before the header, so this happens
    // up front. Empty audio is allowed -> a silent, video-only file.
    let audio: Option<(audio::DecodedAudio, audio::AudioEncoder)> = match audio {
        None => None,
        Some((decoded, plan)) => {
            let enc = audio::open_audio_encoder(&plan, &decoded, true)?;
            Some((decoded, enc))
        }
    };

    // --- in-memory muxer ---
    let sink = Box::into_raw(Box::new(WriteSink { buf: Vec::new() }));
    let avio_buf = av_malloc(4096) as *mut u8;
    let avio = avformat::avio_alloc_context(
        avio_buf,
        4096,
        1, // write flag
        sink as *mut c_void,
        None,
        Some(write_cb),
        None,
    );
    if avio.is_null() {
        drop(Box::from_raw(sink));
        return Err("avio_alloc_context returned null".into());
    }

    let container = std::ffi::CString::new(triple.container).unwrap();
    let mut fmt_ctx: *mut avformat::AVFormatContext = ptr::null_mut();
    if avformat::avformat_alloc_output_context2(
        &mut fmt_ctx,
        ptr::null(),
        container.as_ptr(),
        ptr::null(),
    ) < 0
        || fmt_ctx.is_null()
    {
        avformat::avio_context_free(&mut (avio as *mut _));
        drop(Box::from_raw(sink));
        return Err(format!(
            "could not allocate {} output context",
            triple.container
        ));
    }
    (*fmt_ctx).pb = avio;
    // From here the muxer owns avio; the guard frees fmt_ctx + sink on every path.
    let _mux_guard = MuxGuard { fmt: fmt_ctx, sink };

    let stream = avformat::avformat_new_stream(fmt_ctx, venc as *const _);
    if stream.is_null() {
        return Err("avformat_new_stream returned null".into());
    }
    (*stream).time_base = avformat::AVRational { num: 1, den: fps };
    let stream_tb = (*stream).time_base;
    if avcodec_parameters_from_context((*stream).codecpar as *mut _, vctx) < 0 {
        return Err("avcodec_parameters_from_context failed".into());
    }

    // audio stream (if present)
    let mut audio_stream: *mut avformat::AVStream = ptr::null_mut();
    let mut audio_stream_tb = avformat::AVRational { num: 0, den: 1 };
    if let Some((_, ref enc)) = audio {
        audio_stream = avformat::avformat_new_stream(fmt_ctx, ptr::null());
        if audio_stream.is_null() {
            return Err("avformat_new_stream (audio) returned null".into());
        }
        (*audio_stream).time_base = avformat::AVRational {
            num: 1,
            den: enc.sample_rate,
        };
        audio_stream_tb = (*audio_stream).time_base;
        if avcodec_parameters_from_context((*audio_stream).codecpar as *mut _, enc.ctx) < 0 {
            return Err("avcodec_parameters_from_context (audio) failed".into());
        }
    }

    let mut mux_options: *mut avformat::AVDictionary = ptr::null_mut();
    if triple.container == "mp4" {
        // Our custom AVIO callback is write-only. Fragmented MP4 avoids the
        // normal final seek back to the moov atom and is valid for streaming.
        let key = std::ffi::CString::new("movflags").unwrap();
        let value = std::ffi::CString::new("frag_keyframe+empty_moov+default_base_moof").unwrap();
        if avformat::av_dict_set(&mut mux_options, key.as_ptr(), value.as_ptr(), 0) < 0 {
            return Err("could not set fragmented MP4 options".into());
        }
    }
    let header_result = avformat::avformat_write_header(fmt_ctx, &mut mux_options);
    avformat::av_dict_free(&mut mux_options);
    if header_result < 0 {
        return Err(ffmpeg_err("avformat_write_header failed", header_result));
    }

    // --- scale the source frame to a reusable YUV420P frame ---
    let sws = swscale::sws_getContext(
        (*src).width,
        (*src).height,
        (*src).format,
        width,
        height,
        PIX_YUV420P as c_int,
        SWS_BILINEAR_FLAG,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null(),
    );
    if sws.is_null() {
        return Err("sws_getContext returned null".into());
    }
    let _sws_guard = SwsGuard(sws);

    let yuv = av_frame_alloc();
    if yuv.is_null() {
        return Err("av_frame_alloc (yuv) returned null".into());
    }
    let _yuv_guard = FrameGuard(yuv);
    (*yuv).format = PIX_YUV420P as c_int;
    (*yuv).width = width;
    (*yuv).height = height;
    if av_frame_get_buffer(yuv, 32) < 0 {
        return Err("av_frame_get_buffer failed".into());
    }

    swscale::sws_scale(
        sws,
        (*src).data.as_ptr() as *const *const u8,
        (*src).linesize.as_ptr(),
        0,
        (*src).height,
        (*yuv).data.as_ptr(),
        (*yuv).linesize.as_ptr(),
    );

    let pkt = av_packet_alloc();
    if pkt.is_null() {
        return Err("av_packet_alloc (mux) returned null".into());
    }
    let _pkt_guard = PacketGuard(pkt);

    // Drain encoded packets to the muxer. `?`-friendly.
    let drain = |ctx: *mut AVCodecContext| -> Result<(), String> {
        loop {
            let r = avcodec_receive_packet(ctx, pkt);
            if r == AVERROR_EAGAIN || r == AVERROR_EOF_VAL {
                return Ok(());
            }
            if r < 0 {
                return Err(ffmpeg_err("avcodec_receive_packet failed", r));
            }
            (*pkt).stream_index = (*stream).index;
            // rescale from the encoder's avcodec::AVRational to an avcodec one
            // built from the stream's avformat::AVRational fields (distinct types,
            // same C layout).
            let dst_tb = AVRational {
                num: stream_tb.num,
                den: stream_tb.den,
            };
            av_packet_rescale_ts(pkt, (*vctx).time_base, dst_tb);
            if avformat::av_interleaved_write_frame(fmt_ctx, pkt as *mut _) < 0 {
                av_packet_unref(pkt);
                return Err("av_interleaved_write_frame failed".into());
            }
            av_packet_unref(pkt);
        }
    };

    for i in 0..n_frames {
        (*yuv).pts = i;
        let r = avcodec_send_frame(vctx, yuv);
        if r < 0 {
            return Err(ffmpeg_err("avcodec_send_frame failed", r));
        }
        drain(vctx)?;
    }
    avcodec_send_frame(vctx, ptr::null_mut()); // flush
    drain(vctx)?;

    // --- audio encode: slice the decoded buffer into encoder frames, encode,
    // and hand packets to the muxer (av_interleaved_write_frame reorders by DTS,
    // so writing all audio after all video still produces a correctly interleaved
    // file). ---
    let mut audio_codec_name = "none";
    if let Some((decoded, enc)) = audio.as_ref() {
        audio_codec_name = enc.codec_name;
        let a_pkt = av_packet_alloc();
        if a_pkt.is_null() {
            return Err("av_packet_alloc (audio) returned null".into());
        }
        let _a_pkt_guard = PacketGuard(a_pkt);

        let drain_audio = |offset_pts: i64| -> Result<(), String> {
            loop {
                let r = avcodec_receive_packet(enc.ctx, a_pkt);
                if r == AVERROR_EAGAIN || r == AVERROR_EOF_VAL {
                    return Ok(());
                }
                if r < 0 {
                    return Err(ffmpeg_err("audio receive_packet failed", r));
                }
                (*a_pkt).stream_index = (*audio_stream).index;
                let dst_tb = AVRational {
                    num: audio_stream_tb.num,
                    den: audio_stream_tb.den,
                };
                av_packet_rescale_ts(a_pkt, (*enc.ctx).time_base, dst_tb);
                let _ = offset_pts;
                if avformat::av_interleaved_write_frame(fmt_ctx, a_pkt as *mut _) < 0 {
                    av_packet_unref(a_pkt);
                    return Err("audio interleaved_write_frame failed".into());
                }
                av_packet_unref(a_pkt);
            }
        };

        let mut offset = 0usize;
        let mut pts: i64 = 0;
        while let Some((frame, _fg)) = audio::make_frame(decoded, enc, offset)? {
            (*frame).pts = pts;
            let r = avcodec_send_frame(enc.ctx, frame);
            if r < 0 {
                return Err(ffmpeg_err("audio send_frame failed", r));
            }
            drain_audio(pts)?;
            pts += enc.frame_size as i64;
            offset += enc.frame_size as usize;
        }
        avcodec_send_frame(enc.ctx, ptr::null_mut()); // flush
        drain_audio(pts)?;
    }

    if avformat::av_write_trailer(fmt_ctx) < 0 {
        return Err("av_write_trailer failed".into());
    }

    let bytes = std::mem::take(&mut (*sink).buf);
    if bytes.is_empty() {
        return Err("encode produced an empty buffer".into());
    }

    Ok(Composed {
        bytes,
        container: triple.container,
        mime_type: triple.mime_type,
        video_codec: triple.video,
        audio_codec: audio_codec_name,
        duration_sec: duration,
    })
}
