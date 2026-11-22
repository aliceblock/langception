//! The generic OpenAI-compatible engine (ticket 10): one chat-completions
//! adapter covering OpenAI, OpenRouter, and any compatible provider via a
//! configurable base URL + key + model + temperature. LM Studio shares this
//! adapter under a second engine name (`lmstudio`, local default endpoint,
//! models discovered from `/models` — ticket 10's offline story). Whether a
//! translation leaves the machine (`is_cloud`) follows the base URL's host,
//! so a localhost server is genuinely local and a remote one is badged.
//!
//! Response parsing, request shape, error classification, and model-list
//! parsing are tested offline against golden recorded shapes in
//! `fixtures/openai/` (no network in tests). Live servers are covered by a
//! manual smoke checklist.

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;
use std::time::Duration;

/// A hung endpoint must not stall the loop forever (same rule as Azure;
/// local models can take longer than a cloud call, so this stays generous).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Error-message prefix for the generic engine; the LM Studio engine passes
/// a more specific one. Both reach the same adapter code paths.
pub const GENERIC_LABEL: &str = "OpenAI-compatible";
pub const LM_STUDIO_LABEL: &str = "LM Studio";

/// The engine display name for a base URL pointing at this machine vs one
/// pointing at the internet (ADR-0002's "the user always knows what leaves
/// their computer"). A URL without a scheme is treated as remote — it would
/// not fetch at all anyway.
pub fn is_local_url(base_url: &str) -> bool {
    let Some(host) = base_url
        .trim()
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    // Drop the port. Bracketed IPv6 first ("[::1]:1234" → "::1"); plain
    // hosts split on the port colon ("localhost:1234" → "localhost").
    let host = match host.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((ipv6, _)) => ipv6,
        None => host.split(':').next().unwrap_or(&host),
    };
    host == "localhost"
        || host == "::1"
        || host.split('.').next() == Some("127")
}

/// Builds the chat request's `messages` array — pure so the prompt is
/// testable offline. The system prompt pins the contract (translate only,
/// keep formatting); the user message carries the screen text.
pub fn translation_messages(text: &str, source: SourceLanguage, target: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": format!(
                "You are a translation engine. Translate the user's text from {source} to {target}. \
                 Reply with ONLY the translation — no explanations, no quotes around it, no notes. \
                 Preserve line breaks and surrounding whitespace. If the text is already in {target}, \
                 repeat it unchanged.",
                source = source_name(source),
                target = target_name(target),
            ),
        }),
        serde_json::json!({ "role": "user", "content": text }),
    ]
}

/// Language names for the prompt — models follow names better than ISO
/// codes, and "th" alone reads ambiguous.
pub fn source_name(source: SourceLanguage) -> &'static str {
    match source {
        SourceLanguage::En => "English",
        SourceLanguage::Ja => "Japanese",
        SourceLanguage::Zh => "Chinese",
        SourceLanguage::Ko => "Korean",
    }
}

pub fn target_name(target: &str) -> String {
    match target {
        "th" => "Thai".to_string(),
        other => other.to_string(),
    }
}

/// Builds the chat-completions request body: the chosen model, the
/// temperature, `stream: false` (a server that streams by default would
/// otherwise return SSE this parser cannot read), and the messages.
pub fn request_body(
    model: &str,
    temperature: f64,
    text: &str,
    source: SourceLanguage,
    target: &str,
) -> String {
    serde_json::json!({
        "model": model,
        "temperature": temperature,
        "stream": false,
        "messages": translation_messages(text, source, target),
    })
    .to_string()
}

