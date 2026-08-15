//! In-memory video composition via ffmpeg-wasi.
//!
//! Everything here is unsafe FFI against the prebuilt ffmpeg 7 static archives
//! that `ffmpeg-wasi` ships. There is no filesystem in the sandbox, so the muxer
//! writes through a custom `AVIOContext` callback into a `Vec<u8>` rather than to
//! a path.
//!
//! Codec choice is forced by what the crate ships (verified by symbol inspection
//! of `libavcodec.a`): VP9 and AV1 video, native Opus/AAC audio, and **no H.264**
//! (libx264 is GPL and excluded upstream). Default is therefore WebM/VP9+Opus.

#[cfg(target_arch = "wasm32")]
use std::ffi::CString;
#[cfg(target_arch = "wasm32")]
use std::os::raw::c_char;

/// Which encoders the linked ffmpeg build actually provides.
///
/// This is the spec's "encoder-availability gate": the container/codec decision
/// depends on it, and guessing wrong means discovering it at mux time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderAvailability {
    pub vp9: bool,
    pub av1: bool,
    pub mpeg4: bool,
    pub opus: bool,
    pub aac: bool,
    pub vorbis: bool,
}

impl EncoderAvailability {
    /// Probe the linked build. Safe wrapper over `avcodec_find_encoder_by_name`.
    pub fn probe() -> Self {
        Self {
            vp9: encoder_exists("libvpx-vp9"),
            av1: encoder_exists("libaom-av1"),
            mpeg4: encoder_exists("mpeg4"),
            // ffmpeg's *native* Opus encoder, not libopus (no libopus.a ships).
            // Historically flagged experimental, so the encoder context needs
            // strict_std_compliance = FF_COMPLIANCE_EXPERIMENTAL.
            opus: encoder_exists("opus"),
            aac: encoder_exists("aac"),
            // Vorbis takes any sample rate + FLTP -> the resample-free WebM audio
            // path when the MP3 is not one of Opus's rates.
            vorbis: encoder_exists("vorbis"),
        }
    }

    /// Pick the (video, audio, container) triple, preferring WebM/VP9+Opus.
    pub fn choose(&self) -> Result<Triple, String> {
        if self.vp9 && self.opus {
            Ok(Triple {
                video: "libvpx-vp9",
                audio: "opus",
                container: "webm",
                mime_type: "video/webm",
            })
        } else if self.mpeg4 && self.aac {
            Ok(Triple {
                video: "mpeg4",
                audio: "aac",
                container: "mp4",
                mime_type: "video/mp4",
            })
        } else if self.av1 && self.aac {
            Ok(Triple {
                video: "libaom-av1",
                audio: "aac",
                container: "mp4",
                mime_type: "video/mp4",
            })
        } else {
            Err(format!(
                "no usable (video,audio) encoder pair in this ffmpeg build: {self:?}"
            ))
        }
    }

