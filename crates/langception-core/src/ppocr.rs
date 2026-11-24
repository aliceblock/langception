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
//! `paddle_stub` binary — see `tests/ppocr_sidecar_tests.rs`. The real
//! sidecar is exercised by the manual smoke checklist, like every other
//! live engine.

use crate::ports::{CapturedImage, OcrEngine, OcrError, SourceLanguage, TextBlock};
use crate::sidecar::{base_command, SidecarEngine};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Name of the sidecar executable — the shell's resolver and the release
/// packaging look for exactly this file. In the release bundle it sits
/// beside the ten model files (the launch contract's models dir is the
/// same flat directory); in a dev workspace it comes from the target dir.
pub const SIDECAR_EXE: &str = "pp-ocrv6-sidecar.exe";

/// The bundled directory name the models (and, in a release bundle, the
/// sidecar executable) live in — the shell's fallback path when nothing
/// resolves points here so the spawn error names the layout, not a wrong
/// path.
pub const BUNDLE_DIR_NAME: &str = "pp-ocrv6";

/// The Model Tier an engine value selects. All three ship in the bundle
/// (≈ 6 / 30 / 132 MB ONNX, CPU-only); English, Japanese, and Chinese
/// read through the selected tier, Korean through the official Korean
/// PP-OCRv5 recognizer at every tier (inside the sidecar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The PP-OCRv6 engine as an [`OcrEngine`]: a thin wrapper over the
/// shared [`SidecarEngine`] state machine (lazy spawn, reuse across
/// passes, respawn on death with a notice, restart on (language, tier)
/// change). A pass whose request fails returns an error immediately (the
/// shell's fallback wrapper surfaces it and covers the pass with Windows
/// OCR); the next call spawns fresh.
pub struct PpOcrv6Engine {
    tier: ModelTier,
    engine: SidecarEngine<ModelSelection>,
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
        }
    }

    /// Observes unexpected sidecar deaths (see
    /// [`crate::sidecar::RestartNotice`]).
    pub fn with_restart_notice(mut self, notice: crate::sidecar::RestartNotice) -> Self {
        self.engine = self.engine.with_restart_notice(notice);
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
        self.engine
            .recognize(image, (source, self.tier))
            .await
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
}
