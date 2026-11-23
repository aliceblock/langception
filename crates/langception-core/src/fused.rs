//! The fused vision engines (ticket 12): one model call that reads every
//! Text Block in a captured frame AND translates it — the OCR and Translate
//! stages collapse into one HTTP round trip, for stylized game fonts that
//! defeat traditional OCR. Two adapters, sharing one reply contract and one
//! prompt template ([`crate::prompt::fused_system_prompt`]):
//!
//! - [`GeminiFusedEngine`] posts the PNG as an `inlineData` part of a
//!   `generateContent` call (the same endpoint, key header, and response
//!   envelope as the Gemini translate adapter — [`crate::gemini`]).
//! - [`OpenAiFusedEngine`] posts it as an `image_url` data URL in a
//!   chat-completions call — OpenAI, OpenRouter, and LM Studio's local
//!   vision models included (the same adapter split as
//!   [`crate::openai`], down to the label and the local-URL privacy rule).
//!
//! The model's answer — a JSON array of blocks with image-pixel boxes and
//! Thai text — is parsed by [`parse_blocks_response`], which tolerates the
//! two ways vision models inevitably answer despite the prompt: markdown
//! fences and prose around the array. Parsing, request shapes, and PNG
//! encoding are tested offline against golden recorded shapes in
//! `fixtures/fused/` (no network in tests); live endpoints are covered by
//! a manual smoke checklist.

use crate::openai;
use crate::ports::{
    CapturedImage, FusedBlock, FusedEngine, FusedError, SourceLanguage, TextBlock,
    TranslationGuidance,
};
use crate::prompt;
use async_trait::async_trait;
use base64::Engine as _;
use std::time::Duration;

/// LLM calls can legitimately be slow (vision models think); a hung
/// endpoint must not stall the loop forever either way. Same rule as both
/// translate adapters.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The user-turn text that rides with the image (the full contract is the
/// system prompt; some endpoints ignore a bare image part's position
/// without a text sibling).
const USER_TURN: &str = "Read and translate every text block in this image.";

/// Encodes a captured frame as PNG bytes (BGRA → RGBA; alpha forced opaque —
/// GDI output leaves the alpha byte undefined, and a transparent PNG would
/// blank the very pixels the model is asked to read). The image goes to the
/// model whole, so boxes come back in exact capture pixels and the loop's
/// existing block math needs no rescaling.
pub fn bgra_to_png_bytes(image: &CapturedImage) -> Result<Vec<u8>, FusedError> {
    let mut rgba = Vec::with_capacity(image.bgra.len());
    for pixel in image.bgra.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| FusedError::new(format!("failed to encode the captured image: {e}")))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| FusedError::new(format!("failed to encode the captured image: {e}")))?;
    }
    Ok(png_bytes)
}

/// Base64 of the PNG in the Gemini `inlineData` shape.
pub fn inline_data_part(png_bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "inlineData": {
            "mimeType": "image/png",
            "data": base64::engine::general_purpose::STANDARD.encode(png_bytes),
        }
    })
}

/// `data:` URL of the PNG in the OpenAI-compatible `image_url` shape.
pub fn image_data_url(png_bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png_bytes)
    )
}

/// Builds the Gemini `generateContent` request body — pure so it is
/// testable offline. The fused contract rides in `systemInstruction`; the
/// image (PNG, base64) is the user turn's first part.
pub fn gemini_request_body(
    temperature: f64,
    png_bytes: &[u8],
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    serde_json::json!({
        "systemInstruction": {
            "parts": [{ "text": prompt::fused_system_prompt(source, target, guidance) }]
        },
        "contents": [{
            "role": "user",
            "parts": [inline_data_part(png_bytes), { "text": USER_TURN }],
        }],
        "generationConfig": { "temperature": temperature },
    })
    .to_string()
}

/// Builds the chat-completions request body for an OpenAI-compatible
/// vision model — pure so it is testable offline. Vision content parts
/// (`image_url` with a data URL) are the OpenAI convention LM Studio and
/// the compatible providers follow.
pub fn openai_request_body(
    model: &str,
    temperature: f64,
    png_bytes: &[u8],
    source: SourceLanguage,
    target: &str,
    guidance: &TranslationGuidance,
) -> String {
    serde_json::json!({
        "model": model,
        "temperature": temperature,
        "stream": false,
        "messages": [
            {
                "role": "system",
                "content": prompt::fused_system_prompt(source, target, guidance),
            },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": USER_TURN },
                    { "type": "image_url", "image_url": { "url": image_data_url(png_bytes) } },
                ],
            },
        ],
    })
    .to_string()
}

