//! Orchestrator seam tests — the one real seam per the spec's testing
//! decisions. Every pipeline behavior is asserted externally through the
//! event channel and the translate fake's call record; no internal call
//! order or private types.
//!
//! Frames are contentful: a block list paints a text-derived byte inside
//! each block's box, so a block's pixels change exactly when its text does,
//! and pixel-identical screens are expressed by re-feeding the same frame.
//! That drives the ticket-05 change detection for real: identical frames
//! take the OCR-skipping fast path, changed frames run the dHash pipeline.

use async_trait::async_trait;
use langception_core::glossary::Glossary;
use langception_core::orchestrator::{
    EventOrigin, LoopConfig, LoopEvent, Orchestrator, Stage, Via,
};
use langception_core::ports::{
    CaptureError, CaptureSource, CapturedImage, Clock, ExcludedBlock, OcrEngine, OcrError, Region,
    SourceLanguage, TextBlock, TranslateEngine, TranslateError, Zone,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex, Notify};

const REGION: Region = Region {
    x: 100,
    y: 100,
    width: 600,
    // Tall enough that every block position used below (up to y = 500)
    // sits inside the captured frame — blocks whose rect falls outside it
    // would clamp to an empty rect and always hash to 0.
    height: 600,
};

fn config() -> LoopConfig {
    LoopConfig {
        source_language: SourceLanguage::En,
        cadence: Duration::from_millis(500),
        // The zero window fires on the detecting pass: most tests here
        // exercise dedup, removals, exclusions and errors, not the
        // debounce. The stability-window tests build their own config.
        stability: Duration::ZERO,
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

fn zone(id: &str, name: &str, region: Region) -> Zone {
    Zone {
        id: id.to_string(),
        name: name.to_string(),
        monitor: String::new(),
        region,
    }
}

/// An exclusion covering `region` in SCREEN coordinates (REGION sits at
/// 100,100, so region-relative blocks land here at +100/+100).
fn exclusion(id: &str, x: i32, y: i32, width: u32, height: u32) -> ExcludedBlock {
    ExcludedBlock {
        id: id.to_string(),
        monitor: String::new(),
        region: Region {
            x,
            y,
            width,
            height,
        },
        label: "Hello".to_string(),
    }
}

fn black_image() -> CapturedImage {
    CapturedImage {
        width: REGION.width,
        height: REGION.height,
        bgra: vec![0; (REGION.width * REGION.height * 4) as usize],
    }
}

/// Paints byte `value` into pixel (x, y)'s blue channel, clamped into the
/// region.
fn paint(image: &mut CapturedImage, x: i32, y: i32, value: u8) {
    let x = x.clamp(0, REGION.width as i32 - 1) as u32;
    let y = y.clamp(0, REGION.height as i32 - 1) as u32;
    let index = ((y * REGION.width + x) * 4) as usize;
    image.bgra[index] = value;
}

/// Text-derived marker byte: equal texts select equal marker shapes, so a
/// block's pixels change exactly when its text does.
fn text_mark(text: &str) -> u8 {
    text.bytes().fold(0u8, |acc, byte| acc.wrapping_add(byte))
}

/// Paints two bright vertical bars into the block, whose dHash-cell
/// positions derive from `selector`: the same selector paints the same
/// pixels (stable hash), a different selector lights a different pattern of
/// cells and reliably flips gradient bits — like real text, which changes
/// the bright/dark pattern across the whole box rather than a lone pixel
/// (which box-averaging deliberately dilutes into nothing). Two bars give
/// 64 patterns, so unrelated texts never collide by accident.
fn paint_marker(image: &mut CapturedImage, block: &TextBlock, selector: u8) {
    for (cell, width) in [(selector % 8, 6), ((selector / 8) % 8, 3)] {
        let x = block.x + 2 + cell as i32 * 11;
        for dx in 0..width {
            for dy in 0..block.height as i32 {
                paint(image, x + dx, block.y + dy, 255);
            }
        }
    }
}

/// The frame for a block list (deterministic): black, plus one text-derived
/// marker bar inside each block's box. Equal block lists → pixel-identical
/// frames (the fast path); a changed text changes both the frame checksum
/// and its block's dHash.
fn frame_for(blocks: &[TextBlock]) -> CapturedImage {
    let mut image = black_image();
    for block in blocks {
        paint_marker(&mut image, block, text_mark(&block.text));
    }
    image
}

/// Like [`frame_for`], but every block's marker is selected by `anim` and
/// the corner pixel (outside every test block) shows `corner`. Bumping
/// `corner` forces the OCR path with pixel-identical blocks; bumping `anim`
/// changes every block's pixels without changing its text — animation
/// jitter.
fn frame_animated(blocks: &[TextBlock], corner: u8, anim: u8) -> CapturedImage {
    let mut image = black_image();
    paint(
        &mut image,
        REGION.width as i32 - 1,
        REGION.height as i32 - 1,
        corner,
    );
    for block in blocks {
        paint_marker(&mut image, block, anim);
    }
    image
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
    calls: Arc<Mutex<u32>>,
}

impl FakeOcr {
    fn new() -> (mpsc::Sender<Result<Vec<TextBlock>, OcrError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                results: AsyncMutex::new(rx),
                calls: Arc::new(Mutex::new(0)),
            },
        )
    }
}

