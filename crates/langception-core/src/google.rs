//! The unofficial Google Translate endpoint engine (the zero-config
//! default, see ADR-0002). It is ToS-gray and can rate-limit or break:
//! every failure surfaces as a [`TranslateError`] for visible display.
//!
//! Ticket 02 (ADR-0006): the engine is a CHAIN of legs into Google's
//! translator, tried in order, each with its own cool-down — gtx first
//! (the most durable but most-abused endpoint), then the Chrome
//! Dictionary extension endpoint (a different, quieter quota pool). A
//! third, mobile-web leg was planned but cut before it shipped: scraping
//! the answer out of an HTML page without an HTML parser is more
//! fragility than two legs already buy, and the ADR's consequences cover
//! retiring legs. A leg that answers 429/5xx, is unreachable, or returns
//! a shape we cannot parse parks itself for a minute, doubling per repeat
//! hit up to ten minutes; the request is served by the next warm leg, so
//! the sequential loop never sleeps inside a pass. An error reaches the
//! user only when every leg is cooling.
//!
//! Response parsing is tested offline against golden recorded responses
//! in `fixtures/google/` (no network in tests). The live endpoints are
//! covered by a manual smoke checklist.

use std::sync::{Arc, Mutex};

use crate::orchestrator::SystemClock;
use crate::ports::{Clock, SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;

/// The free, keyless endpoint. See ADR-0002 before "fixing" this default.
const DEFAULT_ENDPOINT: &str = "https://translate.googleapis.com";

/// The Chrome Dictionary extension endpoint: Google's own browser
/// extension ships this key publicly (anyone can extract it from the
/// extension), and answering as that extension is ToS-gray one step
/// beyond gtx. ADR-0006 accepts the trade — a burned key parks this leg
/// and the chain degrades gracefully to gtx alone.
const DICTIONARY_EXTENSION_ENDPOINT: &str =
    "https://dictionaryextension-pa.googleapis.com/v1/dictionaryExtensionData";
const DICTIONARY_EXTENSION_KEY: &str = "AIzaSyA6EEtrDCfBkHV8uU2lgGY-N383ZgAOo7Y";
const DICTIONARY_EXTENSION_REFERER: &str =
    "chrome-extension://mgijmajocgfcbeboacabfgobmjgjcoja";
/// The dictionary-extension leg answers as a mobile Chrome client, the
/// shape that endpoint serves.
const DICTIONARY_EXTENSION_UA: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.6998.108 Mobile Safari/537.36";
/// The gtx leg's honest UA: our app, versioned.
const GTX_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Langception/0.1";

/// Cool-down policy (ADR-0006): the first strike parks a leg for a
/// minute, each further strike doubles the wait, ten minutes is the cap.
/// A served request resets the ladder — the leg proved itself healthy
/// again, so the next outage starts from one minute, not from where the
/// last outage left off.
const COOLDOWN_BASE_MS: u64 = 60_000;
const COOLDOWN_CAP_MS: u64 = 600_000;

fn cooldown_window_ms(strikes: u32) -> u64 {
    let shift = strikes.saturating_sub(1).min(20);
    COOLDOWN_BASE_MS
        .checked_shl(shift)
        .unwrap_or(COOLDOWN_CAP_MS)
        .min(COOLDOWN_CAP_MS)
}

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

/// How a dictionary-extension response failed to give a translation.
#[derive(Debug, PartialEq, Eq)]
pub enum DictExtError {
    /// The body is not the expected JSON shape — the endpoint moved or
    /// the key burned. Park-worthy: the leg is not healthy.
    Shape,
    /// The documented healthy-but-empty answer (`{"status": 404}`): the
    /// leg is fine, this term just has no entry — not park-worthy.
    NoTranslation,
}

/// Parses a `dictionaryExtensionData` response body and returns the
/// translated text. Golden shape: `{"status": 200, "translateResponse":
/// {"translateText": "...", "detectedSourceLanguage": "en", ...}}` — the
/// extension endpoint detects the source language itself (no source
/// parameter exists), so `sl` never rides the request.
pub fn parse_dictionary_extension(body: &str) -> Result<String, DictExtError> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| DictExtError::Shape)?;
    let status = value.get("status").and_then(|v| v.as_i64());
    let translated = value
        .get("translateResponse")
        .and_then(|v| v.get("translateText"))
        .and_then(|v| v.as_str());
    match (status, translated) {
        (Some(200), Some(text)) if !text.is_empty() => Ok(text.to_string()),
        (Some(404), None) => Err(DictExtError::NoTranslation),
        _ => Err(DictExtError::Shape),
    }
}

