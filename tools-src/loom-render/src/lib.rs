//! `loom-render` — Stage 2 of the Loom outreach pipeline.
//!
//! Composes a personalized video from an intro audio track (produced by stage 1,
//! `personalized-loom-outreach-poc`, and parked in Google Drive) and a screenshot
//! of the lead's website, then uploads the result back to Drive.
//!
//! Sandbox constraints that shape this design (see `wit/tool.wit`):
//!   * no process spawn, no threads, no browser  → the website capture is an
//!     external HTTP call to a screenshot service, not something we can do here;
//!   * no filesystem write, single JSON string out → ffmpeg works entirely on
//!     in-memory buffers and the video leaves via Drive, never through the
//!     response (a base64 video in the tool output would blow the model's
//!     context).
//!
//! Drive I/O goes through `tool-invoke` to the `google-drive` tool rather than
//! direct HTTP, so this tool never handles a Google OAuth token and its own
//! egress allowlist stays limited to the screenshot host.

wit_bindgen::generate!({
    world: "sandboxed-tool",
    path: "../../wit/tool.wit",
});

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use near::agent::host;
use serde::Deserialize;

mod audio;
mod encode;
mod render;

struct LoomRenderTool;

/// Screenshot service host. Must match the single entry in the tool's
/// `capabilities.json` http allowlist -- the host rejects anything else.
const SCREENSHOT_HOST: &str = "loom-capture.internal";

const DEFAULT_FPS: u32 = 25;
const MAX_DURATION_SEC: f64 = 120.0;

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "audio_drive_file_id":    { "type": "string", "minLength": 1, "description": "Drive file id of the stage-1 intro audio (MP3)." },
    "website_url":            { "type": "string", "minLength": 1, "description": "Lead's website; captured as the video's visual." },
    "drive_output_folder_id": { "type": "string", "minLength": 1, "description": "Drive folder the rendered video is uploaded to." },
    "aspect_ratio":  { "type": "string", "enum": ["16:9","9:16","1:1"], "default": "16:9" },
    "resolution":    { "type": "string", "enum": ["720p","1080p"], "default": "1080p" },
    "duration_sec":  { "type": "number", "minimum": 1, "maximum": 120, "description": "Defaults to the audio length." },
    "output_name":   { "type": "string", "description": "Drive file name. Defaults to loom-<timestamp>.webm." },
    "probe_encoders": { "type": "boolean", "description": "Diagnostic: report which ffmpeg encoders this build provides and which container/codec pair would be chosen, then stop. Renders nothing." }
  },
  "required": ["audio_drive_file_id","website_url","drive_output_folder_id"],
  "additionalProperties": false
}"#;

#[derive(Debug, Deserialize)]
struct Params {
    audio_drive_file_id: String,
    website_url: String,
    drive_output_folder_id: String,
    #[serde(default = "default_aspect")]
    aspect_ratio: String,
    #[serde(default = "default_resolution")]
    resolution: String,
    #[serde(default)]
    duration_sec: Option<f64>,
    #[serde(default)]
    output_name: Option<String>,
    /// Read from the raw JSON before typed parsing (see execute_inner); kept in
    /// the struct only so a real render call carrying `"probe_encoders": false`
    /// alongside the render fields still satisfies `additionalProperties:false`.
    #[serde(default)]
    #[allow(dead_code)]
    probe_encoders: bool,
}

fn default_aspect() -> String {
    "16:9".to_string()
}
fn default_resolution() -> String {
    "1080p".to_string()
}

impl Params {
    /// Resolve `resolution` + `aspect_ratio` to concrete even dimensions.
    ///
    /// Even is not cosmetic: YUV420P subsamples chroma 2x2, so an odd width or
    /// height is not representable and the encoder rejects it.
    fn dimensions(&self) -> Result<(u32, u32), String> {
        let short_edge: u32 = match self.resolution.as_str() {
            "720p" => 720,
            "1080p" => 1080,
            other => return Err(format!("unsupported resolution '{other}' (720p or 1080p)")),
        };
        let (w, h) = match self.aspect_ratio.as_str() {
            "16:9" => (short_edge * 16 / 9, short_edge),
            "9:16" => (short_edge, short_edge * 16 / 9),
            "1:1" => (short_edge, short_edge),
            other => return Err(format!("unsupported aspect_ratio '{other}' (16:9, 9:16 or 1:1)")),
        };
        Ok((w & !1, h & !1))
    }

    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("audio_drive_file_id", &self.audio_drive_file_id),
            ("website_url", &self.website_url),
            ("drive_output_folder_id", &self.drive_output_folder_id),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{name} must not be empty"));
            }
        }
        if !(self.website_url.starts_with("http://") || self.website_url.starts_with("https://")) {
            return Err("website_url must start with http:// or https://".to_string());
        }
        if let Some(d) = self.duration_sec {
            if !(d.is_finite() && (1.0..=MAX_DURATION_SEC).contains(&d)) {
                return Err(format!(
                    "duration_sec must be between 1 and {MAX_DURATION_SEC} seconds"
                ));
            }
        }
        self.dimensions()?;
        Ok(())
    }
}

