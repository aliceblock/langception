//! The Translation Cache (ticket: google-rate-limit-resilience 01;
//! ADR-0006). A store of earlier Translate Engine outputs reused whenever
//! the same text is translated again for the same language pair and
//! engine, so the free default engine (ADR-0002) stops paying one request
//! per repeat.
//!
//! Placement is fixed by the loop: the orchestrator consults the Glossary
//! first and only then calls the engine, so a caching wrapper sits BELOW
//! the Glossary by construction — a Glossary match never reaches it. The
//! store keeps RAW engine output: Genderless Mode scrubs per serve inside
//! the loop, so a settings flip applies to cached answers the same as
//! fresh ones. LLM guidance (preset, custom prompt, Translation Context)
//! is deliberately NOT part of the key — a text keeps the translation it
//! first received, which is what a cache is for; Clear Translation Cache
//! in Settings is the escape hatch.
//!
//! Persistence is a JSON file beside the config (the app's only other
//! persistence surface is config.toml), written on a short debounce so a
//! burst of captures costs one write, bounded to [`DEFAULT_CAP`] entries
//! with least-recently-used eviction. A missing or corrupt file boots as
//! an empty store — a cache must never take the app down.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ports::{SourceLanguage, TranslateEngine, TranslateError};
use async_trait::async_trait;

/// How long the debounced flush waits after the last store before it
/// touches the disk.
const FLUSH_DELAY: Duration = Duration::from_secs(2);

/// The steady-state entry cap: five thousand short lines is a few hundred
/// KB of JSON — small enough to rewrite wholesale, large enough that a
/// long play session never notices it.
const DEFAULT_CAP: usize = 5_000;

/// One persisted entry. Field order in the JSON is the load contract; the
/// version field on the file guards against future shape changes.
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    engine: String,
    sl: String,
    tl: String,
    text: String,
    translation: String,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<CacheEntry>,
}

/// Map key: everything that identifies a translation. Strings, not enums,
/// so the file format stays engine-agnostic.
type CacheKey = (String, String, String, String);

struct Inner {
    map: HashMap<CacheKey, String>,
    /// Oldest first — the eviction order and the file's write order.
    order: Vec<CacheKey>,
    cap: usize,
}

/// The shared Translation Cache. One instance lives in app state for the
/// whole process; every pipeline build wraps its engines with the same
/// `Arc`, so a restart of the loop (engine change, settings save) keeps
/// the memory warm and only a full app restart starts from the file.
pub struct TranslationCache {
    inner: Mutex<Inner>,
    /// `None` = transient (tests, build-time validation): no file, no
    /// flush thread, everything lives and dies in memory.
    path: Option<PathBuf>,
    file_lock: Mutex<()>,
    dirty: AtomicBool,
    flushing: AtomicBool,
}

impl TranslationCache {
    /// Opens (or creates) the persisted cache at `path`. A missing or
    /// corrupt file boots as an empty store.
    pub fn open(path: PathBuf) -> Arc<Self> {
        Arc::new(Self::build(Some(path), DEFAULT_CAP))
    }

    /// An in-memory-only cache: tests and build-time validation.
    pub fn transient() -> Arc<Self> {
        Arc::new(Self::build(None, DEFAULT_CAP))
    }

    /// An in-memory-only cache with a tight cap — the eviction tests'
    /// window, small enough to fill by hand.
    #[cfg(test)]
    fn transient_with_cap(cap: usize) -> Arc<Self> {
        Arc::new(Self::build(None, cap))
    }

    fn build(path: Option<PathBuf>, cap: usize) -> Self {
        let (map, order) = path
            .as_deref()
            .map(Self::load_file)
            .unwrap_or_else(|| (HashMap::new(), Vec::new()));
        Self {
            inner: Mutex::new(Inner { map, order, cap }),
            path,
            file_lock: Mutex::new(()),
            dirty: AtomicBool::new(false),
            flushing: AtomicBool::new(false),
        }
    }

