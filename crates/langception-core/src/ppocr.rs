//! PP-OCRv6 (pp-ocrv6-sidecar feature, ticket 04): the NEW bundled OCR
//! engine, run through the `pp-ocrv6-sidecar` binary we own (Rust on ONNX
//! Runtime, pipeline adopted from the oar-ocr crate — ADR-0005). The
//! sidecar speaks the same JSON-lines pipe protocol shape as the old one,
//! so the shared `sidecar` machinery drives it unchanged; what this
//! module owns is the new engine's config surface and launch contract:
//!
//! - the config values name the Model Tier explicitly (`pp-ocrv6-tiny` /
//!   `pp-ocrv6-small` / `pp-ocrv6-medium` — there is no separate tier
//!   setting anywhere in config or UI);
//! - the legacy `paddle` value keeps parsing and maps to Small, so an
//!   upgrading user's saved choice silently improves their OCR instead
//!   of resetting or breaking it (spec story 5);
//! - launch arguments are `-models_path=<dir> -language=<en|ja|zh|ko>
//!   -tier=<tiny|small|medium>` (ticket 02's contract) — the model
//!   mapping (which detector/recognizer/dictionary each pair selects,
//!   Korean's tier-independent PP-OCRv5 recognizer included) lives
//!   inside the sidecar; a (language, tier) change is a sidecar restart.
//!
//! Lifecycle behavior is integration-tested offline against the scripted
//! `ocr_stub` binary — see `tests/ppocr_sidecar_tests.rs`. The real
//! sidecar is exercised by the manual smoke checklist, like every other
//! live engine.

use crate::ports::{CapturedImage, OcrEngine, OcrError, SourceLanguage, TextBlock};
use crate::sidecar::{base_command, SidecarEngine};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Name of the sidecar executable — the shell's resolver and the release
/// packaging look for exactly this file. In the release bundle it sits
/// beside the models manifest and the bundled Tiny baseline (the launch
/// contract's models dir — the active models directory since the
/// deferred-model-packs feature — holds the same flat file layout); in a
/// dev workspace it comes from the target dir.
pub const SIDECAR_EXE: &str = "pp-ocrv6-sidecar.exe";

/// The bundled directory name the models (and, in a release bundle, the
/// sidecar executable) live in — the shell's fallback path when nothing
/// resolves points here so the spawn error names the layout, not a wrong
/// path.
pub const BUNDLE_DIR_NAME: &str = "pp-ocrv6";

/// The Model Tier an engine value selects. Only Tiny ships in the bundle
/// (≈ 6 MB ONNX, CPU-only) since the deferred-model-packs feature
/// (ADR-0008); Small and Medium arrive as Model Packs on first use, and
/// the Korean recognizer set is its own pack. English, Japanese, and
/// Chinese read through the selected tier, Korean through the official
/// Korean PP-OCRv5 recognizer at every tier (inside the sidecar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelTier {
    Tiny,
    Small,
    Medium,
}

impl ModelTier {
    /// The tier's spelling in the launch contract and the config value.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelTier::Tiny => "tiny",
            ModelTier::Small => "small",
            ModelTier::Medium => "medium",
        }
    }

    /// The value's spelling in `[ocr] engine` and the Settings dropdown.
    pub fn config_value(self) -> &'static str {
        match self {
            ModelTier::Tiny => "pp-ocrv6-tiny",
            ModelTier::Small => "pp-ocrv6-small",
            ModelTier::Medium => "pp-ocrv6-medium",
        }
    }

    /// The engine's name for notices and log lines — the tier names
    /// itself (spec story 11: the user knows exactly what they selected).
    pub fn display_name(self) -> &'static str {
        match self {
            ModelTier::Tiny => "PP-OCRv6-tiny",
            ModelTier::Small => "PP-OCRv6-small",
            ModelTier::Medium => "PP-OCRv6-medium",
        }
    }

    /// Parses the tier out of a `-tier=` launch value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tiny" => Some(ModelTier::Tiny),
            "small" => Some(ModelTier::Small),
            "medium" => Some(ModelTier::Medium),
            _ => None,
        }
    }
}

