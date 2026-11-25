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
//! The ticket-47 timing tests pin exact stage durations by charging a
//! per-call fake-clock cost to the stage fakes.

use async_trait::async_trait;
use langception_core::glossary::Glossary;
use langception_core::orchestrator::{
    BlockTrace, EventOrigin, LoopConfig, LoopEvent, Orchestrator, ReadDisposition, Stage, Via,
};
use langception_core::ports::{
    CaptureError, CaptureSource, CapturedImage, Clock, ExcludedBlock, FusedBlock, FusedEngine,
    FusedError, OcrEngine, OcrError, Region, SourceLanguage, TextBlock, TranslateEngine,
    TranslateError, Zone,
};
use langception_core::prompt::BuiltinPreset;
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
        genderless: false,
        auto_colors: false,
        translation_prompt_override: None,
        fused_prompt_override: None,
        prompt_preset: None,
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
    /// Fake-clock cost per call (ticket 47); unpinned = free.
    cost: CostPin,
}

impl FakeCapture {
    fn new() -> (mpsc::Sender<CapturedImage>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                images: AsyncMutex::new(rx),
                calls: Arc::new(Mutex::new(0)),
                cost: CostPin::default(),
            },
        )
    }
}

#[async_trait]
impl CaptureSource for FakeCapture {
    async fn capture(&self, _region: Region) -> Result<CapturedImage, CaptureError> {
        *self.calls.lock().unwrap() += 1;
        self.cost.charge();
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
    /// Fake-clock cost per call (ticket 47); unpinned = free.
    cost: CostPin,
}

impl FakeOcr {
    fn new() -> (mpsc::Sender<Result<Vec<TextBlock>, OcrError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                results: AsyncMutex::new(rx),
                calls: Arc::new(Mutex::new(0)),
                cost: CostPin::default(),
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
        self.cost.charge();
        let mut rx = self.results.lock().await;
        rx.recv().await.unwrap_or(Ok(Vec::new()))
    }
}

/// Records every requested translation and answers from a scripted queue
/// (default: `[TH] <text>`). It stands in for the fake HTTP LLM engine of
/// ticket 11: it also records the per-block prompt guidance it was handed
/// (glossary entries + genderless flag), which is what an LLM adapter would
/// fold into its request body.
struct FakeTranslate {
    requests: Mutex<Vec<String>>,
    guided: Mutex<Vec<GuidedCall>>,
    responses: AsyncMutex<mpsc::Receiver<Result<String, TranslateError>>>,
    /// Fake-clock cost per call (ticket 47); unpinned = free.
    cost: CostPin,
}

/// One `translate_guided` call: the text plus the guidance the orchestrator
/// computed for its block.
#[derive(Debug, Clone, PartialEq)]
struct GuidedCall {
    text: String,
    glossary: Vec<(String, String)>,
    genderless: bool,
    custom_base: Option<String>,
    preset: Option<String>,
    recent_translations: Vec<(String, String)>,
}

impl FakeTranslate {
    fn new() -> (mpsc::Sender<Result<String, TranslateError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                requests: Mutex::new(Vec::new()),
                guided: Mutex::new(Vec::new()),
                responses: AsyncMutex::new(rx),
                cost: CostPin::default(),
            },
        )
    }

    fn calls(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn guided_calls(&self) -> Vec<GuidedCall> {
        self.guided.lock().unwrap().clone()
    }

    /// The shared answer path of both port methods: a scripted response
    /// (queued before the tick) wins; otherwise the default translation.
    /// Never awaits an empty channel — that would deadlock the tick.
    async fn answer(&self, text: &str) -> Result<String, TranslateError> {
        let mut rx = self.responses.lock().await;
        match rx.try_recv() {
            Ok(response) => response,
            Err(_) => Ok(format!("[TH] {text}")),
        }
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
        self.cost.charge();
        self.answer(text).await
    }

    async fn translate_guided(
        &self,
        text: &str,
        _source: SourceLanguage,
        _target: &str,
        guidance: &langception_core::ports::TranslationGuidance,
    ) -> Result<String, TranslateError> {
        self.requests.lock().unwrap().push(text.to_string());
        self.cost.charge();
        self.guided.lock().unwrap().push(GuidedCall {
            text: text.to_string(),
            glossary: guidance.glossary.clone(),
            genderless: guidance.genderless,
            custom_base: guidance.custom_base.clone(),
            preset: guidance.preset.clone(),
            recent_translations: guidance.recent_translations.clone(),
        });
        self.answer(text).await
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

/// Interior-mutable fake-clock cost for one stage's fake (ticket 47): the
/// timing tests pin how many ms each stage call takes, and the fake
/// advances the shared clock by that amount per call — exactly what a real
/// stage's latency does to the system clock. Unpinned (the default) the
/// stage is free, so no pre-existing test sees its clock move.
#[derive(Default)]
struct CostPin(Mutex<Option<(Arc<FakeClock>, u64)>>);

impl CostPin {
    /// Pins this stage's per-call cost against the shared fake clock.
    fn pin(&self, clock: &Arc<FakeClock>, ms: u64) {
        *self.0.lock().unwrap() = Some((Arc::clone(clock), ms));
    }

    fn charge(&self) {
        if let Some((clock, ms)) = self.0.lock().unwrap().as_ref() {
            *clock.now.lock().unwrap() += *ms;
        }
    }
}

/// The fused vision engine fake (ticket 12): records every call — the frame
/// it was handed plus the guidance the orchestrator computed (the WHOLE
/// glossary, since a fused call sees the image rather than one block's
/// text) — and answers from a scripted queue (default: no blocks found).
struct FakeFused {
    calls: Mutex<Vec<FusedCallRecord>>,
    responses: AsyncMutex<mpsc::Receiver<Result<Vec<FusedBlock>, FusedError>>>,
    /// Fake-clock cost per call (ticket 47); unpinned = free.
    cost: CostPin,
}

/// One `recognize_and_translate` call.
#[derive(Debug, Clone)]
struct FusedCallRecord {
    width: u32,
    height: u32,
    glossary: Vec<(String, String)>,
    genderless: bool,
    custom_base: Option<String>,
    preset: Option<String>,
    recent_translations: Vec<(String, String)>,
    target: String,
}

impl FakeFused {
    fn new() -> (mpsc::Sender<Result<Vec<FusedBlock>, FusedError>>, Self) {
        let (tx, rx) = mpsc::channel(16);
        (
            tx,
            Self {
                calls: Mutex::new(Vec::new()),
                responses: AsyncMutex::new(rx),
                cost: CostPin::default(),
            },
        )
    }

    fn calls(&self) -> Vec<FusedCallRecord> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl FusedEngine for FakeFused {
    async fn recognize_and_translate(
        &self,
        image: &CapturedImage,
        _source: SourceLanguage,
        target: &str,
        guidance: &langception_core::ports::TranslationGuidance,
    ) -> Result<Vec<FusedBlock>, FusedError> {
        self.calls.lock().unwrap().push(FusedCallRecord {
            width: image.width,
            height: image.height,
            glossary: guidance.glossary.clone(),
            genderless: guidance.genderless,
            custom_base: guidance.custom_base.clone(),
            preset: guidance.preset.clone(),
            recent_translations: guidance.recent_translations.clone(),
            target: target.to_string(),
        });
        self.cost.charge();
        let mut rx = self.responses.lock().await;
        rx.recv().await.unwrap_or(Ok(Vec::new()))
    }

    fn name(&self) -> &str {
        "FakeFused"
    }

    fn is_cloud(&self) -> bool {
        true
    }
}

struct Harness {
    events: mpsc::UnboundedReceiver<LoopEvent>,
    images: mpsc::Sender<CapturedImage>,
    ocr_results: mpsc::Sender<Result<Vec<TextBlock>, OcrError>>,
    responses: mpsc::Sender<Result<String, TranslateError>>,
    fused_responses: mpsc::Sender<Result<Vec<FusedBlock>, FusedError>>,
    capture_calls: Arc<Mutex<u32>>,
    ocr_calls: Arc<Mutex<u32>>,
    /// The Arc'd stage fakes, kept so the timing tests (ticket 47) can pin
    /// per-call fake-clock costs after construction.
    capture: Arc<FakeCapture>,
    ocr: Arc<FakeOcr>,
    translate: Arc<FakeTranslate>,
    fused: Arc<FakeFused>,
    clock: Arc<FakeClock>,
    zones_tx: watch::Sender<Vec<Zone>>,
    paused_tx: watch::Sender<bool>,
    /// Picker suspension (ticket 46): tests send `true` while the (fake)
    /// picker is open with Capture Visibility on.
    suspended_tx: watch::Sender<bool>,
    excluded_tx: watch::Sender<Vec<ExcludedBlock>>,
    /// Translation Context window (ticket 44): tests send the live value
    /// here; 0 = off (every harness's default).
    context_tx: watch::Sender<u32>,
}

impl Harness {
    /// Pins per-call fake-clock stage costs (ticket 47): each capture call
    /// advances the clock `capture_ms`, each read call (OCR or fused)
    /// `read_ms`, each translate call `translate_ms`. The orchestrator's
    /// timing events then carry these exact durations.
    fn pin_stage_costs(&self, capture_ms: u64, read_ms: u64, translate_ms: u64) {
        self.capture.cost.pin(&self.clock, capture_ms);
        self.ocr.cost.pin(&self.clock, read_ms);
        self.fused.cost.pin(&self.clock, read_ms);
        self.translate.cost.pin(&self.clock, translate_ms);
    }
}

/// The harness starts watching one zone (z1 = REGION), like the app does
/// after a first zone was drawn, with no Excluded Blocks in force. With
/// `fused` set the orchestrator runs the ticket-12 fused pipeline: every
/// read is ONE vision-model call and the OCR + translate fakes stay idle.
async fn harness_inner(
    glossary: Glossary,
    stability: Duration,
    genderless: bool,
    fused: bool,
    auto_colors: bool,
    translation_prompt_override: Option<String>,
    fused_prompt_override: Option<String>,
) -> (Harness, Orchestrator) {
    harness_with_config(
        glossary,
        fused,
        LoopConfig {
            stability,
            genderless,
            auto_colors,
            translation_prompt_override,
            fused_prompt_override,
            ..config()
        },
    )
    .await
}

/// The harness around a loop config given wholesale, for inputs that have
/// no parameter of their own above (the prompt preset, ticket 42).
async fn harness_with_config(
    glossary: Glossary,
    fused: bool,
    config: LoopConfig,
) -> (Harness, Orchestrator) {
    let (images_tx, capture) = FakeCapture::new();
    let capture_calls = Arc::clone(&capture.calls);
    let capture = Arc::new(capture);
    let (ocr_tx, ocr) = FakeOcr::new();
    let ocr_calls = Arc::clone(&ocr.calls);
    let ocr = Arc::new(ocr);
    let (responses_tx, translate) = FakeTranslate::new();
    let translate = Arc::new(translate);
    let (fused_responses_tx, fused_fake) = FakeFused::new();
    let fused_fake = Arc::new(fused_fake);
    let clock = Arc::new(FakeClock::new());
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (paused_tx, paused_rx) = watch::channel(false);
    let (suspended_tx, suspended_rx) = watch::channel(false);
    let (excluded_tx, excluded_rx) = watch::channel(Vec::new());
    let (context_tx, context_rx) = watch::channel(0u32);

    let orchestrator = Orchestrator::new(
        Arc::clone(&capture) as Arc<dyn CaptureSource>,
        Arc::clone(&ocr) as Arc<dyn OcrEngine>,
        translate.clone(),
        clock.clone(),
        config,
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    )
    .with_fused(fused.then(|| Arc::clone(&fused_fake) as Arc<dyn FusedEngine>))
    .with_suspended_rx(suspended_rx)
    .with_context_rx(context_rx);

    (
        Harness {
            events: events_rx,
            images: images_tx,
            ocr_results: ocr_tx,
            responses: responses_tx,
            fused_responses: fused_responses_tx,
            capture_calls,
            ocr_calls,
            capture,
            ocr,
            translate,
            fused: fused_fake,
            clock,
            zones_tx,
            paused_tx,
            suspended_tx,
            excluded_tx,
            context_tx,
        },
        orchestrator,
    )
}

async fn harness_with(
    glossary: Glossary,
    stability: Duration,
    genderless: bool,
) -> (Harness, Orchestrator) {
    harness_inner(glossary, stability, genderless, false, false, None, None).await
}

/// The fused pipeline (ticket 12): reads are one vision-model call.
async fn harness_with_fused(
    glossary: Glossary,
    stability: Duration,
    genderless: bool,
) -> (Harness, Orchestrator) {
    harness_inner(glossary, stability, genderless, true, false, None, None).await
}

/// Auto Colors (ticket 14): fired blocks carry sampled per-block colors.
async fn harness_auto_colors() -> (Harness, Orchestrator) {
    harness_inner(Glossary::default(), Duration::ZERO, false, false, true, None, None).await
}

/// Custom LLM prompt bases configured (ticket 37): the overrides ride at
/// loop start, exactly as the shell wires them from `[prompts]`.
async fn harness_with_prompt_overrides(
    translation_prompt_override: Option<String>,
    fused_prompt_override: Option<String>,
) -> (Harness, Orchestrator) {
    harness_inner(
        Glossary::default(),
        Duration::ZERO,
        false,
        false,
        false,
        translation_prompt_override,
        fused_prompt_override,
    )
    .await
}

async fn harness() -> (Harness, Orchestrator) {
    harness_with(Glossary::default(), Duration::ZERO, false).await
}

async fn harness_with_stability(ms: u64) -> (Harness, Orchestrator) {
    harness_with(Glossary::default(), Duration::from_millis(ms), false).await
}

/// Genderless Mode on: the loop scrubs gendered Thai from engine output.
async fn harness_genderless() -> (Harness, Orchestrator) {
    harness_with(Glossary::default(), Duration::ZERO, true).await
}

fn set_zones(h: &Harness, zones: Vec<Zone>) {
    h.zones_tx.send(zones).unwrap();
}

/// The next display event (ticket 47: every pass also emits a Timing event
/// on the same channel; tests that don't pin timings skip them here — the
/// timing tests read them explicitly via [`next_timing`]).
async fn next_event(events: &mut mpsc::UnboundedReceiver<LoopEvent>) -> LoopEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event should arrive within 2s")
            .expect("event channel should stay open");
        if !matches!(event, LoopEvent::Timing { .. }) {
            return event;
        }
    }
}

/// Asserts no event other than the per-pass Timing arrives (ticket 47 made
/// every pass emit one — unchanged ticks included — so the display-event
/// assertions below skip it; the timing tests pin the timing events
/// themselves).
async fn assert_no_event(events: &mut mpsc::UnboundedReceiver<LoopEvent>, what: &str) {
    loop {
        match tokio::time::timeout(Duration::from_millis(100), events.recv()).await {
            // Nothing (else) arrived: the assertion holds.
            Err(_elapsed) => return,
            Ok(None) => panic!("{what}: event channel closed"),
            // The per-pass timing row, not a display event.
            Ok(Some(LoopEvent::Timing { .. })) => continue,
            Ok(Some(other)) => panic!("{what}: got {other:?}"),
        }
    }
}

/// The next Timing event (ticket 47), skipping the display events that
/// fired earlier in the same pass — order-sensitive tests consume those via
/// [`next_event`] first.
async fn next_timing(events: &mut mpsc::UnboundedReceiver<LoopEvent>) -> LoopEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("timing event should arrive within 2s")
            .expect("event channel should stay open");
        if matches!(event, LoopEvent::Timing { .. }) {
            return event;
        }
    }
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
            h.ocr_results.send(Ok(blocks.clone())).await.unwrap();
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
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "a stable change fires exactly once"
    );
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

