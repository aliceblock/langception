//! Orchestrator seam tests — the one real seam per the spec's testing
//! decisions. Every pipeline behavior is asserted externally through the
//! event channel and the translate fake's call record; no internal call
//! order or private types.

use async_trait::async_trait;
use langception_core::glossary::Glossary;
use langception_core::orchestrator::{LoopConfig, LoopEvent, Orchestrator, Stage, Via};
use langception_core::ports::{
    CaptureError, CaptureSource, CapturedImage, Clock, OcrEngine, OcrError, Region,
    SourceLanguage, TextBlock, TranslateEngine, TranslateError,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};

const REGION: Region = Region {
    x: 100,
    y: 100,
    width: 600,
    height: 200,
};

fn config() -> LoopConfig {
    LoopConfig {
        region: REGION,
        source_language: SourceLanguage::En,
        cadence: Duration::from_millis(500),
    }
}

fn block(x: i32, y: i32, text: &str) -> TextBlock {
    TextBlock {
        x,
        y,
        width: 100,
        height: 20,
        text: text.to_string(),
    }
}

fn image() -> CapturedImage {
    CapturedImage {
        width: REGION.width,
        height: REGION.height,
        bgra: vec![0; (REGION.width * REGION.height * 4) as usize],
    }
}

/// Feeds one image per `capture()` call from a channel; closed channel = capture error.
struct FakeCapture {
    images: AsyncMutex<mpsc::Receiver<CapturedImage>>,
    calls: Arc<Mutex<u32>>,
}

impl FakeCapture {
    fn new() -> (mpsc::Sender<CapturedImage>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                images: AsyncMutex::new(rx),
                calls: Arc::new(Mutex::new(0)),
            },
        )
    }
}

#[async_trait]
impl CaptureSource for FakeCapture {
    async fn capture(&self, _region: Region) -> Result<CapturedImage, CaptureError> {
        *self.calls.lock().unwrap() += 1;
        let mut rx = self.images.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| CaptureError::new("capture source closed"))
    }
}

/// Yields one pre-scripted OCR result per call; closed channel = no text found.
struct FakeOcr {
    results: AsyncMutex<mpsc::Receiver<Result<Vec<TextBlock>, OcrError>>>,
}

impl FakeOcr {
    fn new() -> (mpsc::Sender<Result<Vec<TextBlock>, OcrError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (tx, Self { results: AsyncMutex::new(rx) })
    }
}

#[async_trait]
impl OcrEngine for FakeOcr {
    async fn recognize(
        &self,
        _image: &CapturedImage,
        _source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        let mut rx = self.results.lock().await;
        rx.recv().await.unwrap_or(Ok(Vec::new()))
    }
}

/// Records every requested translation and answers from a scripted queue
/// (default: `[TH] <text>`).
struct FakeTranslate {
    requests: Mutex<Vec<String>>,
    responses: AsyncMutex<mpsc::Receiver<Result<String, TranslateError>>>,
}

impl FakeTranslate {
    fn new() -> (mpsc::Sender<Result<String, TranslateError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                requests: Mutex::new(Vec::new()),
                responses: AsyncMutex::new(rx),
            },
        )
    }

    fn calls(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl TranslateEngine for FakeTranslate {
    async fn translate(
        &self,
        text: &str,
        _source: SourceLanguage,
        _target: &str,
    ) -> Result<String, TranslateError> {
        self.requests.lock().unwrap().push(text.to_string());
        // A scripted response (queued before the tick) wins; otherwise the
        // default translation. Never await an empty channel here — that
        // would deadlock the tick under test.
        let mut rx = self.responses.lock().await;
        match rx.try_recv() {
            Ok(response) => response,
            Err(_) => Ok(format!("[TH] {text}")),
        }
    }
}

/// `sleep()` waits for the test to release the gate — each release is one
/// loop tick. `Notify` stores one permit, so a release sent before the loop
/// reaches its sleep still satisfies exactly that sleep.
struct FakeClock {
    gate: Notify,
    now: Mutex<u64>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            gate: Notify::new(),
            now: Mutex::new(0),
        }
    }

    fn set_now_ms(&self, ms: u64) {
        *self.now.lock().unwrap() = ms;
    }

    fn release_tick(&self) {
        self.gate.notify_one();
    }
}

