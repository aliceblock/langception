//! The opt-in auto-switch wrapper (ticket 03, ADR-0006): while every
//! Google leg is cooling down, requests route to a user-chosen secondary
//! Translate Engine, and flow back the moment a Google leg recovers — no
//! loop restart, because this wrapper is built with the pipeline (the
//! shell cannot swap engines at runtime; that restarts the loop and
//! resets per-block tracking).
//!
//! Each switch episode announces through the shell-supplied
//! [`SwitchNotify`] — the same channels as engine errors (tray +
//! Results Window) — once on entering and once on recovering; a request
//! served during an episode announces nothing. Nothing switches
//! silently. The wrapper owns the GOOGLE side of the Translation Cache:
//! it consults it before routing (a cached Google answer still serves
//! while Google cools — a cache hit is free and needs no endpoint) and
//! records Google's fresh answers under the google key — with auto-switch
//! on, the chain itself carries no cache wrapper; this wrapper IS the
//! google side. Secondary answers are cached under the secondary engine's
//! own name by its own cache wrapper, never served later as Google ones.
//!
//! Engagement is the chain's all-legs-cooling state only: a healthy
//! Google that merely could not translate (its dictionary-extension leg
//! holding no entry) surfaces the error instead of switching — the same
//! distinction the chain itself draws between parking a leg and passing
//! on a response.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::google::GoogleTranslateEngine;
use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use crate::translate_cache::TranslationCache;
use async_trait::async_trait;

/// The cache key the shell's Google wrapper uses — the same key this
/// wrapper consults for the free pre-route lookup. The two must not
/// drift: one names the engine in the config, the other reads it.
const GOOGLE_CACHE_KEY: &str = "google";

/// What one switch transition looked like: the human message (tray +
/// Results Window) and the privacy-badge value now in force — `None`
/// when the engine currently serving is local, so the badge disappears
/// rather than naming a cloud engine that isn't running.
pub struct SwitchEvent {
    pub message: String,
    pub badge: Option<String>,
}

/// The shell-supplied announcement port.
pub type SwitchNotify = Arc<dyn Fn(SwitchEvent) + Send + Sync>;

pub struct AutoSwitchEngine {
    google: Arc<GoogleTranslateEngine>,
    secondary: Arc<dyn TranslateEngine>,
    cache: Arc<TranslationCache>,
    serving_secondary: AtomicBool,
    /// Whether the last routed request went to the secondary — the
    /// provenance probe delegates to it (its own cache wrapper reports);
    /// the google side has no wrapper when auto-switch is on, so this
    /// wrapper keeps that flag itself.
    last_via_secondary: AtomicBool,
    /// Whether the google side answered from the google cache.
    google_hit: AtomicBool,
    on_switch: SwitchNotify,
}

impl AutoSwitchEngine {
    pub fn new(
        google: Arc<GoogleTranslateEngine>,
        secondary: Arc<dyn TranslateEngine>,
        cache: Arc<TranslationCache>,
        on_switch: SwitchNotify,
    ) -> Self {
        Self {
            google,
            secondary,
            cache,
            serving_secondary: AtomicBool::new(false),
            last_via_secondary: AtomicBool::new(false),
            google_hit: AtomicBool::new(false),
            on_switch,
        }
    }

    /// The badge value for the engine now taking over: a cloud engine
    /// keeps the badge naming it, a local one removes the badge.
    fn secondary_badge(&self) -> Option<String> {
        if self.secondary.is_cloud() {
            Some(self.secondary.name().to_string())
        } else {
            None
        }
    }

    /// Announces the return to Google — only when an episode was running,
    /// and only once (the atomic flips back).
    fn announce_recovery(&self) {
        if self.serving_secondary.swap(false, Ordering::SeqCst) {
            (self.on_switch)(SwitchEvent {
                message: "Google endpoints recovered — translating via Google again (ADR-0006)"
                    .to_string(),
                badge: Some(self.google.name().to_string()),
            });
        }
    }
}