/// Maps an `[ocr] engine` config value to the tier of THIS engine, or
/// `None` when the value selects a different engine (Windows OCR) or is
/// unknown. The `pp-ocrv6-*` values are derived from [`ModelTier`]'s own
/// vocabulary (one spelling of the tiers, not a second switch over it);
/// the legacy `paddle` value maps to Small per the spec's config-surface
/// decision.
pub fn tier_for_engine_value(value: &str) -> Option<ModelTier> {
    if let Some(tier) = value.strip_prefix("pp-ocrv6-") {
        return ModelTier::parse(tier);
    }
    match value {
        "paddle" => Some(ModelTier::Small),
        _ => None,
    }
}

/// The restart key: one model selection. Both halves matter — the tier's
/// detector and recognizer load at launch, and the language selects the
/// mapping (Korean's v5 recognizer included), so any change restarts the
/// sidecar on the matching models.
type ModelSelection = (SourceLanguage, ModelTier);

/// Builds the launch command for one model selection, per ticket 02's
/// contract. `models_dir` is passed through as given — the resolver
/// produces absolute paths, and the sidecar resolves them itself (no
/// working-directory dance like the old engine's relative configs).
fn launch_command(
    program: &Path,
    models_dir: &Path,
    selection: ModelSelection,
) -> Result<Command, String> {
    let (source, tier) = selection;
    let mut command = base_command(program);
    command
        .arg(format!("-models_path={}", models_dir.display()))
        .arg(format!("-language={}", source.code()))
        .arg(format!("-tier={}", tier.as_str()));
    Ok(command)
}

/// The unsupported-combination rule (ticket 05): tiny's vocabulary
/// carries Latin + CJK ideographs but zero kana, so Japanese+tiny is the
/// only pairing that cannot read as selected — it runs the nearest
/// supported tier, the whole small model set (small detector included:
/// mixing detectors across tiers would lose the per-tier tuned
/// thresholds). Everything else is honored as selected: English and
/// Chinese honor tiny, and Korean's v5 recognizer is tier-independent
/// inside the sidecar — by definition, not a fallback.
/// Pub(crate) so the Model Pack resolver (the `model_packs` module,
/// deferred-model-packs ticket 01) derives required packs from the same
/// rule the launch path runs — one substitution decision, two consumers.
pub(crate) fn effective_selection(source: SourceLanguage, tier: ModelTier) -> ModelSelection {
    match (source, tier) {
        (SourceLanguage::Ja, ModelTier::Tiny) => (SourceLanguage::Ja, ModelTier::Small),
        _ => (source, tier),
    }
}

/// Fires when a pass ran a substituted model set (Japanese with tiny
/// selected — see [`PpOcrv6Engine::with_substitution_notice`]) with the
/// tier the user SELECTED. The callback composes the message itself, so
/// the wording lives with the app's other user-visible strings (the
/// notices seam, ticket 62) instead of baked-in English here; the
/// substituted target is by definition the small tier
/// (`effective_selection`), so the selected tier is all it needs.
pub type SubstitutionNotice = std::sync::Arc<dyn Fn(ModelTier) + Send + Sync>;

/// Fires on the gate's two transitions (deferred-model-packs ticket 02):
/// `Some` when a pass is blocked on pack files (the not-ready state —
/// the structured [`crate::model_packs::NotReady`] feeds the wording
/// composed at the notices seam), and `None` when a later pass finds
/// everything ready, so the loading notice retires the moment reading
/// resumes. One `Some` per incident (a transition into not-ready), never
/// one per pass.
pub type NotReadyNotice =
    std::sync::Arc<dyn Fn(Option<&crate::model_packs::NotReady>) + Send + Sync>;

