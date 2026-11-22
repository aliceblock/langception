//! The Azure AI Translator engine (ticket 09): the keyed, reliable upgrade
//! to the zero-config Google default (ADR-0002). Sized for the F0 free tier
//! (2M chars/month): quota exhaustion, invalid keys, and network failures
//! all surface as classified [`TranslateError`]s for visible display —
//! never a silent stop.
//!
//! Response parsing and error classification are tested offline against
//! golden recorded shapes in `fixtures/azure/` (no network in tests). The
//! live endpoint is covered by a manual smoke checklist.

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;
use std::time::Duration;

/// The global Translator endpoint. Regional F0 resources (every resource
/// has a region) send their region via the `Ocp-Apim-Subscription-Region`
/// header; the global resource needs no header at all.
const ENDPOINT: &str = "https://api.cognitive.microsofttranslator.com";
const API_VERSION: &str = "3.0";

/// Unauthenticated errors come back fast, but a hung endpoint must not
/// stall the loop forever: a request that never settles would freeze the
/// whole pipeline between ticks.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the translate request's query pairs (`api-version`, `from`,
/// `to`) — pure so the request shape is testable offline.
pub fn query_pairs(source: SourceLanguage, target: &str) -> Vec<(&'static str, String)> {
    vec![
        ("api-version", API_VERSION.to_string()),
        ("from", source.code().to_string()),
        ("to", target.to_string()),
    ]
}

/// Builds the request's JSON body: Translator v3 takes an array of text
/// items — one per request here.
pub fn request_body(text: &str) -> String {
    serde_json::to_string(&[serde_json::json!({ "Text": text })])
        .expect("a one-item array of a string always serializes")
}

/// Parses a v3 translate response body and returns the first item's first
/// translation text.
///
/// Golden shape (recorded fixture):
/// `[{"detectedLanguage":{...},"translations":[{"text":"สวัสดี","to":"th"}]}]`
/// — the request sends exactly one text item, so item 0's `translations[0]`
/// is the answer.
pub fn parse_translation_response(body: &str) -> Result<String, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("Azure response is not valid JSON: {e}")))?;

    let items = value.as_array().ok_or_else(|| {
        TranslateError::new("Azure response has unexpected shape: not a text-item array")
    })?;
    let item = items.first().ok_or_else(|| {
        TranslateError::new("Azure response contained no text items")
    })?;

    let translations = item
        .get("translations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| TranslateError::new("Azure response item has no translations"))?;
    let translation = translations.first().ok_or_else(|| {
        TranslateError::new("Azure response item has no translations")
    })?;

    let text = translation
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TranslateError::new("Azure response translation has no translated text"))?;
    Ok(text.to_string())
}

/// Classifies a non-success HTTP response into its user-facing
/// [`TranslateError`]: 401/403 name the key, 429 names the quota/rate
/// limit, everything else carries the API's own message. The service's
/// error body (when present) is appended so details survive.
pub fn classify_error(status: u16, body: &str) -> TranslateError {
    // The service's own detail ({"error":{"message":"…"}}) is worth
    // appending whenever it parses — free-tier quota messages differ from
    // rate-limit ones, and users report them verbatim when asking for help.
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });

    let base = match status {
        401 => "Azure rejected the API key (HTTP 401) — check the key and region in Settings → Engines".to_string(),
        403 => format!(
            "Azure denied access (HTTP {status}) — wrong key, region mismatch, or the resource is out of free-tier access; check Settings → Engines"
        ),
        429 => format!(
            "Azure quota or rate limit hit (HTTP {status}) — the F0 free tier allows 2M chars/month, and the loop retries when the text changes"
        ),
        _ => format!("Azure endpoint returned HTTP {status}"),
    };
    match detail {
        Some(detail) => TranslateError::new(format!("{base}: {detail}")),
        None => TranslateError::new(base),
    }
}

/// The keyed Azure AI Translator engine.
pub struct AzureTranslateEngine {
    client: reqwest::Client,
    key: String,
    /// Region of the resource ("global" or empty = no region header).
    region: String,
}

impl AzureTranslateEngine {
    pub fn new(key: &str, region: &str) -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            key: key.to_string(),
            region: region.trim().to_string(),
        })
    }
}