#[async_trait]
impl OcrEngine for FakeOcr {
    async fn recognize(
        &self,
        _image: &CapturedImage,
        _source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        *self.calls.lock().unwrap() += 1;
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
    ocr_calls: Arc<Mutex<u32>>,
    translate: Arc<FakeTranslate>,
    clock: Arc<FakeClock>,
    zones_tx: watch::Sender<Vec<Zone>>,
    paused_tx: watch::Sender<bool>,
    excluded_tx: watch::Sender<Vec<ExcludedBlock>>,
}

/// The harness starts watching one zone (z1 = REGION), like the app does
/// after a first zone was drawn, with no Excluded Blocks in force.
async fn harness_with(glossary: Glossary, stability: Duration) -> (Harness, Orchestrator) {
    let (images_tx, capture) = FakeCapture::new();
    let capture_calls = Arc::clone(&capture.calls);
    let (ocr_tx, ocr) = FakeOcr::new();
    let ocr_calls = Arc::clone(&ocr.calls);
    let (responses_tx, translate) = FakeTranslate::new();
    let translate = Arc::new(translate);
    let clock = Arc::new(FakeClock::new());
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (paused_tx, paused_rx) = watch::channel(false);
    let (excluded_tx, excluded_rx) = watch::channel(Vec::new());

    let config = LoopConfig {
        stability,
        ..config()
    };
    let orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        translate.clone(),
        clock.clone(),
        config,
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );

    (
        Harness {
            events: events_rx,
            images: images_tx,
            ocr_results: ocr_tx,
            responses: responses_tx,
            capture_calls,
            ocr_calls,
            translate,
            clock,
            zones_tx,
            paused_tx,
            excluded_tx,
        },
        orchestrator,
    )
}

async fn harness() -> (Harness, Orchestrator) {
    harness_with(Glossary::default(), Duration::ZERO).await
}

async fn harness_with_stability(ms: u64) -> (Harness, Orchestrator) {
    harness_with(Glossary::default(), Duration::from_millis(ms)).await
}

fn set_zones(h: &Harness, zones: Vec<Zone>) {
    h.zones_tx.send(zones).unwrap();
}

async fn next_event(events: &mut mpsc::UnboundedReceiver<LoopEvent>) -> LoopEvent {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event should arrive within 2s")
        .expect("event channel should stay open")
}

async fn assert_no_event(events: &mut mpsc::UnboundedReceiver<LoopEvent>, what: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.recv())
            .await
            .is_err(),
        "{what}"
    );
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

/// Sends `frame` to every zone; when `ocr` is Some, also queues one OCR
/// result per zone. Queue OCR results only for ticks that will take the
/// OCR path — a fast-path tick (pixel-identical frame) never calls OCR,
/// and an unconsumed result would surface one tick late.
async fn feed(h: &Harness, zone_count: usize, frame: &CapturedImage, ocr: Option<Vec<TextBlock>>) {
    for _ in 0..zone_count {
        h.images.send(frame.clone()).await.unwrap();
        if let Some(blocks) = &ocr {
            h.ocr_results
                .send(Ok(blocks.clone()))
                .await
                .unwrap();
        }
    }
}

/// Feeds one changed frame + one OCR result per zone (zones are processed
/// in order, so all zones see the same blocks unless the test queues its
/// own results), then the caller runs exactly one tick.
async fn feed_tick(h: &Harness, zone_count: usize, blocks: Vec<TextBlock>) {
    feed(h, zone_count, &frame_for(&blocks), Some(blocks)).await;
}

