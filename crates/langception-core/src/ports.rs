//! Ports: the seams the translation loop depends on. Windows adapters and the
//! Tauri shell implement these; all pipeline behavior is tested against them
//! with fakes.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// A rectangular screen region in physical pixels (origin may be negative on
/// multi-monitor setups — coordinates are relative to the primary monitor's
/// top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// The overlap of the two rectangles, or `None` when they do not
    /// intersect (touching edges are not an intersection). Used by the
    /// Capture Visibility masking (ticket 46): the part of a Langception
    /// window's screen rect that falls inside the captured region is what
    /// gets filled out of the frame.
    pub fn intersection(&self, other: &Region) -> Option<Region> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width as i32).min(other.x + other.width as i32);
        let bottom = (self.y + self.height as i32).min(other.y + other.height as i32);
        let width = right - left;
        let height = bottom - top;
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Region {
            x: left,
            y: top,
            width: width as u32,
            height: height as u32,
        })
    }
}

/// A captured screen region as 8-bit BGRA, top-down rows, stride `width * 4`.
#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A Capture Zone (CONTEXT.md): a user-drawn rectangular screen region
/// captured and translated repeatedly. Several zones are watched at once;
/// each tracks its Text Blocks independently. The `id` is stable across
/// renames and restarts so the loop can keep per-zone tracking state.
/// `monitor` is the system name of the monitor the zone was drawn on
/// (e.g. `\\.\DISPLAY1`); a zone whose monitor is unplugged is disabled,
/// not deleted (empty = the primary monitor, the legacy default).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub monitor: String,
    pub region: Region,
}

/// An Excluded Block (CONTEXT.md): a Text Block position the user marked as
/// not-to-be-translated, consulted in full-screen mode. The `id` is stable
/// so the exclusion can be cleared later; `label` is what the block showed
/// when excluded (display only — matching is purely positional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcludedBlock {
    pub id: String,
    pub monitor: String,
    /// Screen rect in PHYSICAL px (origin = primary monitor's top-left),
    /// the OCR box at click time.
    pub region: Region,
    pub label: String,
}

impl ExcludedBlock {
    /// Whether a Text Block (screen coords) is covered by this exclusion:
    /// the block's rect must overlap the stored rect by at least half the
    /// block's area. A plain point-in-rect test on the block's center would
    /// lose exclusions to OCR jitter of a few pixels; a large overlap still
    /// tolerates it while never matching a merely adjacent block (stacked
    /// lines overlap by ~10%).
    pub fn covers(&self, block: &TextBlock) -> bool {
        let left = self.region.x.max(block.x);
        let top = self.region.y.max(block.y);
        let right = (self.region.x + self.region.width as i32)
            .min(block.x + block.width as i32);
        let bottom = (self.region.y + self.region.height as i32)
            .min(block.y + block.height as i32);
        let overlap_w = (right - left).max(0);
        let overlap_h = (bottom - top).max(0);
        let overlap = overlap_w as f64 * overlap_h as f64;
        let block_area = (block.width as f64 * block.height as f64).max(1.0);
        overlap / block_area >= 0.5
    }
}

/// One contiguous run of text found by an OCR engine, with its bounding box.
/// The unit of tracking, display, and re-translation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextBlock {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub text: String,
}

/// The language Langception expects to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLanguage {
    En,
    Ja,
    Zh,
    Ko,
}

impl SourceLanguage {
    /// Language code used by translate engines.
    pub fn code(self) -> &'static str {
        match self {
            SourceLanguage::En => "en",
            SourceLanguage::Ja => "ja",
            SourceLanguage::Zh => "zh",
            SourceLanguage::Ko => "ko",
        }
    }
}