/// Strips one wrapping markdown code fence (` ```json … ``` `, the shape
/// vision models answer in no matter what the prompt says). A reply that
/// does not start with a fence is returned untouched — the array slice
/// below finds the JSON inside prose anyway.
fn strip_code_fences(text: &str) -> &str {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed;
    }
    let Some(newline) = trimmed.find('\n') else {
        return trimmed; // a bare ``` — nothing to strip
    };
    let body = &trimmed[newline + 1..];
    body.trim()
        .strip_suffix("```")
        .unwrap_or(body)
        .trim()
}

/// One reply element → a [`FusedBlock`]. Keys follow the prompt's pinned
/// shape (`x, y, w, h, text, th`) with the two abbreviation aliases models
/// actually produce (`width`/`height`, `thai`/`translation`). Boxes are
/// clamped into the image's positive quadrant; anything unparseable — a
/// missing translation, a degenerate box, a stray comment object — is
/// skipped rather than failing the whole frame.
fn parse_block_entry(entry: &serde_json::Value) -> Option<FusedBlock> {
    let object = entry.as_object()?;
    // Integers are demanded by the prompt, but models emit `12.0` all the
    // time — accept any JSON number that rounds to an integer.
    let int_field = |keys: &[&str]| -> Option<i32> {
        keys.iter()
            .find_map(|key| object.get(*key))
            .and_then(|value| value.as_f64())
            .map(|value| value.round() as i32)
    };
    let x = int_field(&["x"])?.max(0);
    let y = int_field(&["y"])?.max(0);
    let width = int_field(&["w", "width"])?.max(0);
    let height = int_field(&["h", "height"])?.max(0);
    if width < 1 || height < 1 {
        return None;
    }
    let text = object.get("text")?.as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let translation = ["th", "thai", "translation"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| value.as_str())?
        .trim()
        .to_string();
    if translation.is_empty() {
        return None;
    }
    Some(FusedBlock {
        block: TextBlock {
            x,
            y,
            width: width as u32,
            height: height as u32,
            text,
        },
        translation,
    })
}

/// Parses the model's reply into Fused Blocks.
///
/// Golden shape (recorded fixtures): a JSON array of objects with integer
/// image-pixel boxes and the source + Thai text per block —
/// `[{"x":12,"y":34,"w":200,"h":24,"text":"Start","th":"เริ่ม"}]`.
/// Leniency, in order: markdown fences are stripped, the JSON array is
/// located inside any surrounding prose (first `[` to last `]`), and bad
/// entries are skipped. An empty array is a valid "no text on screen". A
/// reply with no array at all is an error, not a silent empty pass — the
/// user must see why nothing translated.
pub fn parse_blocks_response(text: &str) -> Result<Vec<FusedBlock>, FusedError> {
    let cleaned = strip_code_fences(text);
    let Some(start) = cleaned.find('[') else {
        return Err(FusedError::new(
            "the vision model's reply contained no JSON array — the fused prompt asks for \
             one object per text block with x/y/w/h/text/th",
        ));
    };
    let Some(end) = cleaned.rfind(']') else {
        return Err(FusedError::new(
            "the vision model's reply contained an unterminated JSON array",
        ));
    };
    if end < start {
        return Err(FusedError::new(
            "the vision model's reply contained no JSON array",
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&cleaned[start..=end]).map_err(|e| {
        FusedError::new(format!(
            "the vision model's reply is not valid JSON: {e}"
        ))
    })?;
    let entries = value
        .as_array()
        .ok_or_else(|| FusedError::new("the vision model's reply is not a JSON array"))?;
    Ok(entries
        .iter()
        .filter_map(parse_block_entry)
        .collect::<Vec<_>>())
}

/// The keyed Gemini vision engine: the same AI Studio key and model id as
/// the translate engine ([`GeminiEngine::new`]), driven at the same
/// `generateContent` endpoint with the image attached.
pub struct GeminiFusedEngine {
    client: reqwest::Client,
    api_key: String,
    model: String,
    temperature: f64,
}

impl GeminiFusedEngine {
    /// `model` is a Generative Language model id with vision capability
    /// (e.g. `gemini-2.0-flash`); the key comes from AI Studio. Emptiness
    /// is the settings surface's / loop start's check — construction stays
    /// lenient (the translate engine's rule).
    pub fn new(api_key: &str, model: &str, temperature: f64) -> Result<Self, FusedError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| FusedError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            temperature,
        })
    }
}