/// Re-feeds a frame the zone has already seen (pixel-identical screen):
/// the next tick takes the fast path and no OCR result may be queued.
async fn feed_unchanged(h: &Harness, zone_count: usize, blocks: Vec<TextBlock>) {
    feed(h, zone_count, &frame_for(&blocks), None).await;
}

// ---- Dedup and change detection (ticket 05) ----

#[tokio::test]
async fn changed_text_translates_exactly_once_per_change() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(100, 200, "Hello")];

    // Tick 1: "Hello" appears → translated once.
    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            translation,
            via,
            origin,
            ..
        } => {
            assert_eq!(translation, "[TH] Hello");
            assert_eq!(via, Via::Engine);
            assert_eq!(
                origin,
                EventOrigin::Zone {
                    id: "z1".into(),
                    name: "Zone 1".into()
                }
            );
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 2: pixel-identical screen → the fast path, no OCR, no event.
    feed_unchanged(&h, 1, hello.clone()).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    assert_no_event(&mut h.events, "unchanged block must not emit any event").await;

    // Tick 3: text changes at the same position → its pixels change too →
    // exactly one new translation.
    feed_tick(&h, 1, vec![block(100, 200, "Goodbye")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Goodbye"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Goodbye"),
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn ocr_is_skipped_while_the_frame_is_unchanged() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    assert_eq!(*h.ocr_calls.lock().unwrap(), 1);

    // The same frame again: capture runs (it must — it is what decides),
    // OCR does not. This is what keeps an idle screen cheap.
    feed_unchanged(&h, 1, hello).await;
    orch.tick().await;
    assert_eq!(*h.capture_calls.lock().unwrap(), 2);
    assert_eq!(
        *h.ocr_calls.lock().unwrap(),
        1,
        "unchanged pixels must not re-run OCR"
    );
    assert_no_event(&mut h.events, "the fast path must not emit events").await;
}

#[tokio::test]
async fn ocr_text_jitter_on_unchanged_pixels_is_ignored() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // Same block pixels (only the corner changed, so OCR still runs), but
    // the recognizer reads the text slightly differently: the block's
    // image hash is unchanged, so it is skipped and its translation stands.
    let frame = frame_animated(&hello, 1, text_mark("Hello"));
    feed(&h, 1, &frame, Some(vec![block(0, 0, "Hell0")])).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "text jitter on unchanged pixels must not re-translate"
    );
    assert_no_event(&mut h.events, "no event for a pixel-identical block").await;
}

#[tokio::test]
async fn animation_jitter_never_triggers() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Every pass the block's pixels change (anim) but its text does not:
    // the change never holds stable, so OCR+translate never fire for it.
    for pass in 1..=4u8 {
        h.clock.set_now_ms(pass as u64 * 500);
        let frame = frame_animated(&blocks, pass, pass);
        feed(&h, 1, &frame, Some(blocks.clone())).await;
        orch.tick().await;
    }
    assert!(
        h.translate.calls().is_empty(),
        "animation jitter must never fire, calls were: {:?}",
        h.translate.calls()
    );
    assert_no_event(&mut h.events, "animation jitter must not emit events").await;
}

#[tokio::test]
async fn a_stable_change_fires_exactly_once() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1 (now = 0): the change is detected and held in the window.
    h.clock.set_now_ms(0);
    let frame = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &frame, Some(blocks.clone())).await;
    orch.tick().await;
    assert!(
        h.translate.calls().is_empty(),
        "a change must not fire before its stability window elapses"
    );

    // Tick 2 (now = 500): the same frame → fast path; the window (400 ms)
    // has elapsed → fires once, with the read cached at detection time.
    h.clock.set_now_ms(500);
    feed(&h, 1, &frame, None).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Hello"),
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 3 (now = 1000): still the same pixels → nothing further.
    h.clock.set_now_ms(1000);
    feed(&h, 1, &frame, None).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"], "a stable change fires exactly once");
    assert_no_event(&mut h.events, "no event after the single fire").await;
}