/// The only target language in scope for the MVP.
pub const TARGET_LANGUAGE: &str = "th";

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct CaptureError {
    pub message: String,
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct OcrError {
    pub message: String,
    /// The pass could not run because the engine's model files are not
    /// ready on disk (ADR-0008's not-ready state: the tier's Model Pack
    /// is still downloading). The loop reports the pass as loading and
    /// never covers it with another engine — only the user switching
    /// engines switches engines. Every plain failure leaves this false.
    pub not_ready: bool,
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct TranslateError {
    pub message: String,
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct FusedError {
    pub message: String,
}

macro_rules! error_new {
    ($($name:ident),*) => {
        $(impl $name {
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }
        })*
    };
}
// OcrError is excluded: it carries the not_ready mark, so both of its
// constructors are spelled out below.
error_new!(CaptureError, TranslateError, FusedError);

impl OcrError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            not_ready: false,
        }
    }

    /// The engine's model files are not on disk yet ([`OcrError`]'s
    /// `not_ready` mark) — the pass reports loading, never a fallback.
    pub fn not_ready(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            not_ready: true,
        }
    }
}

/// What the app watches: here, a fixed configured region.
#[async_trait]
pub trait CaptureSource: Send + Sync {
    async fn capture(&self, region: Region) -> Result<CapturedImage, CaptureError>;
}

/// A pluggable component that turns a captured image region into Text Blocks.
#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn recognize(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError>;
}

/// A Text Block read AND translated by one fused vision-LLM call (ticket
/// 12): the block's box and source text exactly as [`OcrEngine`] would
/// report them (region-relative), plus the translation that goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedBlock {
    pub block: TextBlock,
    pub translation: String,
}

/// Extra instructions an LLM-backed engine folds into its prompt (ticket
/// 11): the Glossary entries relevant to the Text Block being translated,
/// and whether Genderless Mode is on. The orchestrator computes both per
/// block; non-LLM engines ignore it ([`TranslateEngine::translate_guided`]'s
/// default), keyed/MT engines translate the bare text as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslationGuidance {
    /// `(source term, thai)` pairs whose term occurs in the block's text,
    /// already filtered by the caller (empty = nothing to inject).
    pub glossary: Vec<(String, String)>,
    /// Genderless Mode (CONTEXT.md) is on: the prompt must ask for Thai
    /// that carries no speaker gender.
    pub genderless: bool,
    /// A user-supplied replacement for the prompt's base instruction
    /// (ticket 37, Settings → Engines → Prompts). Only the base is
    /// replaced: the Glossary injection, the Genderless instruction, the
    /// fused reply contract, and the version tag still attach. `None` or
    /// whitespace-only means the built-in default (the prompt module
    /// normalizes, so callers may pass the raw field). The orchestrator
    /// fills it with the translation-prompt override on the classic path
    /// and the screen-reading one on the fused path.
    pub custom_base: Option<String>,
    /// The System Prompt Preset (CONTEXT.md) whose verbatim wording
    /// `custom_base` is (ticket 42): a built-in's code ("game") or a saved
    /// preset's name. Labels only — the version tag names the preset
    /// instead of saying `custom`, so a recorded request shows which
    /// preset produced it. `None` for the built-in default and for
    /// hand-written text.
    pub preset: Option<String>,
    /// Translation Context (ticket 44): the Capture Zone's last successful
    /// translations as `(source, thai)` pairs, oldest first, already
    /// trimmed to the configured window by the caller (empty = the feature
    /// is off — the prompt renders no section at all). LLM engines render
    /// it as a continuity section ([`crate::prompt`]); non-LLM engines
    /// discard it with the rest of the guidance.
    pub recent_translations: Vec<(String, String)>,
}

impl TranslationGuidance {
    /// No glossary entries, Genderless off — the plain-translation request.
    pub fn plain() -> Self {
        Self::default()
    }
}

/// A pluggable component that translates text from a Source Language to a target.
#[async_trait]
pub trait TranslateEngine: Send + Sync {
    async fn translate(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
    ) -> Result<String, TranslateError>;