/// The PP-OCRv6 engine as an [`OcrEngine`]: a thin wrapper over the
/// shared [`SidecarEngine`] state machine (lazy spawn, reuse across
/// passes, respawn on death with a notice, restart on (language, tier)
/// change), plus the unsupported-combination rule above. When a
/// readiness gate is wired (the deferred-packs shell wiring), a pass
/// whose pack files are missing or stale reports loading and never
/// spawns; everything else behaves as before. A pass whose request
/// fails returns an error immediately (the shell's fallback wrapper
/// surfaces it and covers the pass with Windows OCR — except the
/// not-ready state, which the wrapper passes through untouched); the
/// next call spawns fresh.
pub struct PpOcrv6Engine {
    tier: ModelTier,
    engine: SidecarEngine<ModelSelection>,
    /// Fires when a pass launches a substituted model set (Japanese with
    /// tiny selected) — see [`PpOcrv6Engine::with_substitution_notice`].
    on_substitution: Option<SubstitutionNotice>,
    /// The substituted session a notice was already fired for: one
    /// notice per incident (a transition into the pairing), never one
    /// per pass. Reset by any honored pairing, so leaving Japanese and
    /// re-entering it with tiny notices again.
    noticed_substitution: std::sync::Mutex<Option<ModelSelection>>,
    /// The spawn gate (ADR-0008): when wired, a pass whose model files
    /// are missing or stale in the launch directory reports the
    /// not-ready state — loading notice, error marked `not_ready`, no
    /// spawn. `None` keeps the ungated (bundled-era) behavior.
    gate: Option<std::sync::Arc<crate::model_packs::ReadinessGate>>,
    /// Observes the gate's loading/ready transitions — see
    /// [`NotReadyNotice`].
    on_not_ready: Option<NotReadyNotice>,
    /// Whether a loading notice already fired for the current not-ready
    /// incident: one per incident, re-armed by a ready pass.
    loading_noticed: std::sync::Mutex<bool>,
}

impl PpOcrv6Engine {
    pub fn new(
        program: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        tier: ModelTier,
    ) -> Self {
        Self::with_timeouts(
            program,
            models_dir,
            tier,
            crate::sidecar::INIT_TIMEOUT,
            crate::sidecar::REQUEST_TIMEOUT,
        )
    }

    /// The timeout knobs are constructor settings, not constants, so the
    /// stub tests can run fast while production keeps generous ones.
    pub fn with_timeouts(
        program: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        tier: ModelTier,
        init_timeout: std::time::Duration,
        request_timeout: std::time::Duration,
    ) -> Self {
        let program = program.into();
        let models_dir = models_dir.into();
        let launch = Box::new(move |selection: &ModelSelection| {
            launch_command(&program, &models_dir, *selection)
        });
        Self {
            tier,
            engine: SidecarEngine::with_timeouts("PP-OCRv6", launch, init_timeout, request_timeout),
            on_substitution: None,
            noticed_substitution: std::sync::Mutex::new(None),
            gate: None,
            on_not_ready: None,
            loading_noticed: std::sync::Mutex::new(false),
        }
    }

    /// Observes unexpected sidecar deaths (see
    /// [`crate::sidecar::RestartNotice`]).
    pub fn with_restart_notice(mut self, notice: crate::sidecar::RestartNotice) -> Self {
        self.engine = self.engine.with_restart_notice(notice);
        self
    }

    /// Observes the unsupported-combination rule kicking in (ticket 05):
    /// Japanese text with tiny selected runs on the small model set, and
    /// this fires once per transition into that pairing (the same surface
    /// as the restart notice) so the loop never silently degrades. The
    /// callback receives the SELECTED tier and composes the message (the
    /// notices seam, ticket 62). Fired on the caller's async context,
    /// briefly.
    pub fn with_substitution_notice(mut self, notice: SubstitutionNotice) -> Self {
        self.on_substitution = Some(notice);
        self
    }