#[async_trait]
impl FusedEngine for GeminiFusedEngine {
    async fn recognize_and_translate(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<Vec<FusedBlock>, FusedError> {
        let png = bgra_to_png_bytes(image)?;
        let response = self
            .client
            .post(crate::gemini::generate_content_url(
                crate::gemini::DEFAULT_API_BASE,
                &self.model,
            ))
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .body(gemini_request_body(
                self.temperature,
                &png,
                source,
                target,
                guidance,
            ))
            .send()
            .await
            .map_err(|e| FusedError::new(format!("Gemini endpoint unreachable: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| FusedError::new(format!("Gemini endpoint read failed: {e}")))?;
        if !status.is_success() {
            // Same classification as the translate engine — the error is
            // about the endpoint (key, quota, model name), not the fusion.
            let error = crate::gemini::classify_error(status.as_u16(), &body);
            return Err(FusedError::new(error.message));
        }
        let text = crate::gemini::parse_generate_content_response(&body)
            .map_err(|e| FusedError::new(e.message))?;
        parse_blocks_response(&text)
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        "Gemini"
    }

    /// Every fused call posts a screen image to Google's API (ADR-0002).
    fn is_cloud(&self) -> bool {
        true
    }
}

/// The generic OpenAI-compatible vision engine: one adapter for OpenAI,
/// OpenRouter, LM Studio's local vision models, … (ticket 12). LM Studio
/// shares it under its own label, like [`OpenAiCompatibleEngine`] does for
/// translation.
pub struct OpenAiFusedEngine {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    temperature: f64,
    /// Display name and error prefix: "OpenAI-compatible" or "LM Studio".
    label: &'static str,
}

impl OpenAiFusedEngine {
    /// `base_url` includes the version segment (`…/v1`); `api_key` may be
    /// empty (local servers need none, and the Authorization header is then
    /// simply omitted — the translate adapter's rule).
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        temperature: f64,
        label: &'static str,
    ) -> Result<Self, FusedError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| FusedError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            temperature,
            label,
        })
    }

    /// The auth header only when a key is configured (same rule as the
    /// translate adapter).
    fn auth_header(&self) -> Option<(String, String)> {
        (!self.api_key.is_empty())
            .then(|| ("Authorization".to_string(), format!("Bearer {}", self.api_key)))
    }
}