#[async_trait]
impl Clock for FakeClock {
    async fn sleep(&self, _duration: Duration) {
        self.gate.notified().await;
    }

    fn now_ms(&self) -> u64 {
        *self.now.lock().unwrap()
    }
}

struct Harness {
    events: mpsc::UnboundedReceiver<LoopEvent>,
    images: mpsc::Sender<CapturedImage>,
    ocr_results: mpsc::Sender<Result<Vec<TextBlock>, OcrError>>,
    responses: mpsc::Sender<Result<String, TranslateError>>,
    capture_calls: Arc<Mutex<u32>>,
    translate: Arc<FakeTranslate>,
    clock: Arc<FakeClock>,
}

async fn harness_with(glossary: Glossary) -> (Harness, Orchestrator) {
    let (images_tx, capture) = FakeCapture::new();
    let capture_calls = Arc::clone(&capture.calls);
    let (ocr_tx, ocr) = FakeOcr::new();
    let (responses_tx, translate) = FakeTranslate::new();
    let translate = Arc::new(translate);
    let clock = Arc::new(FakeClock::new());
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    let orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        translate.clone(),
        clock.clone(),
        config(),
        glossary,
        events_tx,
    );

    (
        Harness {
            events: events_rx,
            images: images_tx,
            ocr_results: ocr_tx,
            responses: responses_tx,
            capture_calls,
            translate,
            clock,
        },
        orchestrator,
    )
}

async fn harness() -> (Harness, Orchestrator) {
    harness_with(Glossary::default()).await
}

async fn next_event(events: &mut mpsc::UnboundedReceiver<LoopEvent>) -> LoopEvent {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event should arrive within 2s")
        .expect("event channel should stay open")
}

/// Polls until `cond` holds (fake-clock tests must not rely on fixed sleeps).
async fn wait_until(cond: impl Fn() -> bool, what: &str) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached within 2s: {what}");
}

/// Feeds one image + one OCR result, then the caller runs exactly one tick.
async fn feed_tick(h: &Harness, blocks: Vec<TextBlock>) {
    h.images.send(image()).await.unwrap();
    h.ocr_results.send(Ok(blocks)).await.unwrap();
}

