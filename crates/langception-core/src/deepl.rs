//! The DeepL engine (ticket 09): the second keyed cloud option, sized for
//! the free tier (1M chars total, Thai supported; free keys end with
//! `:fx` and use the `api-free` host). Quota exhaustion, invalid keys, and
//! network failures surface as classified [`TranslateError`]s — never a
//! silent stop.
//!
//! Response parsing, request shape, and error classification are tested
//! offline against golden recorded shapes in `fixtures/deepl/` (no network
//! in tests). The live endpoint is covered by a manual smoke checklist.

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;
use std::time::Duration;

/// Free-tier (`:fx` keys) and pro keys live on different hosts.
const FREE_ENDPOINT: &str = "https://api-free.deepl.com";
const PRO_ENDPOINT: &str = "https://api.deepl.com";

/// A hung endpoint must not stall the loop forever (same rule as Azure).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The host matching a DeepL key: free keys end with `:fx` and only work
/// against `api-free`. A pro key pasted in still works (and vice versa the
/// free tier is rejected with a clear error) — the suffix is authoritative.
pub fn endpoint_for_key(key: &str) -> &'static str {
    if key.ends_with(":fx") {
        FREE_ENDPOINT
    } else {
        PRO_ENDPOINT
    }
}

/// Upper-cased source code — DeepL's codes are the upper-case forms of
/// ours (`en` → `EN`, `zh` → `ZH`).
pub fn source_code(source: SourceLanguage) -> &'static str {
    match source {
        SourceLanguage::En => "EN",
        SourceLanguage::Ja => "JA",
        SourceLanguage::Zh => "ZH",
        SourceLanguage::Ko => "KO",
    }
}

/// Builds the translate request's JSON body. `target` arrives lowercase
/// (`"th"`, our config spelling); DeepL wants upper-case.
pub fn request_body(text: &str, source: SourceLanguage, target: &str) -> String {
    serde_json::json!({
        "text": [text],
        "source_lang": source_code(source),
        "target_lang": target.to_uppercase(),
    })
    .to_string()
}

/// Parses a v2 translate response body and returns the first translation's
/// text.
///
/// Golden shape (recorded fixture):
/// `{"translations":[{"detected_source_language":"EN","text":"สวัสดี"}]}`
/// — the engine sends exactly one text item, so `translations[0]` is the
/// answer.
pub fn parse_translation_response(body: &str) -> Result<String, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("DeepL response is not valid JSON: {e}")))?;

    let translations = value
        .get("translations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            TranslateError::new("DeepL response has unexpected shape: no translations array")
        })?;
    let translation = translations.first().ok_or_else(|| {
        TranslateError::new("DeepL response contained no translations")
    })?;

    let text = translation
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TranslateError::new("DeepL response translation has no translated text"))?;
    Ok(text.to_string())
}

/// Classifies a non-success HTTP response into its user-facing
/// [`TranslateError`]: 401/403 name the key, 456 names the monthly quota,
/// 429 names rate limiting. DeepL's error body is `{"message":"…"}` when
/// present, appended so details survive.
pub fn classify_error(status: u16, body: &str) -> TranslateError {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string));

    let base = match status {
        401 | 403 => format!(
            "DeepL rejected the API key (HTTP {status}) — check the key in Settings → Engines (free keys end with \":fx\")"
        ),
        429 => format!(
            "DeepL rate limit hit (HTTP {status}) — too many requests; the loop retries when the text changes"
        ),
        456 => format!(
            "DeepL quota exhausted (HTTP {status}) — the free tier allows 1M chars total (lifetime); upgrade to a paid plan to keep translating"
        ),
        _ => format!("DeepL endpoint returned HTTP {status}"),
    };
    match detail {
        Some(detail) => TranslateError::new(format!("{base}: {detail}")),
        None => TranslateError::new(base),
    }
}

/// The keyed DeepL engine.
pub struct DeepLTranslateEngine {
    client: reqwest::Client,
    endpoint: &'static str,
    key: String,
}

impl DeepLTranslateEngine {
    pub fn new(key: &str) -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            endpoint: endpoint_for_key(key),
            key: key.trim().to_string(),
        })
    }
}

