//! The Google Gemini engine (ticket 11): the keyed LLM option — bring your
//! own AI Studio key, model chosen in Settings → Engines, Thai target like
//! every engine. A `generateContent` call per Text Block whose system
//! instruction comes from the shared versioned template ([`crate::prompt`]),
//! so the per-block Glossary injection and the Genderless instruction ride
//! here exactly as they ride in the OpenAI-compatible adapter.
//!
//! Response parsing, request shape, URL shape, and error classification are
//! tested offline against golden recorded shapes in `fixtures/gemini/` (no
//! network in tests). The live endpoint is covered by a manual smoke
//! checklist.

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError, TranslationGuidance};
use crate::prompt;
use async_trait::async_trait;
use std::time::Duration;

/// The Generative Language API root — the key's project must have the
/// "Generative Language API" enabled. The version segment (`v1beta`) is the
/// one every current model id is served under.
pub const DEFAULT_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// LLM calls can legitimately be slow (reasoning models think); a hung
/// endpoint must not stall the loop forever either way.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The `generateContent` URL for one model — pure so the shape is testable
/// offline. The model rides in the PATH (not the body, unlike chat
/// completions).
pub fn generate_content_url(base: &str, model: &str) -> String {
    format!(
        "{base}/models/{model}:generateContent",
        base = base.trim().trim_end_matches('/'),
        model = model.trim()
    )
}

/// Builds the `generateContent` request body — pure so it is testable
/// offline. The shared versioned system prompt (Glossary injection +
/// Genderless instruction when present) rides in `systemInstruction`; the
/// screen text is the lone user turn.
pub fn request_body(
    temperature: f64,
    text: &str,
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    serde_json::json!({
        "systemInstruction": {
            "parts": [{ "text": prompt::system_prompt(source, target, guidance) }]
        },
        "contents": [{ "role": "user", "parts": [{ "text": text }] }],
        "generationConfig": { "temperature": temperature },
    })
    .to_string()
}

/// Parses a `generateContent` response body and returns the first
/// candidate's text (all its parts concatenated, trimmed).
///
/// Golden shape (recorded fixture):
/// `{"candidates":[{"content":{"parts":[{"text":"สวัสดีชาวโลก"}],"role":"model"},"finishReason":"STOP"}]}`
/// — the engine sends one candidate, whose parts join into the answer.
/// A safety-blocked request has NO candidates; the reason then sits in
/// `promptFeedback.blockReason` and must surface, not parse as empty.
pub fn parse_generate_content_response(body: &str) -> Result<String, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("Gemini response is not valid JSON: {e}")))?;

    if let Some(reason) = value
        .get("promptFeedback")
        .and_then(|feedback| feedback.get("blockReason"))
        .and_then(|reason| reason.as_str())
    {
        return Err(TranslateError::new(format!(
            "Gemini blocked this text before translating it (safety filter: {reason})"
        )));
    }

    let candidate = value
        .get("candidates")
        .and_then(|candidates| candidates.as_array())
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| {
            TranslateError::new("Gemini response has unexpected shape: no candidates")
        })?;

    let parts = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(|parts| parts.as_array())
        .ok_or_else(|| {
            TranslateError::new("Gemini response candidate has no content parts")
        })?;

    let mut text = String::new();
    for part in parts {
        if let Some(piece) = part.get("text").and_then(|text| text.as_str()) {
            text.push_str(piece);
        }
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        // A candidate that returned nothing names its reason when it has
        // one (SAFETY, RECITATION, MAX_TOKENS…) — far more diagnosable
        // than a bare "no text".
        let finish = candidate
            .get("finishReason")
            .and_then(|reason| reason.as_str())
            .unwrap_or("unknown");
        return Err(TranslateError::new(format!(
            "Gemini returned no text (finish reason: {finish} — the model may have blocked or truncated the translation)"
        )));
    }
    Ok(trimmed.to_string())
}