#[tokio::test]
async fn a_stable_change_fires_once_across_ocrd_frames() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1: change detected, window starts.
    h.clock.set_now_ms(0);
    let frame = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &frame, Some(blocks.clone())).await;
    orch.tick().await;
    assert!(h.translate.calls().is_empty());

    // Tick 2 (now = 500): something ELSE in the zone changed (corner), so
    // OCR runs — but this block's pixels are the same as when the window
    // started → matured → fires exactly once.
    h.clock.set_now_ms(500);
    let corner_changed = frame_animated(&blocks, 1, 7);
    feed(&h, 1, &corner_changed, Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // Tick 3: nothing new → no re-fire.
    h.clock.set_now_ms(1000);
    let corner_changed_again = frame_animated(&blocks, 2, 7);
    feed(&h, 1, &corner_changed_again, Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    assert_no_event(&mut h.events, "no second fire for stable pixels").await;
}

#[tokio::test]
async fn a_change_that_reverts_within_the_window_never_fires() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Establish a fired state: detected at 0, stable at 500 → fired.
    h.clock.set_now_ms(0);
    let base = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &base, Some(blocks.clone())).await;
    orch.tick().await;
    h.clock.set_now_ms(500);
    feed(&h, 1, &base, None).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // The pixels change (window restarts at 1000)...
    h.clock.set_now_ms(1000);
    let changed = frame_animated(&blocks, 0, 9);
    feed(&h, 1, &changed, Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);

    // ...and revert to the shown pixels before the window can mature:
    // the pending change is cancelled, no second translation ever fires.
    h.clock.set_now_ms(1500);
    let reverted = frame_animated(&blocks, 1, 7);
    feed(&h, 1, &reverted, Some(blocks.clone())).await;
    orch.tick().await;
    h.clock.set_now_ms(2000);
    feed(&h, 1, &reverted, None).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "a transient change that reverts must not fire"
    );
    assert_no_event(&mut h.events, "no events for the reverted change").await;
}

// ---- Independent tracking ----

#[tokio::test]
async fn blocks_are_tracked_independently() {
    let (h, mut orch) = harness().await;

    // Two blocks, both translated.
    feed_tick(
        &h,
        1,
        vec![block(100, 200, "Alpha"), block(100, 500, "Beta")],
    )
    .await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Alpha", "Beta"]);

    // Only the first block changed → only it is re-translated. Its position
    // also moved 2px, inside the quantization grid, so it is the same block.
    feed_tick(
        &h,
        1,
        vec![block(102, 200, "Alpha Prime"), block(100, 500, "Beta")],
    )
    .await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Alpha", "Beta", "Alpha Prime"]);
}

#[tokio::test]
async fn zones_are_tracked_independently() {
    let (mut h, mut orch) = harness().await;
    set_zones(
        &h,
        vec![
            zone(
                "z1",
                "Left",
                Region {
                    x: 0,
                    y: 0,
                    width: 200,
                    height: 100,
                },
            ),
            zone(
                "z2",
                "Right",
                Region {
                    x: 400,
                    y: 0,
                    width: 200,
                    height: 100,
                },
            ),
        ],
    );

    // Both zones OCR the same text at the same region-relative position.
    feed_tick(&h, 2, vec![block(10, 10, "Hello")]).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Hello"],
        "each zone translates its own blocks"
    );
    let _ = next_event(&mut h.events).await; // drain tick 1's two events
    let _ = next_event(&mut h.events).await;

    // Same content next tick → both zones take the fast path independently.
    feed_unchanged(&h, 2, vec![block(10, 10, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello"]);
    assert_no_event(
        &mut h.events,
        "unchanged blocks in every zone must not emit events",
    )
    .await;
}

#[tokio::test]
async fn zone_added_later_translates_while_existing_zones_keep_dedup() {
    let (h, mut orch) = harness().await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);

    // A second zone arrives while the loop is running.
    set_zones(
        &h,
        vec![
            zone("z1", "Zone 1", REGION),
            zone(
                "z2",
                "Zone 2",
                Region {
                    x: 800,
                    y: 0,
                    width: 200,
                    height: 100,
                },
            ),
        ],
    );
    // Zone 1 sees the same text on a changed screen (its image hash is
    // unchanged → stays deduped); zone 2 sees new text.
    let z1 = vec![block(0, 0, "Hello")];
    let z2 = vec![block(0, 0, "World")];
    feed(&h, 1, &frame_animated(&z1, 1, text_mark("Hello")), Some(z1)).await;
    feed(&h, 1, &frame_for(&z2), Some(z2)).await;
    orch.tick().await;

    assert_eq!(h.translate.calls(), ["Hello", "World"]);
}

#[tokio::test]
async fn removed_zone_stops_being_watched() {
    let (mut h, mut orch) = harness().await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await; // drain tick 1's event
    let calls_after_first = *h.capture_calls.lock().unwrap();
    assert!(calls_after_first >= 1);

    // All zones deleted: ticks do nothing (no capture, no events).
    set_zones(&h, vec![]);
    orch.tick().await;
    assert_eq!(
        *h.capture_calls.lock().unwrap(),
        calls_after_first,
        "no zones → no capture attempts"
    );
    assert_no_event(&mut h.events, "no zones → no events").await;

    // The same zone id comes back (delete + redraw): its tracking state
    // must have been pruned, so its text translates again.
    set_zones(&h, vec![zone("z1", "Zone 1", REGION)]);
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello"]);
}

#[tokio::test]
async fn zone_dragged_to_a_new_region_starts_fresh() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // The zone is dragged (same id, new region): its block state belonged
    // to the old region's pixels, so the text translates again there.
    set_zones(
        &h,
        vec![zone(
            "z1",
            "Zone 1",
            Region {
                x: 100,
                y: 400,
                width: 600,
                height: 200,
            },
        )],
    );
    feed_tick(&h, 1, hello).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Hello"],
        "a moved zone re-translates: the screen under it is new"
    );
}