/// Call another tool through the host and parse its JSON reply.
fn invoke_tool(alias: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let raw = host::tool_invoke(alias, &params.to_string())
        .map_err(|e| format!("{alias} invocation failed: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("{alias} returned unparseable JSON: {e}"))
}

/// Download the stage-1 audio as raw bytes.
///
/// `binary: true` is what makes this work at all: google-drive's `download_file`
/// used to hard-error on any non-UTF-8 body, which is every MP3 ever made.
/// Added in google-drive-tool v0.3.0.
fn fetch_audio(file_id: &str) -> Result<Vec<u8>, String> {
    let reply = invoke_tool(
        "google-drive",
        &serde_json::json!({
            "action": "download_file",
            "file_id": file_id,
            "binary": true
        }),
    )?;

    let b64 = reply
        .get("content_base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "google-drive returned no content_base64 for {file_id} (encoding={:?}); \
                 the tool must be v0.3.0 or later",
                reply.get("encoding").and_then(|v| v.as_str())
            )
        })?;

    let bytes = B64
        .decode(b64.trim())
        .map_err(|e| format!("audio content_base64 was not valid base64: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("audio file {file_id} is empty"));
    }
    Ok(bytes)
}

/// Capture the lead's website. Out of sandbox by necessity — there is no browser
/// in here, and no way to spawn one.
fn capture_website(url: &str, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({ "url": url, "width": width, "height": height });
    let response = host::http_request(
        "POST",
        &format!("https://{SCREENSHOT_HOST}/screenshot"),
        r#"{"content-type":"application/json"}"#,
        Some(body.to_string().as_bytes()),
        Some(60_000),
    )
    .map_err(|e| format!("screenshot request failed: {e}"))?;

    if response.status < 200 || response.status >= 300 {
        return Err(format!(
            "screenshot service returned status {}",
            response.status
        ));
    }
    if response.body.is_empty() {
        return Err("screenshot service returned an empty image".to_string());
    }
    Ok(response.body)
}

/// Upload the rendered video and return its Drive file id.
fn upload_video(
    folder_id: &str,
    name: &str,
    bytes: &[u8],
    mime: &str,
) -> Result<String, String> {
    let reply = invoke_tool(
        "google-drive",
        &serde_json::json!({
            "action": "upload_file",
            "name": name,
            "content_base64": B64.encode(bytes),
            "mime_type": mime,
            "parent_id": folder_id
        }),
    )?;

    reply
        .get("file")
        .and_then(|f| f.get("id"))
        .or_else(|| reply.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "google-drive upload returned no file id".to_string())
}

fn execute_inner(params_json: &str) -> Result<String, String> {
    // Probe short-circuits BEFORE typed parsing: the diagnostic must need no
    // Drive/URL/folder fields, so gate it on the raw JSON. (It used to run after
    // Params deserialization, which required all three dummy fields -- defeating
    // "the cheapest first check".)
    if serde_json::from_str::<serde_json::Value>(params_json)
        .ok()
        .and_then(|v| v.get("probe_encoders").and_then(|b| b.as_bool()))
        .unwrap_or(false)
    {
        return Ok(render::probe_report());
    }

    let params: Params = serde_json::from_str(params_json)
        .map_err(|e| format!("invalid params: {e}"))?;
    params.validate()?;
    let (width, height) = params.dimensions()?;

    host::log(
        host::LogLevel::Info,
        &format!(
            "loom-render: {}x{} from {}",
            width, height, params.website_url
        ),
    );

    let audio = fetch_audio(&params.audio_drive_file_id)?;
    let screenshot = capture_website(&params.website_url, width, height)?;

    let composed = render::compose(render::Compose {
        screenshot: &screenshot,
        audio_mp3: &audio,
        width,
        height,
        fps: DEFAULT_FPS,
        duration_sec: params.duration_sec,
    })?;

    let name = params
        .output_name
        .clone()
        .unwrap_or_else(|| format!("loom-{}.webm", host::now_millis()));

    let file_id = upload_video(
        &params.drive_output_folder_id,
        &name,
        &composed.bytes,
        composed.mime_type,
    )?;

    // Deliberately does NOT include the video bytes: the response is a single
    // JSON string that lands in the model's context.
    Ok(serde_json::json!({
        "success": true,
        "video_drive_file_id": file_id,
        "container": composed.container,
        "codec": composed.video_codec,
        "audio_codec": composed.audio_codec,
        "duration_sec": composed.duration_sec,
        "width": width,
        "height": height,
        "bytes": composed.bytes.len(),
        "name": name
    })
    .to_string())
}

impl exports::near::agent::tool::Guest for LoomRenderTool {
    fn execute(req: exports::near::agent::tool::Request) -> exports::near::agent::tool::Response {
        match execute_inner(&req.params) {
            Ok(output) => exports::near::agent::tool::Response {
                output: Some(output),
                error: None,
            },
            Err(error) => exports::near::agent::tool::Response {
                output: None,
                error: Some(error),
            },
        }
    }

    fn schema() -> String {
        SCHEMA.to_string()
    }

    fn description() -> String {
        "Compose a personalized Loom-style video from a Drive-hosted intro audio track \
         and a screenshot of the lead's website, then upload the result to Drive. \
         Stage 2 of the Loom outreach pipeline: consumes the audio produced by \
         `personalized-loom-outreach-poc` and returns `video_drive_file_id`. \
         Requires `audio_drive_file_id`, `website_url`, and `drive_output_folder_id`."
            .to_string()
    }
}

export!(LoomRenderTool);

#[cfg(test)]
mod tests {
    use super::*;

    fn params(extra: &str) -> Result<Params, String> {
        let base = format!(
            r#"{{"audio_drive_file_id":"a","website_url":"https://x.com","drive_output_folder_id":"f"{extra}}}"#
        );
        serde_json::from_str::<Params>(&base).map_err(|e| e.to_string())
    }

    #[test]
    fn defaults_are_1080p_16_9() {
        let p = params("").unwrap();
        assert_eq!(p.resolution, "1080p");
        assert_eq!(p.aspect_ratio, "16:9");
        assert_eq!(p.dimensions().unwrap(), (1920, 1080));
    }

    #[test]
    fn dimensions_are_always_even() {
        // YUV420P cannot represent an odd dimension; 9:16 at 720p is the case
        // that actually trips it (720*16/9 = 1280 is fine, but the guard must
        // hold for every combination we advertise).
        for res in ["720p", "1080p"] {
            for ar in ["16:9", "9:16", "1:1"] {
                let p = params(&format!(r#","resolution":"{res}","aspect_ratio":"{ar}""#)).unwrap();
                let (w, h) = p.dimensions().unwrap();
                assert_eq!(w % 2, 0, "{res} {ar} width {w} must be even");
                assert_eq!(h % 2, 0, "{res} {ar} height {h} must be even");
            }
        }
    }

    #[test]
    fn rejects_out_of_range_duration() {
        assert!(params(r#","duration_sec":0.5"#).unwrap().validate().is_err());
        assert!(params(r#","duration_sec":999"#).unwrap().validate().is_err());
        assert!(params(r#","duration_sec":42"#).unwrap().validate().is_ok());
    }

    #[test]
    fn rejects_non_http_url() {
        let p: Params = serde_json::from_str(
            r#"{"audio_drive_file_id":"a","website_url":"file:///etc/passwd","drive_output_folder_id":"f"}"#,
        )
        .unwrap();
        assert!(p.validate().is_err(), "must reject non-http schemes");
    }

    #[test]
    fn probe_needs_no_required_fields() {
        // The whole point of the diagnostic: {"probe_encoders":true} alone must
        // reach the probe, without audio_drive_file_id / website_url / folder.
        let out = execute_inner(r#"{"probe_encoders":true}"#);
        assert!(out.is_ok(), "probe must not require the render fields: {out:?}");
        let v: serde_json::Value = serde_json::from_str(&out.unwrap()).unwrap();
        assert_eq!(v["probe"], true);
    }

    #[test]
    fn schema_is_valid_json_and_matches_the_struct() {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("schema must be JSON");
        let props = schema["properties"].as_object().unwrap();
        for required in ["audio_drive_file_id", "website_url", "drive_output_folder_id"] {
            assert!(props.contains_key(required), "schema missing {required}");
        }
        // additionalProperties:false means a field the struct accepts but the
        // schema omits can never be sent by the model.
        for field in ["aspect_ratio", "resolution", "duration_sec", "output_name"] {
            assert!(props.contains_key(field), "schema missing {field}");
        }
    }
}