/// Classifies a non-success HTTP response into its user-facing
/// [`TranslateError`]. Gemini's quirks, both encoded here: an invalid key
/// comes back as 400 INVALID_ARGUMENT ("API key not valid"), not 401 — the
/// body, not the HTTP status, identifies it (matched on the message text
/// and the machine-readable `error.status`, so a reworded message still
/// classifies) — and a typo'd model id is a 404 (models live in the URL
/// path). 429 names the free-tier quota. The service's `error.message` is
/// appended so details survive.
pub fn classify_error(status: u16, body: &str) -> TranslateError {
    let error = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").cloned());
    let detail = error
        .as_ref()
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(str::to_string);
    let status_field = error
        .as_ref()
        .and_then(|error| error.get("status"))
        .and_then(|status| status.as_str())
        .map(str::to_string);

    let invalid_key = detail
        .as_deref()
        .is_some_and(|message| message.to_lowercase().contains("api key"))
        || status_field
            .as_deref()
            .is_some_and(|status| status.to_uppercase().contains("API_KEY"));

    let base = match status {
        400 if invalid_key => format!(
            "Gemini rejected the API key (HTTP {status}) — check the key in Settings → Engines"
        ),
        400 => format!(
            "Gemini rejected the request (HTTP {status}) — usually a bad model name; check Settings → Engines"
        ),
        401 | 403 => format!(
            "Gemini denied access (HTTP {status}) — the key is wrong, lacks access, or its project has no Generative Language API enabled; check Settings → Engines"
        ),
        404 => format!(
            "Gemini has no such model (HTTP {status}) — check the model name in Settings → Engines"
        ),
        429 => format!(
            "Gemini quota or rate limit hit (HTTP {status}) — the free tier caps requests per minute and per day; the loop retries when the text changes"
        ),
        _ => format!("Gemini endpoint returned HTTP {status}"),
    };
    match detail {
        Some(detail) => TranslateError::new(format!("{base}: {detail}")),
        None => TranslateError::new(base),
    }
}

/// The keyed Gemini engine.
pub struct GeminiEngine {
    client: reqwest::Client,
    api_key: String,
    model: String,
    temperature: f64,
}