#[async_trait]
impl FusedEngine for OpenAiFusedEngine {
    async fn recognize_and_translate(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<Vec<FusedBlock>, FusedError> {
        let png = bgra_to_png_bytes(image)?;
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Content-Type", "application/json")
            .body(openai_request_body(
                &self.model,
                self.temperature,
                &png,
                source,
                target,
                guidance,
            ));
        if let Some((name, value)) = self.auth_header() {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|e| {
            // Connection refused names the URL and the "is the server
            // running?" hint — ticket 10's first-class failure, identical
            // here (LM Studio's server is down just the same).
            FusedError::new(openai::unreachable_message(
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
            .map_err(|e| {
                FusedError::new(format!("{label} server read failed: {e}", label = self.label))
            })?;
        if !status.is_success() {
            let error = openai::classify_error(status.as_u16(), &body);
            return Err(FusedError::new(error.message));
        }
        let text =
            openai::parse_chat_completion(&body).map_err(|e| FusedError::new(e.message))?;
        parse_blocks_response(&text)
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        self.label
    }

    /// Whether screen images leave the machine follows the base URL — the
    /// translate adapter's rule: a loopback server stays local, anything
    /// else is cloud.
    fn is_cloud(&self) -> bool {
        !openai::is_local_url(&self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::guided;

    fn image(width: u32, height: u32, paint: impl FnOnce(&mut [u8])) -> CapturedImage {
        let mut image = CapturedImage {
            width,
            height,
            bgra: vec![0; (width * height * 4) as usize],
        };
        paint(&mut image.bgra);
        image
    }

    /// Sets pixel (x, y) to BGRA (0, 0, value, 255).
    fn paint(bgra: &mut [u8], width: u32, x: u32, y: u32, value: u8) {
        let i = ((y * width + x) * 4) as usize;
        bgra[i] = 0;
        bgra[i + 1] = 0;
        bgra[i + 2] = value;
        bgra[i + 3] = 255;
    }

    // ---- PNG encoding ----

    #[test]
    fn bgra_encodes_to_png_with_swapped_channels_and_opaque_alpha() {
        let img = image(2, 2, |bgra| {
            paint(bgra, 2, 0, 0, 200);
            paint(bgra, 2, 1, 1, 100);
            // An undefined alpha byte (GDI leaves it 0) must not produce a
            // transparent PNG.
            bgra[3] = 0;
            bgra[7] = 0;
        });
        let png_bytes = bgra_to_png_bytes(&img).unwrap();

        let decoder = png::Decoder::new(png_bytes.as_slice());
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(info.color_type, png::ColorType::Rgba);
        // Pixel (0, 0): red channel carries the BGRA blue byte (200).
        assert_eq!(&decoded[0..4], &[200, 0, 0, 255]);
        // Pixel (1, 1) — row stride 2 px, so offset 1*8 + 1*4.
        assert_eq!(&decoded[12..16], &[100, 0, 0, 255]);
    }

    // ---- Request shapes ----

    #[test]
    fn gemini_body_carries_system_contract_and_inline_image() {
        let img = image(4, 4, |_| {});
        let png = bgra_to_png_bytes(&img).unwrap();
        let body = gemini_request_body(
            0.2,
            &png,
            SourceLanguage::En,
            "th",
            &guided(&[("Vertin", "เวอร์ติน")], true),
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let system = value["systemInstruction"]["parts"][0]["text"].as_str().unwrap();
        assert!(system.contains("JSON array"), "was: {system}");
        assert!(system.contains("- Vertin → เวอร์ติน"), "was: {system}");
        assert!(system.contains("Genderless Mode"), "was: {system}");
        assert!(system.contains("(fused-prompt v2)"), "was: {system}");
        let parts = value["contents"][0]["parts"].as_array().unwrap();
        let inline = &parts[0]["inlineData"];
        assert_eq!(inline["mimeType"], "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(inline["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, png, "the base64 payload must round-trip the PNG");
        assert_eq!(value["generationConfig"]["temperature"], 0.2);
    }

    #[test]
    fn openai_body_carries_system_contract_and_image_data_url() {
        let img = image(4, 4, |_| {});
        let png = bgra_to_png_bytes(&img).unwrap();
        let body = openai_request_body(
            "gpt-4o-mini",
            0.2,
            &png,
            SourceLanguage::Ja,
            "th",
            &TranslationGuidance::plain(),
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["stream"], false);
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        let system = messages[0]["content"].as_str().unwrap();
        assert!(system.contains("JSON array"), "was: {system}");
        assert!(system.contains("Japanese"), "was: {system}");
        assert!(system.contains("(fused-prompt v2)"), "was: {system}");
        assert_eq!(messages[1]["role"], "user");
        let parts = messages[1]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        let encoded = url
            .strip_prefix("data:image/png;base64,")
            .expect("data URL shape");
        let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        assert_eq!(decoded, png, "the data URL must round-trip the PNG");
    }

    // ---- Response parsing ----

    #[test]
    fn parses_golden_success_array() {
        let body = include_str!("../fixtures/fused/blocks.json");
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(
            blocks,
            vec![
                FusedBlock {
                    block: TextBlock {
                        x: 12,
                        y: 34,
                        width: 200,
                        height: 24,
                        text: "Start".into(),
                    },
                    translation: "เริ่ม".into(),
                },
                FusedBlock {
                    block: TextBlock {
                        x: 12,
                        y: 70,
                        width: 310,
                        height: 24,
                        text: "Press ESC to exit".into(),
                    },
                    translation: "กด ESC เพื่อออก".into(),
                },
            ]
        );
    }

    #[test]
    fn strips_markdown_fences() {
        let body = include_str!("../fixtures/fused/fenced.txt");
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block.text, "Hello");
        assert_eq!(blocks[0].translation, "สวัสดี");
    }

    #[test]
    fn finds_the_array_inside_surrounding_prose() {
        let body = r#"Sure! Here are the text blocks I found:
[{"x": 0, "y": 0, "w": 50, "h": 10, "text": "Play", "th": "เล่น"}]
Hope that helps!"#;
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].translation, "เล่น");
    }

    #[test]
    fn width_height_and_translation_aliases_are_accepted() {
        let body = r#"[{"x": 1, "y": 2, "width": 30, "height": 12, "text": "Quest", "thai": "เควสต์"},
                       {"x": 3, "y": 4, "w": 8, "h": 8, "text": "Shop", "translation": "ร้านค้า"}]"#;
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block.width, 30);
        assert_eq!(blocks[0].translation, "เควสต์");
        assert_eq!(blocks[1].translation, "ร้านค้า");
    }

    #[test]
    fn integral_floats_are_accepted_for_boxes() {
        let body = r#"[{"x": 10.0, "y": 20.4, "w": 30.6, "h": 10.0, "text": "HP", "th": "พลังชีวิต"}]"#;
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(blocks[0].block, TextBlock {
            x: 10,
            y: 20,
            width: 31,
            height: 10,
            text: "HP".into(),
        });
    }

    #[test]
    fn invalid_entries_are_skipped_and_valid_ones_survive() {
        // A missing translation, a zero width, a missing box, an empty
        // text, and a stray non-object all drop; two good blocks stay.
        let body = r#"[
            {"x": 1, "y": 1, "w": 10, "h": 10, "text": "no translation"},
            {"x": 1, "y": 1, "w": 0, "h": 10, "text": "zero width", "th": "x"},
            {"x": 1, "y": 1, "text": "no box", "th": "x"},
            {"x": 1, "y": 1, "w": 10, "h": 10, "text": "   ", "th": "x"},
            "a stray string",
            {"x": 5, "y": 6, "w": 7, "h": 8, "text": "Keep", "th": "คงไว้"},
            {"x": -3, "y": -4, "w": 7, "h": 8, "text": "Clamped", "th": "คลัมป์"}
        ]"#;
        let blocks = parse_blocks_response(body).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block.text, "Keep");
        assert_eq!(blocks[1].block.x, 0, "negative coordinates clamp to 0");
        assert_eq!(blocks[1].block.y, 0);
    }

    #[test]
    fn empty_array_means_no_text_on_screen() {
        assert!(parse_blocks_response("[]").unwrap().is_empty());
        assert!(parse_blocks_response("```\n[]\n```").unwrap().is_empty());
    }

    #[test]
    fn reply_without_an_array_is_an_error_not_a_silent_pass() {
        let err = parse_blocks_response("I see no text in this image.").unwrap_err();
        assert!(err.message.contains("no JSON array"), "was: {}", err.message);
        let err = parse_blocks_response("[{broken").unwrap_err();
        assert!(
            err.message.contains("unterminated"),
            "was: {}",
            err.message
        );
    }

    #[test]
    fn an_object_wrapped_array_is_recovered_by_the_array_slice() {
        // The prose-locator (first `[` to last `]`) also rescues a model
        // that wrapped its array in an object.
        let blocks = parse_blocks_response(r#"{"blocks": [{"x": 1, "y": 2, "w": 3, "h": 4, "text": "Go", "th": "ไป"}]}"#)
            .unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].translation, "ไป");
    }