#[async_trait]
impl TranslateEngine for AzureTranslateEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let mut request = self
            .client
            .post(format!("{ENDPOINT}/translate"))
            .header("Ocp-Apim-Subscription-Key", &self.key)
            .header("Content-Type", "application/json")
            .query(&query_pairs(source, target))
            .body(request_body(text));
        // The region header belongs to regional (geographic) resources; a
        // global resource must send none.
        if !self.region.is_empty() {
            request = request.header("Ocp-Apim-Subscription-Region", &self.region);
        }
        let response = request.send().await.map_err(|e| {
            TranslateError::new(format!("Azure endpoint unreachable: {e}"))
        })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("Azure endpoint read failed: {e}")))?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &body));
        }
        parse_translation_response(&body)
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        "Azure AI Translator"
    }

    /// Every translation posts screen text to Azure (ADR-0002).
    fn is_cloud(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_golden_success_response() {
        let body = include_str!("../fixtures/azure/success.json");
        assert_eq!(parse_translation_response(body).unwrap(), "สวัสดีชาวโลก");
    }

    #[test]
    fn parses_text_with_escapes_and_unicode() {
        // A hand-built shape exercising JSON escapes: Thai text with quotes
        // and newlines must survive the round-trip exactly.
        let body = r#"[{"translations":[{"text":"\"บรรทัดแรก\"\nบรรทัดที่สอง","to":"th"}]}]"#;
        assert_eq!(
            parse_translation_response(body).unwrap(),
            "\"บรรทัดแรก\"\nบรรทัดที่สอง"
        );
    }

    #[test]
    fn uses_translations_first_entry() {
        // Defensive shape: extra items or translations must not confuse the
        // parse — the engine always sends one item and reads the first.
        let body = r#"[{"translations":[{"text":"หนึ่ง","to":"th"},{"text":"สอง","to":"en"}]}]"#;
        assert_eq!(parse_translation_response(body).unwrap(), "หนึ่ง");
    }

    #[test]
    fn malformed_json_is_a_translate_error() {
        let err = parse_translation_response("<html>blocked</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
    }

    #[test]
    fn empty_response_array_is_a_translate_error() {
        let err = parse_translation_response("[]").unwrap_err();
        assert!(err.message.contains("no text items"), "was: {}", err.message);
    }

    #[test]
    fn response_without_translations_is_a_translate_error() {
        let err = parse_translation_response(r#"[{"detectedLanguage":{"language":"en"}}]"#).unwrap_err();
        assert!(err.message.contains("no translations"), "was: {}", err.message);
    }

    #[test]
    fn translation_without_text_is_a_translate_error() {
        let err = parse_translation_response(r#"[{"translations":[{"to":"th"}]}]"#).unwrap_err();
        assert!(err.message.contains("no translated text"), "was: {}", err.message);
    }

    // ---- Request shape ----

    #[test]
    fn query_carries_api_version_from_and_to() {
        let pairs = query_pairs(SourceLanguage::Ja, "th");
        assert_eq!(pairs[0], ("api-version", "3.0".to_string()));
        assert_eq!(pairs[1], ("from", "ja".to_string()));
        assert_eq!(pairs[2], ("to", "th".to_string()));
    }

    #[test]
    fn body_wraps_text_in_one_item() {
        assert_eq!(request_body("Pay 500 gold"), r#"[{"Text":"Pay 500 gold"}]"#);
    }

    // ---- Error classification ----

    #[test]
    fn invalid_key_names_the_key() {
        let body = include_str!("../fixtures/azure/error_invalid_key.json");
        let err = classify_error(401, body);
        assert!(err.message.contains("API key"), "was: {}", err.message);
        assert!(err.message.contains("401"), "was: {}", err.message);
        assert!(
            err.message.contains("credentials are missing or invalid"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn quota_error_names_the_quota() {
        let body = include_str!("../fixtures/azure/error_quota.json");
        let err = classify_error(429, body);
        assert!(
            err.message.contains("quota"),
            "was: {}",
            err.message
        );
        assert!(err.message.contains("429"), "was: {}", err.message);
        assert!(
            err.message.contains("Monthly quota exceeded"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn forbidden_names_the_key_or_access() {
        let err = classify_error(403, "");
        assert!(err.message.contains("403"), "was: {}", err.message);
        assert!(err.message.contains("access"), "was: {}", err.message);
    }

    #[test]
    fn server_errors_carry_the_service_message() {
        let body = r#"{"error":{"code":500000,"message":"One of the server components is unavailable."}}"#;
        let err = classify_error(500, body);
        assert!(err.message.contains("500"), "was: {}", err.message);
        assert!(
            err.message.contains("One of the server components"),
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
        let engine = AzureTranslateEngine::new("k", "global").unwrap();
        assert_eq!(engine.name(), "Azure AI Translator");
        assert!(engine.is_cloud());
    }
}