/// Parses a chat-completions response body and returns the first choice's
/// message content, trimmed.
///
/// Golden shape (recorded fixture; LM Studio serves the same shape):
/// `{"choices":[{"message":{"role":"assistant","content":"สวัสดีชาวโลก"},...}]}`
/// — the engine sends no `n`, so `choices[0]` is the answer.
pub fn parse_chat_completion(body: &str) -> Result<String, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("LLM response is not valid JSON: {e}")))?;

    let content = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .ok_or_else(|| {
            TranslateError::new("LLM response has unexpected shape: no choices[0].message.content")
        })?;

    let text = content.as_str().ok_or_else(|| {
        TranslateError::new(
            "LLM response message content is not text (the model may refuse to answer or the server may be streaming)",
        )
    })?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(TranslateError::new(
            "LLM response contained no text (the model may be out of context or refuse the request)",
        ));
    }
    Ok(trimmed.to_string())
}

/// Classifies a non-success HTTP response into its user-facing
/// [`TranslateError`]: 401/403 name the key, 404 names the model or URL
/// (the two OpenAI-compatible ways to answer "no such route"), 429 names
/// the rate limit. The service's `error.message` (the OpenAI-compatible
/// error body, LM Studio included) is appended so details survive.
pub fn classify_error(status: u16, body: &str) -> TranslateError {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });

    let base = match status {
        401 | 403 => format!(
            "The endpoint rejected the API key (HTTP {status}) — check the key in Settings → Engines"
        ),
        404 => "The endpoint answered HTTP 404 — the model name or base URL is wrong (base URL must include /v1; check Settings → Engines)".to_string(),
        429 => format!(
            "The endpoint rate-limited translation (HTTP {status}) — slow the Capture cadence or raise the provider's limit"
        ),
        _ => format!("The endpoint returned HTTP {status}"),
    };
    match detail {
        Some(detail) => TranslateError::new(format!("{base}: {detail}")),
        None => TranslateError::new(base),
    }
}

/// One message shape for every unreachable-endpoint failure, so a refused
/// connection names the URL and — when reqwest classifies it as a connect
/// error (`is_connect`: TCP refused, DNS failure) — says what that usually
/// means ("is the server running?"). Pure so the wording is testable
/// offline; reqwest's own Display buries the cause in its source chain.
fn unreachable_message(label: &str, url: &str, error: impl std::fmt::Display, connect: bool) -> String {
    let mut message = format!("{label} server unreachable at {url}: {error}");
    if connect {
        message.push_str(" — connection refused (is the server running?)");
    }
    message
}

/// Parses a `GET /models` response body into sorted model ids (the LM
/// Studio dropdown, ticket 10). Duplicates (a server can list one model
/// under several quantizations... same id twice) collapse.
pub fn parse_models_response(body: &str) -> Result<Vec<String>, TranslateError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| TranslateError::new(format!("models response is not valid JSON: {e}")))?;
    let data = value
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| TranslateError::new("models response has unexpected shape: no data array"))?;
    let mut ids: Vec<String> = data
        .iter()
        .filter_map(|model| model.get("id").and_then(|id| id.as_str()))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The generic OpenAI-compatible engine: one adapter for OpenAI,
/// OpenRouter, LM Studio, llama.cpp, vLLM, … (ticket 10).
pub struct OpenAiCompatibleEngine {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    temperature: f64,
    /// Display name and error prefix: "OpenAI-compatible" or "LM Studio".
    label: &'static str,
}

impl OpenAiCompatibleEngine {
    /// `base_url` includes the version segment (`…/v1`) — every
    /// OpenAI-compatible server routes `/chat/completions` and `/models`
    /// under it. `api_key` may be empty: local servers need no key, and the
    /// Authorization header is simply omitted then.
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        temperature: f64,
        label: &'static str,
    ) -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            temperature,
            label,
        })
    }

    /// The auth header only when a key is configured — LM Studio ignores
    /// (and llama.cpp rejects nothing over) a missing header, and OpenAI
    /// requires it, so the presence follows the key.
    fn auth_header(&self) -> Option<(String, String)> {
        (!self.api_key.is_empty())
            .then(|| ("Authorization".to_string(), format!("Bearer {}", self.api_key)))
    }

    async fn get_models(&self) -> Result<String, TranslateError> {
        let mut request = self
            .client
            .get(format!("{}/models", self.base_url))
            .header("Accept", "application/json");
        if let Some((name, value)) = self.auth_header() {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|e| {
            TranslateError::new(unreachable_message(
                self.label,
                &self.base_url,
                &e,
                e.is_connect(),
            ))
        })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("{label} server read failed: {e}", label = self.label)))?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &body));
        }
        Ok(body)
    }

    /// The ticket-10 health check / model discovery: one `GET /models`
    /// round-trip answers both — a refused connection names the URL ("is
    /// the server running?"), a success returns the model ids. Pure HTTP,
    /// no OCR involvement.
    pub async fn check_endpoint(&self) -> Result<Vec<String>, TranslateError> {
        let body = self.get_models().await?;
        parse_models_response(&body)
    }
}