// ---- Pause ----

#[tokio::test]
async fn paused_loop_skips_ticks_and_keeps_dedup_across_pause() {
    let (h, orch) = harness().await;
    let task = tokio::spawn(orch.run());
    let blocks = vec![block(0, 0, "Hello")];

    // Paused from the start: releasing ticks must not capture.
    h.paused_tx.send(true).unwrap();
    h.clock.release_tick();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        *h.capture_calls.lock().unwrap(),
        0,
        "paused loop must not capture"
    );

    // Resume: the queued frame/OCR is consumed by the first tick.
    h.paused_tx.send(false).unwrap();
    feed(&h, 1, &frame_for(&blocks), Some(blocks.clone())).await;
    wait_until(
        || h.translate.calls() == ["Hello"],
        "translated after resume",
    )
    .await;

    // Pause again, release the loop into the pause, then resume: the
    // pixel-identical frame is still deduped (pause never resets tracking
    // state, so unchanged blocks are not retranslated on resume).
    h.paused_tx.send(true).unwrap();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    feed(&h, 1, &frame_for(&blocks), None).await;
    h.paused_tx.send(false).unwrap();
    wait_until(
        || *h.capture_calls.lock().unwrap() >= 2,
        "resumed tick captured",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "dedup must survive pause/resume"
    );

    task.abort();
}

// ---- Glossary, errors, one-shot ----

#[tokio::test]
async fn glossary_match_shows_glossary_thai_and_never_calls_engine() {
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO).await;
    let blocks = vec![block(0, 0, "  vertin  ")];

    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;

    assert!(
        h.translate.calls().is_empty(),
        "glossary match must short-circuit the engine, calls were: {:?}",
        h.translate.calls()
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            translation, via, ..
        } => {
            assert_eq!(translation, "เวอร์ติน");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // And it still dedups: same pixels next tick → no event, no engine call.
    feed_unchanged(&h, 1, blocks).await;
    orch.tick().await;
    assert!(h.translate.calls().is_empty());
    assert_no_event(
        &mut h.events,
        "unchanged glossary block must not emit any event",
    )
    .await;
}