    /// Gates every pass on the effective selection's model files being
    /// on disk (deferred-model-packs ticket 02, ADR-0008): a missing or
    /// stale pack reports the loading state through
    /// [`PpOcrv6Engine::with_not_ready_notice`] and an `OcrError` marked
    /// `not_ready` (the fallback wrapper passes those through — the loop
    /// never covers them with another engine), and never spawns. A ready
    /// verdict has also assembled the launch directory, so the spawn
    /// reads complete model files. `gate.verdict` runs per pass — cheap
    /// by design — so a pack landing mid-session is picked up by the
    /// next pass, no app restart.
    pub fn with_readiness_gate(
        mut self,
        gate: std::sync::Arc<crate::model_packs::ReadinessGate>,
    ) -> Self {
        self.gate = Some(gate);
        self
    }

    /// Observes the gate's loading/ready transitions — see
    /// [`NotReadyNotice`]. Fired on the caller's async context, briefly.
    pub fn with_not_ready_notice(mut self, notice: NotReadyNotice) -> Self {
        self.on_not_ready = Some(notice);
        self
    }
}

#[async_trait]
impl OcrEngine for PpOcrv6Engine {
    async fn recognize(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        let requested = (source, self.tier);
        let effective = effective_selection(source, self.tier);
        let substituted = effective != requested;
        // The gate runs before anything else, so a blocked pass touches
        // neither notice state: the pack files of the EFFECTIVE
        // selection must be on disk before any spawn is attempted
        // (ADR-0008 — lazy packs, never a loud spawn failure).
        if let Some(gate) = &self.gate {
            match gate.verdict(source, self.tier) {
                crate::model_packs::Readiness::Ready => {
                    // Reading resumed: a fired loading notice retires on
                    // this transition (only on it — steady-state ready
                    // passes stay silent), and the next not-ready
                    // incident is a new one worth its own notice.
                    let mut noticed = self
                        .loading_noticed
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if *noticed {
                        *noticed = false;
                        if let Some(notice) = &self.on_not_ready {
                            notice(None);
                        }
                    }
                }
                crate::model_packs::Readiness::NotReady(not_ready) => {
                    let mut noticed = self
                        .loading_noticed
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if !*noticed {
                        *noticed = true;
                        if let Some(notice) = &self.on_not_ready {
                            notice(Some(&not_ready));
                        }
                    }
                    return Err(OcrError::not_ready(format!(
                        "{} is loading — {}",
                        self.tier.display_name(),
                        not_ready.summary()
                    )));
                }
                crate::model_packs::Readiness::Broken(reason) => {
                    // A broken manifest or bundle is not the loading
                    // state — nothing a download fixes. A plain error:
                    // the shell's fallback wrapper names it and covers
                    // the pass, the old broken-bundle behavior.
                    return Err(OcrError::new(format!(
                        "{}: cannot verify its model files — {}",
                        self.tier.display_name(),
                        reason
                    )));
                }
            }
        }
        if !substituted {
            // An honored pairing ran: reset so the next transition into
            // the substituted pairing is a new incident worth its own
            // notice.
            *self
                .noticed_substitution
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
        }
        let result = self.engine.recognize(image, effective).await;
        // The notice claims the small model set RAN, so it fires after a
        // pass that actually ran it — one per substituted session (the
        // dedupe key is the selection itself, so continuous Japanese
        // passes notice once while a language switch away and back
        // re-notices: each marks a model restart). A failed pass stays
        // silent here; the fallback wrapper's warning names why the
        // engine could not run at all.
        if substituted && result.is_ok() {
            let mut noticed = self
                .noticed_substitution
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if *noticed != Some(effective) {
                *noticed = Some(effective);
                if let Some(notice) = &self.on_substitution {
                    notice(self.tier);
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_vocabulary_roundtrips_through_the_launch_contract() {
        for tier in [ModelTier::Tiny, ModelTier::Small, ModelTier::Medium] {
            assert_eq!(ModelTier::parse(tier.as_str()), Some(tier));
            assert_eq!(
                ModelTier::parse(tier.config_value().trim_start_matches("pp-ocrv6-")),
                Some(tier),
            );
        }
        assert_eq!(ModelTier::parse("xl"), None);
        assert_eq!(ModelTier::parse(""), None);
    }

    #[test]
    fn every_engine_value_maps_to_a_tier_and_the_legacy_value_to_small() {
        assert_eq!(tier_for_engine_value("pp-ocrv6-tiny"), Some(ModelTier::Tiny));
        assert_eq!(tier_for_engine_value("pp-ocrv6-small"), Some(ModelTier::Small));
        assert_eq!(
            tier_for_engine_value("pp-ocrv6-medium"),
            Some(ModelTier::Medium),
        );
        // Spec story 5: the upgrading user's choice keeps working — the
        // legacy bundled-engine value silently improves to small.
        assert_eq!(tier_for_engine_value("paddle"), Some(ModelTier::Small));
        // Windows OCR and unknown names select no tier of this engine.
        assert_eq!(tier_for_engine_value("windows"), None);
        assert_eq!(tier_for_engine_value("tesseract"), None);
    }

    #[test]
    fn launch_arguments_follow_the_ticket_02_contract() {
        let dir = Path::new("E:/models/pp-ocrv6");
        let command = launch_command(
            Path::new("pp-ocrv6-sidecar.exe"),
            dir,
            (SourceLanguage::Ko, ModelTier::Tiny),
        )
        .unwrap();
        // Command's args are not directly inspectable on all platforms,
        // but the program name is, and the arg spelling is pinned by the
        // sidecar's own parser tests — build the same strings here.
        let expected = [
            format!("-models_path={}", dir.display()),
            "-language=ko".to_string(),
            "-tier=tiny".to_string(),
        ];
        let debug = format!("{command:?}");
        for arg in expected {
            assert!(debug.contains(&arg), "launch args must carry {arg}: {debug}");
        }
    }

    #[test]
    fn every_tier_display_and_config_spelling_is_self_describing() {
        assert_eq!(ModelTier::Tiny.display_name(), "PP-OCRv6-tiny");
        assert_eq!(ModelTier::Small.display_name(), "PP-OCRv6-small");
        assert_eq!(ModelTier::Medium.display_name(), "PP-OCRv6-medium");
        assert_eq!(ModelTier::Tiny.config_value(), "pp-ocrv6-tiny");
        assert_eq!(ModelTier::Small.config_value(), "pp-ocrv6-small");
        assert_eq!(ModelTier::Medium.config_value(), "pp-ocrv6-medium");
    }

    #[test]
    fn japanese_with_tiny_is_the_only_substituted_pairing() {
        // The unsupported-combination rule (ticket 05): tiny's vocabulary
        // has no kana, so Japanese runs on the nearest supported tier —
        // the whole small pipeline (mixing detectors across tiers would
        // lose the per-tier tuned thresholds).
        assert_eq!(
            effective_selection(SourceLanguage::Ja, ModelTier::Tiny),
            (SourceLanguage::Ja, ModelTier::Small),
        );
        // Everything else is honored as selected: tiny is fine for
        // English and Chinese (Latin + CJK ideographs), medium and small
        // cover Japanese, and Korean's v5 recognizer is tier-independent
        // inside the sidecar.
        for tier in [ModelTier::Tiny, ModelTier::Small, ModelTier::Medium] {
            assert_eq!(
                effective_selection(SourceLanguage::En, tier),
                (SourceLanguage::En, tier),
            );
            assert_eq!(
                effective_selection(SourceLanguage::Zh, tier),
                (SourceLanguage::Zh, tier),
            );
            assert_eq!(
                effective_selection(SourceLanguage::Ko, tier),
                (SourceLanguage::Ko, tier),
            );
        }
        // Small and medium cover Japanese as selected; tiny is the only
        // substitution (asserted above).
        for tier in [ModelTier::Small, ModelTier::Medium] {
            assert_eq!(
                effective_selection(SourceLanguage::Ja, tier),
                (SourceLanguage::Ja, tier),
            );
        }
    }
}
