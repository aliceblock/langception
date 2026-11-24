//! The pipe protocol (ticket 02): pure request/reply logic, byte-compatible
//! with the bundled engine's JSON-lines shape so the app's existing sidecar
//! machinery parses our replies unchanged.
//!
//! Request (one JSON line per task, exactly what the app's
//! `langception_core::sidecar::encode_task` sends): `{"image_base64": "<base64 PNG>"}`.
//! Reply (one JSON line per task): code 100 carries recognized text blocks
//! (`box` quad + optional `score` + `text`, the recorded engine's field
//! order), code 101 is the normal empty result, any other code is an error
//! whose `data` is the human-readable message.
//!
//! Everything here is unit-tested offline — no process, no models.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// The `data` message of the normal empty reply. The client treats any
/// code-101 message starting with "No text found" as an empty result, not
/// a failure — the recorded engine's wording, kept verbatim.
pub const NO_TEXT_MESSAGE: &str = "No text found in image.";

/// Error code for a task the sidecar could not even run (malformed JSON,
/// bad base64, undecodable image) — the recorded engine uses the same
/// 3xx range for input errors.
pub const TASK_ERROR: i32 = 300;
/// Error code for a failure inside the OCR pipeline itself. The client
/// treats every non-100/101 code identically (an error pass whose `data`
/// is the message); the split only makes the sidecar's stderr/log side
/// readable.
pub const PIPELINE_ERROR: i32 = 500;

// ---- Requests ----

#[derive(Debug, Deserialize)]
struct TaskLine {
    image_base64: String,
}

/// Parses one task line into the image bytes it carries. Extra JSON
/// fields are ignored (the client sends exactly one today; being tolerant
/// costs nothing), a missing `image_base64` or undecodable base64 is an
/// error naming the problem.
pub fn parse_task(line: &str) -> Result<Vec<u8>, String> {
    let task: TaskLine = serde_json::from_str(line)
        .map_err(|e| format!("malformed task JSON ({e}); expected {{\"image_base64\": \"...\"}}"))?;
    base64::engine::general_purpose::STANDARD
        .decode(task.image_base64.as_bytes())
        .map_err(|e| format!("failed to decode base64 image: {e}"))
}

// ---- Replies ----

/// One recognized text region in the reply's block shape: a polygon
/// (capture-space pixels, any winding — the client takes the axis-aligned
/// hull) and the recognized text. `score` rides along for debugging
/// exactly as the recorded engine's reply does; the client ignores it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplyRegion {
    #[serde(rename = "box")]
    pub box_points: Vec<[i32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    pub text: String,
}

/// The success reply: code 100 with the recognized blocks.
pub fn success_reply(regions: &[ReplyRegion]) -> String {
    serde_json::to_string(&SuccessReply { code: 100, data: regions })
        .expect("reply serialization cannot fail")
}

/// The normal empty reply: code 101 (blank region, not a failure).
pub fn no_text_reply() -> String {
    serde_json::to_string(&PlainReply { code: 101, data: NO_TEXT_MESSAGE })
        .expect("reply serialization cannot fail")
}

/// An error reply: any code the client does not special-case, its `data`
/// the human-readable message. The process stays alive — the client
/// surfaces the message for the pass and keeps using the sidecar.
pub fn error_reply(code: i32, message: &str) -> String {
    serde_json::to_string(&PlainReply { code, data: message })
        .expect("reply serialization cannot fail")
}

/// Field order matches the recorded engine's reply byte-for-byte
/// (`code` first, then `data`).
#[derive(Serialize)]
struct SuccessReply<'a> {
    code: i32,
    data: &'a [ReplyRegion],
}