    /// [`Self::translate`] with per-block LLM guidance (ticket 11): LLM
    /// engines fold `guidance` into their prompt (glossary injection +
    /// genderless instruction, [`crate::prompt`]); every other engine keeps
    /// this default, which discards the guidance and translates the bare
    /// text — so existing engines and test fakes need no changes.
    async fn translate_guided(
        &self,
        text: &str,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<String, TranslateError> {
        let _ = guidance;
        self.translate(text, source, target).await
    }

    /// Display name of the engine (privacy badge, Engines tab).
    fn name(&self) -> &str {
        "engine"
    }

    /// Whether the most recent `translate`/`translate_guided` call on this
    /// engine answered from a cache — the Debug Log's read provenance
    /// (ticket 04) needs it without reshaping every engine's return type.
    /// The loop is the only caller and reads it right after the await;
    /// calls are sequential per task, so no lock is needed. Engines that
    /// never cache keep the default.
    fn last_answer_cached(&self) -> bool {
        let _ = self;
        false
    }

    /// Whether this engine sends screen text off-machine. The shell consults
    /// it whenever a pipeline becomes active: a cloud engine must surface the
    /// privacy badge (spec: "the user always knows what leaves their
    /// computer"). Engines that never call out keep the default `false`.
    fn is_cloud(&self) -> bool {
        false
    }
}

/// A vision-LLM engine that reads AND translates in one call (ticket 12):
/// the whole captured image goes in, Text Blocks with positions and their
/// Thai translation come out — the separate [`OcrEngine`] and
/// [`TranslateEngine`] stages are bypassed. For stylized game fonts that
/// defeat traditional OCR. The loop still gates the call with change
/// detection: an unchanged frame never reaches the engine.
#[async_trait]
pub trait FusedEngine: Send + Sync {
    /// Reads every Text Block in `image` (region-relative boxes, like an
    /// OCR engine's) and returns each with its translation. `guidance`
    /// carries the prompt-level Glossary injection (the fused call sees the
    /// whole image, so it gets ALL entries — there is no OCR text to filter
    /// by) and the Genderless flag ([`crate::prompt::fused_system_prompt`]
    /// is the shared template).
    async fn recognize_and_translate(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
        target: &str,
        guidance: &TranslationGuidance,
    ) -> Result<Vec<FusedBlock>, FusedError>;

    /// Display name of the engine (privacy badge, Engines tab).
    fn name(&self) -> &str {
        "fused engine"
    }