#[async_trait]
impl TranslateEngine for OpenAiCompatibleEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Content-Type", "application/json")
            .body(request_body(&self.model, self.temperature, text, source, target));
        if let Some((name, value)) = self.auth_header() {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|e| {
            // Connection refused lands here: the message carries the URL and
            // (via is_connect) the "is the server running?" hint — ticket 10
            // makes the refused case a first-class, visible failure.
            TranslateError::new(unreachable_message(
                self.label,
                &self.base_url,
                &e,
                e.is_connect(),
            ))
        })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TranslateError::new(format!("{label} server read failed: {e}", label = self.label)))?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &body));
        }
        parse_chat_completion(&body)
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        self.label
    }

    /// Whether screen text leaves the machine follows the base URL: a
    /// localhost/loopback server stays local (no badge), anything else is
    /// cloud (badge shows) — regardless of which engine name was picked,
    /// because the LM Studio fields accept any URL by design.
    fn is_cloud(&self) -> bool {
        !is_local_url(&self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Local-URL detection (drives the privacy badge) ----

    #[test]
    fn loopback_and_localhost_are_local() {
        assert!(is_local_url("http://localhost:1234/v1"));
        assert!(is_local_url("http://localhost/v1"));
        assert!(is_local_url("http://127.0.0.1:1234/v1"));
        assert!(is_local_url("http://127.9.8.7/v1"));
        assert!(is_local_url("http://[::1]:1234/v1"));
        assert!(is_local_url("HTTP://LOCALHOST:1234/V1"));
    }

    #[test]
    fn remote_urls_are_not_local() {
        assert!(!is_local_url("https://api.openai.com/v1"));
        assert!(!is_local_url("https://openrouter.ai/api/v1"));
        assert!(!is_local_url("http://192.168.1.10:1234/v1"));
        assert!(!is_local_url("localhost-mirror.example.com"));
        assert!(!is_local_url("no-scheme-here"));
        // A hostname merely STARTING with "localhost" must not count.
        assert!(!is_local_url("http://localhost.evil.example/v1"));
    }

    // ---- Request shape ----

    #[test]
    fn body_carries_model_temperature_and_two_messages() {
        let body = request_body("gpt-4o-mini", 0.2, "Pay 500 gold", SourceLanguage::En, "th");
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["temperature"], 0.2);
        assert_eq!(value["stream"], false);
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "system prompt + the text");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Pay 500 gold");
    }

    #[test]
    fn system_prompt_names_both_languages() {
        let messages = translation_messages("สวัสดี", SourceLanguage::Ja, "th");
        let prompt = messages[0]["content"].as_str().unwrap();
        assert!(prompt.contains("Japanese"), "was: {prompt}");
        assert!(prompt.contains("Thai"), "was: {prompt}");
    }

    #[test]
    fn chinese_source_maps_to_chinese_in_the_prompt() {
        let messages = translation_messages("你好", SourceLanguage::Zh, "th");
        assert!(
            messages[0]["content"].as_str().unwrap().contains("Chinese"),
            "zh must say Chinese, not the code"
        );
    }

    // ---- Response parsing ----

    #[test]
    fn parses_golden_success_response() {
        let body = include_str!("../fixtures/openai/success.json");
        assert_eq!(parse_chat_completion(body).unwrap(), "สวัสดีชาวโลก");
    }

    #[test]
    fn parses_content_with_escapes_and_unicode() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"\"บรรทัดแรก\"\nบรรทัดที่สอง"}}]}"#;
        assert_eq!(
            parse_chat_completion(body).unwrap(),
            "\"บรรทัดแรก\"\nบรรทัดที่สอง"
        );
    }

    #[test]
    fn uses_first_choice_and_trims_padding() {
        let body = r#"{"choices":[{"message":{"content":"  หนึ่ง  "}},{"message":{"content":"สอง"}}]}"#;
        assert_eq!(parse_chat_completion(body).unwrap(), "หนึ่ง");
    }

    #[test]
    fn malformed_json_is_a_translate_error() {
        let err = parse_chat_completion("<html>blocked</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
    }

    #[test]
    fn response_without_choices_is_a_translate_error() {
        let err = parse_chat_completion(r#"{"object":"chat.completion"}"#).unwrap_err();
        assert!(err.message.contains("unexpected shape"), "was: {}", err.message);
    }

    #[test]
    fn null_content_is_a_translate_error() {
        // Refusals stream into `content` as null on several servers.
        let err = parse_chat_completion(r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#)
            .unwrap_err();
        assert!(err.message.contains("not text"), "was: {}", err.message);
    }

    #[test]
    fn empty_content_is_a_translate_error() {
        let err = parse_chat_completion(r#"{"choices":[{"message":{"content":"   "}}]}"#).unwrap_err();
        assert!(err.message.contains("no text"), "was: {}", err.message);
    }

    // ---- Error classification ----

    #[test]
    fn invalid_key_names_the_key() {
        let body = include_str!("../fixtures/openai/error_invalid_key.json");
        let err = classify_error(401, body);
        assert!(err.message.contains("rejected the API key"), "was: {}", err.message);
        assert!(err.message.contains("401"), "was: {}", err.message);
        assert!(
            err.message.contains("Incorrect API key provided"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn forbidden_classifies_like_an_auth_error() {
        let err = classify_error(403, "");
        assert!(err.message.contains("403"), "was: {}", err.message);
        assert!(err.message.contains("key"), "was: {}", err.message);
    }

    #[test]
    fn rate_limit_names_the_limit() {
        let body = include_str!("../fixtures/openai/error_rate_limit.json");
        let err = classify_error(429, body);
        assert!(err.message.contains("rate-limited"), "was: {}", err.message);
        assert!(err.message.contains("429"), "was: {}", err.message);
        assert!(
            err.message.contains("Rate limit reached"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn not_found_names_the_model_or_url() {
        // A base URL without /v1, or a typo'd model, is the classic 404 on
        // these servers — the message must point at the two fields.
        let err = classify_error(404, r#"{"error":{"message":"No such model: gpt-5-turbo"}}"#);
        assert!(err.message.contains("404"), "was: {}", err.message);
        assert!(err.message.contains("model name or base URL"), "was: {}", err.message);
        assert!(
            err.message.contains("No such model"),
            "the service message must survive: {}",
            err.message
        );
    }

    #[test]
    fn server_errors_carry_the_service_message() {
        let err = classify_error(500, r#"{"error":{"message":"The server had an error"}}"#);
        assert!(err.message.contains("500"), "was: {}", err.message);
        assert!(
            err.message.contains("The server had an error"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn non_json_error_body_still_classifies() {
        let err = classify_error(503, "<html>gateway</html>");
        assert!(err.message.contains("503"), "was: {}", err.message);
    }

    #[test]
    fn unreachable_message_names_url_and_hints_on_refusal() {
        // The connection-refused case (ticket 10's first-class failure): the
        // message must name the URL and say what it likely means.
        let refused = unreachable_message("LM Studio", "http://localhost:1234/v1", "tcp error", true);
        assert!(refused.contains("http://localhost:1234/v1"), "was: {refused}");
        assert!(refused.contains("connection refused"), "was: {refused}");
        assert!(refused.contains("is the server running?"), "was: {refused}");
        assert!(refused.contains("LM Studio"), "was: {refused}");
        // Other send errors (timeout, TLS) stay free of the hint.
        let other = unreachable_message("LM Studio", "http://localhost:1234/v1", "timed out", false);
        assert!(!other.contains("connection refused"), "was: {other}");
    }

    // ---- Models list (LM Studio discovery) ----

    #[test]
    fn parses_golden_models_list_sorted() {
        let body = include_str!("../fixtures/openai/models_list.json");
        assert_eq!(
            parse_models_response(body).unwrap(),
            vec![
                "gpt-4o-mini".to_string(),
                "mistral-nemo-instruct-2407".to_string(),
                "qwen2.5-7b-instruct".to_string(),
            ]
        );
    }

    #[test]
    fn duplicate_model_ids_collapse() {
        let body = r#"{"data":[{"id":"same"},{"id":"same"},{"id":"other"}]}"#;
        assert_eq!(
            parse_models_response(body).unwrap(),
            vec!["other".to_string(), "same".to_string()]
        );
    }

    #[test]
    fn empty_model_list_is_fine() {
        // A running server with no loaded models: valid, just empty — the
        // UI shows "no models" rather than an error.
        assert!(parse_models_response(r#"{"data":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn malformed_models_body_is_an_error() {
        let err = parse_models_response("<html>refused</html>").unwrap_err();
        assert!(err.message.contains("not valid JSON"), "was: {}", err.message);
        let err = parse_models_response(r#"{"models":["m"]}"#).unwrap_err();
        assert!(err.message.contains("no data array"), "was: {}", err.message);
    }

    // ---- Engine identity ----

    #[test]
    fn cloud_urls_name_the_generic_engine_and_badge_cloud() {
        let engine =
            OpenAiCompatibleEngine::new("https://api.openai.com/v1", "sk", "gpt-4o-mini", 0.2, GENERIC_LABEL)
                .unwrap();
        assert_eq!(engine.name(), "OpenAI-compatible");
        assert!(engine.is_cloud());
    }

    #[test]
    fn localhost_urls_are_badged_local_for_any_engine_name() {
        let engine = OpenAiCompatibleEngine::new(
            "http://localhost:1234/v1",
            "",
            "qwen2.5-7b-instruct",
            0.2,
            LM_STUDIO_LABEL,
        )
        .unwrap();
        assert_eq!(engine.name(), "LM Studio");
        assert!(!engine.is_cloud(), "a local server must not show the ☁ badge");
    }

    #[test]
    fn trailing_slash_in_base_url_is_normalized() {
        let engine = OpenAiCompatibleEngine::new("http://localhost:1234/v1/", "", "m", 0.2, LM_STUDIO_LABEL)
            .unwrap();
        // The URL shape is internal; verify via the models path it would
        // build — through the public constructor there is no other surface,
        // so assert on is_cloud's parse of the stored URL instead and keep
        // the real check in the golden request tests.
        assert!(is_local_url(&engine.base_url));
        assert!(!engine.base_url.ends_with('/'), "was: {}", engine.base_url);
    }

    #[test]
    fn empty_key_sends_no_auth_header() {
        let engine = OpenAiCompatibleEngine::new("http://localhost:1234/v1", "  ", "m", 0.2, LM_STUDIO_LABEL)
            .unwrap();
        assert!(engine.auth_header().is_none());
        let keyed =
            OpenAiCompatibleEngine::new("https://api.openai.com/v1", "sk-x", "m", 0.2, GENERIC_LABEL).unwrap();
        assert_eq!(
            keyed.auth_header(),
            Some(("Authorization".to_string(), "Bearer sk-x".to_string()))
        );
    }
}
