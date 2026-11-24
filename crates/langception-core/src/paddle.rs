//! PaddleOCR (ticket 31): the bundled OCR engine, run through the
//! PaddleOCR-json sidecar — the same offline engine Umi-OCR and
//! LunaTranslator use (Apache-2.0). Since the pp-ocrv6-sidecar feature's
//! ticket 04 this is the OLD engine: the config's "paddle" value now maps
//! to the new PP-OCRv6 engine (see `ppocr.rs`), and this implementation
//! stays only until ticket 06 deletes it with the old bundle (the manual
//! smoke checklist still drives it against the old archive's layout).
//!
//! Everything protocol-shaped lives in the shared `sidecar` machinery
//! (both engines speak the same JSON-lines pipe protocol); what remains
//! here is the old engine's launch contract: the sidecar executable and
//! models-dir names, and the per-language config file that selects the
//! recognition model. Lifecycle behavior is integration-tested offline
//! against the scripted `paddle_stub` binary — see
//! `tests/paddle_sidecar_tests.rs`.

use crate::ports::{CapturedImage, OcrEngine, OcrError, SourceLanguage, TextBlock};
use crate::sidecar::{base_command, SidecarEngine};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Name of the sidecar executable inside the bundled directory — the shell's
/// resolver looks for exactly this file.
pub const SIDECAR_EXE: &str = "PaddleOCR-json.exe";

/// The bundled models directory, a sibling of [`SIDECAR_EXE`]; the
/// per-language config files inside it (`config_en.txt`, …) select the
/// recognition model.
pub const MODELS_DIR: &str = "models";

/// The bundled config file that selects the recognition model for one
/// Source Language (the archive ships EN/JA/ZH/KO; a language switch is a
/// sidecar restart with the matching config — models are launch-time only).
pub fn config_file_for(source: SourceLanguage) -> &'static str {
    match source {
        SourceLanguage::En => "config_en.txt",
        SourceLanguage::Ja => "config_japan.txt",
        SourceLanguage::Zh => "config_chinese.txt",
        SourceLanguage::Ko => "config_korean.txt",
    }
}

/// Builds the launch command for one Source Language: the bundled layout
/// is exe + `models/` siblings, and the config files inside `models/`
/// reference `models/...` relative paths — the engine resolves them
/// against its working directory, so CWD is the exe's directory (how
/// Umi-OCR launches it too).
fn launch_command(
    program: &Path,
    models_dir: &Path,
    config: &'static str,
) -> Result<Command, String> {
    let root = models_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let models_name = models_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "models directory has no name".to_string())?;
    let mut command = base_command(program);
    command
        .arg(format!("-models_path={models_name}"))
        .arg(format!("-config_path={models_name}/{config}"))
        .current_dir(&root);
    Ok(command)
}

/// The bundled PaddleOCR engine as an [`OcrEngine`]: a thin wrapper over
/// the shared [`SidecarEngine`] state machine (lazy spawn, reuse across
/// passes, respawn on death with a notice, restart on language change).
pub struct PaddleOcrEngine {
    engine: SidecarEngine<&'static str>,
}

impl PaddleOcrEngine {
    pub fn new(program: impl Into<PathBuf>, models_dir: impl Into<PathBuf>) -> Self {
        Self::with_timeouts(program, models_dir, crate::sidecar::INIT_TIMEOUT, crate::sidecar::REQUEST_TIMEOUT)
    }

    /// The timeout knobs are constructor settings, not constants, so the
    /// stub tests can run fast while production keeps generous ones.
    pub fn with_timeouts(
        program: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        init_timeout: std::time::Duration,
        request_timeout: std::time::Duration,
    ) -> Self {
        let program = program.into();
        let models_dir = models_dir.into();
        let launch = Box::new(move |config: &&'static str| {
            launch_command(&program, &models_dir, config)
        });
        Self {
            engine: SidecarEngine::with_timeouts("PaddleOCR", launch, init_timeout, request_timeout),
        }
    }

    /// Observes unexpected sidecar deaths (see
    /// [`crate::sidecar::RestartNotice`]).
    pub fn with_restart_notice(
        mut self,
        notice: crate::sidecar::RestartNotice,
    ) -> Self {
        self.engine = self.engine.with_restart_notice(notice);
        self
    }
}

#[async_trait]
impl OcrEngine for PaddleOcrEngine {
    async fn recognize(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        self.engine.recognize(image, config_file_for(source)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_language_has_a_bundled_model_config() {
        assert_eq!(config_file_for(SourceLanguage::En), "config_en.txt");
        assert_eq!(config_file_for(SourceLanguage::Ja), "config_japan.txt");
        assert_eq!(config_file_for(SourceLanguage::Zh), "config_chinese.txt");
        assert_eq!(config_file_for(SourceLanguage::Ko), "config_korean.txt");
    }
}