#[derive(Serialize)]
struct PlainReply<'a> {
    code: i32,
    data: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's view of a reply, mirrored from the core crate's
    /// `sidecar::parse_reply`: it
    /// reads `code`, treats `data` by code, and reads each block's `box`
    /// and `text` (ignoring everything else, such as `score`).
    #[derive(Deserialize)]
    struct ClientReply {
        code: i32,
        #[serde(default)]
        data: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct ClientBlock {
        #[serde(rename = "box")]
        box_points: Vec<[i32; 2]>,
        #[serde(default)]
        text: String,
    }

    #[test]
    fn success_reply_carries_blocks_in_the_recorded_shape() {
        let reply = success_reply(&[
            ReplyRegion {
                box_points: vec![[13, 5], [161, 5], [161, 27], [13, 27]],
                score: Some(0.954_276),
                text: "飞舞的因果交流".into(),
            },
            ReplyRegion {
                box_points: vec![[210, 5], [348, 5], [348, 27], [210, 27]],
                score: Some(0.881),
                text: "第二行文本".into(),
            },
        ]);
        // Field order parity with the recorded engine replies.
        assert!(
            reply.starts_with("{\"code\":100,\"data\":["),
            "{reply}"
        );
        let parsed: ClientReply = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed.code, 100);
        let blocks: Vec<ClientBlock> =
            serde_json::from_value(parsed.data).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].box_points, vec![[13, 5], [161, 5], [161, 27], [13, 27]]);
        assert_eq!(blocks[0].text, "飞舞的因果交流");
        assert_eq!(blocks[1].box_points, vec![[210, 5], [348, 5], [348, 27], [210, 27]]);
        assert_eq!(blocks[1].text, "第二行文本");
    }

    #[test]
    fn no_text_reply_is_code_101_and_reads_as_empty_on_the_client() {
        let reply = no_text_reply();
        assert_eq!(
            reply,
            "{\"code\":101,\"data\":\"No text found in image.\"}"
        );
        let parsed: ClientReply = serde_json::from_str(&reply).unwrap();
        // The client's empty-result rule: code 101 + data starting with
        // "No text found" (sidecar.rs's parse_reply).
        assert_eq!(parsed.code, 101);
        assert!(
            parsed
                .data
                .as_str()
                .unwrap()
                .starts_with("No text found"),
            "{}",
            parsed.data
        );
    }

    #[test]
    fn error_replies_carry_the_message_for_the_client_to_surface() {
        for (code, message) in [
            (300, "failed to decode base64 image: bad input"),
            (500, "OCR failed: recognition model error"),
        ] {
            let reply = error_reply(code, message);
            assert!(
                reply.starts_with(&format!("{{\"code\":{code},\"data\":")),
                "{reply}"
            );
            let parsed: ClientReply = serde_json::from_str(&reply).unwrap();
            assert_eq!(parsed.code, code);
            assert_eq!(parsed.data.as_str(), Some(message));
        }
    }

    #[test]
    fn empty_result_region_list_still_produces_a_code_100_shape() {
        // An empty block list never goes out as a success reply — the
        // main loop maps it to no_text_reply — but the function itself
        // must not panic or emit a bare "[]" either way.
        let reply = success_reply(&[]);
        assert_eq!(reply, "{\"code\":100,\"data\":[]}");
    }

    #[test]
    fn parse_task_returns_the_original_image_bytes() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let line = format!("{{\"image_base64\": \"{encoded}\"}}");
        assert_eq!(parse_task(&line).unwrap(), png);
    }

    #[test]
    fn parse_task_tolerates_surrounding_whitespace_and_extra_fields() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([7u8; 4]);
        let line = format!("  {{\"image_base64\": \"{encoded}\", \"path\": \"x.png\"}}\n");
        assert_eq!(parse_task(&line).unwrap(), [7u8; 4]);
    }

    #[test]
    fn malformed_tasks_are_errors_that_name_the_problem() {
        // Not JSON.
        let e = parse_task("hello").unwrap_err();
        assert!(e.contains("malformed task JSON"), "{e}");
        // JSON without the image field.
        let e = parse_task("{\"path\": \"x.png\"}").unwrap_err();
        assert!(e.contains("image_base64"), "{e}");
        // Bad base64.
        let e = parse_task("{\"image_base64\": \"!!not-base64!!\"}").unwrap_err();
        assert!(e.contains("base64"), "{e}");
        // Empty line.
        assert!(parse_task("").is_err());
    }
}