#[tokio::test]
async fn changed_text_translates_exactly_once_per_change() {
    let (mut h, mut orch) = harness().await;

    // Tick 1: "Hello" appears → translated once.
    feed_tick(&h, vec![block(100, 200, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "[TH] Hello");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 2: same text at the same position → no re-translation, no event.
    feed_tick(&h, vec![block(100, 200, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), h.events.recv())
            .await
            .is_err(),
        "unchanged block must not emit any event"
    );

    // Tick 3: text changes at the same position → exactly one new translation.
    feed_tick(&h, vec![block(100, 200, "Goodbye")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Goodbye"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Goodbye"),
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn blocks_are_tracked_independently() {
    let (h, mut orch) = harness().await;

    // Two blocks, both translated.
    feed_tick(&h, vec![block(100, 200, "Alpha"), block(100, 500, "Beta")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Alpha", "Beta"]);

    // Only the first block changed → only it is re-translated. Its position
    // also moved 2px, inside the quantization grid, so it is the same block.
    feed_tick(&h, vec![block(102, 200, "Alpha Prime"), block(100, 500, "Beta")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Alpha", "Beta", "Alpha Prime"]);
}

#[tokio::test]
async fn glossary_match_shows_glossary_thai_and_never_calls_engine() {
    let mut glossary = Glossary::default();
    glossary.0.insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary).await;

    feed_tick(&h, vec![block(0, 0, "  vertin  ")]).await;
    orch.tick().await;

    assert!(
        h.translate.calls().is_empty(),
        "glossary match must short-circuit the engine, calls were: {:?}",
        h.translate.calls()
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "เวอร์ติน");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // And it still dedups: same block next tick → no event, no engine call.
    feed_tick(&h, vec![block(0, 0, "  vertin  ")]).await;
    orch.tick().await;
    assert!(h.translate.calls().is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), h.events.recv())
            .await
            .is_err(),
        "unchanged glossary block must not emit any event"
    );
}

#[tokio::test]
async fn engine_error_surfaces_visibly_and_is_not_hammered() {
    let (mut h, mut orch) = harness().await;

    // Script the single engine call this tick should make to fail.
    h.responses
        .send(Err(TranslateError::new("HTTP 429 rate limited")))
        .await
        .unwrap();
    h.images.send(image()).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, message, .. } => {
            assert_eq!(stage, Stage::Translate);
            assert!(message.contains("429"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Same text again → no retry (no engine hammering while the user reads the error).
    feed_tick(&h, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), h.events.recv())
            .await
            .is_err(),
        "failed block must not be retried while text is unchanged"
    );

    // Changed text → engine tried again despite the earlier failure.
    feed_tick(&h, vec![block(0, 0, "Hello there")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello there"]);
}

#[tokio::test]
async fn capture_and_ocr_errors_surface_with_their_stage() {
    let (mut h, mut orch) = harness().await;

    // Capture failure: source closed before this tick.
    drop(h.images);
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Capture),
        other => panic!("expected Error, got {other:?}"),
    }

    // OCR failure: fresh harness, image queued but the recognizer errors.
    let (mut h2, mut orch2) = harness().await;
    h2.images.send(image()).await.unwrap();
    h2.ocr_results
        .send(Err(OcrError::new("no language pack installed")))
        .await
        .unwrap();
    orch2.tick().await;
    match next_event(&mut h2.events).await {
        LoopEvent::Error { stage, message, .. } => {
            assert_eq!(stage, Stage::Ocr);
            assert!(message.contains("language pack"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn whitespace_blocks_are_ignored() {
    let (mut h, mut orch) = harness().await;

    feed_tick(
        &h,
        vec![
            block(0, 0, "   "),
            block(0, 50, ""),
            block(0, 100, "Real text"),
        ],
    )
    .await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Real text"]);
    let _ = next_event(&mut h.events).await; // only the real block produces an event
    assert!(
        tokio::time::timeout(Duration::from_millis(100), h.events.recv())
            .await
            .is_err(),
        "whitespace blocks must not produce events"
    );
}

#[tokio::test]
async fn run_loop_paces_ticks_with_the_clock_and_keeps_dedup_across_ticks() {
    let (h, orch) = harness().await;

    let task = tokio::spawn(orch.run());

    // Tick 1 runs as soon as an image is available; then the loop sleeps on
    // the fake clock gate (the stored Notify permit satisfies that sleep).
    h.images.send(image()).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    h.clock.release_tick();
    wait_until(|| h.translate.calls() == ["Hello"], "first tick translated").await;

    // Tick 2: the released gate wakes the sleep; the OCR result is unchanged,
    // so no new engine call happens even across loop ticks.
    h.images.send(image()).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    h.clock.release_tick();
    wait_until(
        || *h.capture_calls.lock().unwrap() >= 2,
        "second tick ran",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "unchanged block must not re-translate across loop ticks"
    );

    task.abort();
}

#[tokio::test]
async fn event_timestamps_come_from_the_clock() {
    let (mut h, mut orch) = harness().await;

    h.clock.set_now_ms(12345);
    h.images.send(image()).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { at_ms, .. } => assert_eq!(at_ms, 12345),
        other => panic!("expected Translated, got {other:?}"),
    }
}