/// One response from a leg endpoint.
pub(crate) struct LegResponse {
    pub status: u16,
    pub body: String,
}

/// The HTTP seam under the leg chain: production speaks reqwest, the
/// tests script responses per call. Headers and query arrive fully built
/// from the leg.
#[async_trait]
pub(crate) trait LegTransport: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<LegResponse, String>;
}

struct ReqwestTransport {
    client: reqwest::Client,
}

#[async_trait]
impl LegTransport for ReqwestTransport {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<LegResponse, String> {
        let mut request = self.client.get(url).query(query);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("endpoint unreachable: {e}"))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| format!("endpoint read failed: {e}"))?;
        Ok(LegResponse { status, body })
    }
}

/// One endpoint into Google's translator, with its cool-down ledger.
struct Leg {
    kind: LegKind,
    cooldown: Mutex<Cooldown>,
}

enum LegKind {
    Gtx,
    DictionaryExtension,
}

#[derive(Default)]
struct Cooldown {
    strikes: u32,
    next_ok_ms: u64,
}

/// What one leg attempt came back with. `Park` marks the leg unhealthy
/// (rate limit, outage, unreachable, shape change); `Pass` marks a
/// healthy leg that could not translate this text.
enum LegOutcome {
    Translated(String),
    Park(String),
    Pass(String),
}

impl Leg {
    /// Whether the leg's cool-down window has passed.
    fn available(&self, now_ms: u64) -> bool {
        self.cooldown
            .lock()
            .expect("google leg lock")
            .next_ok_ms <= now_ms
    }

    /// Parks the leg for a window that doubles with each consecutive
    /// strike, capped at [`COOLDOWN_CAP_MS`].
    fn park(&self, now_ms: u64) {
        let mut cooldown = self.cooldown.lock().expect("google leg lock");
        cooldown.strikes += 1;
        cooldown.next_ok_ms = now_ms + cooldown_window_ms(cooldown.strikes);
    }

    /// A served request clears the ladder — the leg proved healthy.
    fn reset(&self) {
        *self.cooldown.lock().expect("google leg lock") = Cooldown::default();
    }
}

impl LegKind {
    fn name(&self) -> &'static str {
        match self {
            Self::Gtx => "gtx",
            Self::DictionaryExtension => "dictionary extension",
        }
    }

    async fn request(
        &self,
        transport: &dyn LegTransport,
        text: &str,
        sl: &str,
        tl: &str,
    ) -> LegOutcome {
        let (url, headers, query) = match self {
            Self::Gtx => (
                format!("{DEFAULT_ENDPOINT}/translate_a/single"),
                vec![("User-Agent", GTX_UA)],
                vec![
                    ("client", "gtx"),
                    ("sl", sl),
                    ("tl", tl),
                    ("dt", "t"),
                    ("q", text),
                ],
            ),
            Self::DictionaryExtension => (
                DICTIONARY_EXTENSION_ENDPOINT.to_string(),
                vec![
                    ("User-Agent", DICTIONARY_EXTENSION_UA),
                    ("x-referer", DICTIONARY_EXTENSION_REFERER),
                ],
                vec![
                    ("language", tl),
                    ("key", DICTIONARY_EXTENSION_KEY),
                    ("term", text),
                    ("strategy", "2"),
                ],
            ),
        };
        let leg = self.name();
        let response = match transport.get(&url, &headers, &query).await {
            Ok(response) => response,
            Err(e) => {
                return LegOutcome::Park(format!(
                    "Google [{leg}] endpoint unreachable: {e}"
                ));
            }
        };
        if !(200..300).contains(&response.status) {
            return LegOutcome::Park(format!(
                "Google [{}] endpoint returned HTTP {} (rate limit or outage; see ADR-0002/0006)",
                leg, response.status
            ));
        }
        match self {
            Self::Gtx => match parse_translation_response(&response.body) {
                Ok(translated) => LegOutcome::Translated(translated),
                Err(e) => LegOutcome::Park(format!("Google [gtx] {e}")),
            },
            Self::DictionaryExtension => match parse_dictionary_extension(&response.body) {
                Ok(translated) => LegOutcome::Translated(translated),
                Err(DictExtError::Shape) => LegOutcome::Park(
                    "Google [dictionary extension] response has an unexpected shape (endpoint moved or key burned; see ADR-0006)".to_string(),
                ),
                Err(DictExtError::NoTranslation) => LegOutcome::Pass(
                    "Google [dictionary extension] holds no entry for this text".to_string(),
                ),
            },
        }
    }
}