    // ---- Engine identity ----

    #[test]
    fn gemini_fused_engine_names_itself_and_badges_cloud() {
        let engine = GeminiFusedEngine::new("k", "gemini-2.0-flash", 0.2).unwrap();
        assert_eq!(engine.name(), "Gemini");
        assert!(engine.is_cloud());
    }

    #[test]
    fn openai_fused_engine_follows_the_local_url_rule() {
        let local = OpenAiFusedEngine::new(
            "http://localhost:1234/v1",
            "",
            "qwen2-vl",
            0.2,
            openai::LM_STUDIO_LABEL,
        )
        .unwrap();
        assert_eq!(local.name(), "LM Studio");
        assert!(!local.is_cloud(), "a local server must not show the ☁ badge");
        let cloud = OpenAiFusedEngine::new(
            "https://api.openai.com/v1",
            "sk",
            "gpt-4o-mini",
            0.2,
            openai::GENERIC_LABEL,
        )
        .unwrap();
        assert_eq!(cloud.name(), "OpenAI-compatible");
        assert!(cloud.is_cloud());
    }

    #[test]
    fn empty_key_sends_no_auth_header_on_the_fused_engine() {
        let engine = OpenAiFusedEngine::new("http://localhost:1234/v1", "  ", "m", 0.2, "LM Studio")
            .unwrap();
        assert!(engine.auth_header().is_none());
        let keyed = OpenAiFusedEngine::new("https://api.openai.com/v1", "sk-x", "m", 0.2, "x")
            .unwrap();
        assert_eq!(
            keyed.auth_header(),
            Some(("Authorization".to_string(), "Bearer sk-x".to_string()))
        );
    }
}