// ---- Text-level dedup (ticket 22) ----
//
// Pixel-level change detection cannot see that a re-read found the SAME
// text: jitter around unchanged glyphs (animation, antialiasing) changes a
// block's dHash, the stability window matures, and the engine would be
// called again with identical text. These tests pin the text-level
// suppression on the shared fire path: a read whose trimmed text equals
// the block's last SHOWN translation is dropped — no engine call, no
// event, for both reading modes.

#[tokio::test]
async fn identical_text_on_jittering_pixels_translates_once() {
    let (mut h, mut orch) = harness().await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1: "Hello" appears → translated once.
    feed(&h, 1, &frame_animated(&blocks, 0, 7), Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // Ticks 2–3: the pixels keep changing (animation) but every read says
    // "Hello" — no engine call, no event: the overlay already shows that
    // translation.
    for anim in [8u8, 9] {
        let frame = frame_animated(&blocks, anim, anim);
        feed(&h, 1, &frame, Some(blocks.clone())).await;
        orch.tick().await;
    }
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "identical text on changed pixels must not re-translate"
    );
    assert_no_event(&mut h.events, "suppressed re-reads must not emit").await;

    // A genuine text change still translates normally. ("Farewell" rather
    // than "Goodbye": text_mark("Goodbye") paints the same marker cells as
    // anim 9 above, which would leave the block's pixels — and dHash —
    // unchanged on this tick.)
    let changed = frame_animated(&blocks, 10, text_mark("Farewell"));
    feed(&h, 1, &changed, Some(vec![block(0, 0, "Farewell")])).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Farewell"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Farewell"),
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn suppression_also_gates_matured_fast_path_fires() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1 (t=0): change detected, window starts.
    h.clock.set_now_ms(0);
    let frame_a = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &frame_a, Some(blocks.clone())).await;
    orch.tick().await;
    assert!(h.translate.calls().is_empty());

    // Tick 2 (t=500): the same frame → fast path → the matured pending
    // fires; "Hello" is translated and shown.
    h.clock.set_now_ms(500);
    feed(&h, 1, &frame_a, None).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // Ticks 3–6: the pixels change twice more, each change holds stable
    // and matures on a fast-path tick, but every read still says "Hello" —
    // each matured fire is suppressed: no engine call, no event.
    for (corner, anim) in [(232u8, 8u8), (208, 9)] {
        h.clock.set_now_ms(corner as u64 * 10);
        let frame = frame_animated(&blocks, corner, anim);
        feed(&h, 1, &frame, Some(blocks.clone())).await;
        orch.tick().await;
        assert_eq!(
            h.translate.calls(),
            ["Hello"],
            "no re-fire while the text is unchanged"
        );

        h.clock.set_now_ms(corner as u64 * 10 + 500);
        feed(&h, 1, &frame, None).await;
        orch.tick().await;
        assert_eq!(
            h.translate.calls(),
            ["Hello"],
            "the matured fast-path fire must be suppressed for identical text"
        );
    }
    assert_no_event(&mut h.events, "suppressed matured fires must not emit").await;
}

#[tokio::test]
async fn suppression_gates_the_in_pass_matured_fire_too() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Establish the shown translation: detected at t=0, fired on the
    // fast path at t=500.
    h.clock.set_now_ms(0);
    let base = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &base, Some(blocks.clone())).await;
    orch.tick().await;
    h.clock.set_now_ms(500);
    feed(&h, 1, &base, None).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);
    let _ = next_event(&mut h.events).await;

    // The block's pixels change (anim 8, window restarts at t=1000), then
    // hold while something ELSE in the zone changes: the window matures
    // during a later OCR pass, not on the fast path — and the identical
    // text is suppressed at that branch too.
    h.clock.set_now_ms(1000);
    feed(&h, 1, &frame_animated(&blocks, 1, 8), Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"]);

    h.clock.set_now_ms(1500);
    feed(&h, 1, &frame_animated(&blocks, 2, 8), Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "the in-pass matured fire must be suppressed for identical text"
    );
    assert_no_event(&mut h.events, "suppressed in-pass fires must not emit").await;
}

#[tokio::test]
async fn a_failed_translation_is_retried_on_changed_pixels_despite_identical_text() {
    let (mut h, mut orch) = harness().await;

    // Tick 1: the engine fails for "Hello" — the error surfaces and
    // nothing is shown, so there is no shown text to suppress against.
    h.responses
        .send(Err(TranslateError::new("HTTP 503 unavailable")))
        .await
        .unwrap();
    feed(
        &h,
        1,
        &frame_animated(&[block(0, 0, "Hello")], 0, 7),
        Some(vec![block(0, 0, "Hello")]),
    )
    .await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Translate),
        other => panic!("expected Error, got {other:?}"),
    }
    assert_eq!(h.translate.calls(), ["Hello"]);

    // Tick 2: the pixels change (jitter) but the text is the SAME. The
    // earlier fire failed, so the suppression record is empty — the engine
    // is tried again (retry-on-pixel-change is preserved for failures).
    feed(
        &h,
        1,
        &frame_animated(&[block(0, 0, "Hello")], 1, 8),
        Some(vec![block(0, 0, "Hello")]),
    )
    .await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Hello"],
        "a failed read must be retried when the pixels change again"
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "[TH] Hello"),
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 3: now that a translation IS shown, identical re-reads are
    // suppressed again.
    feed(
        &h,
        1,
        &frame_animated(&[block(0, 0, "Hello")], 2, 9),
        Some(vec![block(0, 0, "Hello")]),
    )
    .await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello", "Hello"]);
    assert_no_event(&mut h.events, "suppressed again after the successful retry").await;
}

#[tokio::test]
async fn reappearance_after_removal_retranslates_identical_text() {
    let (mut h, mut orch) = harness().await;
    let hello = vec![block(0, 0, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // Gone for two consecutive passes → removed (the state, including the
    // shown-text record, is dropped).
    feed_tick(&h, 1, vec![]).await;
    orch.tick().await;
    feed_unchanged(&h, 1, vec![]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { .. } => {}
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }

    // It reappears with the same text on jittered pixels: the cleared
    // record must not suppress the fresh translation.
    feed(&h, 1, &frame_animated(&hello, 1, 9), Some(hello)).await;
    orch.tick().await;
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Hello"],
        "reappearance re-translates even though the text is unchanged"
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { .. } => {}
        other => panic!("expected Translated for the returning block, got {other:?}"),
    }
}

#[tokio::test]
async fn fused_path_suppresses_identical_text_too() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1: read + translated in one fused call.
    feed_fused(
        &h,
        &frame_for(&blocks),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(h.fused.calls().len(), 1);

    // Tick 2: the frame changed (animation), the model reads the same
    // "Hello" — its answer is dropped before display: no event. The model
    // itself WAS called (suppression sits after the read, on the fire).
    feed_fused(
        &h,
        &frame_animated(&blocks, 1, 8),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]),
    )
    .await;
    orch.tick().await;
    assert_eq!(h.fused.calls().len(), 2);
    assert_no_event(&mut h.events, "an identical fused answer must not re-emit").await;

    // Tick 3: a genuine text change flows through normally.
    feed_fused(
        &h,
        &frame_animated(&blocks, 2, text_mark("Goodbye")),
        Ok(vec![fused_block(0, 0, "Goodbye", "ลาก่อน")]),
    )
    .await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "ลาก่อน"),
        other => panic!("expected Translated, got {other:?}"),
    }
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

// ---- Picker suspension (ticket 46) ----

#[tokio::test]
async fn suspended_loop_skips_ticks_and_resume_keeps_dedup() {
    let (h, orch) = harness().await;
    let task = tokio::spawn(orch.run());
    let blocks = vec![block(0, 0, "Hello")];

    // Translate once so there is dedup state to protect across the
    // suspension — the picker must not cost a re-translation of the same
    // pixels when it closes.
    feed(&h, 1, &frame_for(&blocks), Some(blocks.clone())).await;
    wait_until(
        || h.translate.calls() == ["Hello"],
        "translated before suspension",
    )
    .await;

    // Picker open (suspension on): releases tick but no capture happens.
    h.suspended_tx.send(true).unwrap();
    h.clock.release_tick();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        *h.capture_calls.lock().unwrap(),
        1,
        "suspended loop must not capture"
    );

    // Picker closed: the loop resumes, captures the pixel-identical frame,
    // and the unchanged block stays deduped — no second translation.
    h.suspended_tx.send(false).unwrap();
    h.clock.release_tick();
    wait_until(
        || *h.capture_calls.lock().unwrap() >= 2,
        "resumed tick captured",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "dedup must survive suspension/resume"
    );

    task.abort();
}

#[tokio::test]
async fn suspension_holds_together_with_pause_until_both_release() {
    let (h, orch) = harness().await;
    let task = tokio::spawn(orch.run());

    // Both flags in force: releasing EITHER one alone keeps the loop held.
    h.paused_tx.send(true).unwrap();
    h.suspended_tx.send(true).unwrap();
    h.clock.release_tick();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(*h.capture_calls.lock().unwrap(), 0, "still held");

    h.paused_tx.send(false).unwrap();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        *h.capture_calls.lock().unwrap(),
        0,
        "suspension still holds after the user paused-resumed"
    );

    h.suspended_tx.send(false).unwrap();
    h.clock.release_tick();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        *h.capture_calls.lock().unwrap(),
        1,
        "the loop ticks once both flags released"
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
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;
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

    h.images
        .send(frame_for(&[block(0, 0, "Hello")]))
        .await
        .unwrap();
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
    h.images
        .send(frame_for(&[block(0, 0, "Hello")]))
        .await
        .unwrap();
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
    assert_no_event(
        &mut h.events,
        "the excluded block must stay silent across ticks",
    )
    .await;
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
    assert_eq!(
        h.translate.calls(),
        ["Hello"],
        "no re-translation while excluded"
    );
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
    h.images
        .send(frame_for(&[block(100, 200, "Hello")]))
        .await
        .unwrap();
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

// ---- Genderless Mode (ticket 08) ----

#[tokio::test]
async fn genderless_mode_scrubs_engine_output() {
    let (mut h, mut orch) = harness_genderless().await;

    // The fake engine returns gendered Thai (particles + first-person
    // pronouns) for three blocks; the displayed text must be genderless.
    for gendered in ["สวัสดีครับ", "ผมไม่รู้", "ได้นะครับ"]
    {
        h.responses.send(Ok(gendered.to_string())).await.unwrap();
    }
    feed_tick(
        &h,
        1,
        vec![
            block(0, 0, "Hello"),
            block(0, 50, "Bye"),
            block(0, 100, "Okay"),
        ],
    )
    .await;
    orch.tick().await;

    // The scrubber is output-side only: the engine still sees the OCR text.
    assert_eq!(
        h.translate.calls(),
        ["Hello", "Bye", "Okay"],
        "the request side is untouched by the scrubber"
    );
    for expected in ["สวัสดี", "ฉันไม่รู้", "ได้"] {
        match next_event(&mut h.events).await {
            LoopEvent::Translated { translation, via, .. } => {
                assert_eq!(translation, expected);
                assert_eq!(
                    via,
                    Via::Engine,
                    "scrubbed output still comes via the engine"
                );
            }
            other => panic!("expected Translated \"{expected}\", got {other:?}"),
        }
    }
}