    fn load_file(path: &Path) -> (HashMap<CacheKey, String>, Vec<CacheKey>) {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return (HashMap::new(), Vec::new());
        };
        let Ok(file) = serde_json::from_str::<CacheFile>(&raw) else {
            return (HashMap::new(), Vec::new());
        };
        if file.version != 1 {
            return (HashMap::new(), Vec::new());
        }
        let mut map = HashMap::new();
        let mut order = Vec::with_capacity(file.entries.len());
        for entry in file.entries {
            let key = (
                entry.engine,
                entry.sl,
                entry.tl,
                entry.text,
            );
            if map.insert(key.clone(), entry.translation).is_none() {
                order.push(key);
            }
        }
        (map, order)
    }

    /// The cache key for a request — everything that identifies one
    /// translation.
    fn key(engine: &str, sl: &str, tl: &str, text: &str) -> CacheKey {
        (
            engine.to_string(),
            sl.to_string(),
            tl.to_string(),
            text.to_string(),
        )
    }

    /// Moves an already-present key to the newest end of the eviction
    /// order.
    fn touch(inner: &mut Inner, key: CacheKey) {
        if let Some(pos) = inner.order.iter().position(|k| *k == key) {
            inner.order.remove(pos);
            inner.order.push(key);
        }
    }

    /// The cached translation for this exact request, if seen before. A
    /// hit refreshes the entry's recency.
    pub fn lookup(&self, engine: &str, sl: &str, tl: &str, text: &str) -> Option<String> {
        let key = Self::key(engine, sl, tl, text);
        let mut inner = self.inner.lock().expect("cache lock");
        let hit = inner.map.get(&key).cloned();
        if hit.is_some() {
            Self::touch(&mut inner, key);
        }
        hit
    }

    /// Records a fresh engine answer, evicting the least-recently-used
    /// entries past the cap, and schedules the debounced flush.
    pub fn store(self: &Arc<Self>, engine: &str, sl: &str, tl: &str, text: &str, translation: String) {
        let key = Self::key(engine, sl, tl, text);
        {
            let mut inner = self.inner.lock().expect("cache lock");
            if inner.map.insert(key.clone(), translation).is_none() {
                inner.order.push(key);
            } else {
                Self::touch(&mut inner, key);
            }
            while inner.map.len() > inner.cap {
                let evicted = inner.order.remove(0);
                inner.map.remove(&evicted);
            }
        }
        self.schedule_flush();
    }

    /// Empties the store and removes the file, so the next identical text
    /// re-requests. A flush that was already in flight serializes against
    /// the same file lock, so it can only ever write the emptied store —
    /// never resurrect the pre-clear entries.
    pub fn clear(&self) {
        {
            let mut inner = self.inner.lock().expect("cache lock");
            inner.map.clear();
            inner.order.clear();
        }
        self.dirty.store(false, Ordering::SeqCst);
        if let Some(path) = &self.path {
            let _guard = self.file_lock.lock().expect("cache file lock");
            let _ = std::fs::remove_file(path);
        }
    }

    /// Entries currently held.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("cache lock").map.len()
    }

    /// Whether the store holds no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Marks dirty and, when no flush is already waiting, parks a thread
    /// for [`FLUSH_DELAY`] then writes. Transient caches never touch disk.
    fn schedule_flush(self: &Arc<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.dirty.store(true, Ordering::SeqCst);
        if self.flushing.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(FLUSH_DELAY);
            me.flushing.store(false, Ordering::SeqCst);
            if me.dirty.swap(false, Ordering::SeqCst) {
                me.write_file(&path);
            }
        });
    }

    /// Writes the store to disk right now — the debounce's write step and
    /// the deterministic path the tests use. No exit hook calls this: the
    /// debounce loses at most [`FLUSH_DELAY`] of entries on quit, which a
    /// cache may.
    pub fn flush_now(&self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.write_file(&path);
    }

    fn write_file(&self, path: &Path) {
        // The file lock spans snapshot AND write, so a concurrent clear
        // cannot slip between them and let a stale snapshot resurrect
        // entries the clear just removed (the clear takes the same lock to
        // delete the file).
        let _guard = self.file_lock.lock().expect("cache file lock");
        let entries: Vec<CacheEntry> = {
            let inner = self.inner.lock().expect("cache lock");
            inner
                .order
                .iter()
                .filter_map(|key| {
                    inner.map.get(key).map(|translation| CacheEntry {
                        engine: key.0.clone(),
                        sl: key.1.clone(),
                        tl: key.2.clone(),
                        text: key.3.clone(),
                        translation: translation.clone(),
                    })
                })
                .collect()
        };
        let file = CacheFile {
            version: 1,
            entries,
        };
        let Ok(json) = serde_json::to_string_pretty(&file) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// A Translate Engine that serves repeats from a [`TranslationCache`] and
/// records every fresh answer into it. The shell builds this around every
/// real engine (the cache is keyed by the engine's config name, so
/// engines never serve each other's answers), which puts it below the
/// Glossary: the orchestrator only reaches the engine when the Glossary
/// missed.
///
/// `translate_guided` is overridden deliberately — the port's default
/// would funnel into plain `translate` and strip the LLM guidance
/// (Glossary injection, Genderless instruction, Translation Context) from
/// every miss. Guidance is still not part of the cache key: a text keeps
/// the translation it first received (see the module docs).
///
/// Errors are never cached — a rate-limited moment must not freeze a
/// failure into the store — and `name`/`is_cloud` delegate so the privacy
/// badge keeps naming the engine that actually translates.
pub struct CachingTranslateEngine {
    inner: Arc<dyn TranslateEngine>,
    engine_key: String,
    cache: Arc<TranslationCache>,
}

impl CachingTranslateEngine {
    pub fn new(
        inner: Arc<dyn TranslateEngine>,
        engine_key: &str,
        cache: Arc<TranslationCache>,
    ) -> Self {
        Self {
            inner,
            engine_key: engine_key.to_string(),
            cache,
        }
    }

    fn cached(&self, text: &str, sl: &str, target: &str) -> Option<String> {
        self.cache.lookup(&self.engine_key, sl, target, text)
    }

    fn remember(&self, text: &str, sl: &str, target: &str, translation: &str) {
        self.cache
            .store(&self.engine_key, sl, target, text, translation.to_string());
    }
}

#[async_trait]
impl TranslateEngine for CachingTranslateEngine {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError> {
        let sl = source.code();
        if let Some(hit) = self.cached(text, sl, target) {
            return Ok(hit);
        }
        let translated = self.inner.translate(text, source, target).await?;
        self.remember(text, sl, target, &translated);
        Ok(translated)
    }

    async fn translate_guided(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
        guidance: &crate::ports::TranslationGuidance,
    ) -> Result<String, TranslateError> {
        let sl = source.code();
        if let Some(hit) = self.cached(text, sl, target) {
            return Ok(hit);
        }
        let translated = self
            .inner
            .translate_guided(text, source, target, guidance)
            .await?;
        self.remember(text, sl, target, &translated);
        Ok(translated)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_cloud(&self) -> bool {
        self.inner.is_cloud()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory for persistence tests, cleaned up on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "langception-cache-test-{}-{tag}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn store_then_lookup_hits() {
        let cache = TranslationCache::transient();
        cache.store("google", "en", "th", "Hello", "สวัสดี".into());
        assert_eq!(
            cache.lookup("google", "en", "th", "Hello"),
            Some("สวัสดี".to_string())
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn lookup_is_keyed_by_engine_pair_and_text() {
        let cache = TranslationCache::transient();
        cache.store("google", "en", "th", "Hello", "สวัสดี".into());
        // A different engine, pair, or text is a different entry — a miss.
        assert_eq!(cache.lookup("azure", "en", "th", "Hello"), None);
        assert_eq!(cache.lookup("google", "ja", "th", "Hello"), None);
        assert_eq!(cache.lookup("google", "en", "th", "Hello!"), None);
    }

    #[test]
    fn hit_refreshes_recency_so_the_lru_keeps_hot_entries() {
        let cache = TranslationCache::transient_with_cap(2);
        cache.store("google", "en", "th", "A", "a".into());
        cache.store("google", "en", "th", "B", "b".into());
        // Touching A makes B the oldest; the next insert evicts B, not A.
        assert!(cache.lookup("google", "en", "th", "A").is_some());
        cache.store("google", "en", "th", "C", "c".into());
        assert!(cache.lookup("google", "en", "th", "A").is_some());
        assert_eq!(cache.lookup("google", "en", "th", "B"), None);
        assert!(cache.lookup("google", "en", "th", "C").is_some());
    }

    #[test]
    fn overwrite_keeps_one_entry_per_key() {
        let cache = TranslationCache::transient();
        cache.store("google", "en", "th", "A", "old".into());
        cache.store("google", "en", "th", "A", "new".into());
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.lookup("google", "en", "th", "A"),
            Some("new".to_string())
        );
    }

    #[test]
    fn persists_across_reopen() {
        let dir = TempDir::new("reopen");
        let path = dir.file("cache.json");
        {
            let cache = TranslationCache::open(path.clone());
            cache.store("google", "en", "th", "Pay 500 gold", "จ่าย 500 ทอง".into());
            cache.flush_now();
        }
        let reopened = TranslationCache::open(path);
        assert_eq!(
            reopened.lookup("google", "en", "th", "Pay 500 gold"),
            Some("จ่าย 500 ทอง".to_string())
        );
    }

    #[test]
    fn corrupt_file_boots_empty() {
        let dir = TempDir::new("corrupt");
        let path = dir.file("cache.json");
        std::fs::write(&path, "<html>blocked</html>").expect("plant corrupt file");
        let cache = TranslationCache::open(path);
        assert!(cache.is_empty());
        // And the store still works after a corrupt boot.
        cache.store("google", "en", "th", "Hello", "สวัสดี".into());
        assert_eq!(
            cache.lookup("google", "en", "th", "Hello"),
            Some("สวัสดี".to_string())
        );
    }

    #[test]
    fn clear_empties_store_and_removes_file() {
        let dir = TempDir::new("clear");
        let path = dir.file("cache.json");
        let cache = TranslationCache::open(path.clone());
        cache.store("google", "en", "th", "Hello", "สวัสดี".into());
        cache.flush_now();
        assert!(path.exists());

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.lookup("google", "en", "th", "Hello"), None);
        assert!(!path.exists());
        // Reopening boots as an empty store, not the pre-clear data.
        assert!(TranslationCache::open(path).is_empty());
    }

    // ---- the engine wrapper ----

    use std::sync::atomic::AtomicUsize;

    /// A stub engine that counts where it was entered and can fail once,
    /// so the tests can watch which path each call took.
    struct CountingEngine {
        plain_calls: AtomicUsize,
        guided_calls: AtomicUsize,
        fail_first: AtomicBool,
    }
    impl CountingEngine {
        fn new() -> Self {
            Self {
                plain_calls: AtomicUsize::new(0),
                guided_calls: AtomicUsize::new(0),
                fail_first: AtomicBool::new(false),
            }
        }
        fn fail_first(self) -> Self {
            self.fail_first.store(true, Ordering::SeqCst);
            self
        }
        fn plain_calls(&self) -> usize {
            self.plain_calls.load(Ordering::SeqCst)
        }
        fn guided_calls(&self) -> usize {
            self.guided_calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl TranslateEngine for CountingEngine {
        async fn translate(
            &self,
            _text: &str,
            _source: SourceLanguage,
            _target: &str,
        ) -> Result<String, TranslateError> {
            if self.fail_first.swap(false, Ordering::SeqCst) {
                return Err(TranslateError::new("HTTP 429 (rate limited)"));
            }
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            Ok("แปลแล้ว".into())
        }
        async fn translate_guided(
            &self,
            _text: &str,
            _source: SourceLanguage,
            _target: &str,
            _guidance: &crate::ports::TranslationGuidance,
        ) -> Result<String, TranslateError> {
            if self.fail_first.swap(false, Ordering::SeqCst) {
                return Err(TranslateError::new("HTTP 429 (rate limited)"));
            }
            self.guided_calls.fetch_add(1, Ordering::SeqCst);
            Ok("แปลแล้ว".into())
        }
        fn name(&self) -> &str {
            "Counting Test Engine (cloud)"
        }
        fn is_cloud(&self) -> bool {
            true
        }
    }

    fn wrapped(engine: Arc<CountingEngine>) -> super::CachingTranslateEngine {
        super::CachingTranslateEngine::new(
            engine,
            "google",
            TranslationCache::transient(),
        )
    }

    #[tokio::test]
    async fn same_text_twice_is_one_engine_call() {
        let engine = Arc::new(CountingEngine::new());
        let wrapped = wrapped(Arc::clone(&engine));
        let first = wrapped.translate("Hello", SourceLanguage::En, "th").await;
        let second = wrapped.translate("Hello", SourceLanguage::En, "th").await;
        assert_eq!(first.unwrap(), "แปลแล้ว");
        assert_eq!(second.unwrap(), "แปลแล้ว");
        assert_eq!(engine.plain_calls(), 1, "the repeat must come from the cache");
    }

    #[tokio::test]
    async fn guided_miss_routes_through_translate_guided_not_translate() {
        // The orchestrator calls translate_guided; an LLM engine folds the
        // Glossary + Genderless + Translation Context into its prompt
        // there. A caching wrapper that let the port default call plain
        // translate would silently strip that guidance from every miss.
        let engine = Arc::new(CountingEngine::new());
        let wrapped = wrapped(Arc::clone(&engine));
        let guidance = crate::ports::TranslationGuidance::plain();
        let out = wrapped
            .translate_guided("Hello", SourceLanguage::En, "th", &guidance)
            .await;
        assert_eq!(out.unwrap(), "แปลแล้ว");
        assert_eq!(engine.guided_calls(), 1);
        assert_eq!(engine.plain_calls(), 0);
    }

    #[tokio::test]
    async fn guided_hit_serves_from_the_cache() {
        let engine = Arc::new(CountingEngine::new());
        let wrapped = wrapped(Arc::clone(&engine));
        let guidance = crate::ports::TranslationGuidance::plain();
        wrapped
            .translate_guided("Hello", SourceLanguage::En, "th", &guidance)
            .await
            .expect("first call");
        wrapped
            .translate_guided("Hello", SourceLanguage::En, "th", &guidance)
            .await
            .expect("second call");
        assert_eq!(engine.guided_calls(), 1, "the repeat must come from the cache");
    }

    #[tokio::test]
    async fn an_engine_error_is_never_cached() {
        let engine = Arc::new(CountingEngine::new().fail_first());
        let wrapped = wrapped(Arc::clone(&engine));
        let first = wrapped.translate("Hello", SourceLanguage::En, "th").await;
        assert!(first.is_err(), "the planted failure must surface");
        let second = wrapped.translate("Hello", SourceLanguage::En, "th").await;
        assert_eq!(second.unwrap(), "แปลแล้ว");
        // The second call reached the engine after the error (the failure
        // was never cached) — plain_calls is 1 now — and the third is the
        // cached answer, costing the engine nothing.
        let third = wrapped.translate("Hello", SourceLanguage::En, "th").await;
        assert_eq!(third.unwrap(), "แปลแล้ว");
        assert_eq!(engine.plain_calls(), 1);
    }

    #[tokio::test]
    async fn wrapper_delegates_name_and_is_cloud() {
        let wrapped = wrapped(Arc::new(CountingEngine::new()));
        assert_eq!(wrapped.name(), "Counting Test Engine (cloud)");
        assert!(wrapped.is_cloud());
    }

    #[tokio::test]
    async fn the_cache_stores_the_raw_engine_output() {
        // Genderless Mode scrubs per serve inside the loop, so the store
        // must keep what the engine said — never a post-processed answer
        // (a settings flip would then freeze into the cache).
        struct GenderedEngine;
        #[async_trait]
        impl TranslateEngine for GenderedEngine {
            async fn translate(
                &self,
                _text: &str,
                _source: SourceLanguage,
                _target: &str,
            ) -> Result<String, TranslateError> {
                Ok("สวัสดีครับ".into())
            }
            fn name(&self) -> &str {
                "gendered"
            }
        }
        let cache = TranslationCache::transient();
        let wrapped = super::CachingTranslateEngine::new(
            Arc::new(GenderedEngine),
            "google",
            Arc::clone(&cache),
        );
        let out = wrapped.translate("Hi", SourceLanguage::En, "th").await;
        assert_eq!(out.unwrap(), "สวัสดีครับ");
        assert_eq!(
            cache.lookup("google", "en", "th", "Hi"),
            Some("สวัสดีครับ".to_string()),
            "the store keeps the raw answer, not a scrubbed one"
        );
    }

    #[tokio::test]
    async fn engines_do_not_leak_into_each_others_cache_entries() {
        let cache = TranslationCache::transient();
        let google = Arc::new(CountingEngine::new());
        let azure = Arc::new(CountingEngine::new());
        let google_wrapped = super::CachingTranslateEngine::new(
            Arc::clone(&google) as Arc<dyn TranslateEngine>,
            "google",
            Arc::clone(&cache),
        );
        let azure_wrapped = super::CachingTranslateEngine::new(
            Arc::clone(&azure) as Arc<dyn TranslateEngine>,
            "azure",
            cache,
        );
        let a = google_wrapped.translate("Hello", SourceLanguage::En, "th").await;
        let b = azure_wrapped.translate("Hello", SourceLanguage::En, "th").await;
        assert_eq!(a.unwrap(), "แปลแล้ว");
        assert_eq!(b.unwrap(), "แปลแล้ว");
        assert_eq!(google.plain_calls(), 1);
        assert_eq!(azure.plain_calls(), 1, "a google answer must never serve an azure request");
    }
}
