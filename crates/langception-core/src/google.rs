//! The unofficial Google Translate endpoint engine (the zero-config default,
//! see ADR-0002). It is ToS-gray and can rate-limit or break: every failure
//! surfaces as a [`TranslateError`] for visible display.
//!
//! Response parsing is tested offline against golden recorded responses in
//! `fixtures/google/` (no network in tests). The live endpoint itself is
//! covered by a manual smoke checklist.

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;

/// The free, keyless endpoint. See ADR-0002 before "fixing" this default.
const DEFAULT_ENDPOINT: &str = "https://translate.googleapis.com";

/// Parses a `translate_a/single?client=gtx&dt=t` response body and returns
/// the concatenated translated text.
///
/// Golden shape (all three recorded fixtures):
/// `[[["สวัสดี","Hello",null,null,3,...],["จ่าย 500 ทอง...","Pay 500 gold...",...]],null,"en",...]`
/// — element 0 is the array of segments; each segment's item 0 is the
/// translated piece. Long inputs come back split across segments, so the
/// full translation is the concatenation in order.
pub fn parse_translation_response(body: &str) -> Result<String, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("Google response is not valid JSON: {e}")))?;

    let segments = value
        .get(0)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            TranslateError::new("Google response has unexpected shape: no segment array")
        })?;

    if segments.is_empty() {
        return Err(TranslateError::new("Google response contained no segments"));
    }

    let mut translated = String::new();
    for segment in segments {
        let piece = segment
            .get(0)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                TranslateError::new("Google response segment has no translated text")
            })?;
        translated.push_str(piece);
    }
    Ok(translated)
}

/// The default Translate Engine: unofficial Google endpoint, no key needed.
pub struct GoogleTranslateEngine {
    client: reqwest::Client,
}

impl GoogleTranslateEngine {
    pub fn new() -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Langception/0.1")
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl TranslateEngine for GoogleTranslateEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let response = self
            .client
            .get(format!("{DEFAULT_ENDPOINT}/translate_a/single"))
            .query(&[
                ("client", "gtx"),
                ("sl", source.code()),
                ("tl", target),
                ("dt", "t"),
                ("q", text),
            ])
            .send()
            .await
            .map_err(|e| TranslateError::new(format!("Google endpoint unreachable: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("Google endpoint read failed: {e}")))?;

        if !status.is_success() {
            return Err(TranslateError::new(format!(
                "Google endpoint returned HTTP {status} (rate limit or outage; see ADR-0002)"
            )));
        }
        parse_translation_response(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_segment_golden_response() {
        let body = include_str!("../fixtures/google/en_single_segment.json");
        assert_eq!(
            parse_translation_response(body).unwrap(),
            "สวัสดีชาวโลก"
        );
    }

    #[test]
    fn parses_multi_segment_golden_response_in_order() {
        let body = include_str!("../fixtures/google/en_multi_segment.json");
        assert_eq!(
            parse_translation_response(body).unwrap(),
            "มังกรเฝ้าสะพาน จ่าย 500 ทองเพื่อผ่าน"
        );
    }

    #[test]
    fn parses_japanese_golden_response() {
        let body = include_str!("../fixtures/google/ja_basic.json");
        assert_eq!(parse_translation_response(body).unwrap(), "สวัสดีตอนเช้า");
    }

    #[test]
    fn malformed_json_is_a_translate_error() {
        let err = parse_translation_response("<html>blocked</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
    }

    #[test]
    fn empty_response_is_a_translate_error() {
        let err = parse_translation_response("[]").unwrap_err();
        assert!(
            err.message.contains("unexpected shape") || err.message.contains("no segments"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn segment_without_text_is_a_translate_error() {
        let err = parse_translation_response("[[[],null,\"en\"]]").unwrap_err();
        assert!(
            err.message.contains("no translated text") || err.message.contains("unexpected shape"),
            "was: {}",
            err.message
        );
    }
}