#[async_trait]
impl TranslateEngine for AutoSwitchEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let sl = source.code();
        if let Some(hit) = self.cache.lookup(GOOGLE_CACHE_KEY, sl, target, text) {
            self.google_hit.store(true, Ordering::SeqCst);
            self.last_via_secondary.store(false, Ordering::SeqCst);
            return Ok(hit);
        }
        self.google_hit.store(false, Ordering::SeqCst);
        if !self.google.all_cooling() {
            match self.google.translate(text, source, target).await {
                Ok(translated) => {
                    self.announce_recovery();
                    self.last_via_secondary.store(false, Ordering::SeqCst);
                    self.cache
                        .store(GOOGLE_CACHE_KEY, sl, target, text, translated.clone());
                    return Ok(translated);
                }
                Err(google_error) => {
                    if !self.google.all_cooling() {
                        // Google answered healthy but could not translate
                        // (its no-entry shapes): surface, never switch.
                        return Err(google_error);
                    }
                    // Every leg parked DURING this request — the first
                    // request of an outage still gets the secondary.
                }
            }
        }
        if !self.serving_secondary.swap(true, Ordering::SeqCst) {
            (self.on_switch)(SwitchEvent {
                message: format!(
                    "Google is rate-limited — translating via {} until a Google endpoint recovers (ADR-0006)",
                    self.secondary.name()
                ),
                badge: self.secondary_badge(),
            });
        }
        self.last_via_secondary.store(true, Ordering::SeqCst);
        self.secondary.translate(text, source, target).await
    }

    /// Same routing as [`Self::translate`]; the guidance must reach
    /// whichever engine actually serves (an LLM secondary folds the
    /// Glossary injection, Genderless instruction, and Translation
    /// Context into its prompt there — the port's default would strip
    /// it, the same trap the cache wrapper avoids).
    async fn translate_guided(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
        guidance: &crate::ports::TranslationGuidance,
    ) -> Result<String, TranslateError> {
        let sl = source.code();
        if let Some(hit) = self.cache.lookup(GOOGLE_CACHE_KEY, sl, target, text) {
            self.google_hit.store(true, Ordering::SeqCst);
            self.last_via_secondary.store(false, Ordering::SeqCst);
            return Ok(hit);
        }
        self.google_hit.store(false, Ordering::SeqCst);
        if !self.google.all_cooling() {
            match self.google.translate_guided(text, source, target, guidance).await {
                Ok(translated) => {
                    self.announce_recovery();
                    self.last_via_secondary.store(false, Ordering::SeqCst);
                    self.cache
                        .store(GOOGLE_CACHE_KEY, sl, target, text, translated.clone());
                    return Ok(translated);
                }
                Err(google_error) => {
                    if !self.google.all_cooling() {
                        return Err(google_error);
                    }
                }
            }
        }
        if !self.serving_secondary.swap(true, Ordering::SeqCst) {
            (self.on_switch)(SwitchEvent {
                message: format!(
                    "Google is rate-limited — translating via {} until a Google endpoint recovers (ADR-0006)",
                    self.secondary.name()
                ),
                badge: self.secondary_badge(),
            });
        }
        self.last_via_secondary.store(true, Ordering::SeqCst);
        self.secondary
            .translate_guided(text, source, target, guidance)
            .await
    }

    /// The privacy badge names Google at build time and follows the
    /// switches through [`SwitchEvent::badge`]; the engine as a whole is
    /// cloud because Google is.
    fn name(&self) -> &str {
        self.google.name()
    }

    fn is_cloud(&self) -> bool {
        true
    }

    /// A google-side answer's provenance is this wrapper's own flag (it
    /// IS the google cache); a secondary-side answer delegates to the
    /// secondary's cache wrapper.
    fn last_answer_cached(&self) -> bool {
        if self.last_via_secondary.load(Ordering::SeqCst) {
            self.secondary.last_answer_cached()
        } else {
            self.google_hit.load(Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::LegTransport;
    use crate::ports::Clock;
    use crate::test_support::{
        no_translation_body, ok_gtx, rate_limited, server_error, FakeClock, ScriptedTransport,
    };
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    /// A counting secondary: records which path it was entered by, and
    /// can serve from a cache-wrapped form in the tests that need it.
    struct CountingSecondary {
        plain_calls: AtomicUsize,
        guided_calls: AtomicUsize,
    }
    impl CountingSecondary {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                plain_calls: AtomicUsize::new(0),
                guided_calls: AtomicUsize::new(0),
            })
        }
        fn plain_calls(&self) -> usize {
            self.plain_calls.load(Ordering::SeqCst)
        }
        fn guided_calls(&self) -> usize {
            self.guided_calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl TranslateEngine for CountingSecondary {
        async fn translate(
            &self,
            _text: &str,
            _source: SourceLanguage,
            _target: &str,
        ) -> Result<String, TranslateError> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            Ok("คำแปลจากตัวสำรอง".into())
        }
        async fn translate_guided(
            &self,
            _text: &str,
            _source: SourceLanguage,
            _target: &str,
            _guidance: &crate::ports::TranslationGuidance,
        ) -> Result<String, TranslateError> {
            self.guided_calls.fetch_add(1, Ordering::SeqCst);
            Ok("คำแปลจากตัวสำรอง".into())
        }
        fn name(&self) -> &str {
            "Gemini (test)"
        }
        fn is_cloud(&self) -> bool {
            true
        }
    }

    fn recording_notify() -> (SwitchNotify, Arc<Mutex<Vec<SwitchEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let notify: SwitchNotify = Arc::new(move |event| {
            sink.lock().unwrap().push(event);
        });
        (notify, events)
    }

    fn wrapper(
        script: Arc<ScriptedTransport>,
        secondary: Arc<dyn TranslateEngine>,
        cache: Arc<TranslationCache>,
        events: Arc<Mutex<Vec<SwitchEvent>>>,
        clock: Arc<FakeClock>,
    ) -> (AutoSwitchEngine, Arc<GoogleTranslateEngine>) {
        let google = Arc::new(GoogleTranslateEngine::with_parts(
            script as Arc<dyn LegTransport>,
            Arc::clone(&clock) as Arc<dyn Clock>,
        ));
        let on_switch: SwitchNotify = Arc::new(move |event| {
            events.lock().unwrap().push(event);
        });
        let engine = AutoSwitchEngine::new(
            Arc::clone(&google),
            secondary,
            cache,
            on_switch,
        );
        (engine, google)
    }

    #[tokio::test]
    async fn a_warm_google_serves_and_announces_nothing() {
        let script = ScriptedTransport::new(vec![ok_gtx()]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let (engine, _) = wrapper(
            Arc::clone(&script),
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            TranslationCache::transient(),
            Arc::clone(&events),
            Arc::clone(&clock),
        );
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("google warm");
        assert!(!out.is_empty());
        assert_eq!(secondary.plain_calls(), 0);
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn all_cooling_routes_to_the_secondary_and_announces_the_episode() {
        let script = ScriptedTransport::new(vec![rate_limited(), rate_limited(), server_error()]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let (engine, _) = wrapper(
            Arc::clone(&script),
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            TranslationCache::transient(),
            Arc::clone(&events),
            Arc::clone(&clock),
        );
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("the secondary answers while google cools");
        assert_eq!(out, "คำแปลจากตัวสำรอง");
        assert_eq!(secondary.plain_calls(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "one announcement for the episode start");
        assert!(
            events[0].message.contains("rate-limited"),
            "the message must say why: {:?}",
            events[0].message
        );
        assert_eq!(events[0].badge.as_deref(), Some("Gemini (test)"));
    }

    #[tokio::test]
    async fn recovery_announces_once_and_serving_during_an_episode_is_silent() {
        let script = ScriptedTransport::new(vec![
            rate_limited(), // t=0: all three legs park
            rate_limited(),
            server_error(),
            ok_gtx(), // t=61s: google warm again — serves
            ok_gtx(), // t=62s: still warm — nothing announced
        ]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let (engine, _) = wrapper(
            Arc::clone(&script),
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            TranslationCache::transient(),
            Arc::clone(&events),
            Arc::clone(&clock),
        );
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("episode: secondary serves");
        clock.advance(61_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("recovered: google serves");
        clock.advance(1_000);
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("still google");
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2, "enter + recover only");
        assert!(events[1].message.contains("recovered"));
        assert_eq!(events[1].badge.as_deref(), Some("Google Translate (unofficial)"));
    }

    #[tokio::test]
    async fn the_google_cache_serves_even_while_google_cools() {
        let script = ScriptedTransport::new(vec![
            ok_gtx(),       // primes the google cache entry
            rate_limited(), // then all three legs park
            rate_limited(),
            server_error(),
        ]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let cache = TranslationCache::transient();
        let (engine, _) = wrapper(Arc::clone(&script), Arc::clone(&secondary) as Arc<dyn TranslateEngine>, Arc::clone(&cache), Arc::clone(&events), Arc::clone(&clock));
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("first call primes the cache");
        clock.advance(1_000); // both google legs now cooling
        let out = engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("the cached google answer serves through the outage");
        assert!(!out.is_empty());
        assert_eq!(secondary.plain_calls(), 0, "no secondary call for a cache hit");
        assert!(
            events.lock().unwrap().is_empty(),
            "a cache hit is not a switch episode"
        );
    }

    #[tokio::test]
    async fn a_healthy_but_useless_google_failure_does_not_switch() {
        // gtx and chrome-ex park, but the dictionary extension answered
        // healthy-with-no entry: google is not "all cooling", so the
        // error surfaces and the secondary stays untouched.
        let script = ScriptedTransport::new(vec![rate_limited(), rate_limited(), no_translation_body()]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let (engine, _) = wrapper(
            Arc::clone(&script),
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            TranslationCache::transient(),
            Arc::clone(&events),
            Arc::clone(&clock),
        );
        let out = engine.translate("Hello", SourceLanguage::En, "th").await;
        assert!(out.is_err(), "the google error surfaces");
        assert_eq!(secondary.plain_calls(), 0);
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_guided_path_reaches_the_secondary_with_its_guidance() {
        let script = ScriptedTransport::new(vec![rate_limited(), rate_limited(), server_error()]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let (engine, _) = wrapper(
            Arc::clone(&script),
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            TranslationCache::transient(),
            Arc::clone(&events),
            Arc::clone(&clock),
        );
        let guidance = crate::test_support::guided(&[("dragon", "มังกร")], true);
        let out = engine
            .translate_guided("Hello", SourceLanguage::En, "th", &guidance)
            .await
            .expect("the secondary answers");
        assert_eq!(out, "คำแปลจากตัวสำรอง");
        assert_eq!(secondary.guided_calls(), 1, "guided, not plain");
    }

    #[tokio::test]
    async fn secondary_answers_cache_under_the_secondary_engine_name() {
        let script = ScriptedTransport::new(vec![rate_limited(), rate_limited(), server_error()]);
        let secondary = CountingSecondary::new();
        let (_notify, events) = recording_notify();
        let clock = FakeClock::at(0);
        let cache = TranslationCache::transient();
        // The shell cache-wraps the secondary with its own engine key —
        // reproduce that here, or nothing stores under its name.
        let secondary_wrapped = crate::translate_cache::CachingTranslateEngine::new(
            Arc::clone(&secondary) as Arc<dyn TranslateEngine>,
            "Gemini (test)",
            Arc::clone(&cache),
        );
        let (engine, _) = wrapper(Arc::clone(&script), Arc::new(secondary_wrapped) as Arc<dyn TranslateEngine>, Arc::clone(&cache), Arc::clone(&events), Arc::clone(&clock));
        engine
            .translate("Hello", SourceLanguage::En, "th")
            .await
            .expect("episode serves");
        assert_eq!(
            cache.lookup("Gemini (test)", "en", "th", "Hello"),
            Some("คำแปลจากตัวสำรอง".to_string()),
            "cached under the producing engine"
        );
        assert_eq!(cache.lookup("google", "en", "th", "Hello"), None);
    }
}