/// The default Translate Engine: a chain of legs into the unofficial
/// Google endpoint, no key needed from the user.
pub struct GoogleTranslateEngine {
    legs: Vec<Leg>,
    transport: Arc<dyn LegTransport>,
    clock: Arc<dyn Clock>,
}

impl GoogleTranslateEngine {
    pub fn new() -> Result<Self, TranslateError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| TranslateError::new(format!("failed to build HTTP client: {e}")))?;
        Ok(Self::with_parts(
            Arc::new(ReqwestTransport { client }),
            Arc::new(SystemClock),
        ))
    }

    /// The test seam: a scripted transport and a movable clock, so the
    /// chain and its cool-downs run offline and instantly.
    pub(crate) fn with_parts(transport: Arc<dyn LegTransport>, clock: Arc<dyn Clock>) -> Self {
        Self {
            legs: vec![
                Leg {
                    kind: LegKind::Gtx,
                    cooldown: Mutex::new(Cooldown::default()),
                },
                Leg {
                    kind: LegKind::DictionaryExtension,
                    cooldown: Mutex::new(Cooldown::default()),
                },
            ],
            transport,
            clock,
        }
    }

    /// Whether every leg is parked right now — the auto-switch wrapper's
    /// engagement signal (ADR-0006): a chain in this state cannot serve a
    /// request from any Google endpoint.
    pub(crate) fn all_cooling(&self) -> bool {
        let now = self.clock.now_ms();
        self.legs.iter().all(|leg| !leg.available(now))
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
        let sl = source.code();
        let mut last: Option<TranslateError> = None;
        let mut all_cooling = true;
        for leg in &self.legs {
            if !leg.available(self.clock.now_ms()) {
                continue;
            }
            all_cooling = false;
            match leg.kind.request(self.transport.as_ref(), text, sl, target).await {
                LegOutcome::Translated(translated) => {
                    leg.reset();
                    return Ok(translated);
                }
                LegOutcome::Park(message) => {
                    leg.park(self.clock.now_ms());
                    last = Some(TranslateError::new(message));
                }
                LegOutcome::Pass(message) => {
                    last = Some(TranslateError::new(message));
                }
            }
        }
        if all_cooling {
            return Err(TranslateError::new(
                "every Google endpoint leg is cooling down after rate limiting — translation retries when the text changes (ADR-0006)",
            ));
        }
        Err(last
            .unwrap_or_else(|| TranslateError::new("Google chain produced no answer")))
    }

    /// The privacy badge names this engine while it is active.
    fn name(&self) -> &str {
        "Google Translate (unofficial)"
    }

    /// Every translation posts screen text to Google's endpoint (ADR-0002).
    fn is_cloud(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    //! The leg chain + cool-down (ticket 02, ADR-0006) and the response
    //! parsers, against a scripted transport and a movable clock — no
    //! network, no waiting. The harness lives in `test_support`.

    use super::*;
    use crate::ports::Clock;
    use crate::test_support::{
        no_translation_body, ok_dictext, ok_gtx, server_error, status, FakeClock,
        ScriptedTransport,
    };
    use std::sync::Arc;

    fn engine(
        transport: Arc<ScriptedTransport>,
        clock: Arc<FakeClock>,
    ) -> GoogleTranslateEngine {
        GoogleTranslateEngine::with_parts(
            transport as Arc<dyn LegTransport>,
            clock as Arc<dyn Clock>,
        )
    }

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

    #[test]
    fn parses_the_dictionary_extension_golden_response() {
        let body = include_str!("../fixtures/google/dictionaryextension.json");
        assert_eq!(
            parse_dictionary_extension(body).unwrap(),
            "Supondo que os associados de Rishushu na Companhia Comercial Baert estejam de posse do mesmo documento, suspeito que o próximo ataque à nossa caravana de suprimentos ocorrerá bem ali, no Sudário Central, a sudoeste do Portão do Lobo Branco."
        );
    }

    #[test]
    fn dictionary_extension_malformed_is_a_shape_error() {
        assert!(matches!(
            parse_dictionary_extension("<html>blocked</html>"),
            Err(DictExtError::Shape)
        ));
        assert!(matches!(
            parse_dictionary_extension("{\"status\": 500}"),
            Err(DictExtError::Shape)
        ));
    }

    #[test]
    fn dictionary_extension_404_shape_is_no_translation() {
        assert!(matches!(
            parse_dictionary_extension("{\"status\": 404}"),
            Err(DictExtError::NoTranslation)
        ));
    }

    #[tokio::test]
    async fn a_5xx_or_an_unreachable_leg_still_serves_from_the_next() {
        let script = ScriptedTransport::new(vec![
            server_error(), // t=0    gtx: a 5xx parks the same as a 429
            ok_dictext(),   // t=0    dictext serves
            ok_dictext(),   // t=59s  gtx still down; dictext serves
            Err("connection refused".into()), // t=61s gtx: unreachable parks too
            ok_dictext(),   // t=61s  dictext serves
            ok_dictext(),   // t=120s gtx still down (strike-2 window); dictext serves
        ]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("a 5xx falls through to the next leg");
        assert!(!out.is_empty());
        clock.advance(59_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("second call");
        clock.advance(2_000); // t=61s: the 5xx window expired — the leg is tried again
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("an unreachable transport parks the leg the same way");
        clock.advance(59_000); // t=120s: strike-2 window (until 181s) still holds
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("third call");
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext", "dictext", "gtx", "dictext", "dictext"],
            "both the 5xx and the unreachable shapes park the gtx leg"
        );
    }


    #[tokio::test]
    async fn a_rate_limited_leg_serves_from_the_next() {
        let script = ScriptedTransport::new(vec![status(429), ok_dictext()]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("the chain must answer from the second leg");
        assert!(out.starts_with("Supondo que os associados"));
        assert_eq!(script.contacted(), ["gtx", "dictext"]);
    }

    #[tokio::test]
    async fn a_cooling_leg_is_skipped_until_its_window_expires() {
        let script = ScriptedTransport::new(vec![
            status(429), // t=0: gtx parks until t=60_000
            ok_dictext(),
            ok_dictext(), // t=59_000: gtx still cooling — only dictext runs
            ok_gtx(),     // t=61_000: gtx's window expired — it is first again
        ]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("first call");
        assert_eq!(script.contacted(), ["gtx", "dictext"]);

        clock.advance(59_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("second call");
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext", "dictext"],
            "at t=59s the cooling gtx leg must receive no request"
        );

        clock.advance(2_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("third call");
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext", "dictext", "gtx"],
            "at t=61s the gtx leg is warm again"
        );
    }

    #[tokio::test]
    async fn repeat_hits_double_the_window_and_cap_at_ten_minutes() {
        // Consecutive strikes (no success in between — a success resets the
        // ladder): 60s → 120s → 240s → 480s → capped at 600s.
        // Pop order = request order: legs try gtx first, then dictext.
        let script = ScriptedTransport::new(vec![
            status(429),  // t=0    gtx: strike 1 → down until t=60s
            ok_dictext(), // t=0    dictext: serves
            status(429),  // t=61s  gtx: strike 2 → down until t=181s
            ok_dictext(), // t=61s  dictext: serves
            status(429),  // t=182s gtx: strike 3 → down until t=422s
            ok_dictext(), // t=182s dictext: serves
            status(429),  // t=423s gtx: strike 4 → down until t=903s
            ok_dictext(), // t=423s dictext: serves
            status(429),  // t=904s gtx: strike 5 → 960s capped at 600 → t=1504s
            ok_dictext(), // t=904s dictext: serves
            ok_dictext(), // t=1000s gtx still capped-down; dictext serves
        ]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=0");
        clock.advance(61_000);
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=61s");
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext", "gtx", "dictext"],
            "strike 2 parks gtx until t=181s"
        );
        clock.advance(121_000); // t=182s — the 120s window expired
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=182s");
        assert_eq!(
            script.contacted().len(),
            6,
            "the window expired: gtx fires (and strikes again), dictext serves"
        );
        clock.advance(241_000); // t=423s — strike-3 window (until 422s) expired
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=423s");
        assert_eq!(script.contacted().len(), 8, "strike 4 fires");
        clock.advance(481_000); // t=904s — strike-4 window (until 903s) expired
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=904s");
        assert_eq!(script.contacted().len(), 10, "strike 5 fires, capped");
        clock.advance(96_000); // t=1000s — the capped 600s window holds until 1504s
        engine.translate("Hello", SourceLanguage::En, "th").await.expect("t=1000s");
        assert_eq!(
            script.contacted().last(),
            Some(&"dictext"),
            "the cap must hold gtx down well past the doubled windows"
        );
        assert_eq!(script.contacted().len(), 11, "gtx stayed parked under the cap");
    }

    #[tokio::test]
    async fn every_leg_cooling_is_an_error_with_no_requests() {
        let script = ScriptedTransport::new(vec![status(429), status(500)]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        let first = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await;
        assert!(first.is_err(), "both legs parked: the request fails");
        assert_eq!(script.contacted(), ["gtx", "dictext"]);

        let second = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await;
        assert!(second.is_err());
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext"],
            "the all-cooling path must fire no requests at all"
        );
        let message = second.unwrap_err().message;
        assert!(
            message.contains("cooling"),
            "the error must name the cool-down state: {message}"
        );
    }

    #[tokio::test]
    async fn a_malformed_response_parks_the_leg_like_a_rate_limit() {
        let script = ScriptedTransport::new(vec![
            Ok(LegResponse {
                status: 200,
                body: "<html>blocked</html>".into(),
            }),
            ok_dictext(),
            ok_dictext(), // next call: gtx parked — only dictext runs
        ]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("the next leg answers");
        assert!(!out.is_empty());
        clock.advance(59_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("second call");
        assert_eq!(
            script.contacted(),
            ["gtx", "dictext", "dictext"],
            "a shape change parks the leg the same as a rate limit"
        );
    }

    #[tokio::test]
    async fn a_no_translation_answer_parks_nothing() {
        // The dictionary extension answers healthy-but-empty for this term
        // (its documented 404 shape): the leg is fine, so the next call
        // must try it again instead of waiting out a cool-down.
        let script = ScriptedTransport::new(vec![
            status(429),
            no_translation_body(),
            ok_dictext(),
        ]);
        let clock = FakeClock::at(0);
        let engine = engine(Arc::clone(&script), Arc::clone(&clock));
        let out = engine.translate("Hello", SourceLanguage::En, "th").await;
        assert!(out.is_err(), "no leg could translate: an error surfaces");
        clock.advance(1_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("the healthy-but-empty leg is warm and serves the retry");
        assert_eq!(script.contacted(), ["gtx", "dictext", "dictext"]);
    }
}