    /// Whether this engine sends screen images off-machine — same contract
    /// as [`TranslateEngine::is_cloud`], and the fused engine is the one
    /// receiving screen data when it is active, so the shell badges from it.
    fn is_cloud(&self) -> bool {
        false
    }
}

/// Time abstraction for the loop: pacing between ticks and event timestamps.
#[async_trait]
pub trait Clock: Send + Sync {
    async fn sleep(&self, duration: Duration);
    fn now_ms(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclusion(x: i32, y: i32, width: u32, height: u32) -> ExcludedBlock {
        ExcludedBlock {
            id: "x1".into(),
            monitor: r"\\.\DISPLAY1".into(),
            region: Region {
                x,
                y,
                width,
                height,
            },
            label: "Start".into(),
        }
    }

    /// A keyed/MT engine (Google, Azure) as the port models it: it
    /// implements only `translate` and keeps the default `translate_guided`.
    struct BareEngine {
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TranslateEngine for BareEngine {
        async fn translate(
            &self,
            text: &str,
            _source: SourceLanguage,
            _target: &str,
        ) -> Result<String, TranslateError> {
            self.seen.lock().unwrap().push(text.to_string());
            Ok("คำแปล".into())
        }
    }

    #[tokio::test]
    async fn a_non_llm_engine_never_sees_the_prompt_guidance() {
        // Ticket 37's "keyed/MT engines are unchanged regardless of the
        // override text", a System Prompt Preset included (ticket 42), and
        // Translation Context included (ticket 44): the port's default
        // translate_guided discards the guidance wholesale — a custom base,
        // its preset label, and the zone's recent-translation pairs
        // included — so such an engine is called with the bare text exactly
        // as before.
        let engine = BareEngine {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let guidance = TranslationGuidance {
            glossary: vec![("Vertin".into(), "เวอร์ติน".into())],
            genderless: true,
            custom_base: Some("Translate like a pirate.".into()),
            preset: Some("game".into()),
            recent_translations: vec![("Hello".into(), "สวัสดี".into())],
        };
        let thai = engine
            .translate_guided("Vertin picks up the case", SourceLanguage::En, "th", &guidance)
            .await
            .unwrap();
        assert_eq!(thai, "คำแปล");
        assert_eq!(
            *engine.seen.lock().unwrap(),
            vec!["Vertin picks up the case".to_string()],
            "the bare text, nothing else — no guidance can reach a keyed engine"
        );
    }

    fn block_at(x: i32, y: i32, width: u32, height: u32) -> TextBlock {
        TextBlock {
            x,
            y,
            width,
            height,
            text: "Start".into(),
        }
    }

    #[test]
    fn covers_exact_same_rect() {
        let e = exclusion(100, 200, 120, 24);
        assert!(e.covers(&block_at(100, 200, 120, 24)));
    }

    #[test]
    fn covers_small_jitter() {
        // A few px of OCR jitter keep >50% of the block overlapping.
        let e = exclusion(100, 200, 120, 24);
        assert!(e.covers(&block_at(103, 203, 120, 24)));
    }

    #[test]
    fn does_not_cover_moved_block() {
        let e = exclusion(100, 200, 120, 24);
        // Shifted by more than half the box: overlap is way under half.
        assert!(!e.covers(&block_at(170, 200, 120, 24)));
        assert!(!e.covers(&block_at(100, 226, 120, 24)));
    }

    #[test]
    fn does_not_cover_stacked_neighbor_line() {
        // A second line 4px below a 24px-tall box overlaps by ~4/24 ≈ 17%.
        let e = exclusion(100, 200, 120, 24);
        assert!(!e.covers(&block_at(100, 220, 120, 24)));
    }

    #[test]
    fn covers_larger_block_contained_in_exclusion() {
        // The stored exclusion is bigger than the re-appearing block.
        let e = exclusion(90, 190, 200, 60);
        assert!(e.covers(&block_at(100, 200, 120, 24)));
    }

    // ---- Region::intersection (ticket 46's capture-mask geometry) ----

    fn region(x: i32, y: i32, width: u32, height: u32) -> Region {
        Region { x, y, width, height }
    }

    #[test]
    fn intersection_of_overlapping_rects_is_the_overlap() {
        let a = region(0, 0, 100, 100);
        let b = region(50, 60, 200, 200);
        assert_eq!(a.intersection(&b), Some(region(50, 60, 50, 40)));
    }

    #[test]
    fn intersection_of_contained_rect_is_the_inner_one() {
        let outer = region(-50, -50, 400, 400);
        let inner = region(10, 20, 30, 40);
        assert_eq!(outer.intersection(&inner), Some(inner));
        assert_eq!(inner.intersection(&outer), Some(inner));
    }

    #[test]
    fn disjoint_and_touching_rects_intersect_to_none() {
        let a = region(0, 0, 100, 100);
        // Fully apart.
        assert_eq!(a.intersection(&region(200, 0, 10, 10)), None);
        // Edge-touching is not an intersection: a mask exactly beside the
        // captured region must fill nothing.
        assert_eq!(a.intersection(&region(100, 0, 50, 50)), None);
        // Negative origins compose correctly (multi-monitor).
        assert_eq!(
            a.intersection(&region(-50, -50, 55, 55)),
            Some(region(0, 0, 5, 5))
        );
    }
}