#[tokio::test]
async fn engine_error_surfaces_visibly_and_is_not_hammered() {
    let (mut h, mut orch) = harness().await;

    // Script the single engine call this tick should make to fail.
    h.responses
        .send(Err(TranslateError::new("HTTP 429 rate limited")))
        .await
        .unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, message, .. } => {
            assert_eq!(stage, Stage::Translate);
            assert!(message.contains("429"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Same pixels again → no retry (no engine hammering while the user
    // reads the error); the failed change counts as handled.
    feed_unchanged(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    assert_no_event(
        &mut h.events,
        "failed block must not be retried while pixels are unchanged",
    )
    .await;

    // Changed pixels → engine tried again despite the earlier failure.
    feed_tick(&h, 1, vec![block(0, 0, "Hello there")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello there"]);
}

#[tokio::test]
async fn capture_and_ocr_errors_surface_with_their_stage_and_zone() {
    let (mut h, mut orch) = harness().await;

    // Capture failure: source closed before this tick.
    drop(h.images);
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, message, .. } => {
            assert_eq!(stage, Stage::Capture);
            assert!(message.contains("Zone 1"), "was: {message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // OCR failure: fresh harness, image queued but the recognizer errors.
    let (mut h2, mut orch2) = harness().await;
    h2.images.send(black_image()).await.unwrap();
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
async fn ocr_error_retries_on_the_next_pass() {
    let (mut h, mut orch) = harness().await;
    let blocks = vec![block(0, 0, "Hello")];

    // OCR fails on the first pass: the frame is not recorded, so the very
    // next pass retries OCR even if the screen is unchanged.
    h.images.send(frame_for(&blocks)).await.unwrap();
    h.ocr_results
        .send(Err(OcrError::new("temporary failure")))
        .await
        .unwrap();
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // the Ocr error
    assert_eq!(*h.ocr_calls.lock().unwrap(), 1);

    feed_tick(&h, 1, blocks).await;
    orch.tick().await;
    assert_eq!(
        *h.ocr_calls.lock().unwrap(),
        2,
        "a failed OCR pass must be retried, not cached as unchanged"
    );
    assert_eq!(h.translate.calls(), ["Hello"]);
}

#[tokio::test]
async fn capture_once_translates_with_oneshot_origin_and_no_dedup() {
    let (mut h, mut orch) = harness().await;

    h.images.send(frame_for(&[block(0, 0, "Hello")])).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    orch.capture_once(REGION).await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { origin, .. } => assert_eq!(origin, EventOrigin::OneShot),
        other => panic!("expected Translated, got {other:?}"),
    }

    // One-shot has no tracking state: the same text translates again.
    h.images.send(frame_for(&[block(0, 0, "Hello")])).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(0, 0, "Hello")]))
        .await
        .unwrap();
    orch.capture_once(REGION).await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello"]);

    // The zone loop's tracking state was untouched: z1's first sight of the
    // same text still translates through the normal path.
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello", "Hello"]);
}

#[tokio::test]
async fn captured_blocks_are_reported_in_screen_coordinates() {
    let (mut h, mut orch) = harness().await;
    set_zones(
        &h,
        vec![zone(
            "z1",
            "Zone 1",
            Region {
                x: 1000,
                y: 500,
                width: 200,
                height: 100,
            },
        )],
    );

    feed_tick(&h, 1, vec![block(10, 20, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { block, .. } => {
            assert_eq!(block.x, 1010, "block.x = zone origin + region-relative x");
            assert_eq!(block.y, 520, "block.y = zone origin + region-relative y");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn event_timestamps_come_from_the_clock() {
    let (mut h, mut orch) = harness().await;

    h.clock.set_now_ms(12345);
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { at_ms, .. } => assert_eq!(at_ms, 12345),
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- Removals ----

#[tokio::test]
async fn disappeared_blocks_are_removed_with_matching_keys() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(100, 200, "Hello")];

    // Tick 1: "Hello" at (100, 200) → grid key "25,50".
    feed_tick(&h, 1, hello).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { key, block, .. } => {
            assert_eq!(key, "25,50", "region-relative grid key");
            assert_eq!(block.x, 200, "screen coordinates");
            assert_eq!(block.y, 300);
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 2: the block is gone — first miss clears nothing yet.
    feed_tick(&h, 1, vec![]).await;
    orch.tick().await;
    assert_no_event(&mut h.events, "one missed pass must not remove a block yet").await;

    // Tick 3: the screen is unchanged since tick 2 (still no block) — the
    // fast path advances the miss count too, and the second consecutive
    // miss removes it.
    feed_unchanged(&h, 1, vec![]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { origin, keys, .. } => {
            assert_eq!(
                origin,
                EventOrigin::Zone {
                    id: "z1".into(),
                    name: "Zone 1".into()
                }
            );
            assert_eq!(keys, vec!["25,50".to_string()]);
        }
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }
}

#[tokio::test]
async fn one_flaky_empty_pass_does_not_clear_a_block() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated

    // One pass OCRs nothing (Windows OCR glitch), then the block is back.
    // The glitch must not remove anything, and the returning block must
    // still be deduped (same pixels → no re-translation).
    feed_tick(&h, 1, vec![]).await;
    orch.tick().await;
    feed_tick(&h, 1, hello).await;
    orch.tick().await;
    assert_no_event(&mut h.events, "a flaky empty pass must produce no events").await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "no re-translation after the glitch"
    );
}

#[tokio::test]
async fn removals_list_only_blocks_that_disappeared() {
    let (mut h, mut orch) = harness().await;

    // Two blocks: "Alpha" at (0, 0) → "0,0"; "Beta" at (0, 50) → "0,12".
    feed_tick(&h, 1, vec![block(0, 0, "Alpha"), block(0, 50, "Beta")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Alpha
    let _ = next_event(&mut h.events).await; // Beta

    // Only Beta disappears: two missed passes later the removal names Beta
    // alone, and unchanged Alpha produces no Translated event.
    feed_tick(&h, 1, vec![block(0, 0, "Alpha")]).await;
    orch.tick().await;
    feed_unchanged(&h, 1, vec![block(0, 0, "Alpha")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { keys, .. } => assert_eq!(keys, vec!["0,12".to_string()]),
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }
    assert_no_event(
        &mut h.events,
        "surviving blocks must not emit Translated events",
    )
    .await;
}

#[tokio::test]
async fn failed_capture_or_ocr_keeps_previous_blocks_visible() {
    let (mut h, mut orch) = harness().await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // drain Translated

    // Capture fails this tick: nothing provably disappeared, so no removal.
    drop(h.images);
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Capture),
        other => panic!("expected Error, got {other:?}"),
    }
    assert_no_event(&mut h.events, "a failed capture must not remove blocks").await;

    // Same for a failed OCR pass: fresh harness, a changed image queued
    // (so OCR runs), and the recognizer errors.
    let (mut h2, mut orch2) = harness().await;
    feed_tick(&h2, 1, vec![block(0, 0, "Hello")]).await;
    orch2.tick().await;
    let _ = next_event(&mut h2.events).await;
    let blocks = vec![block(0, 0, "Hello")];
    let frame = frame_animated(&blocks, 1, text_mark("Hello"));
    h2.images.send(frame).await.unwrap();
    h2.ocr_results
        .send(Err(OcrError::new("pack missing")))
        .await
        .unwrap();
    orch2.tick().await;
    match next_event(&mut h2.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Ocr),
        other => panic!("expected Error, got {other:?}"),
    }
    assert_no_event(&mut h2.events, "a failed OCR pass must not remove blocks").await;
}

#[tokio::test]
async fn blocks_moving_or_changing_produce_new_keys_and_remove_the_old_ones() {
    let (mut h, mut orch) = harness().await;

    // "Hello" sits at (0, 0); next ticks it moved 100px down (new grid cell).
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    feed_tick(&h, 1, vec![block(0, 100, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { key, .. } => assert_eq!(key, "0,25"),
        other => panic!("expected Translated, got {other:?}"),
    }
    assert_no_event(
        &mut h.events,
        "the first missed pass at the old position removes nothing yet",
    )
    .await;

    // Second pass at the new position (pixel-identical screen → fast path):
    // the old key hits its second miss.
    feed_unchanged(&h, 1, vec![block(0, 100, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { keys, .. } => assert_eq!(keys, vec!["0,0".to_string()]),
        other => panic!("expected BlocksRemoved for the old position, got {other:?}"),
    }
}

#[tokio::test]
async fn text_returning_after_removal_is_retranslated() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated

    // Gone for two consecutive passes → removed (pill cleared)…
    feed_tick(&h, 1, vec![]).await;
    orch.tick().await;
    feed_unchanged(&h, 1, vec![]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { .. } => {}
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }

    // …then the same pixels come back at the same place: they must
    // translate again (the removal dropped the block's state, so the
    // screen is not left untranslated).
    feed_tick(&h, 1, hello).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { .. } => {}
        other => panic!("expected Translated for the returning block, got {other:?}"),
    }
}

#[tokio::test]
async fn whitespace_blocks_are_ignored() {
    let (mut h, mut orch) = harness().await;

    feed_tick(
        &h,
        1,
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
    assert_no_event(&mut h.events, "whitespace blocks must not produce events").await;
}

#[tokio::test]
async fn run_loop_paces_ticks_with_the_clock_and_keeps_dedup_across_ticks() {
    let (h, orch) = harness().await;
    let blocks = vec![block(0, 0, "Hello")];

    let task = tokio::spawn(orch.run());

    // Tick 1 runs as soon as a frame is available; then the loop sleeps on
    // the fake clock gate (the stored Notify permit satisfies that sleep).
    feed(&h, 1, &frame_for(&blocks), Some(blocks.clone())).await;
    h.clock.release_tick();
    wait_until(|| h.translate.calls() == ["Hello"], "first tick translated").await;

    // Tick 2: the released gate wakes the sleep; the frame is pixel-identical
    // (fast path), so no new engine call happens even across loop ticks.
    feed(&h, 1, &frame_for(&blocks), None).await;
    h.clock.release_tick();
    wait_until(|| *h.capture_calls.lock().unwrap() >= 2, "second tick ran").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "unchanged block must not re-translate across loop ticks"
    );

    task.abort();
}

// ---- Excluded Blocks (ticket 04) ----

#[tokio::test]
async fn excluded_blocks_are_never_translated_or_displayed() {
    let (mut h, mut orch) = harness().await;

    // block(100, 200) sits at screen (200, 300); the exclusion covers it.
    // A second block at (100, 500) stays translatable.
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 20)])
        .unwrap();
    let blocks = vec![block(100, 200, "Hello"), block(100, 500, "World")];
    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;

    assert_eq!(
        h.translate.calls(),
        ["World"],
        "the covered block must not reach the translate engine"
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] World"),
        other => panic!("expected Translated for the uncovered block, got {other:?}"),
    }
    assert_no_event(
        &mut h.events,
        "the excluded block must not produce any event",
    )
    .await;

    // And it stays skipped while the exclusion is in force (never deduped
    // into silence — it is simply not part of the pipeline).
    feed_unchanged(&h, 1, blocks).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["World"]);
    assert_no_event(&mut h.events, "the excluded block must stay silent across ticks").await;
}

#[tokio::test]
async fn exclusion_tolerates_ocr_jitter_but_not_new_text() {
    let (mut h, mut orch) = harness().await;

    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 20)])
        .unwrap();
    // 3px of jitter: still overlapping the exclusion by ~85%.
    feed_tick(&h, 1, vec![block(103, 203, "Hello")]).await;
    orch.tick().await;
    assert!(
        h.translate.calls().is_empty(),
        "a jittered block at an excluded position must stay excluded"
    );
    assert_no_event(&mut h.events, "no events for the jittered excluded block").await;

    // A different block elsewhere is untouched by the exclusion.
    feed_tick(&h, 1, vec![block(0, 0, "Hi")]).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hi"]);
}

#[tokio::test]
async fn clearing_an_exclusion_translates_the_block_again() {
    let (mut h, mut orch) = harness().await;
    let blocks = vec![block(100, 200, "Hello")];

    // Tick 1: block visible and translated ("pill on screen").
    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // The user marks it excluded in Adjust Mode: skipped from here on.
    // The exclusion change forces an OCR pass even though the screen has
    // not changed, so the block is dropped immediately. (The forced pass
    // calls OCR, so a result must be queued — feed_tick, not feed_unchanged.)
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 20)])
        .unwrap();
    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"], "no re-translation while excluded");
    assert_no_event(&mut h.events, "no events for the newly excluded block").await;

    // The user clears the exclusion in the Results Window: the block must
    // be translated again (its tracking state was dropped while excluded).
    h.excluded_tx.send(vec![]).unwrap();
    feed_tick(&h, 1, blocks).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Hello"],
        "clearing the exclusion re-translates the block"
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Hello"),
        other => panic!("expected Translated after clearing, got {other:?}"),
    }
}

#[tokio::test]
async fn one_shot_pass_honors_exclusions() {
    let (mut h, orch) = harness().await;

    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 20)])
        .unwrap();
    h.images.send(frame_for(&[block(100, 200, "Hello")])).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(100, 200, "Hello")]))
        .await
        .unwrap();
    orch.capture_once(REGION).await;

    assert!(
        h.translate.calls().is_empty(),
        "one-shot must skip excluded blocks too, calls were: {:?}",
        h.translate.calls()
    );
    assert_no_event(
        &mut h.events,
        "the excluded block must not produce a one-shot event",
    )
    .await;
}