#[async_trait]
impl TranslateEngine for DeepLTranslateEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let response = self
            .client
            .post(format!("{}/v2/translate", self.endpoint))
            .header("Authorization", format!("DeepL-Auth-Key {}", self.key))
            .header("Content-Type", "application/json")
            .body(request_body(text, source, target))
            .send()
            .await
            .map_err(|e| TranslateError::new(format!("DeepL endpoint unreachable: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("DeepL endpoint read failed: {e}")))?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &body));
        }
        parse_translation_response(&body)
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        "DeepL"
    }

    /// Every translation posts screen text to DeepL (ADR-0002).
    fn is_cloud(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_keys_get_the_free_host() {
        assert_eq!(
            endpoint_for_key("abc123:fx"),
            "https://api-free.deepl.com"
        );
    }

    #[test]
    fn pro_keys_get_the_pro_host() {
        assert_eq!(endpoint_for_key("abc123"), "https://api.deepl.com");
    }

    #[test]
    fn body_carries_one_text_upper_cased_langs() {
        assert_eq!(
            request_body("Pay 500 gold", SourceLanguage::En, "th"),
            r#"{"source_lang":"EN","target_lang":"TH","text":["Pay 500 gold"]}"#
        );
    }

    #[test]
    fn chinese_source_maps_to_zh() {
        let body = request_body("你好", SourceLanguage::Zh, "th");
        assert!(body.contains(r#""source_lang":"ZH""#), "was: {body}");
    }

    // ---- Response parsing ----

    #[test]
    fn parses_golden_success_response() {
        let body = include_str!("../fixtures/deepl/success.json");
        assert_eq!(parse_translation_response(body).unwrap(), "สวัสดีชาวโลก");
    }

    #[test]
    fn parses_text_with_escapes_and_unicode() {
        let body = r#"{"translations":[{"detected_source_language":"EN","text":"\"บรรทัดแรก\"\nบรรทัดที่สอง"}]}"#;
        assert_eq!(
            parse_translation_response(body).unwrap(),
            "\"บรรทัดแรก\"\nบรรทัดที่สอง"
        );
    }

    #[test]
    fn uses_translations_first_entry() {
        let body = r#"{"translations":[{"text":"หนึ่ง"},{"text":"สอง"}]}"#;
        assert_eq!(parse_translation_response(body).unwrap(), "หนึ่ง");
    }

    #[test]
    fn malformed_json_is_a_translate_error() {
        let err = parse_translation_response("<html>blocked</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
    }

    #[test]
    fn empty_translations_is_a_translate_error() {
        let err = parse_translation_response(r#"{"translations":[]}"#).unwrap_err();
        assert!(err.message.contains("no translations"), "was: {}", err.message);
    }

    #[test]
    fn response_without_translations_is_a_translate_error() {
        let err = parse_translation_response("{}").unwrap_err();
        assert!(err.message.contains("unexpected shape"), "was: {}", err.message);
    }

    #[test]
    fn translation_without_text_is_a_translate_error() {
        let err =
            parse_translation_response(r#"{"translations":[{"detected_source_language":"EN"}]}"#)
                .unwrap_err();
        assert!(err.message.contains("no translated text"), "was: {}", err.message);
    }

    // ---- Error classification ----

    #[test]
    fn invalid_key_names_the_key() {
        let body = include_str!("../fixtures/deepl/error_invalid_key.json");
        let err = classify_error(403, body);
        assert!(err.message.contains("API key"), "was: {}", err.message);
        assert!(err.message.contains("403"), "was: {}", err.message);
        assert!(
            err.message.contains("Wrong or missing authorization key"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn quota_error_names_the_quota() {
        let body = include_str!("../fixtures/deepl/error_quota.json");
        let err = classify_error(456, body);
        assert!(err.message.contains("quota"), "was: {}", err.message);
        assert!(err.message.contains("456"), "was: {}", err.message);
        assert!(
            err.message.contains("Monthly translation limit"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn rate_limit_is_named() {
        let err = classify_error(429, "");
        assert!(err.message.contains("429"), "was: {}", err.message);
        assert!(
            err.message.contains("too many requests"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn server_errors_carry_the_service_message() {
        let body = r#"{"message":"Internal server error"}"#;
        let err = classify_error(500, body);
        assert!(err.message.contains("500"), "was: {}", err.message);
        assert!(
            err.message.contains("Internal server error"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn non_json_error_body_still_classifies() {
        let err = classify_error(503, "<html>gateway</html>");
        assert!(err.message.contains("503"), "was: {}", err.message);
    }

    // ---- Engine identity ----

    #[test]
    fn names_itself_for_the_privacy_badge() {
        // Construction builds only the HTTP client — no network.
        let engine = DeepLTranslateEngine::new("k:fx").unwrap();
        assert_eq!(engine.name(), "DeepL");
        assert!(engine.is_cloud());
    }
}