impl GeminiEngine {
    /// `model` is a Generative Language model id (e.g. `gemini-4-hydrogen`);
    /// the key comes from AI Studio. Emptiness is the settings surface's /
    /// loop start's check (like Azure's key) — construction stays lenient.
    pub fn new(api_key: &str, model: &str, temperature: f64) -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            temperature,
        })
    }

    /// One `generateContent` round trip — the request side both port
    /// methods share.
    async fn generate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<String, TranslateError> {
        let response = self
            .client
            .post(generate_content_url(DEFAULT_API_BASE, &self.model))
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .body(request_body(self.temperature, text, source, target, guidance))
            .send()
            .await
            .map_err(|e| TranslateError::new(format!("Gemini endpoint unreachable: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("Gemini endpoint read failed: {e}")))?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &body));
        }
        parse_generate_content_response(&body)
    }
}

#[async_trait]
impl TranslateEngine for GeminiEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        self.generate(text, source, target, &TranslationGuidance::plain())
            .await
    }

    /// LLM engine (ticket 11): the block's relevant Glossary entries and
    /// the Genderless flag ride in the system instruction.
    async fn translate_guided(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<String, TranslateError> {
        self.generate(text, source, target, guidance).await
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        "Gemini"
    }

    /// Every translation posts screen text to Google's API (ADR-0002).
    fn is_cloud(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::guided;

    // ---- URL + request shape ----

    #[test]
    fn url_carries_the_model_in_its_path() {
        assert_eq!(
            generate_content_url(DEFAULT_API_BASE, "gemini-2.0-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        // A base with a trailing slash and a padded model must not double up.
        assert_eq!(
            generate_content_url("https://example.com/v1beta/", " gemini-pro "),
            "https://example.com/v1beta/models/gemini-pro:generateContent"
        );
    }

    #[test]
    fn body_carries_system_instruction_user_text_and_temperature() {
        let body = request_body(
            0.2,
            "Pay 500 gold",
            SourceLanguage::En,
            "th",
            &TranslationGuidance::plain(),
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let system = value["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(system.contains("translation engine"), "was: {system}");
        assert!(system.contains("Thai"), "was: {system}");
        assert!(system.contains("(translation-prompt v3)"), "was: {system}");
        let contents = value["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Pay 500 gold");
        assert_eq!(value["generationConfig"]["temperature"], 0.2);
    }

    #[test]
    fn glossary_and_genderless_ride_in_the_system_instruction() {
        let body = request_body(
            0.2,
            "Vertin picks up the case",
            SourceLanguage::En,
            "th",
            &guided(&[("Vertin", "เวอร์ติน")], true),
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let system = value["systemInstruction"]["parts"][0]["text"].as_str().unwrap();
        assert!(system.contains("- Vertin → เวอร์ติน"), "was: {system}");
        assert!(system.contains("Genderless Mode"), "was: {system}");
        // The user turn stays the bare screen text.
        assert_eq!(value["contents"][0]["parts"][0]["text"], "Vertin picks up the case");
    }

    // ---- Response parsing ----

    #[test]
    fn parses_golden_success_response() {
        let body = include_str!("../fixtures/gemini/success.json");
        assert_eq!(parse_generate_content_response(body).unwrap(), "สวัสดีชาวโลก");
    }

    #[test]
    fn concatenates_multi_part_answers_in_order() {
        // Long answers come back split across parts.
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"บรรทัดแรก"},{"text":"\nบรรทัดที่สอง"}]}}]}"#;
        assert_eq!(
            parse_generate_content_response(body).unwrap(),
            "บรรทัดแรก\nบรรทัดที่สอง"
        );
    }

    #[test]
    fn uses_the_first_candidate_and_trims_padding() {
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"  หนึ่ง  "}]}},{"content":{"parts":[{"text":"สอง"}]}}]}"#;
        assert_eq!(parse_generate_content_response(body).unwrap(), "หนึ่ง");
    }

    #[test]
    fn parts_without_text_are_skipped() {
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"คำแปล"},{"other":"x"}]}}]}"#;
        assert_eq!(parse_generate_content_response(body).unwrap(), "คำแปล");
    }

    #[test]
    fn safety_blocked_response_is_a_translate_error() {
        let body = include_str!("../fixtures/gemini/blocked.json");
        let err = parse_generate_content_response(body).unwrap_err();
        assert!(err.message.contains("blocked"), "was: {}", err.message);
        assert!(err.message.contains("SAFETY"), "was: {}", err.message);
    }

    #[test]
    fn response_without_candidates_is_a_translate_error() {
        let err = parse_generate_content_response(r#"{"modelVersion":"gemini-2.0-flash"}"#).unwrap_err();
        assert!(err.message.contains("no candidates"), "was: {}", err.message);
    }

    #[test]
    fn candidate_without_parts_is_a_translate_error() {
        let err = parse_generate_content_response(r#"{"candidates":[{"finishReason":"STOP"}]}"#)
            .unwrap_err();
        assert!(err.message.contains("no content parts"), "was: {}", err.message);
    }

    #[test]
    fn empty_answer_names_the_finish_reason() {
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"  "}]},"finishReason":"SAFETY"}]}"#;
        let err = parse_generate_content_response(body).unwrap_err();
        assert!(
            err.message.contains("no text") && err.message.contains("SAFETY"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn malformed_json_is_a_translate_error() {
        let err = parse_generate_content_response("<html>blocked</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
    }

    // ---- Error classification ----

    #[test]
    fn invalid_key_names_the_key_even_over_http_400() {
        // Gemini's invalid-key answer is a 400, not a 401 — the message
        // must still point at the key.
        let body = include_str!("../fixtures/gemini/error_invalid_key.json");
        let err = classify_error(400, body);
        assert!(err.message.contains("rejected the API key"), "was: {}", err.message);
        assert!(err.message.contains("400"), "was: {}", err.message);
        assert!(
            err.message.contains("API key not valid"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn invalid_key_is_recognized_by_status_field_too() {
        // The machine-readable status survives message rewording: a body
        // whose text no longer says "API key" still classifies as a key
        // rejection when error.status names it.
        let body = r#"{"error":{"code":400,"message":"Request failed.","status":"API_KEY_INVALID"}}"#;
        let err = classify_error(400, body);
        assert!(
            err.message.contains("rejected the API key"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn other_400s_point_at_the_request_shape() {
        let err = classify_error(400, r#"{"error":{"message":"Invalid JSON payload."}}"#);
        assert!(err.message.contains("rejected the request"), "was: {}", err.message);
        assert!(
            err.message.contains("Invalid JSON payload"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn denied_access_names_key_or_api_enablement() {
        let body = r#"{"error":{"code":403,"message":"Requests from referers with no referer are blocked."}}"#;
        let err = classify_error(403, body);
        assert!(err.message.contains("403"), "was: {}", err.message);
        assert!(err.message.contains("denied access"), "was: {}", err.message);
        assert!(
            err.message.contains("Generative Language API"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn quota_error_names_the_quota() {
        let body = include_str!("../fixtures/gemini/error_rate_limit.json");
        let err = classify_error(429, body);
        assert!(err.message.contains("quota"), "was: {}", err.message);
        assert!(err.message.contains("429"), "was: {}", err.message);
        assert!(
            err.message.contains("Resource has been exhausted"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn not_found_names_the_model() {
        let body = r#"{"error":{"code":404,"message":"Model 'gemini-99' not found for API version v1beta."}}"#;
        let err = classify_error(404, body);
        assert!(err.message.contains("404"), "was: {}", err.message);
        assert!(err.message.contains("model name"), "was: {}", err.message);
        assert!(
            err.message.contains("Model 'gemini-99' not found"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn server_errors_carry_the_service_message() {
        let err = classify_error(500, r#"{"error":{"message":"An internal error has occurred."}}"#);
        assert!(err.message.contains("500"), "was: {}", err.message);
        assert!(
            err.message.contains("internal error"),
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
        let engine = GeminiEngine::new("k", "gemini-2.0-flash", 0.2).unwrap();
        assert_eq!(engine.name(), "Gemini");
        assert!(engine.is_cloud());
    }
}