#[tokio::test]
async fn genderless_disabled_passes_engine_output_through() {
    let (mut h, mut orch) = harness().await;

    h.responses.send(Ok("สวัสดีครับ".to_string())).await.unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => {
            assert_eq!(
                translation, "สวัสดีครับ",
                "with the mode off, engine output passes through untouched"
            );
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn genderless_mode_never_scrubs_glossary_output() {
    // A Glossary pin is the user's own Thai — the short-circuit sits
    // before the scrubber, so even a gendered entry shows verbatim.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "ข้าคือเวอร์ตินค่ะ".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, true).await;

    feed_tick(&h, 1, vec![block(0, 0, "  vertin  ")]).await;
    orch.tick().await;

    assert!(
        h.translate.calls().is_empty(),
        "glossary match must still short-circuit the engine"
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            translation, via, ..
        } => {
            assert_eq!(translation, "ข้าคือเวอร์ตินค่ะ", "pinned Thai shows verbatim");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- LLM prompt guidance (ticket 11) ----
//
// Prompt construction is asserted at the pipeline seam: the fake engine
// records exactly the guidance the orchestrator computed for the block —
// the same guidance a real LLM adapter (OpenAI-compatible / Gemini) folds
// into its request body. The adapters' request-side rendering of that
// guidance is covered by their offline golden tests (prompt / openai /
// gemini modules).

#[tokio::test]
async fn glossary_entries_appearing_in_the_block_ride_in_the_engine_guidance() {
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());
    glossary
        .0
        .insert("Regulus".to_string(), "เรกุลุส".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;

    // "Vertin" occurs in the sentence; "Regulus" does not. The block is
    // NOT a whole-block match, so the engine is called — with the relevant
    // entry injected, never the irrelevant one.
    feed_tick(&h, 1, vec![block(0, 0, "Vertin picks up the case")]).await;
    orch.tick().await;

    assert_eq!(
        h.translate.guided_calls(),
        vec![GuidedCall {
            text: "Vertin picks up the case".into(),
            glossary: vec![("Vertin".into(), "เวอร์ติน".into())],
            genderless: false,
            custom_base: None,
            preset: None,
            recent_translations: Vec::new(),
        }],
        "exactly the in-sentence glossary entry must reach the engine"
    );
    // The translation flows through the engine path.
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "[TH] Vertin picks up the case");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn blocks_without_glossary_terms_carry_empty_guidance() {
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;

    feed_tick(&h, 1, vec![block(0, 0, "Pay 500 gold")]).await;
    orch.tick().await;

    assert_eq!(
        h.translate.guided_calls(),
        vec![GuidedCall {
            text: "Pay 500 gold".into(),
            glossary: Vec::new(),
            genderless: false,
            custom_base: None,
            preset: None,
            recent_translations: Vec::new(),
        }],
        "no relevant term → an empty glossary injection"
    );
    let _ = next_event(&mut h.events).await;
}

#[tokio::test]
async fn genderless_mode_rides_in_the_engine_guidance() {
    let (mut h, mut orch) = harness_genderless().await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;

    assert_eq!(
        h.translate.guided_calls(),
        vec![GuidedCall {
            text: "Hello".into(),
            glossary: Vec::new(),
            genderless: true,
            custom_base: None,
            preset: None,
            recent_translations: Vec::new(),
        }],
        "the mode-on flag must reach the LLM prompt path"
    );
    let _ = next_event(&mut h.events).await;

    // Mode off → the flag stays false (a fresh harness, default config).
    let (mut h2, mut orch2) = harness().await;
    feed_tick(&h2, 1, vec![block(0, 0, "Hello")]).await;
    orch2.tick().await;
    assert!(!h2.translate.guided_calls()[0].genderless);
    let _ = next_event(&mut h2.events).await;
}

#[tokio::test]
async fn in_sentence_glossary_terms_come_back_per_glossary_in_engine_output() {
    // The full ticket-11 story: "Vertin picks up the case" reaches the LLM
    // with the pinned translation injected, and the model's glossary-faithful
    // answer is what gets displayed (via the engine).
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;
    h.responses
        .send(Ok("เวอร์ตินหยิบคดีขึ้นมา".to_string()))
        .await
        .unwrap();

    feed_tick(&h, 1, vec![block(0, 0, "Vertin picks up the case")]).await;
    orch.tick().await;

    assert_eq!(h.translate.calls(), ["Vertin picks up the case"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            translation,
            via,
            ..
        } => {
            assert_eq!(translation, "เวอร์ตินหยิบคดีขึ้นมา");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn glossary_and_genderless_guidance_compose_for_llm_engines() {
    // Reverse: 1999's combination — pinned names AND no speaker gender, in
    // one guidance payload.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, true).await;

    feed_tick(&h, 1, vec![block(0, 0, "Vertin speaks")]).await;
    orch.tick().await;

    let call = &h.translate.guided_calls()[0];
    assert_eq!(
        call.glossary,
        vec![("Vertin".to_string(), "เวอร์ติน".to_string())]
    );
    assert!(call.genderless);
    let _ = next_event(&mut h.events).await;
}

// ---- Custom LLM prompt overrides (ticket 37) ----
//
// The overrides are configured once (Settings → Engines → Prompts) and ride
// in LoopConfig like the language: the classic path hands the translation
// override to every per-block guidance, the fused path hands the
// screen-reading override to the fused engine's guidance. Non-LLM engines
// discard the guidance wholesale (the port's default), so Google and Azure
// never see a custom prompt.

#[tokio::test]
async fn the_translation_prompt_override_rides_in_the_engine_guidance() {
    let (mut h, mut orch) = harness_with_prompt_overrides(
        Some("Translate {source} speech casually.".into()),
        None,
    )
    .await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;

    assert_eq!(
        h.translate.guided_calls(),
        vec![GuidedCall {
            text: "Hello".into(),
            glossary: Vec::new(),
            genderless: false,
            custom_base: Some("Translate {source} speech casually.".into()),
            preset: None,
            recent_translations: Vec::new(),
        }],
        "the custom translation base must reach the LLM prompt path"
    );
    let _ = next_event(&mut h.events).await;
}

#[tokio::test]
async fn the_screen_reading_override_rides_in_the_fused_guidance_not_the_translate_one() {
    // The fused pipeline with the screen-reading override: the classic
    // wrapper builds no fused engine, so the fused harness is built
    // directly (fused = true).
    let (h, mut orch) = harness_inner(
        Glossary::default(),
        Duration::ZERO,
        false,
        true,
        false,
        None,
        Some("Read the screen like {source} manga.".into()),
    )
    .await;

    // The fused fake answers from its scripted queue: one empty answer (no
    // text on screen) is enough for this guidance-level check.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Start")]),
        Ok(vec![]),
    )
    .await;
    orch.tick().await;

    let calls = h.fused.calls();
    assert_eq!(calls.len(), 1, "the read must be one fused call");
    assert_eq!(
        calls[0].custom_base.as_deref(),
        Some("Read the screen like {source} manga."),
        "the screen-reading override must reach the fused engine"
    );
    // The classic path never ran: no OCR, no translate engine call.
    assert!(h.translate.calls().is_empty());
    assert_eq!(*h.ocr_calls.lock().unwrap(), 0);
}

// ---- System Prompt Presets (ticket 42) ----
//
// The shell resolves the config into the two prompt texts AND the name of
// the preset they are the wording of; both ride in LoopConfig next to each
// other. The orchestrator only hands the name through — the prompt module
// turns it into the version tag.

#[tokio::test]
async fn the_prompt_preset_rides_in_the_translation_engine_guidance() {
    let (mut h, mut orch) = harness_with_config(
        Glossary::default(),
        false,
        LoopConfig {
            translation_prompt_override: Some(BuiltinPreset::Game.texts().translation),
            prompt_preset: Some("game".into()),
            ..config()
        },
    )
    .await;

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;

    assert_eq!(
        h.translate.guided_calls()[0].preset.as_deref(),
        Some("game"),
        "the LLM engine must be told which preset its prompt is the wording of"
    );
    let _ = next_event(&mut h.events).await;
}

#[tokio::test]
async fn the_prompt_preset_rides_in_the_fused_guidance() {
    let (h, mut orch) = harness_with_config(
        Glossary::default(),
        true,
        LoopConfig {
            fused_prompt_override: Some(BuiltinPreset::Manga.texts().screen_reading),
            prompt_preset: Some("manga".into()),
            ..config()
        },
    )
    .await;

    feed_fused(&h, &frame_for(&[block(0, 0, "Hello")]), Ok(vec![])).await;
    orch.tick().await;

    assert_eq!(
        h.fused.calls()[0].preset.as_deref(),
        Some("manga"),
        "the fused engine must be told which preset its prompt is the wording of"
    );
}

// ---- Translation Context (ticket 44) ----
//
// Each zone keeps a rolling buffer of its last X successful translations,
// riding in the same per-block guidance the Glossary uses. Recorded only on
// shown emissions — failures and suppressed identical re-reads (ticket 22)
// never churn it — and the window applies live: the watch value is read per
// pass, so a settings change lands on the very next translation.

#[tokio::test]
async fn with_the_feature_off_nothing_buffers_and_no_guidance_rides() {
    let (mut h, mut orch) = harness().await;

    for text in ["Hello", "Goodbye"] {
        feed_tick(&h, 1, vec![block(0, 0, text)]).await;
        orch.tick().await;
        let _ = next_event(&mut h.events).await;
    }
    let calls = h.translate.guided_calls();
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert!(
            call.recent_translations.is_empty(),
            "the default window (0) must buffer nothing: {:?}",
            call.recent_translations
        );
    }
}

#[tokio::test]
async fn the_zone_context_rides_in_the_engine_guidance_oldest_first() {
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(3).unwrap();

    // Translation 1: no priors.
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // Translation 2: sees exactly the first pair.
    feed_tick(&h, 1, vec![block(0, 0, "Goodbye")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[1].recent_translations,
        vec![("Hello".to_string(), "[TH] Hello".to_string())],
        "the second call must carry the first translation"
    );

    // Translation 3: the first two, oldest first.
    feed_tick(&h, 1, vec![block(0, 0, "Farewell")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[2].recent_translations,
        vec![
            ("Hello".to_string(), "[TH] Hello".to_string()),
            ("Goodbye".to_string(), "[TH] Goodbye".to_string()),
        ]
    );

    // Translation 4 with three priors: exactly those three pairs, oldest →
    // newest (the acceptance criterion's shape).
    feed_tick(&h, 1, vec![block(0, 0, "See you")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[3].recent_translations,
        vec![
            ("Hello".to_string(), "[TH] Hello".to_string()),
            ("Goodbye".to_string(), "[TH] Goodbye".to_string()),
            ("Farewell".to_string(), "[TH] Farewell".to_string()),
        ]
    );
}

#[tokio::test]
async fn the_window_evicts_oldest_first() {
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(2).unwrap();

    for text in ["Alpha", "Beta", "Gamma", "Delta"] {
        feed_tick(&h, 1, vec![block(0, 0, text)]).await;
        orch.tick().await;
        let _ = next_event(&mut h.events).await;
    }
    assert_eq!(
        h.translate.guided_calls()[3].recent_translations,
        vec![
            ("Beta".to_string(), "[TH] Beta".to_string()),
            ("Gamma".to_string(), "[TH] Gamma".to_string()),
        ],
        "a window of 2 must hold the last two translations only"
    );
}

#[tokio::test]
async fn a_failed_translation_never_enters_the_zone_context() {
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(3).unwrap();

    // The engine fails for "Hello": the error surfaces, nothing is shown,
    // nothing records.
    h.responses
        .send(Err(TranslateError::new("HTTP 503 unavailable")))
        .await
        .unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Translate),
        other => panic!("expected Error, got {other:?}"),
    }

    // "Goodbye" succeeds; its guidance carries NO Hello pair.
    feed_tick(&h, 1, vec![block(0, 0, "Goodbye")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert!(
        h.translate.guided_calls()[1]
            .recent_translations
            .is_empty(),
        "a failed translation must never enter the buffer"
    );

    // The next translation sees only the successful pair.
    feed_tick(&h, 1, vec![block(0, 0, "Farewell")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[2].recent_translations,
        vec![("Goodbye".to_string(), "[TH] Goodbye".to_string())]
    );
}

#[tokio::test]
async fn suppressed_identical_rereads_do_not_churn_the_zone_context() {
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(3).unwrap();

    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // The pixels jitter (animation) but the read says "Hello" again: the
    // ticket-22 suppression drops it — no engine call, no event, and the
    // buffer must not gain a duplicate pair.
    for anim in [8u8, 9] {
        let frame = frame_animated(&[block(0, 0, "Hello")], anim, anim);
        feed(&h, 1, &frame, Some(vec![block(0, 0, "Hello")])).await;
        orch.tick().await;
    }
    assert_eq!(h.translate.calls(), ["Hello"], "the re-read is suppressed");
    assert_no_event(&mut h.events, "a suppressed re-read must not emit").await;

    // A genuine change: its guidance still carries exactly one pair.
    feed_tick(&h, 1, vec![block(0, 0, "Goodbye")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[1].recent_translations,
        vec![("Hello".to_string(), "[TH] Hello".to_string())],
        "suppressed reads must not churn the buffer"
    );
}

#[tokio::test]
async fn zone_context_does_not_bleed_across_zones() {
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(3).unwrap();
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

    // Zone 1 sees "Hello", zone 2 sees "World" (each pull takes one queued
    // frame/result, zones processed in order).
    feed(
        &h,
        1,
        &frame_for(&[block(10, 10, "Hello")]),
        Some(vec![block(10, 10, "Hello")]),
    )
    .await;
    feed(
        &h,
        1,
        &frame_for(&[block(10, 10, "World")]),
        Some(vec![block(10, 10, "World")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    let _ = next_event(&mut h.events).await;

    // Each zone's second translation chains off ITS OWN first one only.
    feed(
        &h,
        1,
        &frame_for(&[block(10, 10, "Goodbye")]),
        Some(vec![block(10, 10, "Goodbye")]),
    )
    .await;
    feed(
        &h,
        1,
        &frame_for(&[block(10, 10, "Later")]),
        Some(vec![block(10, 10, "Later")]),
    )
    .await;
    orch.tick().await;
    let calls = h.translate.guided_calls();
    assert_eq!(calls.len(), 4, "z1 Hello, z2 World, z1 Goodbye, z2 Later");
    assert_eq!(
        calls[2].recent_translations,
        vec![("Hello".to_string(), "[TH] Hello".to_string())],
        "zone 1's context"
    );
    assert_eq!(
        calls[3].recent_translations,
        vec![("World".to_string(), "[TH] World".to_string())],
        "zone 2's context — z1's pairs must not bleed in"
    );
}

#[tokio::test]
async fn changing_the_window_applies_live_without_a_restart() {
    let (mut h, mut orch) = harness().await;

    // Window 0 (default): the translation shows but nothing records.
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // Raise to 1 mid-session: the NEXT translation still sees nothing
    // (nothing buffered while off) and becomes the first recorded pair.
    h.context_tx.send(1).unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Goodbye")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert!(h.translate.guided_calls()[1].recent_translations.is_empty());
    feed_tick(&h, 1, vec![block(0, 0, "Farewell")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[2].recent_translations,
        vec![("Goodbye".to_string(), "[TH] Goodbye".to_string())],
        "the window applies live: the pair recorded after the raise rides"
    );

    // Back to 0: nothing injects and nothing is kept — re-enabling later
    // starts from what is translated next, never from a stale backlog.
    h.context_tx.send(0).unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "See you")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert!(h.translate.guided_calls()[3].recent_translations.is_empty());
    h.context_tx.send(2).unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Later")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert!(h.translate.guided_calls()[4].recent_translations.is_empty());
    feed_tick(&h, 1, vec![block(0, 0, "Again")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[5].recent_translations,
        vec![("Later".to_string(), "[TH] Later".to_string())],
        "a re-enabled window builds from fresh translations only"
    );
}

#[tokio::test]
async fn fused_calls_carry_the_zone_context_too() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    h.context_tx.send(2).unwrap();

    // First call: no priors; the block's answer displays and records.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Hello")]),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert!(h.fused.calls()[0].recent_translations.is_empty());

    // A changed frame runs the fused call again: its guidance carries the
    // first pair, exactly as the classic path injects it.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Goodbye")]),
        Ok(vec![fused_block(0, 0, "Goodbye", "ลาก่อน")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.fused.calls()[1].recent_translations,
        vec![("Hello".to_string(), "สวัสดี".to_string())],
        "the fused prompt must see its zone's recent translations"
    );

    // The third call carries both pairs, oldest first.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Farewell")]),
        Ok(vec![fused_block(0, 0, "Farewell", "ราตรีสวัสดิ์")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.fused.calls()[2].recent_translations,
        vec![
            ("Hello".to_string(), "สวัสดี".to_string()),
            ("Goodbye".to_string(), "ลาก่อน".to_string()),
        ]
    );
}

#[tokio::test]
async fn the_recorded_pair_is_the_displayed_thai() {
    // Genderless Mode on: the engine's gendered answer is scrubbed before
    // display, and the scrubbed form is what the context records — the
    // consistency target is what the user actually saw.
    let (mut h, mut orch) = harness_genderless().await;
    h.context_tx.send(2).unwrap();
    h.responses.send(Ok("สวัสดีครับ".to_string())).await.unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "สวัสดี"),
        other => panic!("expected Translated, got {other:?}"),
    }

    feed_tick(&h, 1, vec![block(0, 0, "Goodbye")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[1].recent_translations,
        vec![("Hello".to_string(), "สวัสดี".to_string())],
        "the post-scrub Thai must be recorded, not the raw engine output"
    );
}

#[tokio::test]
async fn a_glossary_pinned_display_records_too() {
    // A whole-block Glossary match never calls the engine, but it IS a
    // shown translation — the buffer records it like any other emission.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;
    h.context_tx.send(2).unwrap();

    feed_tick(&h, 1, vec![block(0, 0, "  vertin  ")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => assert_eq!(translation, "เวอร์ติน"),
        other => panic!("expected Translated, got {other:?}"),
    }
    assert!(h.translate.calls().is_empty(), "the engine was never called");

    feed_tick(&h, 1, vec![block(0, 0, "Other")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[0].recent_translations,
        vec![("vertin".to_string(), "เวอร์ติน".to_string())],
        "the pinned display must ride as context for the next block"
    );
}

#[tokio::test]
async fn multi_line_answers_record_flattened_onto_one_line() {
    // The prompt section renders one `- source → translation` line per
    // pair, so a multi-line answer records flattened — the displayed text
    // keeps its line breaks regardless.
    let (mut h, mut orch) = harness().await;
    h.context_tx.send(1).unwrap();
    h.responses
        .send(Ok("line one\nline two".to_string()))
        .await
        .unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Paragraph")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => {
            assert_eq!(translation, "line one\nline two", "display keeps the breaks")
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    feed_tick(&h, 1, vec![block(0, 0, "Next")]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    assert_eq!(
        h.translate.guided_calls()[1].recent_translations,
        vec![("Paragraph".to_string(), "line one line two".to_string())],
        "the record must be single-line"
    );
}

// ---- Prompt construction through the pipeline seam, fake HTTP engine (ticket 11) ----
//
// The trait-level tests above assert what the orchestrator COMPUTES; this
// one composes the other half: the REAL OpenAI-compatible adapter driven by
// a pipeline tick, with a localhost HTTP server standing in for the LLM
// endpoint. The captured request body proves the guidance actually renders
// into the prompt; the golden response proves the loop completes with the
// engine's answer.

/// A localhost HTTP server that records every request body and answers
/// each with `response` (a canned chat-completions JSON, served with
/// `Connection: close` so every request is its own connection).
async fn fake_http_engine(response: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = bodies.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let recorded = recorded.clone();
            tokio::spawn(async move {
                // Headers first: read until the blank line.
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end = loop {
                    let n = socket.read(&mut chunk).await.expect("read request headers");
                    if n == 0 {
                        panic!("connection closed before headers completed");
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let content_length = String::from_utf8_lossy(&buffer[..header_end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if !name.trim().eq_ignore_ascii_case("content-length") {
                            return None;
                        }
                        value.trim().parse::<usize>().ok()
                    })
                    .unwrap_or(0);
                // Then exactly Content-Length bytes of body.
                while buffer.len() < header_end + content_length {
                    let n = socket.read(&mut chunk).await.expect("read request body");
                    if n == 0 {
                        panic!("connection closed before the request body completed");
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                recorded.lock().unwrap().push(
                    String::from_utf8_lossy(&buffer[header_end..header_end + content_length])
                        .into_owned(),
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                socket.write_all(reply.as_bytes()).await.ok();
                socket.shutdown().await.ok();
            });
        }
    });
    (format!("http://{addr}/v1"), bodies)
}

#[tokio::test]
async fn pipeline_tick_sends_the_guided_prompt_to_the_http_engine() {
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());

    let (url, bodies) =
        fake_http_engine(include_str!("../fixtures/openai/success.json")).await;
    let engine = langception_core::openai::OpenAiCompatibleEngine::new(
        &url,
        "test-key",
        "test-model",
        0.2,
        langception_core::openai::GENERIC_LABEL,
    )
    .unwrap();

    let (images_tx, capture) = FakeCapture::new();
    let (ocr_tx, ocr) = FakeOcr::new();
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let (_zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (_paused_tx, paused_rx) = watch::channel(false);
    let (_excluded_tx, excluded_rx) = watch::channel(Vec::new());

    let mut orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        Arc::new(engine),
        Arc::new(langception_core::orchestrator::SystemClock),
        LoopConfig {
            genderless: true,
            ..config()
        },
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );

    let blocks = vec![block(0, 0, "Vertin picks up the case")];
    images_tx.send(frame_for(&blocks)).await.unwrap();
    ocr_tx.send(Ok(blocks)).await.unwrap();

    // The tick awaits the full HTTP round trip — when it returns, the
    // request has been captured and the response parsed.
    tokio::time::timeout(Duration::from_secs(5), orchestrator.tick())
        .await
        .expect("the pipeline tick (with its HTTP call) should finish within 5s");

    // The captured body IS the prompt construction: the system message
    // carries the injected glossary entry and the genderless instruction;
    // the user message stays the bare screen text. (The guard is scoped:
    // the assert below awaits, and a std lock must not span it.)
    let (call_count, captured) = {
        let recorded = bodies.lock().unwrap();
        (recorded.len(), recorded.first().cloned().unwrap_or_default())
    };
    assert_eq!(
        call_count, 1,
        "one tick = one engine call, bodies were: {captured:?}"
    );
    let body: serde_json::Value = serde_json::from_str(&captured).unwrap();
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("- Vertin → เวอร์ติน"), "was: {system}");
    assert!(system.contains("Genderless Mode"), "was: {system}");
    assert!(system.contains("(translation-prompt v3)"), "was: {system}");
    assert_eq!(body["messages"][1]["content"], "Vertin picks up the case");

    // And the loop completed with the engine's answer from the golden body.
    match next_event(&mut events).await {
        LoopEvent::Translated {
            translation,
            via,
            ..
        } => {
            assert_eq!(translation, "สวัสดีชาวโลก");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn pipeline_tick_sends_a_custom_prompt_to_the_http_engine() {
    // Ticket 37 rendered end to end: a custom base replaces the built-in
    // instruction in the real adapter's request, with the placeholders
    // substituted and the appendices (Glossary, Genderless, custom-marked
    // version tag) still attached.
    let (url, bodies) =
        fake_http_engine(include_str!("../fixtures/openai/success.json")).await;
    let engine = langception_core::openai::OpenAiCompatibleEngine::new(
        &url,
        "test-key",
        "test-model",
        0.2,
        langception_core::openai::GENERIC_LABEL,
    )
    .unwrap();

    let (images_tx, capture) = FakeCapture::new();
    let (ocr_tx, ocr) = FakeOcr::new();
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let (_zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (_paused_tx, paused_rx) = watch::channel(false);
    let (_excluded_tx, excluded_rx) = watch::channel(Vec::new());

    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());

    let mut orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        Arc::new(engine),
        Arc::new(langception_core::orchestrator::SystemClock),
        LoopConfig {
            genderless: true,
            translation_prompt_override: Some(
                "Translate the user's {source} dialogue into {target} for a novel.".into(),
            ),
            ..config()
        },
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );

    let blocks = vec![block(0, 0, "Vertin picks up the case")];
    images_tx.send(frame_for(&blocks)).await.unwrap();
    ocr_tx.send(Ok(blocks)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), orchestrator.tick())
        .await
        .expect("the pipeline tick (with its HTTP call) should finish within 5s");
    let (call_count, captured) = {
        let recorded = bodies.lock().unwrap();
        (recorded.len(), recorded.first().cloned().unwrap_or_default())
    };
    assert_eq!(call_count, 1);
    let body: serde_json::Value = serde_json::from_str(&captured).unwrap();
    let system = body["messages"][0]["content"].as_str().unwrap();
    // The custom base with substituted placeholders...
    assert!(
        system.contains("Translate the user's English dialogue into Thai for a novel."),
        "was: {system}"
    );
    assert!(
        !system.contains("translation engine"),
        "the built-in base must be gone, was: {system}"
    );
    // ...and the appendices still attached.
    assert!(system.contains("- Vertin → เวอร์ติน"), "was: {system}");
    assert!(system.contains("Genderless Mode"), "was: {system}");
    assert!(
        system.contains("(translation-prompt v3 custom)"),
        "was: {system}"
    );
    assert_eq!(body["messages"][1]["content"], "Vertin picks up the case");

    // The loop completed with the engine's answer from the golden body.
    match next_event(&mut events).await {
        LoopEvent::Translated { via, .. } => assert_eq!(via, Via::Engine),
        other => panic!("expected Translated, got {other:?}"),
    }
}

/// One pipeline tick through the REAL OpenAI-compatible adapter against
/// the fake HTTP engine, translating one block of `text`: the recorded
/// request bodies and the loop's event receiver.
async fn http_pipeline_tick(
    config: LoopConfig,
    glossary: Glossary,
    text: &str,
) -> (Arc<Mutex<Vec<String>>>, mpsc::UnboundedReceiver<LoopEvent>) {
    let (url, bodies) = fake_http_engine(include_str!("../fixtures/openai/success.json")).await;
    let engine = langception_core::openai::OpenAiCompatibleEngine::new(
        &url,
        "test-key",
        "test-model",
        0.2,
        langception_core::openai::GENERIC_LABEL,
    )
    .unwrap();

    let (images_tx, capture) = FakeCapture::new();
    let (ocr_tx, ocr) = FakeOcr::new();
    let (events_tx, events) = mpsc::unbounded_channel();
    let (_zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (_paused_tx, paused_rx) = watch::channel(false);
    let (_excluded_tx, excluded_rx) = watch::channel(Vec::new());

    let mut orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        Arc::new(engine),
        Arc::new(langception_core::orchestrator::SystemClock),
        config,
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );

    let blocks = vec![block(0, 0, text)];
    images_tx.send(frame_for(&blocks)).await.unwrap();
    ocr_tx.send(Ok(blocks)).await.unwrap();

    // The tick awaits the full HTTP round trip — when it returns, the
    // request has been captured and the response parsed.
    tokio::time::timeout(Duration::from_secs(5), orchestrator.tick())
        .await
        .expect("the pipeline tick (with its HTTP call) should finish within 5s");
    (bodies, events)
}

#[tokio::test]
async fn pipeline_tick_sends_a_preset_prompt_to_the_http_engine() {
    // Ticket 42 rendered end to end: the Game preset's wording reaches the
    // real adapter's request in place of the built-in default, tagged with
    // the preset's name, with the appendices still attached.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("Vertin".to_string(), "เวอร์ติน".to_string());
    let game = BuiltinPreset::Game.texts();

    let (bodies, mut events) = http_pipeline_tick(
        LoopConfig {
            genderless: true,
            translation_prompt_override: Some(game.translation),
            prompt_preset: Some("game".into()),
            ..config()
        },
        glossary,
        "Vertin picks up the case",
    )
    .await;

    let captured = {
        let recorded = bodies.lock().unwrap();
        assert_eq!(recorded.len(), 1, "one tick = one engine call");
        recorded[0].clone()
    };
    let body: serde_json::Value = serde_json::from_str(&captured).unwrap();
    let system = body["messages"][0]["content"].as_str().unwrap();
    // The Game wording, placeholders substituted, not the default's...
    assert!(
        system.starts_with(
            "You are a translation engine for a video game. Translate the user's text from \
             English to Thai."
        ),
        "was: {system}"
    );
    assert!(system.contains("single-word label"), "was: {system}");
    assert!(
        !system.contains("You are a translation engine. Translate the user's text"),
        "the built-in default base must be gone, was: {system}"
    );
    // ...every appendix attached...
    assert!(system.contains("- Vertin → เวอร์ติน"), "was: {system}");
    assert!(system.contains("Genderless Mode"), "was: {system}");
    // ...and the tag names the preset that produced it.
    assert!(
        system.ends_with("(translation-prompt v3 preset:game)"),
        "was: {system}"
    );

    match next_event(&mut events).await {
        LoopEvent::Translated { via, .. } => assert_eq!(via, Via::Engine),
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- Fused pipeline (ticket 12) ----
//
// The fused engine collapses the OCR and Translate stages into ONE
// vision-model call per changed frame. Everything downstream — Excluded
// Blocks, per-block dHash dedup, the stability window, removals, the
// genderless scrub — is shared with the classic pipeline and asserted here
// through the same seam. The harness's fused variant wires a FakeFused in
// place of both the OCR fake and the translate fake.

/// A fused answer for one block: region-relative box + the Thai the model
/// answered with it.
fn fused_block(x: i32, y: i32, text: &str, thai: &str) -> FusedBlock {
    FusedBlock {
        block: block(x, y, text),
        translation: thai.to_string(),
    }
}

/// Feeds one changed frame and one fused answer (never queues an OCR
/// result: in fused mode the OCR fake must stay idle).
async fn feed_fused(
    h: &Harness,
    frame: &CapturedImage,
    answer: Result<Vec<FusedBlock>, FusedError>,
) {
    h.images.send(frame.clone()).await.unwrap();
    h.fused_responses.send(answer).await.unwrap();
}

#[tokio::test]
async fn fused_call_answers_blocks_with_translations_in_one_call() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;

    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Hello")]),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]),
    )
    .await;
    orch.tick().await;

    // The fused call replaced both stages: neither fake ever ran.
    assert_eq!(*h.ocr_calls.lock().unwrap(), 0);
    assert!(h.translate.calls().is_empty());
    assert_eq!(h.fused.calls().len(), 1);
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            key,
            block,
            translation,
            via,
            origin,
            ..
        } => {
            assert_eq!(translation, "สวัสดี");
            assert_eq!(via, Via::Engine);
            // The model's box is region-relative; the event carries screen
            // coordinates (REGION sits at 100,100), like any OCR box.
            assert_eq!((block.x, block.y), (100, 100));
            assert_eq!(key, "0,0");
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
}

#[tokio::test]
async fn unchanged_frame_never_reinvokes_the_fused_engine() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    let frame = frame_for(&[block(0, 0, "Hello")]);

    feed_fused(&h, &frame, Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")])).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // The pixel-identical frame takes the fast path: capture + hashing
    // only — the vision model is not called, nothing is re-emitted.
    h.images.send(frame.clone()).await.unwrap();
    orch.tick().await;
    assert_eq!(h.fused.calls().len(), 1);
    assert_no_event(&mut h.events, "an unchanged frame must not re-translate").await;
}

#[tokio::test]
async fn unchanged_blocks_in_a_changed_frame_are_not_resent() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    let alpha = block(0, 0, "Alpha");
    let beta = block(300, 0, "Beta");

    // Tick 1: both blocks read and translated.
    feed_fused(
        &h,
        &frame_for(&[alpha.clone(), beta.clone()]),
        Ok(vec![
            fused_block(0, 0, "Alpha", "อัลฟา"),
            fused_block(300, 0, "Beta", "เบตา"),
        ]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;
    let _ = next_event(&mut h.events).await;

    // Tick 2: only Alpha's pixels change; the frame checksum flips so the
    // fused call runs, and the model still reports BOTH blocks. Beta's
    // pixels hash to what its current translation was shown for — its
    // answer is dropped and its pill stays untouched.
    let mut changed = black_image();
    paint_marker(&mut changed, &alpha, text_mark("Alpha 2"));
    paint_marker(&mut changed, &beta, text_mark("Beta"));
    feed_fused(
        &h,
        &changed,
        Ok(vec![
            fused_block(0, 0, "Alpha 2", "อัลฟาสอง"),
            fused_block(300, 0, "Beta", "เบตา"),
        ]),
    )
    .await;
    orch.tick().await;

    // Exactly one new event — Alpha's, with the new answer.
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => {
            assert_eq!(translation, "อัลฟาสอง");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
    assert_no_event(&mut h.events, "Beta must not be re-emitted").await;
}

#[tokio::test]
async fn fused_call_receives_the_whole_glossary_and_genderless_flag() {
    // A fused call sees the image, not a block's text: there is nothing to
    // filter Glossary entries by, so ALL of them ride in the prompt, along
    // with the Genderless flag — the same guidance seam as ticket 11,
    // computed frame-wide instead of per block.
    let mut glossary = Glossary::default();
    glossary.0.insert("Vertin".to_string(), "เวอร์ติน".to_string());
    glossary.0.insert("勇者".to_string(), "ผู้กล้า".to_string());
    let (h, mut orch) = harness_with_fused(glossary, Duration::ZERO, true).await;

    feed_fused(&h, &frame_for(&[block(0, 0, "Hello")]), Ok(vec![])).await;
    orch.tick().await;

    let call = &h.fused.calls()[0];
    assert_eq!(
        call.glossary.len(),
        2,
        "every entry rides: {:?}",
        call.glossary
    );
    assert!(call.genderless);
    assert_eq!(call.target, "th");
    assert_eq!(
        (call.width, call.height),
        (REGION.width, REGION.height),
        "the whole captured frame goes to the model"
    );
}

#[tokio::test]
async fn whole_block_glossary_pin_overrides_the_fused_answer() {
    let mut glossary = Glossary::default();
    glossary.0.insert("Start".to_string(), "เริ่ม".to_string());
    let (mut h, mut orch) = harness_with_fused(glossary, Duration::ZERO, false).await;

    // The model answers something else for the pinned block; the user's
    // own Thai wins, marked as coming from the Glossary.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Start")]),
        Ok(vec![fused_block(0, 0, "Start", "โมเดลตอบ")]),
    )
    .await;
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "เริ่ม");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn genderless_scrub_applies_to_fused_output() {
    // The prompt asks the model for genderless Thai; the rule-based scrub
    // stays belt-and-braces over its answer, exactly like the engine path.
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, true).await;

    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Hello")]),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดีครับ")]),
    )
    .await;
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "สวัสดี");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn fused_error_surfaces_and_retries_on_the_next_pass() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    let frame = frame_for(&[block(0, 0, "Hello")]);

    feed_fused(&h, &frame, Err(FusedError::new("endpoint unreachable"))).await;
    orch.tick().await;

    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, message, .. } => {
            assert_eq!(stage, Stage::Fused);
            assert!(message.contains("endpoint unreachable"), "was: {message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // last_frame stayed behind: re-feeding the SAME frame retries the
    // fused call — a failure must not arm the unchanged-screen fast path.
    feed_fused(&h, &frame, Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")])).await;
    orch.tick().await;
    assert_eq!(h.fused.calls().len(), 2);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => {
            assert_eq!(translation, "สวัสดี");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn stability_window_gates_fused_reads() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::from_millis(400), false).await;
    let frame = frame_for(&[block(0, 0, "Hello")]);

    // Tick 1 (clock 0): the change is detected and the model answered, but
    // the block must hold stable — nothing fires yet, the answer is held
    // in the pending change.
    feed_fused(&h, &frame, Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")])).await;
    orch.tick().await;
    assert_no_event(&mut h.events, "the stability window has not elapsed").await;

    // Tick 2 (clock 500): the SAME frame takes the fast path — no second
    // fused call — and the matured pending fires with the cached read.
    h.clock.set_now_ms(500);
    h.images.send(frame.clone()).await.unwrap();
    orch.tick().await;
    assert_eq!(h.fused.calls().len(), 1, "the fast path must not re-call");
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, .. } => {
            assert_eq!(translation, "สวัสดี");
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // Tick 3: the translation was shown for these pixels — no re-fire.
    h.clock.set_now_ms(1000);
    h.images.send(frame.clone()).await.unwrap();
    orch.tick().await;
    assert_no_event(&mut h.events, "fired pixels must not re-fire").await;
}

#[tokio::test]
async fn fused_blocks_are_removed_after_two_misses() {
    let (mut h, mut orch) =
        harness_with_fused(Glossary::default(), Duration::ZERO, false).await;

    // Tick 1: the block reads and translates.
    feed_fused(
        &h,
        &frame_for(&[block(0, 0, "Hello")]),
        Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]),
    )
    .await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await;

    // Tick 2: the text is gone (the frame changed elsewhere); the model
    // reports no blocks — miss 1 of 2.
    let mut gone = black_image();
    paint(&mut gone, 599, 599, 1);
    feed_fused(&h, &gone, Ok(vec![])).await;
    orch.tick().await;
    assert_no_event(&mut h.events, "one miss must not remove the block").await;

    // Tick 3: the same frame takes the fast path and advances the miss —
    // the block is removed and the overlay is told to clear it.
    h.images.send(gone.clone()).await.unwrap();
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { keys, .. } => {
            assert_eq!(keys, vec!["0,0".to_string()]);
        }
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }
}

#[tokio::test]
async fn one_shot_capture_uses_the_fused_engine() {
    let (mut h, orch) = harness_with_fused(Glossary::default(), Duration::ZERO, false).await;

    h.images
        .send(frame_for(&[block(0, 0, "Hello")]))
        .await
        .unwrap();
    h.fused_responses
        .send(Ok(vec![fused_block(0, 0, "Hello", "สวัสดี")]))
        .await
        .unwrap();
    orch.capture_once(REGION).await;

    // One fused call, no OCR, no translate; the event carries OneShot and
    // the screen-converted box, like any one-shot result.
    assert_eq!(*h.ocr_calls.lock().unwrap(), 0);
    assert!(h.translate.calls().is_empty());
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            block,
            translation,
            origin,
            ..
        } => {
            assert_eq!(translation, "สวัสดี");
            assert_eq!((block.x, block.y), (100, 100));
            assert_eq!(origin, EventOrigin::OneShot);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- The real OpenAI-compatible vision adapter through the seam (ticket 12) ----
//
// As in ticket 11's HTTP test: a REAL adapter driven by a pipeline tick,
// POSTing to a localhost server that captures the request body (the image
// data URL and the fused prompt verifiable), with the golden chat answer —
// whose content IS the JSON blocks array — flowing back through the loop.

#[tokio::test]
async fn pipeline_tick_sends_the_image_to_the_http_fused_engine() {
    let mut glossary = Glossary::default();
    glossary.0.insert("Start".to_string(), "เริ่ม".to_string());

    let (url, bodies) =
        fake_http_engine(include_str!("../fixtures/fused/chat_completion_success.json")).await;
    let engine = langception_core::fused::OpenAiFusedEngine::new(
        &url,
        "test-key",
        "vision-model",
        0.2,
        langception_core::openai::GENERIC_LABEL,
    )
    .unwrap();

    let (images_tx, capture) = FakeCapture::new();
    let (_ocr_tx, ocr) = FakeOcr::new();
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let (_zones_tx, zones_rx) = watch::channel(vec![zone("z1", "Zone 1", REGION)]);
    let (_paused_tx, paused_rx) = watch::channel(false);
    let (_excluded_tx, excluded_rx) = watch::channel(Vec::new());

    let mut orchestrator = Orchestrator::new(
        Arc::new(capture),
        Arc::new(ocr),
        Arc::new(FakeTranslate::new().1),
        Arc::new(langception_core::orchestrator::SystemClock),
        LoopConfig {
            genderless: true,
            ..config()
        },
        glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    )
    .with_fused(Some(Arc::new(engine)));

    images_tx
        .send(frame_for(&[block(100, 200, "Start")]))
        .await
        .unwrap();

    // The tick awaits the full HTTP round trip — when it returns, the
    // request has been captured and the answer parsed into blocks.
    tokio::time::timeout(Duration::from_secs(5), orchestrator.tick())
        .await
        .expect("the fused tick (with its HTTP call) should finish within 5s");

    let (call_count, captured) = {
        let recorded = bodies.lock().unwrap();
        (recorded.len(), recorded.first().cloned().unwrap_or_default())
    };
    assert_eq!(call_count, 1, "one tick = one fused call, body: {captured:?}");
    let body: serde_json::Value = serde_json::from_str(&captured).unwrap();
    assert_eq!(body["model"], "vision-model");
    assert_eq!(body["stream"], false);
    // The system message is the fused contract, with the glossary riding.
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("JSON array"), "was: {system}");
    assert!(system.contains("(fused-prompt v3)"), "was: {system}");
    assert!(system.contains("- Start → เริ่ม"), "was: {system}");
    assert!(system.contains("Genderless Mode"), "was: {system}");
    // The image rides as the OpenAI vision content part.
    let parts = body["messages"][1]["content"].as_array().unwrap();
    let url = parts[1]["image_url"]["url"].as_str().unwrap();
    assert!(
        url.starts_with("data:image/png;base64,"),
        "was: {}…",
        &url[..60.min(url.len())]
    );

    // The answer's blocks flow through the loop like any translation:
    // model box (100,200) + REGION origin (100,100) = screen (200,300).
    // The block's whole text ("Start") has a pinned Glossary entry, so the
    // displayed Thai is the pin — deterministic even when it coincides
    // with the model's answer here.
    match next_event(&mut events).await {
        LoopEvent::Translated {
            block,
            translation,
            via,
            ..
        } => {
            assert_eq!((block.x, block.y), (200, 300));
            assert_eq!(block.text, "Start");
            assert_eq!(translation, "เริ่ม");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}


// ---- Auto Colors (ticket 14) ----

/// Paints a solid-color rectangle (full RGB, unlike [`paint`]'s blue
/// channel) — the fixture for color sampling.
fn paint_rgb(image: &mut CapturedImage, x: i32, y: i32, w: u32, h: u32, rgb: (u8, u8, u8)) {
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let index = (((y + dy) * REGION.width as i32 + (x + dx)) * 4) as usize;
            image.bgra[index] = rgb.2; // BGRA layout
            image.bgra[index + 1] = rgb.1;
            image.bgra[index + 2] = rgb.0;
        }
    }
}

/// Black frame with two white text bars inside `block`'s box — the
/// canonical sampleable block (white text on black, everywhere else black).
fn white_bars_frame(block: &TextBlock) -> CapturedImage {
    let mut image = black_image();
    for row in [0, 8] {
        paint_rgb(
            &mut image,
            block.x + 4,
            block.y + row,
            block.width - 8,
            4,
            (255, 255, 255),
        );
    }
    image
}

#[tokio::test]
async fn auto_colors_off_sends_no_style() {
    let (mut h, mut orch) = harness().await;
    feed_tick(&h, 1, vec![block(100, 200, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { style, .. } => {
            assert_eq!(style, None, "no sampled colors without the toggle");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn auto_colors_attach_sampled_colors_to_the_fire() {
    let (mut h, mut orch) = harness_auto_colors().await;
    let hello = block(100, 200, "Hello");
    let frame = white_bars_frame(&hello);
    h.images.send(frame).await.unwrap();
    h.ocr_results.send(Ok(vec![hello.clone()])).await.unwrap();
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { key, style, .. } => {
            assert_eq!(key, "25,50", "region-relative grid key");
            let style = style.expect("Auto Colors on + sampleable frame = colors");
            assert_eq!(style.text_color, "#ffffff");
            assert_eq!(style.pill_color, "#000000");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn auto_colors_sampling_failure_falls_back_to_none() {
    let (mut h, mut orch) = harness_auto_colors().await;
    // A monochrome frame (plain black — NOT frame_for, whose marker bars
    // would give the block a real second color) has no distinct text
    // color: sampling fails and the event carries no style, so the
    // overlay keeps the manual colors.
    let frame = black_image();
    h.images.send(frame).await.unwrap();
    h.ocr_results
        .send(Ok(vec![block(100, 200, "Hello")]))
        .await
        .unwrap();
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { style, .. } => {
            assert_eq!(style, None, "unsampleable block = manual style fallback");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

#[tokio::test]
async fn auto_colors_ride_fused_reads_too() {
    let (mut h, mut orch) = harness_inner(
        Glossary::default(),
        Duration::ZERO,
        false,
        true,
        true,
        None,
        None,
    )
    .await;
    let hello = block(100, 200, "Hello");
    let frame = white_bars_frame(&hello);
    h.images.send(frame).await.unwrap();
    h.fused_responses
        .send(Ok(vec![FusedBlock {
            block: hello.clone(),
            translation: "[TH] Hello".into(),
        }]))
        .await
        .unwrap();
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, style, .. } => {
            assert_eq!(translation, "[TH] Hello");
            let style = style.expect("fused fires carry sampled colors too");
            assert_eq!(style.text_color, "#ffffff");
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- Multi-line merge (ticket 23) ----
//
// The classic read path merges consecutive OCR lines that visually form one
// paragraph into ONE Text Block before tracking (the pure pass and its
// geometry tests live in `core/src/merge.rs`). These pin the end-to-end
// behavior: the merged block translates, dedups, and is excluded as one.

#[tokio::test]
async fn stacked_lines_translate_as_one_merged_block() {
    let (mut h, mut orch) = harness().await;
    // Two lines of one paragraph (20px lines, 4px leading) — Windows OCR's
    // per-line output.
    let lines = vec![block(100, 200, "Hello"), block(100, 224, "World")];

    feed_tick(&h, 1, lines.clone()).await;
    orch.tick().await;

    // ONE engine call with the joined text — not two per-line calls.
    assert_eq!(h.translate.calls(), ["Hello World"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated {
            key,
            block,
            translation,
            ..
        } => {
            assert_eq!(translation, "[TH] Hello World");
            // The union rect in screen coordinates (REGION sits at
            // 100,100), keyed by the merged block's top-left like any
            // block.
            assert_eq!((block.x, block.y), (200, 300));
            assert_eq!((block.width, block.height), (100, 44));
            assert_eq!(block.text, "Hello World");
            assert_eq!(key, "25,50");
        }
        other => panic!("expected Translated, got {other:?}"),
    }

    // The merged block dedups as one: pixel-identical screen → fast path,
    // no re-translation, no event.
    feed_unchanged(&h, 1, lines).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello World"]);
    assert_no_event(&mut h.events, "the merged block must dedup as one").await;
}

#[tokio::test]
async fn an_exclusion_covering_a_merged_block_suppresses_it() {
    let (mut h, mut orch) = harness().await;
    // The two stacked lines merge into one block at screen
    // (200, 300, 100×44); the exclusion covers the whole merged rect.
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 44)])
        .unwrap();

    feed_tick(
        &h,
        1,
        vec![block(100, 200, "Hello"), block(100, 224, "World")],
    )
    .await;
    orch.tick().await;

    assert!(
        h.translate.calls().is_empty(),
        "a merged block under an exclusion must not reach the engine, calls were: {:?}",
        h.translate.calls()
    );
    assert_no_event(&mut h.events, "an excluded merged block must not emit").await;
}

#[tokio::test]
async fn one_shot_capture_merges_stacked_lines_too() {
    let (mut h, orch) = harness().await;
    let lines = vec![block(0, 0, "Hello"), block(0, 24, "World")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(h.translate.calls(), ["Hello World"]);
    match next_event(&mut h.events).await {
        LoopEvent::Translated { block, origin, .. } => {
            assert_eq!((block.x, block.y, block.width, block.height), (100, 100, 100, 44));
            assert_eq!(origin, EventOrigin::OneShot);
        }
        other => panic!("expected Translated, got {other:?}"),
    }
}

// ---- One-shot batched translation (ticket 69) ----
//
// A one-shot is the "translate this long set of sentences" gesture, so the
// classic pass sends every block it read in ONE engine request instead of
// one request per line. The fused one-shot always was one call; zone passes
// keep per-block translation (their dedup and Translation Context are per
// block) and stay pinned by the zone tests above.

#[tokio::test]
async fn one_shot_translates_spread_blocks_in_one_request() {
    // Lines that do NOT merge (far apart rows leave the OCR as separate
    // blocks) join into one request, and the reply — the engine contract's
    // "Preserve line breaks" — splits back onto each block's own pill. A
    // blank line the model inserted between answers is not an answer.
    let (mut h, orch) = harness().await;
    let lines = vec![block(0, 0, "Hello"), block(0, 200, "Bye")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();
    h.responses
        .send(Ok("สวัสดี\n\nลาก่อน".into()))
        .await
        .unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(
        h.translate.calls(),
        ["Hello\nBye"],
        "one request for the whole capture, calls were: {:?}",
        h.translate.calls()
    );
    for ((x, y, height), thai) in [((100, 100, 20), "สวัสดี"), ((100, 300, 20), "ลาก่อน")] {
        match next_event(&mut h.events).await {
            LoopEvent::Translated {
                block,
                translation,
                origin,
                ..
            } => {
                assert_eq!((block.x, block.y, block.height), (x, y, height));
                assert_eq!(translation, thai);
                assert_eq!(origin, EventOrigin::OneShot);
            }
            other => panic!("expected Translated \"{thai}\", got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_batch_reply_that_does_not_map_back_falls_back_to_per_block() {
    // The engine answered one line for two blocks: the batch cannot split
    // it back onto the blocks, so the pass falls back to per-block requests
    // — which cannot misalign — rather than showing a wrong pill or one
    // blob across the capture.
    let (mut h, orch) = harness().await;
    let lines = vec![block(0, 0, "Hello"), block(0, 200, "Bye")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();
    h.responses.send(Ok("สวัสดีและลาก่อน".into())).await.unwrap();
    h.responses.send(Ok("[TH] Hello".into())).await.unwrap();
    h.responses.send(Ok("[TH] Bye".into())).await.unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(
        h.translate.calls(),
        ["Hello\nBye", "Hello", "Bye"],
        "the batch attempt, then one request per block, calls were: {:?}",
        h.translate.calls()
    );
    for thai in ["[TH] Hello", "[TH] Bye"] {
        match next_event(&mut h.events).await {
            LoopEvent::Translated { translation, .. } => assert_eq!(translation, thai),
            other => panic!("expected Translated \"{thai}\", got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_one_shot_glossary_pin_never_rides_the_batch() {
    // A whole-block Glossary match is the user's own Thai: it pins without
    // the engine — and without joining the batch — while the unpinned
    // block's batch-of-one goes through the pre-69 lone-block path.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, orch) = harness_with(glossary, Duration::ZERO, false).await;
    let lines = vec![block(0, 0, "vertin"), block(0, 200, "Bye")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(h.translate.calls(), ["Bye"], "the pin skips the engine");
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "เวอร์ติน");
            assert_eq!(via, Via::Glossary);
        }
        other => panic!("expected the glossary Translated first, got {other:?}"),
    }
    match next_event(&mut h.events).await {
        LoopEvent::Translated { translation, via, .. } => {
            assert_eq!(translation, "[TH] Bye");
            assert_eq!(via, Via::Engine);
        }
        other => panic!("expected the engine Translated second, got {other:?}"),
    }
}

#[tokio::test]
async fn genderless_scrubs_every_line_of_a_batch_reply() {
    // Ticket 66's scrub rides the batch the same way it rides a per-block
    // reply: line by line, after the split maps lines back onto blocks.
    let (mut h, orch) = harness_genderless().await;
    let lines = vec![block(0, 0, "Hello"), block(0, 200, "Bye")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();
    h.responses
        .send(Ok("ผมจะไป\nหนูจะมา".into()))
        .await
        .unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(h.translate.calls(), ["Hello\nBye"]);
    for thai in ["ฉันจะไป", "ฉันจะมา"] {
        match next_event(&mut h.events).await {
            LoopEvent::Translated { translation, .. } => assert_eq!(translation, thai),
            other => panic!("expected Translated \"{thai}\", got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_batched_one_shot_charges_the_translate_stage_once() {
    // Ticket 47's pinning, ticket 69 edition: three blocks in one request
    // are ONE engine charge, not three — and every block still counts as
    // translated in the timing row.
    let (mut h, orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    let lines = vec![
        block(0, 0, "Hello"),
        block(0, 200, "Bye"),
        block(0, 400, "Okay"),
    ];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();
    h.responses
        .send(Ok("สวัสดี\nลาก่อน\nโอเค".into()))
        .await
        .unwrap();

    orch.capture_once(REGION).await;

    assert_eq!(h.translate.calls(), ["Hello\nBye\nOkay"]);
    for _ in 0..3 {
        next_event(&mut h.events).await; // the three Translated rows
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            origin,
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(origin, EventOrigin::OneShot);
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40));
            assert_eq!(translate_ms, Some(30), "one request, one charge");
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 3);
            assert_eq!(total_ms, 75);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[test]
fn stage_name_matches_the_serde_spelling() {
    // `Stage::name()` (the app log's stage tag, ticket 27) and the serde
    // spelling must never drift apart: both feed diagnostics consumers.
    for (stage, spelling) in [
        (Stage::Capture, "capture"),
        (Stage::Ocr, "ocr"),
        (Stage::Translate, "translate"),
        (Stage::Fused, "fused"),
    ] {
        assert_eq!(stage.name(), spelling);
        assert_eq!(serde_json::to_value(stage).unwrap(), spelling);
    }
}

// ---- Per-pass stage timings (ticket 47) ----
//
// The pinned-duration tests: each fake stage advances the shared fake
// clock by its cost per call, so a timing event's stage durations are
// exact, and the stages sum to the pass total (the pass's own hashing and
// bookkeeping cost no fake-clock time).

#[tokio::test]
async fn every_pass_emits_a_timing_event_with_pinned_stage_durations() {
    let (mut h, mut orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    let hello = vec![block(100, 200, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;

    // The translated row fires mid-pass, the timing row at its end.
    match next_event(&mut h.events).await {
        LoopEvent::Translated { .. } => {}
        other => panic!("expected Translated first, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            origin,
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            read_skipped,
            reads,
            ..
        } => {
            assert_eq!(
                origin,
                EventOrigin::Zone {
                    id: "z1".into(),
                    name: "Zone 1".into()
                }
            );
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40));
            assert_eq!(translate_ms, Some(30));
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 1);
            assert!(!read_skipped, "this pass read the frame");
            // Ticket 48: the pass's read trace rides the timing event.
            assert_eq!(reads.len(), 1);
            assert_eq!(reads[0].text, "Hello");
            assert_eq!(reads[0].disposition, ReadDisposition::TranslatedEngine);
            // capture + ocr + translate sum to the total, within rounding.
            assert_eq!(capture_ms + ocr_ms.unwrap() + translate_ms.unwrap(), total_ms);
            assert_eq!(total_ms, 75);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unchanged_tick_emits_a_timing_event_with_zero_blocks() {
    let (mut h, mut orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    let hello = vec![block(100, 200, "Hello")];

    feed_tick(&h, 1, hello.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    let _ = next_timing(&mut h.events).await; // pass 1's timing row

    // Pixel-identical screen: the fast path — no OCR, no translation, but
    // still a timing row (capture can be the slow stage on a large zone),
    // reporting zero blocks instead of being silently dropped. No fire was
    // attempted, so the translate stage never ran (ticket 50): `None`, not
    // `Some(0)` — a no-fire pass must not count into the summary's stats.
    feed_unchanged(&h, 1, hello.clone()).await;
    orch.tick().await;
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, None);
            assert_eq!(translate_ms, None);
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 0);
            // The pass total is capture only — the stage durations plus the
            // pass's own bookkeeping sum to it.
            assert_eq!(total_ms, 5);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_fused_pass_reports_the_fused_stage_not_ocr_or_translate() {
    let (mut h, mut orch) = harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    h.pin_stage_costs(5, 60, 30);
    let hello = block(100, 200, "Hello");
    h.fused_responses
        .send(Ok(vec![FusedBlock {
            block: hello.clone(),
            translation: "[TH] Hello".into(),
        }]))
        .await
        .unwrap();

    feed_tick(&h, 1, vec![hello.clone()]).await;
    orch.tick().await;

    let _ = next_event(&mut h.events).await; // Translated
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, None);
            // The fused stage replaced both: no ocr/translate row, the
            // fused call's duration instead.
            assert_eq!(translate_ms, None);
            assert_eq!(fused_ms, Some(60));
            assert_eq!(blocks_translated, 1);
            assert_eq!(capture_ms + fused_ms.unwrap(), total_ms);
            assert_eq!(total_ms, 65);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_fast_path_fire_charges_its_translate_time_to_its_own_pass() {
    let (mut h, mut orch) = harness_with_stability(1000).await;
    h.pin_stage_costs(5, 40, 30);
    let hello = vec![block(100, 200, "Hello")];

    // Pass 1 detects the change; the stability window holds the fire.
    feed_tick(&h, 1, hello.clone()).await;
    h.clock.set_now_ms(1000);
    orch.tick().await;
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40));
            // The stability window holds the fire: the translate stage
            // never ran this pass (ticket 50), so it reports `None`.
            assert_eq!(translate_ms, None);
            assert_eq!(blocks_translated, 0);
        }
        other => panic!("expected Timing, got {other:?}"),
    }

    // Pass 2: pixel-identical screen (the fast path) — the pending matured
    // and fires with its cached read. Its engine time is THIS pass's
    // translate stage.
    feed_unchanged(&h, 1, hello.clone()).await;
    h.clock.set_now_ms(3000);
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { .. } => {}
        other => panic!("expected the matured Translated, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, None);
            assert_eq!(translate_ms, Some(30));
            assert_eq!(blocks_translated, 1);
            assert_eq!(total_ms, 35);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn one_shot_passes_emit_timing_rows_like_zone_passes() {
    let (mut h, orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    let lines = vec![block(0, 0, "Hello")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();

    orch.capture_once(REGION).await;

    match next_event(&mut h.events).await {
        LoopEvent::Translated { origin, .. } => assert_eq!(origin, EventOrigin::OneShot),
        other => panic!("expected Translated, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            origin,
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(origin, EventOrigin::OneShot);
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40));
            assert_eq!(translate_ms, Some(30));
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 1);
            assert_eq!(total_ms, 75);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_capture_still_emits_a_timing_event() {
    let (mut h, mut orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);

    // Capture fails this tick: the Error event names the stage, and the
    // timing row still reports the duration the capture managed up to the
    // failure — a hanging capture is when that number matters most.
    drop(h.images);
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Capture),
        other => panic!("expected Error, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            total_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, None);
            // The pass died in capture: no read, no fire, no translate
            // stage (ticket 50) — `None`, not `Some(0)`.
            assert_eq!(translate_ms, None);
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 0);
            assert_eq!(total_ms, 5);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn translate_time_accumulates_across_a_pass_blocks() {
    let (mut h, mut orch) = harness().await;
    h.pin_stage_costs(0, 0, 30);
    let blocks = vec![block(100, 200, "Hello"), block(100, 400, "World")];

    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;

    let _ = next_event(&mut h.events).await; // Translated "Hello"
    let _ = next_event(&mut h.events).await; // Translated "World"
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            translate_ms: Some(ms),
            blocks_translated,
            total_ms,
            ..
        } => {
            // Each block's engine call charged the pass's translate stage.
            assert_eq!(ms, 60);
            assert_eq!(blocks_translated, 2);
            assert_eq!(total_ms, 60);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_read_pass_where_nothing_fires_reports_no_translate_stage() {
    // Ticket 50: a classic pass whose read produced only blocks that drop
    // out before the fire — unchanged, excluded, empty — never entered the
    // Translate stage. Its timing row reports `translate_ms: None` so the
    // debug window's translate stats count real stage passes only.
    let (mut h, mut orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    let hello = block(100, 200, "Hello"); // fires in pass 1, unchanged in pass 2
    let blank = block(100, 300, "   "); // trims to nothing
    let skipped = block(100, 400, "Skipped"); // under the exclusion below
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 500, 100, 20)])
        .unwrap();

    // Pass 1 fires Hello through the engine; the other two drop out.
    // The stage ran once.
    feed_tick(&h, 1, vec![hello.clone(), blank.clone(), skipped.clone()]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Translated { via, .. } => assert_eq!(via, Via::Engine),
        other => panic!("expected Translated, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            ocr_ms,
            translate_ms,
            blocks_translated,
            total_ms,
            ..
        } => {
            assert_eq!(ocr_ms, Some(40));
            assert_eq!(translate_ms, Some(30));
            assert_eq!(blocks_translated, 1);
            assert_eq!(total_ms, 75);
        }
        other => panic!("expected Timing, got {other:?}"),
    }

    // Pass 2: the frame changed only at a corner (OCR re-runs), but every
    // block drops out again — Hello's pixels match its shown translation
    // (unchanged), the other two are excluded/empty. No fire was attempted.
    let frame = frame_animated(std::slice::from_ref(&hello), 1, text_mark("Hello"));
    feed(
        &h,
        1,
        &frame,
        Some(vec![hello.clone(), blank.clone(), skipped.clone()]),
    )
    .await;
    orch.tick().await;
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            ocr_ms,
            translate_ms,
            blocks_translated,
            total_ms,
            ..
        } => {
            assert_eq!(ocr_ms, Some(40), "this pass read the frame");
            assert_eq!(translate_ms, None, "no fire, no translate stage");
            assert_eq!(blocks_translated, 0);
            // The pass total is capture + read only.
            assert_eq!(total_ms, 45);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_glossary_only_fire_reports_an_honest_some_zero() {
    // Ticket 50's honest zero: a Glossary-pinned fire genuinely costs ~0ms
    // — the stage RAN and answered from the pinned table without the
    // engine — so `Some(0)` is the true reading and this min-0 stays.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;
    h.pin_stage_costs(5, 40, 30);
    let blocks = vec![block(0, 0, "  vertin  ")];

    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;

    assert!(
        h.translate.calls().is_empty(),
        "glossary match must short-circuit the engine, calls were: {:?}",
        h.translate.calls()
    );
    match next_event(&mut h.events).await {
        LoopEvent::Translated { via, .. } => assert_eq!(via, Via::Glossary),
        other => panic!("expected Translated, got {other:?}"),
    }
    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            capture_ms,
            ocr_ms,
            translate_ms,
            blocks_translated,
            total_ms,
            ..
        } => {
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40));
            // The stage ran (Glossary short-circuit) and cost no fake-clock
            // time: an honest `Some(0)`, counted into the summary like any
            // other fire.
            assert_eq!(translate_ms, Some(0));
            assert_eq!(blocks_translated, 1);
            assert_eq!(total_ms, 45);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
}

#[tokio::test]
async fn a_one_shot_pass_where_nothing_fires_reports_no_translate_stage() {
    // Ticket 50: one-shot passes follow the same rule — every block dropped
    // out before the fire (excluded / empty), so no translate stage ran and
    // the timing row reports `None`.
    let (mut h, orch) = harness().await;
    h.pin_stage_costs(5, 40, 30);
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 300, 100, 20)])
        .unwrap();
    let lines = vec![block(100, 200, "Hello"), block(100, 400, "   ")];
    h.images.send(frame_for(&lines)).await.unwrap();
    h.ocr_results.send(Ok(lines)).await.unwrap();

    orch.capture_once(REGION).await;

    match next_timing(&mut h.events).await {
        LoopEvent::Timing {
            origin,
            capture_ms,
            ocr_ms,
            translate_ms,
            fused_ms,
            blocks_translated,
            ..
        } => {
            assert_eq!(origin, EventOrigin::OneShot);
            assert_eq!(capture_ms, 5);
            assert_eq!(ocr_ms, Some(40), "this pass read the frame");
            assert_eq!(translate_ms, None, "no fire, no translate stage");
            assert_eq!(fused_ms, None);
            assert_eq!(blocks_translated, 0);
        }
        other => panic!("expected Timing, got {other:?}"),
    }
    assert_no_event(&mut h.events, "no block fired, so nothing is shown").await;
}

// ---- Per-pass read trace (ticket 48) ----
//
// Every block a read pass produced appears in that pass's Timing event
// with its words and the disposition the loop decided. Each disposition
// is pinned on the exact scenario whose silent suppression the trace
// exists to expose (the ticket-05/22 paths), so a regression in a
// suppression rule breaks its pinned entry here.

/// The next Timing event's read trace, as `(read_skipped, reads)`.
async fn next_reads(events: &mut mpsc::UnboundedReceiver<LoopEvent>) -> (bool, Vec<BlockTrace>) {
    match next_timing(events).await {
        LoopEvent::Timing {
            read_skipped,
            reads,
            ..
        } => (read_skipped, reads),
        other => panic!("expected Timing, got {other:?}"),
    }
}

/// Pins one trace entry: this exact text was read and exactly this
/// disposition decided for it. Exact equality (not field-spot checks) so
/// the Waiting/Gone payloads are pinned too.
fn expect_read(reads: &[BlockTrace], text: &str, disposition: ReadDisposition) {
    assert!(
        reads
            .iter()
            .any(|r| r.text == text && r.disposition == disposition),
        "expected a {disposition:?} trace for {text:?}, got {reads:?}"
    );
}

#[tokio::test]
async fn a_read_pass_traces_every_block_with_its_disposition() {
    let (mut h, mut orch) = harness().await;
    let hello = block(100, 200, "Hello"); // fires (zero stability window)
    let blocked = block(100, 400, "Skipped"); // under the exclusion below
    // Exclusions are screen coordinates; the zone sits at (100, 100).
    h.excluded_tx
        .send(vec![exclusion("x1", 200, 500, 100, 20)])
        .unwrap();

    feed_tick(&h, 1, vec![hello.clone(), blocked.clone()]).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated Hello
    let (read_skipped, reads) = next_reads(&mut h.events).await;
    assert!(!read_skipped, "this pass read the frame");
    assert_eq!(reads.len(), 2, "every block the read produced: {reads:?}");
    expect_read(&reads, "Hello", ReadDisposition::TranslatedEngine);
    expect_read(&reads, "Skipped", ReadDisposition::Excluded);

    // Pass 2: the frame changes only at a corner (OCR must re-run), but
    // Hello's pixels are identical to what its shown translation was
    // rendered for → the trace names it "unchanged", the invisible skip
    // that used to be the only unexplained zero.
    let frame = frame_animated(std::slice::from_ref(&hello), 1, text_mark("Hello"));
    feed(&h, 1, &frame, Some(vec![hello.clone()])).await;
    orch.tick().await;
    let (_, reads) = next_reads(&mut h.events).await;
    assert_eq!(reads.len(), 1);
    expect_read(&reads, "Hello", ReadDisposition::Unchanged);
}

#[tokio::test]
async fn a_suppressed_read_traces_its_text_and_reason() {
    let (mut h, mut orch) = harness().await;
    let blocks = vec![block(0, 0, "Hello")];

    // Tick 1: "Hello" appears → translated once, traced as engine.
    feed(&h, 1, &frame_animated(&blocks, 0, 7), Some(blocks.clone())).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    let (_, reads) = next_reads(&mut h.events).await;
    expect_read(&reads, "Hello", ReadDisposition::TranslatedEngine);

    // Tick 2: the pixels jitter but the read says "Hello" again — the
    // fire-path dedup suppresses it (no engine call, no event). The trace
    // is the only place this read shows up.
    let frame = frame_animated(&blocks, 8, 8);
    feed(&h, 1, &frame, Some(blocks.clone())).await;
    orch.tick().await;
    assert_eq!(h.translate.calls(), ["Hello"], "no re-request");
    let (_, reads) = next_reads(&mut h.events).await;
    assert_eq!(reads.len(), 1);
    expect_read(&reads, "Hello", ReadDisposition::Suppressed);
}

#[tokio::test]
async fn waiting_traces_the_stability_window_then_flips_to_translated() {
    let (mut h, mut orch) = harness_with_stability(1000).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Pass 1 (t=0): the change is detected; the window just started.
    h.clock.set_now_ms(0);
    let frame_a = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &frame_a, Some(blocks.clone())).await;
    orch.tick().await;
    let (_, reads) = next_reads(&mut h.events).await;
    expect_read(
        &reads,
        "Hello",
        ReadDisposition::Waiting {
            elapsed_ms: 0,
            total_ms: 1000,
        },
    );

    // Pass 2 (t=600): the block still holds (corner change so OCR re-runs,
    // the block's own pixels identical to the pending's). Not matured yet —
    // the trace carries the elapsed/total the UI counts down with.
    h.clock.set_now_ms(600);
    let frame_b = frame_animated(&blocks, 1, 7);
    feed(&h, 1, &frame_b, Some(blocks.clone())).await;
    orch.tick().await;
    let (_, reads) = next_reads(&mut h.events).await;
    expect_read(
        &reads,
        "Hello",
        ReadDisposition::Waiting {
            elapsed_ms: 600,
            total_ms: 1000,
        },
    );

    // Pass 3 (t=1000): the window has elapsed → the fire happens on THIS
    // read pass, so the trace's final entry is translated-engine.
    h.clock.set_now_ms(1000);
    let frame_c = frame_animated(&blocks, 2, 7);
    feed(&h, 1, &frame_c, Some(blocks.clone())).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    let (_, reads) = next_reads(&mut h.events).await;
    expect_read(&reads, "Hello", ReadDisposition::TranslatedEngine);
}

#[tokio::test]
async fn a_fast_path_pass_is_marked_and_lists_only_its_matured_fires() {
    let (mut h, mut orch) = harness_with_stability(400).await;
    let blocks = vec![block(0, 0, "Hello")];

    // Pass 1 (t=0): change detected, window starts (OCR ran).
    h.clock.set_now_ms(0);
    let frame_a = frame_animated(&blocks, 0, 7);
    feed(&h, 1, &frame_a, Some(blocks.clone())).await;
    orch.tick().await;
    let (read_skipped, reads) = next_reads(&mut h.events).await;
    assert!(!read_skipped);
    expect_read(
        &reads,
        "Hello",
        ReadDisposition::Waiting {
            elapsed_ms: 0,
            total_ms: 400,
        },
    );

    // Pass 2 (t=500): pixel-identical screen → the fast path. The row is
    // marked "OCR skipped — frame unchanged" and its only trace entry is
    // the matured fire off the cached read.
    h.clock.set_now_ms(500);
    feed(&h, 1, &frame_a, None).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    let (read_skipped, reads) = next_reads(&mut h.events).await;
    assert!(read_skipped, "the fast path never reads");
    assert_eq!(reads.len(), 1, "only the matured fire's entry: {reads:?}");
    expect_read(&reads, "Hello", ReadDisposition::TranslatedCached);
}

#[tokio::test]
async fn gone_traces_missing_blocks_with_miss_counts() {
    let (mut h, mut orch) = harness().await;
    let blocks = vec![block(0, 0, "Hello")];

    // Pass 1: "Hello" fires.
    feed_tick(&h, 1, blocks.clone()).await;
    orch.tick().await;
    let _ = next_event(&mut h.events).await; // Translated
    let _ = next_reads(&mut h.events).await;

    // Pass 2: OCR returns nothing (Windows OCR's intermittent empty) —
    // the trace says the block is gone from the read, first miss.
    feed_tick(&h, 1, Vec::new()).await;
    orch.tick().await;
    let (_, reads) = next_reads(&mut h.events).await;
    assert_eq!(reads.len(), 1);
    expect_read(
        &reads,
        "Hello",
        ReadDisposition::Gone { misses: 1 },
    );

    // Pass 3: empty again — second miss removes the block (the
    // BlocksRemoved event rides the same pass, before its timing row).
    // The frame changes at a corner so this pass runs OCR (a pixel-proof
    // screen would take the fast path, whose removals are advance-only
    // and trace-free — the fast-path test pins that side).
    let frame = frame_animated(&[], 1, 9);
    feed(&h, 1, &frame, Some(Vec::new())).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::BlocksRemoved { keys, .. } => assert_eq!(keys, ["0,0"]),
        other => panic!("expected BlocksRemoved, got {other:?}"),
    }
    let (_, reads) = next_reads(&mut h.events).await;
    expect_read(&reads, "Hello", ReadDisposition::Gone { misses: 2 });
}

#[tokio::test]
async fn a_failed_translation_pushes_no_trace_entry() {
    let (mut h, mut orch) = harness().await;

    // The engine call fails: the Error event names the failure and the
    // trace stays empty — a failed read is the Error row's story, not a
    // translated/suppressed entry.
    h.responses
        .send(Err(TranslateError::new("HTTP 429 rate limited")))
        .await
        .unwrap();
    feed_tick(&h, 1, vec![block(0, 0, "Hello")]).await;
    orch.tick().await;
    match next_event(&mut h.events).await {
        LoopEvent::Error { stage, .. } => assert_eq!(stage, Stage::Translate),
        other => panic!("expected Error, got {other:?}"),
    }
    let (_, reads) = next_reads(&mut h.events).await;
    assert!(
        reads.is_empty(),
        "a failed translation leaves the trace to the Error event: {reads:?}"
    );
}

#[tokio::test]
async fn one_shot_traces_only_translated_and_empty_dispositions() {
    // The classic one-shot: no tracking state exists, so the only possible
    // trace entries are translated / excluded / empty. (Whitespace noise
    // never reaches it — merge_lines drops it first — so the empty entry
    // is pinned on the fused path below.)
    let (mut h, orch) = harness().await;
    let hello = block(0, 0, "Hello");
    h.images.send(frame_for(std::slice::from_ref(&hello))).await.unwrap();
    h.ocr_results.send(Ok(vec![hello.clone()])).await.unwrap();

    orch.capture_once(REGION).await;

    let _ = next_event(&mut h.events).await; // Translated
    let (read_skipped, reads) = next_reads(&mut h.events).await;
    assert!(!read_skipped);
    assert_eq!(reads.len(), 1);
    expect_read(&reads, "Hello", ReadDisposition::TranslatedEngine);
}

#[tokio::test]
async fn a_fused_one_shot_traces_translated_and_empty() {
    // The fused one-shot reads + answers in one call: the trace names the
    // fused route, and a fused block whose text trims to nothing traces as
    // empty — unlike the classic path, the fused answer skips merge_lines,
    // so whitespace noise actually reaches the empty branch.
    let (mut h, orch) = harness_with_fused(Glossary::default(), Duration::ZERO, false).await;
    h.fused_responses
        .send(Ok(vec![
            FusedBlock {
                block: block(0, 0, "Hello"),
                translation: "[TH] Hello".into(),
            },
            FusedBlock {
                block: block(200, 0, "   "),
                translation: "   ".into(),
            },
        ]))
        .await
        .unwrap();
    h.images
        .send(frame_for(&[block(0, 0, "Hello"), block(200, 0, "   ")]))
        .await
        .unwrap();

    orch.capture_once(REGION).await;

    let _ = next_event(&mut h.events).await; // Translated
    let (_, reads) = next_reads(&mut h.events).await;
    assert_eq!(reads.len(), 2);
    expect_read(&reads, "Hello", ReadDisposition::TranslatedFused);
    expect_read(&reads, "", ReadDisposition::Empty);
}

#[test]
fn disposition_serde_spellings_match_the_ui_labels() {
    // The debug window switches on these exact strings (debug.js
    // readReason/countKey): the serde tag and the JS must not drift.
    use serde_json::json;
    for (disposition, value) in [
        (
            ReadDisposition::TranslatedEngine,
            json!({"disposition": "translated_engine"}),
        ),
        (
            ReadDisposition::TranslatedGlossary,
            json!({"disposition": "translated_glossary"}),
        ),
        (
            ReadDisposition::TranslatedFused,
            json!({"disposition": "translated_fused"}),
        ),
        (
            ReadDisposition::TranslatedCached,
            json!({"disposition": "translated_cached"}),
        ),
        (
            ReadDisposition::Suppressed,
            json!({"disposition": "suppressed"}),
        ),
        (
            ReadDisposition::Waiting {
                elapsed_ms: 40,
                total_ms: 1000,
            },
            json!({"disposition": "waiting", "elapsed_ms": 40, "total_ms": 1000}),
        ),
        (
            ReadDisposition::Unchanged,
            json!({"disposition": "unchanged"}),
        ),
        (
            ReadDisposition::Excluded,
            json!({"disposition": "excluded"}),
        ),
        (ReadDisposition::Empty, json!({"disposition": "empty"})),
        (
            ReadDisposition::Gone { misses: 1 },
            json!({"disposition": "gone", "misses": 1}),
        ),
    ] {
        assert_eq!(serde_json::to_value(disposition).unwrap(), value);
    }
}

#[tokio::test]
async fn a_glossary_fire_traces_as_translated_glossary() {
    // A Glossary whole-block match pins the shown Thai and the engine is
    // never called — the trace names the route, not the engine.
    let mut glossary = Glossary::default();
    glossary
        .0
        .insert("VERTIN".to_string(), "เวอร์ติน".to_string());
    let (mut h, mut orch) = harness_with(glossary, Duration::ZERO, false).await;

    feed_tick(&h, 1, vec![block(0, 0, "  vertin  ")]).await;
    orch.tick().await;

    assert!(h.translate.calls().is_empty(), "engine must stay idle");
    let _ = next_event(&mut h.events).await; // Translated
    let (_, reads) = next_reads(&mut h.events).await;
    // The words are the trimmed read, the route is the glossary's.
    expect_read(&reads, "vertin", ReadDisposition::TranslatedGlossary);
}