    pub fn choose_for_audio(&self, audio_codec: &'static str) -> Result<Triple, String> {
        match audio_codec {
            "opus" | "vorbis" if self.vp9 => Ok(Triple {
                video: "libvpx-vp9",
                audio: audio_codec,
                container: "webm",
                mime_type: "video/webm",
            }),
            "aac" if self.mpeg4 => Ok(Triple {
                video: "mpeg4",
                audio: "aac",
                container: "mp4",
                mime_type: "video/mp4",
            }),
            "aac" if self.av1 => Ok(Triple {
                video: "libaom-av1",
                audio: "aac",
                container: "mp4",
                mime_type: "video/mp4",
            }),
            _ => Err(format!(
                "no compatible video/container pair for audio codec {audio_codec}"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Triple {
    pub video: &'static str,
    pub audio: &'static str,
    pub container: &'static str,
    pub mime_type: &'static str,
}

#[cfg(target_arch = "wasm32")]
fn encoder_exists(name: &str) -> bool {
    let Ok(c_name) = CString::new(name) else {
        return false;
    };
    // SAFETY: ffmpeg's encoder registry is static and read-only after init;
    // the pointer is borrowed, never freed by us.
    unsafe {
        !ffmpeg_wasi::avcodec::avcodec_find_encoder_by_name(c_name.as_ptr() as *const c_char)
            .is_null()
    }
}

/// Host builds have no ffmpeg (the crate ships wasm-only archives). Reporting
/// "absent" rather than faking availability keeps the host honest: any code path
/// that needs a real encoder fails loudly instead of pretending to work.
#[cfg(not(target_arch = "wasm32"))]
fn encoder_exists(_name: &str) -> bool {
    false
}

pub struct Compose<'a> {
    pub screenshot: &'a [u8],
    /// Held for the audio slice (MP3 decode -> Opus encode); unused in slice 1.
    #[allow(dead_code)]
    pub audio_mp3: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_sec: Option<f64>,
}

pub struct Composed {
    pub bytes: Vec<u8>,
    pub container: &'static str,
    pub mime_type: &'static str,
    pub video_codec: &'static str,
    pub audio_codec: &'static str,
    pub duration_sec: f64,
}

/// Report what this ffmpeg build can encode, as JSON. Used by the
/// `probe_encoders` diagnostic and by the runtime smoke test -- symbol
/// inspection of the shipped `.a` files says what *should* link; this says what
/// actually resolved in the sandbox.
pub fn probe_report() -> String {
    let a = EncoderAvailability::probe();
    let chosen = a.choose();
    serde_json::json!({
        "success": true,
        "probe": true,
        "encoders": {
            "libvpx-vp9": a.vp9,
            "libaom-av1": a.av1,
            "mpeg4": a.mpeg4,
            "opus": a.opus,
            "aac": a.aac,
            "vorbis": a.vorbis,
        },
        "chosen": match &chosen {
            Ok(t) => serde_json::json!({
                "video": t.video, "audio": t.audio,
                "container": t.container, "mime_type": t.mime_type
            }),
            Err(e) => serde_json::json!({ "error": e }),
        }
    })
    .to_string()
}

pub fn compose(input: Compose<'_>) -> Result<Composed, String> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::encode::encode(&input)
    }
    // Host builds have no ffmpeg; compose is a no-op the runtime never hits.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = &input;
        Err("video encode requires the wasm target".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_webm_vp9_opus() {
        let a = EncoderAvailability {
            vp9: true,
            av1: true,
            mpeg4: true,
            opus: true,
            aac: true,
            vorbis: true,
        };
        let t = a.choose().unwrap();
        assert_eq!(t.container, "webm");
        assert_eq!(t.video, "libvpx-vp9");
        assert_eq!(t.audio, "opus");
    }

    #[test]
    fn falls_back_to_mp4_mpeg4_aac_without_opus() {
        let a = EncoderAvailability {
            vp9: true,
            av1: true,
            mpeg4: true,
            opus: false,
            aac: true,
            vorbis: true,
        };
        let t = a.choose().unwrap();
        assert_eq!(t.container, "mp4");
        assert_eq!(t.video, "mpeg4");
        assert_eq!(t.audio, "aac");
    }

    #[test]
    fn routes_aac_audio_to_mp4() {
        let a = EncoderAvailability {
            vp9: true,
            av1: true,
            mpeg4: true,
            opus: true,
            aac: true,
            vorbis: true,
        };
        let t = a.choose_for_audio("aac").unwrap();
        assert_eq!(t.container, "mp4");
        assert_eq!(t.video, "mpeg4");
        assert_eq!(t.audio, "aac");
    }

    #[test]
    fn errors_when_no_pair_is_usable() {
        let a = EncoderAvailability {
            vp9: true,
            av1: false,
            mpeg4: false,
            opus: false,
            aac: false,
            vorbis: false,
        };
        assert!(
            a.choose().is_err(),
            "video without audio is not a usable pair"
        );
    }
}
