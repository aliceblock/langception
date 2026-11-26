//! The settings window (ticket 06): one surface for every config the app
//! keeps, doubling as the first-run wizard host. The Results Window stays
//! the live dashboard; this window owns the *editing*: General (language,
//! cadence, stability, Genderless Mode, start-with-Windows), Engines
//! (engine card + OCR language-pack detection), Glossary (full
//! management + JSON import/export), Profiles (ticket 07: per-app
//! configuration, each with its own Glossary layer — entry editing plus
//! the same JSON import/export as the global Glossary, ticket 33), Text
//! Style, Hotkeys (rebinds apply live), About (with the Update Check,
//! ticket 28: the manual check button, the opt-in launch-time check, and
//! the download-page action).
//!
//! The wizard is a view inside this window: on the first launch (no config
//! file when the app started) the shell opens it automatically and the page
//! shows the wizard before the tabs. Skipping it never blocks anything —
//! zero-config still works (ADR-0002's keyless Google engine is the
//! default). Re-running it later is an About-tab link.
//!
//! Threading mirrors shell.rs: every command that can create a window or
//! (re)start the loop goes through [`crate::shell::run_off_main`].

use crate::shell::{
    apply_always_on_top_from_config, apply_capture_visibility_from_config, apply_state,
    current_config, ghost_free_hide, ghost_free_show, profile_edited, push_warning,
    restart_loop_if_running, run_off_main, with_config_mut, AppState, HotkeyStatus,
};
use langception_core::config::{AppConfig, HotkeySettings};
use langception_core::glossary::Glossary;
use langception_core::model_packs::{delete_unused_packs, pack_status, PackId, PackStatus};
use langception_core::pack_downloader::PackDownloadState;
use langception_core::ports::{SourceLanguage, TARGET_LANGUAGE};
use langception_core::profile::{GenderlessOverride, Profile};
use langception_core::prompt::{BuiltinPreset, PromptTexts};
use langception_core::prompt_preset::{ActivePromptPreset, EffectivePrompts};
use langception_core::update::UpdateCheckOutcome;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::Shortcut;

pub const SETTINGS_LABEL: &str = "settings";

/// Opens the settings window, or focuses it when it is already open (it
/// hides instead of closing, so reopening keeps the page's state).
///
/// Threading: MUST NOT run on the main thread (creates/shows windows — see
/// `run_off_main`; every caller routes through it).
pub fn open_settings(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        ghost_free_show(&window);
        let _ = window.set_focus();
        return Ok(());
    }

    let window =
        WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("settings.html".into()))
            .title("Langception — Settings")
            .inner_size(740.0, 640.0)
            // Minimums measured where the layout actually breaks (ticket 17):
            // the tab strip ends at 552px, the config forms need ~510px.
            .min_inner_size(620.0, 440.0)
            .resizable(true)
            // Frameless (ticket 16): the page draws its own title bar with a
            // data-tauri-drag-region (drag + double-click maximize) and
            // min/close buttons; close still hides to tray.
            .decorations(false)
            .center()
            .visible(false)
            .build()
            .map_err(|e| format!("cannot open the settings window: {e}"))?;

    // Pin (or don't) per config before the first show (ticket 34) — a
    // broken config falls back to today's off default inside the helper.
    apply_always_on_top_from_config(app);

    // Same rule as the Results Window (ADR-0003): Langception's own windows
    // are never translation sources. Capture Visibility (ticket 46) picks
    // the protection per config — exclusion (off, the default) or masking
    // (on) — applied to this window (and any others present) before the
    // first show.
    apply_capture_visibility_from_config(app);
    window
        .show()
        .map_err(|e| format!("cannot show the settings window: {e}"))?;
    let _ = window.set_focus();
    Ok(())
}

// ---- Commands: window + wizard ----

#[tauri::command]
pub(crate) async fn open_settings_window(app: AppHandle) -> Result<(), String> {
    run_off_main(app, open_settings).await
}

/// Ticket 39: the settings window's end of the close handshake. CloseRequested
/// prevents the close and pings the page (`settings-request-close`); the page
/// owns the dirty state, so it warns about unsaved edits first and calls this
/// to do the actual hide once the user chose to dismiss. Threading: MUST NOT
/// run on the main thread (window manipulation — see `run_off_main`).
#[tauri::command]
pub(crate) async fn hide_settings(app: AppHandle) -> Result<(), String> {
    run_off_main(app, |app| {
        if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
            // Cloak + hide (ghost-free — see shell.rs's set_cloak): a bare
            // hide leaves the DWM surface composably dead, which a later
            // webview creation turns into the white-sheet ghost.
            ghost_free_hide(&window);
        }
        Ok(())
    })
    .await
}

/// The Engines tab / wizard re-probe. WinRT pack enumeration blocks and
/// needs COM, so it runs on the blocking pool (never the main thread).
#[tauri::command]
pub(crate) async fn get_ocr_pack_status() -> Result<Vec<crate::ocr_detect::OcrPackStatus>, String> {
    tauri::async_runtime::spawn_blocking(crate::ocr_detect::detect_ocr_packs)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

/// The wizard's "install the missing pack" jump: the language list lives at
/// Time & Language → Language & region (explorer accepts shell URIs).
#[tauri::command]
pub(crate) fn open_language_pack_settings() {
    let _ = std::process::Command::new("explorer")
        .arg("ms-settings:regionlanguage")
        .spawn();
}

// ---- Model Packs (deferred-model-packs ticket 04) ----
// The Engines tab's download surface: per-pack state (disk + download
// activity + required-by-the-current-selection), the start/cancel/Retry
// controls, and the delete-unused cleanup. The downloader lives on
// [`AppState`] (shell.rs built it at setup); these commands are the
// window's observers-and-actuators over it — closing this window drops
// nothing (ADR-0008: the lifecycle is core-owned).

/// What the pack commands answer when the resolved bundle carries no
/// models manifest (the legacy fetched layout, still the dev workspace's
/// shape): every model is already on disk, so there is nothing to
/// download, cancel, or clean up — the page hides its pack surface.
const NO_PACK_MANAGER: &str =
    "no models manifest — this build's models are already on disk (nothing to download)";

/// One pack's state as the Engines tab renders it: the on-disk half
/// (hashed against the manifest), the downloader's activity half, and
/// whether the CURRENT effective selection requires the pack (the delete
/// button's keep-set is the complement). Pack names stay Latin in Thai,
/// the same call ticket 02's notices made (`display_name` is brand-like).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackStateDto {
    pub pack: PackId,
    pub display_name: &'static str,
    /// "ready" | "missing" | "stale".
    pub disk: &'static str,
    /// The downloader's activity state — the same wire shape its events
    /// carry (`{"state": "not-downloading"}` / `downloading` / `failed`).
    pub download: PackDownloadState,
    pub required: bool,
    /// The pinned total of the pack's files — what a download fetches,
    /// for the reclaim-size note.
    pub total_bytes: u64,
}

/// The Engines tab's pack snapshot. `managed: false` = no models manifest
/// (see [`NO_PACK_MANAGER`]); the page hides its pack surface entirely.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackStatesDto {
    pub managed: bool,
    pub packs: Vec<PackStateDto>,
}

fn disk_code(status: &PackStatus) -> &'static str {
    match status {
        PackStatus::Ready => "ready",
        PackStatus::Missing { .. } => "missing",
        PackStatus::Stale { .. } => "stale",
    }
}

fn parse_pack_id(pack: &str) -> Result<PackId, String> {
    PackId::ALL.iter().copied().find(|id| id.as_str() == pack).ok_or_else(|| {
        format!(
            "unknown pack \"{pack}\" — packs: {}",
            PackId::ALL
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// The downloader the shell built at setup, cloned out of state.
pub(crate) fn pack_downloader_of(app: &AppHandle) -> Option<std::sync::Arc<langception_core::pack_downloader::PackDownloader>> {
    app.state::<AppState>()
        .pack_downloader
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// The saved selection the pack surface renders against: the global OCR
/// engine value + Source Language (the EFFECTIVE-selection inputs — the
/// required-set derivation handles the Japanese substitution and the
/// Korean recognizer themselves).
pub(crate) fn current_selection(app: &AppHandle) -> Result<(String, SourceLanguage), String> {
    let state = app.state::<AppState>();
    let guard = state
        .config
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    match &*guard {
        Some(Ok(config)) => Ok((config.ocr.engine.clone(), config.source.language)),
        Some(Err(message)) => Err(message.clone()),
        None => Err("config not loaded yet".into()),
    }
}

/// The snapshot itself, pure over its inputs so the shell tests can pin
/// the DTO without an app. The disk half hashes every pinned file of
/// every pack (`pack_status`), so callers keep it off the main thread.
fn pack_states_snapshot(
    downloader: Option<&langception_core::pack_downloader::PackDownloader>,
    active_dir: &std::path::Path,
    ocr_engine: &str,
    language: SourceLanguage,
) -> PackStatesDto {
    let Some(downloader) = downloader else {
        return PackStatesDto {
            managed: false,
            packs: Vec::new(),
        };
    };
    let manifest = downloader.manifest();
    let required =
        langception_core::model_packs::required_packs_for_engine_value(ocr_engine, language);
    let packs = PackId::ALL
        .iter()
        .map(|id| {
            let status = pack_status(manifest.pack(*id), active_dir);
            PackStateDto {
                pack: *id,
                display_name: id.display_name(),
                disk: disk_code(&status),
                download: downloader.state(*id),
                required: required.is_some_and(|required| required.contains(id)),
                total_bytes: manifest.pack(*id).files.iter().map(|f| f.size).sum(),
            }
        })
        .collect();
    PackStatesDto {
        managed: true,
        packs,
    }
}

/// A fresh snapshot for a command's response, so the page re-renders from
/// one round-trip (an actuation and its consequences in one payload).
async fn fresh_pack_states(app: &AppHandle) -> Result<PackStatesDto, String> {
    let downloader = pack_downloader_of(app);
    let (engine_value, language) = current_selection(app)?;
    tauri::async_runtime::spawn_blocking(move || {
        pack_states_snapshot(
            downloader.as_deref(),
            &crate::shell::active_models_dir(),
            &engine_value,
            language,
        )
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))
}

/// The Engines tab's pack states. The disk half hashes tens of MB, so it
/// runs on the blocking pool like the OCR language-pack probe above.
#[tauri::command]
pub(crate) async fn get_pack_states(app: AppHandle) -> Result<PackStatesDto, String> {
    fresh_pack_states(&app).await
}

/// Starts one pack's download — the Engines tab's Retry / Download
/// controls, and the re-selection path for a pack canceled earlier
/// (ADR-0008: nothing downloads on its own; the tab is the only surface).
/// The downloader owns the honesty: a Ready pack is never re-downloaded,
/// a second start while one runs is declined, and both land in the page's
/// snapshot rather than as errors.
#[tauri::command]
pub(crate) async fn pack_start_download(app: AppHandle, pack: String) -> Result<PackStatesDto, String> {
    let downloader = pack_downloader_of(&app).ok_or_else(|| NO_PACK_MANAGER.to_string())?;
    let id = parse_pack_id(&pack)?;
    let outcome = downloader.start(id);
    crate::logging::log(
        "packs",
        format!(
            "pack download requested from the Engines tab: {} — {outcome:?}",
            id.display_name()
        ),
    );
    fresh_pack_states(&app).await
}

/// Cancels one pack's in-flight download: the transfer aborts, the staged
/// temp dies, and the pack reports not-downloaded again (zero residue —
/// the downloader's contract). A cancel landing inside the final atomic
/// install completes as Finished instead: verified bytes are kept, never
/// discarded.
#[tauri::command]
pub(crate) async fn pack_cancel_download(app: AppHandle, pack: String) -> Result<PackStatesDto, String> {
    let downloader = pack_downloader_of(&app).ok_or_else(|| NO_PACK_MANAGER.to_string())?;
    let id = parse_pack_id(&pack)?;
    let canceled = downloader.cancel(id);
    crate::logging::log(
        "packs",
        if canceled {
            format!("pack download cancel requested: {}", id.display_name())
        } else {
            format!("pack cancel ignored — nothing in flight: {}", id.display_name())
        },
    );
    fresh_pack_states(&app).await
}

/// The cleanup button (spec story 14): removes exactly the pack files the
/// CURRENT effective selection does not require — the shared dictionary a
/// kept pack pins and the bundled Tiny baseline survive
/// ([`delete_unused_packs`]'s guards), and a kept pack's stale files stay
/// (its download refreshes them). Returns the removed file names so the
/// page can say what was reclaimed. Nothing deletes a pack automatically;
/// this explicit command is the only deletion path in the app.
#[tauri::command]
pub(crate) async fn pack_delete_unused(app: AppHandle) -> Result<Vec<String>, String> {
    let downloader = pack_downloader_of(&app).ok_or_else(|| NO_PACK_MANAGER.to_string())?;
    let (engine_value, language) = current_selection(&app).map_err(|e| format!("{e} — nothing is deleted"))?;
    let mut keep = langception_core::model_packs::required_packs_for_engine_value(&engine_value, language)
        .unwrap_or(&[])
        .to_vec();
    // A pack with a download in flight is as in-use as a required one:
    // its install would re-land whatever this command deleted, and the
    // page would report a reclaim that never happened. The Cancel button
    // is how a downloading pack becomes deletable.
    for id in PackId::ALL {
        if matches!(
            downloader.state(id),
            langception_core::pack_downloader::PackDownloadState::Downloading { .. }
        ) && !keep.contains(&id)
        {
            keep.push(id);
        }
    }
    let manifest = downloader.manifest().clone();
    let active_dir = crate::shell::active_models_dir();
    let removed = tauri::async_runtime::spawn_blocking(move || {
        delete_unused_packs(&manifest, &active_dir, &keep)
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?
    .map_err(|e| e.to_string())?;
    crate::logging::log(
        "packs",
        if removed.is_empty() {
            "delete-unused: nothing to remove".to_string()
        } else {
            format!("delete-unused removed {} file(s)", removed.len())
        },
    );
    Ok(removed)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct About {
    pub product_name: String,
    pub version: String,
    pub config_path: String,
    /// The `logs` folder beside the config (ticket 27); the About tab
    /// shows it next to the "Go to logs directory" button.
    pub logs_path: String,
    pub target_language: &'static str,
}

#[tauri::command]
pub(crate) fn get_about(app: AppHandle) -> About {
    let state = app.state::<AppState>();
    About {
        product_name: app.package_info().name.clone(),
        version: app.package_info().version.to_string(),
        config_path: state.config_path.display().to_string(),
        logs_path: crate::logging::logs_dir(&state.config_path).display().to_string(),
        target_language: TARGET_LANGUAGE,
    }
}

// ---- Engines tab (tickets 09 + 10 + 11) ----

/// Matches the config's OCR default for a page that (somehow) sends no
/// ocrEngine field — always the config-side default, never an error.
fn default_rate_limit_fallback_dto() -> String {
    "none".to_string()
}

fn default_ocr_engine_dto() -> String {
    langception_core::config::DEFAULT_OCR_ENGINE.to_string()
}

/// The Engines tab's editing surface: the global default engine, every
/// configurable engine's settings, and the reading mode (ticket 12: classic
/// OCR vs the fused vision call). Keys and URLs round-trip to the page so
/// edits prefill; the window is local-only. The LLM prompt base
/// instructions (ticket 37) and the System Prompt Preset selection (ticket
/// 42) ride here too — they live on the same tab and apply with the same
/// save.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnginesDto {
    pub default_engine: String,
    /// Which engine reads on-screen text (ticket 31): "windows" or one of
    /// the bundled PP-OCRv6 tiers "pp-ocrv6-tiny" | "pp-ocrv6-small" |
    /// "pp-ocrv6-medium" (pp-ocrv6-sidecar ticket 04; the name is the
    /// tier — there is no separate tier setting). The legacy "paddle"
    /// value still arrives from old configs and maps to small in the
    /// config→engine direction; the dropdown shows the four current
    /// options (the page maps a saved "paddle" to its tier for display).
    /// Defaults to Windows OCR (ticket 55).
    #[serde(default = "default_ocr_engine_dto")]
    pub ocr_engine: String,
    /// "ocr" | "fused" (ticket 12).
    pub reading: String,
    /// The auto-switch secondary (ADR-0006, ticket 03): "none" or an
    /// engine name — spelling and completeness validated on save.
    #[serde(default = "default_rate_limit_fallback_dto")]
    pub rate_limit_fallback: String,
    pub azure: AzureEngineDto,
    /// The generic OpenAI-compatible endpoint (ticket 10).
    pub openai: OpenAiEngineDto,
    /// LM Studio's local server (ticket 10; no key — its server needs none).
    pub lmstudio: LmStudioEngineDto,
    /// Google Gemini (ticket 11; the endpoint is Google's fixed API root).
    pub gemini: GeminiEngineDto,
    /// Custom base instruction for the per-block translation prompt
    /// (ticket 37; empty = the built-in default).
    pub translation_prompt: String,
    /// Custom base instruction for the fused screen-reading prompt
    /// (ticket 37; empty = the built-in default).
    pub screen_reading_prompt: String,
    /// The System Prompt Preset selection (ticket 42): a built-in's code, a
    /// saved preset's name, or "custom". Read back, it is the selection the
    /// config actually holds. Sent, it applies the selection together with
    /// the two texts above (which may be an edited preset's — the config
    /// then stores "custom"); absent, the prompts are left untouched — the
    /// wizard's Azure upgrade passes through that way.
    #[serde(default)]
    pub prompt_preset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AzureEngineDto {
    pub key: String,
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiEngineDto {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStudioEngineDto {
    pub base_url: String,
    pub model: String,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiEngineDto {
    pub api_key: String,
    pub model: String,
    pub temperature: f64,
}

/// Clears the Translation Cache (ADR-0006, ticket 01): the store empties
/// and its file is removed, so the next identical text re-requests. The
/// button lives in Settings → Engines, next to the engine cards.
#[tauri::command]
pub(crate) fn clear_translation_cache(app: AppHandle) {
    app.state::<AppState>().translation_cache.clear();
}

#[tauri::command]
pub(crate) fn get_engines(app: AppHandle) -> Result<EnginesDto, String> {
    match &*app.state::<AppState>().config.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(Ok(config)) => Ok(EnginesDto {
            default_engine: config.engine.clone(),
            ocr_engine: config.ocr.engine.clone(),
            reading: config.reading.code().to_string(),
            rate_limit_fallback: config.engines.rate_limit_fallback.clone(),
            azure: AzureEngineDto {
                key: config.engines.azure.key.clone(),
                region: config.engines.azure.region.clone(),
            },
            openai: OpenAiEngineDto {
                base_url: config.engines.openai.base_url.clone(),
                api_key: config.engines.openai.api_key.clone(),
                model: config.engines.openai.model.clone(),
                temperature: config.engines.openai.temperature,
            },
            lmstudio: LmStudioEngineDto {
                base_url: config.engines.lmstudio.base_url.clone(),
                model: config.engines.lmstudio.model.clone(),
                temperature: config.engines.lmstudio.temperature,
            },
            gemini: GeminiEngineDto {
                api_key: config.engines.gemini.api_key.clone(),
                model: config.engines.gemini.model.clone(),
                temperature: config.engines.gemini.temperature,
            },
            translation_prompt: config.prompts.translation.clone(),
            screen_reading_prompt: config.prompts.screen_reading.clone(),
            prompt_preset: Some(active_prompt_preset_code(config)),
        }),
        Some(Err(message)) => Err(message.clone()),
        None => Err("config not loaded yet".into()),
    }
}

/// Saves the Engines tab. On success returns the System Prompt Preset
/// selection the config holds after the save (ticket 42) — the page shows
/// that, not what it asked for: an edited preset is stored as "custom".
#[tauri::command]
pub(crate) async fn set_engines(app: AppHandle, engines: EnginesDto) -> Result<String, String> {
    let default_engine = engines.default_engine.trim().to_lowercase();
    if !langception_core::config::TRANSLATE_ENGINES.contains(&default_engine.as_str()) {
        return Err(format!(
            "unknown engine \"{default_engine}\" — engines: {}",
            langception_core::config::TRANSLATE_ENGINES.join(", ")
        ));
    }
    // The OCR engine (ticket 31): the config spelling is enforced here;
    // whether the engine actually runs is the loop's business — a broken
    // bundle is covered per-pass by the Windows-OCR fallback, and a
    // Model Pack still downloading (deferred-model-packs, ADR-0008)
    // reports loading without ever falling back. The selection saves
    // immediately either way, so the engine that starts when the pack
    // lands is exactly the one saved (spec story 19).
    let ocr_engine = engines.ocr_engine.trim().to_lowercase();
    if !langception_core::config::OCR_ENGINES.contains(&ocr_engine.as_str()) {
        return Err(format!(
            "unknown OCR engine \"{ocr_engine}\" — engines: {}",
            langception_core::config::OCR_ENGINES.join(", ")
        ));
    }
    let azure_key = engines.azure.key.trim().to_string();
    let azure_region = engines.azure.region.trim().to_string();
    // The reading mode (ticket 12): classic OCR or the fused vision call.
    // Whether the chosen engine can actually serve fused reads is the
    // loop start's check (a Profile's override can change the engine);
    // here only the config spelling is enforced.
    let reading = match engines.reading.as_str() {
        "ocr" => langception_core::config::ReadingMode::Ocr,
        "fused" => langception_core::config::ReadingMode::Fused,
        other => {
            return Err(format!(
                "unknown reading mode \"{other}\" — modes: ocr, fused"
            ))
        }
    };
    // A keyed default without its key would fail the loop at start; catch
    // it here, where the user is looking. Keys can still be saved for a
    // later switch (or a Profile's override) while the default stays keyless.
    if default_engine == "azure" && azure_key.is_empty() {
        return Err("Azure needs an API key — create one with the link above, then paste it".into());
    }
    // Same rule for the LLM engines (ticket 10): completeness is enforced
    // only for the SELECTED default — the other engine's fields may be
    // half-filled and saved for later (a Profile may pick them).
    let openai_base = engines.openai.base_url.trim().to_string();
    let openai_key = engines.openai.api_key.trim().to_string();
    let openai_model = engines.openai.model.trim().to_string();
    let openai_temperature = engines.openai.temperature;
    let lmstudio_base = engines.lmstudio.base_url.trim().to_string();
    let lmstudio_model = engines.lmstudio.model.trim().to_string();
    let lmstudio_temperature = engines.lmstudio.temperature;
    let gemini_key = engines.gemini.api_key.trim().to_string();
    let gemini_model = engines.gemini.model.trim().to_string();
    let gemini_temperature = engines.gemini.temperature;
    // The LLM prompts (tickets 37 + 42): free text, no shape to validate
    // here. With a preset selection the config's own rule decides what the
    // texts amount to (it trims them, and stores "custom" when they are not
    // the preset's verbatim wording); without one the prompts are not
    // touched at all.
    let translation_prompt = engines.translation_prompt;
    let screen_reading_prompt = engines.screen_reading_prompt;
    let prompt_preset = engines.prompt_preset;
    for (label, temperature) in [
        ("OpenAI-compatible", openai_temperature),
        ("LM Studio", lmstudio_temperature),
        ("Gemini", gemini_temperature),
    ] {
        if !(0.0..=2.0).contains(&temperature) {
            return Err(format!("{label} temperature must be between 0 and 2"));
        }
    }
    if default_engine == "openai" {
        if openai_base.is_empty() {
            return Err("OpenAI-compatible engine needs a base URL — add one above".into());
        }
        if openai_model.is_empty() {
            return Err("OpenAI-compatible engine needs a model — type one above".into());
        }
    }
    if default_engine == "lmstudio" {
        if lmstudio_base.is_empty() {
            return Err("LM Studio engine needs a server URL — add one above".into());
        }
        if lmstudio_model.is_empty() {
            return Err(
                "LM Studio engine needs a model — check the server above and pick one from the list"
                    .into(),
            );
        }
    }
    if default_engine == "gemini" {
        if gemini_key.is_empty() {
            return Err(
                "Gemini needs an API key — create one with the link above, then paste it".into(),
            );
        }
        if gemini_model.is_empty() {
            return Err("Gemini engine needs a model — type one above".into());
        }
    }
    // The auto-switch secondary (ADR-0006, ticket 03): spelling here, and
    // its completeness too — a fallback without its key would fail the
    // loop at start, so catch it where the user is looking. It engages
    // only while the Google engine is the one cooling down, so "google"
    // itself is not a valid fallback.
    let rate_limit_fallback = engines.rate_limit_fallback.trim().to_lowercase();
    if rate_limit_fallback != "none" {
        if rate_limit_fallback == "google" {
            return Err("The auto-switch fallback cannot be Google — Google is the engine that cools down; pick azure, openai, lmstudio, or gemini".into());
        }
        if !langception_core::config::TRANSLATE_ENGINES.contains(&rate_limit_fallback.as_str()) {
            return Err(format!(
                "unknown auto-switch fallback \"{rate_limit_fallback}\" — \"none\", or: {}",
                langception_core::config::TRANSLATE_ENGINES.join(", ")
            ));
        }
        if rate_limit_fallback == "azure" && azure_key.is_empty() {
            return Err("The auto-switch fallback is Azure, but no API key is set — add one in the Azure card above, or set the fallback to never switch".into());
        }
        if rate_limit_fallback == "openai" && (openai_base.is_empty() || openai_model.is_empty()) {
            return Err("The auto-switch fallback is OpenAI-compatible, but its base URL or model is missing — complete the card above, or set the fallback to never switch".into());
        }
        if rate_limit_fallback == "lmstudio" && (lmstudio_base.is_empty() || lmstudio_model.is_empty()) {
            return Err("The auto-switch fallback is LM Studio, but its server URL or model is missing — complete the card above, or set the fallback to never switch".into());
        }
        if rate_limit_fallback == "gemini" && (gemini_key.is_empty() || gemini_model.is_empty()) {
            return Err("The auto-switch fallback is Gemini, but its API key or model is missing — complete the card above, or set the fallback to never switch".into());
        }
    }
    run_off_main(app, move |app| {
        let active_preset = with_config_mut(app, |config| {
            config.engine = default_engine.clone();
            // The OCR engine is read when the loop starts; the restart
            // below applies a switch live (ticket 31) — no app restart.
            config.ocr.engine = ocr_engine.clone();
            config.reading = reading;
            config.engines.rate_limit_fallback = rate_limit_fallback.clone();
            config.engines.azure.key = azure_key.clone();
            config.engines.azure.region = azure_region.clone();
            config.engines.openai.base_url = openai_base.clone();
            config.engines.openai.api_key = openai_key.clone();
            config.engines.openai.model = openai_model.clone();
            config.engines.openai.temperature = openai_temperature;
            config.engines.lmstudio.base_url = lmstudio_base.clone();
            config.engines.lmstudio.model = lmstudio_model.clone();
            config.engines.lmstudio.temperature = lmstudio_temperature;
            config.engines.gemini.api_key = gemini_key.clone();
            config.engines.gemini.model = gemini_model.clone();
            config.engines.gemini.temperature = gemini_temperature;
            if let Some(name) = &prompt_preset {
                config.select_prompt_preset(
                    name,
                    PromptTexts {
                        translation: translation_prompt.clone(),
                        screen_reading: screen_reading_prompt.clone(),
                    },
                )?;
            }
            // The selection the config holds once the texts are stored.
            Ok(active_prompt_preset_code(config))
        })?;
        // The engine, its endpoint settings, and the prompts are read when
        // the loop starts: restart so the change visibly applies.
        restart_loop_if_running(app);
        // The tier the selection just saved may need packs that are not on
        // disk (deferred-model-packs ticket 04, ADR-0008): the download is
        // a consequence of the selection, started the moment it saves —
        // the Engines tab reports progress through the pack events, and
        // the loop's passes report loading (ticket 02) meanwhile.
        crate::shell::start_required_pack_downloads(app);
        apply_state(app);
        Ok(active_preset)
    })
    .await
}

/// The result of the LM Studio card's server check (ticket 10): one
/// `GET /models` round-trip answers reachability AND model discovery.
/// Failures come back as `reachable: false` with a plain-language message
/// (connection refused names the URL) so the card can show an inline
/// status instead of a toast.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LmStudioCheck {
    pub reachable: bool,
    pub models: Vec<String>,
    pub message: String,
}

/// Live check for the LM Studio card: pings the server URL as typed (the
/// saved value is NOT required — the check validates the field before
/// saving) and lists the models it serves. Pure HTTP, no state touched.
#[tauri::command]
pub(crate) async fn check_lmstudio(base_url: String) -> Result<LmStudioCheck, String> {
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(
            "Enter the LM Studio server URL first (default: http://localhost:1234/v1)".into(),
        );
    }
    let engine = langception_core::openai::OpenAiCompatibleEngine::new(
        &base_url,
        "",
        "unused",
        0.0,
        langception_core::openai::LM_STUDIO_LABEL,
    )
    .map_err(|e| e.message)?;
    match engine.check_endpoint().await {
        Ok(models) => {
            let message = if models.is_empty() {
                "Server reachable, but it serves no models — load one in LM Studio (Developer tab), then check again"
                    .to_string()
            } else {
                format!(
                    "Server reachable — {count} model{s} available",
                    count = models.len(),
                    s = if models.len() == 1 { "" } else { "s" }
                )
            };
            Ok(LmStudioCheck {
                reachable: true,
                models,
                message,
            })
        }
        Err(error) => Ok(LmStudioCheck {
            reachable: false,
            models: Vec::new(),
            message: error.message,
        }),
    }
}

/// Live check for the Engines tab: translates one phrase through the named
/// engine (empty = the current default) with the keys as-saved. The Thai
/// result comes back on success; quota/key/network failures come back as
/// the engine's classified error, shown in the page's toast.
#[tauri::command]
pub(crate) async fn test_engine(app: AppHandle, engine: String) -> Result<String, String> {
    let config = current_config(&app)?;
    let requested = engine.trim().to_lowercase();
    let chosen = if requested.is_empty() {
        config.engine.clone()
    } else {
        requested
    };
    // A transient store: the live check is verification, not translation —
    // its sample phrase must never persist into the user's cache
    // (the same neutrality the fused build-time validation keeps).
    let translate = crate::shell::build_translate_engine(
        &config,
        Some(&chosen),
        &langception_core::translate_cache::TranslationCache::transient(),
        None,
    )?;
    translate
        .translate("Hello", SourceLanguage::En, TARGET_LANGUAGE)
        .await
        .map_err(|e| e.message)
}

/// The Engines tab's and wizard's key-creation links, plus the About tab's
/// developer identity links (site / support / email). Whitelisted — a
/// webview window must never become a general URL opener.
#[tauri::command]
pub(crate) fn open_engine_link(url: String) -> Result<(), String> {
    const ALLOWED: [&str; 5] = [
        "https://portal.azure.com",
        "https://aistudio.google.com/apikey",
        // The developer's own pages (About tab).
        "https://intception.app",
        "https://intception.app/donate",
        "mailto:arm@intception.app",
    ];
    if !ALLOWED.contains(&url.as_str()) {
        return Err(format!("link \"{url}\" is not one of the whitelisted pages"));
    }
    if url.to_ascii_lowercase().starts_with("mailto:") {
        // Protocol dispatch to the default mail client: `start` hits the
        // registered handler; explorer only resolves URLs itself.
        std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn()
            .map_err(|e| format!("cannot open the link: {e}"))?;
        return Ok(());
    }
    std::process::Command::new("explorer")
        .arg(url)
        .spawn()
        .map_err(|e| format!("cannot open the link: {e}"))?;
    Ok(())
}

// ---- System Prompt Presets (ticket 42) ----

/// The selection code the config holds — what `set_engines` and the preset
/// commands return, so the page shows reality, not what it asked for: an
/// edited preset is stored as "custom", a deleted selected one as
/// "general".
fn active_prompt_preset_code(config: &AppConfig) -> String {
    config.active_prompt_preset().code().to_string()
}

/// One System Prompt Preset as the Engines tab lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptPresetDto {
    /// The selector's value: a built-in's code or a saved preset's name.
    pub name: String,
    /// The selector's label: a built-in's display name ("Game"), or a
    /// saved preset's name as typed.
    pub label: String,
    /// Built-ins cannot be renamed, deleted, or overwritten by save-as.
    pub builtin: bool,
    pub translation: String,
    pub screen_reading: String,
}

/// Every System Prompt Preset, in selector order: the four built-ins, then
/// the saved ones in config order. The texts let the page show a preset's
/// wording the moment it is picked, before anything is saved.
#[tauri::command]
pub(crate) fn list_prompt_presets(app: AppHandle) -> Result<Vec<PromptPresetDto>, String> {
    let config = current_config(&app)?;
    let mut presets: Vec<PromptPresetDto> = BuiltinPreset::ALL
        .iter()
        .map(|builtin| {
            let texts = builtin.texts();
            PromptPresetDto {
                name: builtin.code().to_string(),
                label: builtin.name().to_string(),
                builtin: true,
                translation: texts.translation,
                screen_reading: texts.screen_reading,
            }
        })
        .collect();
    presets.extend(config.prompt_presets.iter().map(|preset| PromptPresetDto {
        label: preset.name.clone(),
        name: preset.name.clone(),
        builtin: false,
        translation: preset.translation.clone(),
        screen_reading: preset.screen_reading.clone(),
    }));
    Ok(presets)
}

/// One pair of base instructions as the Engines tab's prompt fields show it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTextsDto {
    pub translation: String,
    pub screen_reading: String,
}

/// The built-in default's two base instructions (ticket 68) — exactly what a
/// blank pair of prompt fields (the General preset) sends. The page shows
/// this wording in the fields' placeholders: General's stored texts are
/// empty by design (blank IS the default), so without this an empty field
/// explains itself only with "leave empty to use the built-in instruction".
/// Placeholders, never values — loading the wording into a field and saving
/// would store it as an override and flip the selection to custom.
#[tauri::command]
pub(crate) fn default_prompt_texts() -> PromptTextsDto {
    let texts = langception_core::prompt::default_texts();
    PromptTextsDto {
        translation: texts.translation,
        screen_reading: texts.screen_reading,
    }
}

/// The prompt inputs a running loop sends (ticket 43), for deciding
/// whether a preset edit needs a restart: the active Profile's binding —
/// the Profile the loop is currently translating under — resolved against
/// the config, or the global texts and selection when no Profile matches
/// or it inherits. So renaming or deleting a preset that only a bound
/// (and active) Profile uses also counts as a change, while edits that
/// touch nothing in force skip the restart (which would reset tracking
/// and re-translate whatever is on screen).
fn prompts_in_force(app: &AppHandle, config: &AppConfig) -> EffectivePrompts {
    let active_id = app
        .state::<AppState>()
        .active_profile
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let binding = config
        .profiles
        .iter()
        .find(|profile| Some(&profile.id) == active_id.as_ref())
        .and_then(|profile| profile.prompt_preset.clone());
    config.effective_prompt_inputs(binding.as_deref())
}

/// Runs one System Prompt Preset edit against the config and applies it:
/// returns the selection the config holds afterwards. The loop restarts
/// only when the prompts in force changed.
fn edit_prompt_presets(
    app: &AppHandle,
    edit: impl FnOnce(&mut AppConfig) -> Result<ActivePromptPreset, String>,
) -> Result<String, String> {
    let (changed, active) = with_config_mut(app, |config| {
        let before = prompts_in_force(app, config);
        edit(config)?;
        Ok((
            before != prompts_in_force(app, config),
            active_prompt_preset_code(config),
        ))
    })?;
    if changed {
        restart_loop_if_running(app);
    }
    apply_state(app);
    Ok(active)
}

/// "Save as preset": stores the two prompts as displayed under `name` and
/// selects it. A saved preset of that name is replaced only with
/// `overwrite` — the page asks first; a built-in's name is never taken.
#[tauri::command]
pub(crate) async fn save_prompt_preset(
    app: AppHandle,
    name: String,
    translation: String,
    screen_reading: String,
    overwrite: bool,
) -> Result<String, String> {
    run_off_main(app, move |app| {
        edit_prompt_presets(app, |config| {
            config.save_prompt_preset(
                &name,
                PromptTexts {
                    translation,
                    screen_reading,
                },
                overwrite,
            )
        })
    })
    .await
}

/// Renames a saved preset; the selection follows it.
#[tauri::command]
pub(crate) async fn rename_prompt_preset(
    app: AppHandle,
    name: String,
    new_name: String,
) -> Result<String, String> {
    run_off_main(app, move |app| {
        edit_prompt_presets(app, |config| config.rename_prompt_preset(&name, &new_name))
    })
    .await
}

/// Deletes a saved preset. Deleting the selected one returns the selection
/// to General and clears both prompts — the returned "general" tells the
/// page to clear its fields.
#[tauri::command]
pub(crate) async fn delete_prompt_preset(app: AppHandle, name: String) -> Result<String, String> {
    run_off_main(app, move |app| {
        edit_prompt_presets(app, |config| config.delete_prompt_preset(&name))
    })
    .await
}

// ---- General tab ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettingsDto {
    pub language: String,
    /// The UI Language (ticket 62): "en" | "th" — which language the
    /// app's own windows and tray render in. Saved with the tab like every
    /// General field; the Settings window applies it live on save, the
    /// other windows at their next load, the tray at the next state
    /// refresh (the menu is rebuilt per apply_state).
    pub ui_language: String,
    pub cadence_ms: u32,
    pub stability_ms: u32,
    /// Translation Context (ticket 44): how many of each Capture Zone's
    /// recent translations ride into the LLM prompts (0 = off).
    pub context_translations: u32,
    /// The Genderless Mode global toggle (ticket 08).
    pub genderless: bool,
    /// Which ordinary window is pinned above other apps (ticket 34) —
    /// applied to the live windows the moment this saves.
    pub results_on_top: bool,
    pub settings_on_top: bool,
    /// Capture Visibility (ticket 46, "Show Langception in screen
    /// captures") — applied to the live windows the moment this saves.
    pub capture_visible: bool,
    /// Start-with-Windows — OS-managed (the Run registry key on Windows,
    /// not config.toml), but saved through this command like every other
    /// General field so the tab has one save path.
    pub start_with_windows: bool,
}

#[tauri::command]
pub(crate) async fn set_general(
    app: AppHandle,
    settings: GeneralSettingsDto,
) -> Result<(), String> {
    let language = match settings.language.as_str() {
        "en" => SourceLanguage::En,
        "ja" => SourceLanguage::Ja,
        "zh" => SourceLanguage::Zh,
        "ko" => SourceLanguage::Ko,
        other => return Err(format!("unknown source language \"{other}\"")),
    };
    // The UI Language (ticket 62): the same code()/from_code() pair the
    // sibling enums use, so the accepted spellings are the config enum's.
    let ui_language =
        langception_core::config::UiLanguage::from_code(settings.ui_language.as_str())
            .ok_or_else(|| format!("unknown UI language \"{}\"", settings.ui_language))?;
    // Translation Context (ticket 44): same cap the config validation
    // enforces, checked here so the user is looking at the field.
    let context_translations = settings.context_translations;
    if context_translations > langception_core::config::MAX_CONTEXT_TRANSLATIONS {
        return Err(format!(
            "Translation context must be between 0 and {}",
            langception_core::config::MAX_CONTEXT_TRANSLATIONS
        ));
    }
    // Copies for the restart decision below (the originals move into the
    // config edit): only the loop-start fields need a restart to apply —
    // the context window applies live through its watch channel.
    let restart_language = language;
    let restart_cadence = settings.cadence_ms;
    let restart_stability = settings.stability_ms;
    let restart_genderless = settings.genderless;
    run_off_main(app, move |app| {
        // Start-with-Windows is OS-managed (the Run registry key), not
        // config.toml — applied here so it saves with the rest of the tab.
        // It runs BEFORE the config write: a failed registration must not
        // leave the config saved while the form reverts (ticket 51's
        // "fields never lie"). A no-op value skips the registry write.
        let manager = app.autolaunch();
        let current = manager.is_enabled().unwrap_or(false);
        if current != settings.start_with_windows {
            if settings.start_with_windows {
                manager
                    .enable()
                    .map_err(|e| format!("cannot enable start-with-Windows: {e}"))?;
            } else {
                manager
                    .disable()
                    .map_err(|e| format!("cannot disable start-with-Windows: {e}"))?;
            }
        }
        let before = current_config(app)?;
        with_config_mut(app, |config| {
            config.source.language = language;
            config.ui.ui_language = ui_language;
            config.capture.cadence_ms = settings.cadence_ms;
            config.capture.stability_ms = settings.stability_ms;
            config.context_translations = settings.context_translations;
            config.genderless = settings.genderless;
            config.windows.results_on_top = settings.results_on_top;
            config.windows.settings_on_top = settings.settings_on_top;
            config.windows.capture_visible = settings.capture_visible;
            Ok(())
        })?;
        // The on-top flags apply to the live windows immediately (ticket
        // 34) — no relaunch, no reopen; a hidden window takes the flag
        // for its next show. Capture Visibility applies the same way
        // (ticket 46): the exclusion flip is instant, and the mask
        // provider reads the flag per capture, so the very next frame
        // follows without a loop restart.
        apply_always_on_top_from_config(app);
        apply_capture_visibility_from_config(app);
        // Translation Context (ticket 44) applies live: the running loop
        // reads the window per pass, so the next translation uses it —
        // with a 0 the buffer empties and the prompts return to today's
        // shape, all without a restart.
        let state = app.state::<AppState>();
        let _ = state.context_tx.send(context_translations);
        // Language/cadence/stability/genderless are read when the loop
        // starts: restart only when one of them actually changed (a
        // language change must reset tracking anyway — the OCR engine is
        // rebuilt), so a context-only edit never resets per-block state.
        let restart_worthy = before.source.language != restart_language
            || before.capture.cadence_ms != restart_cadence
            || before.capture.stability_ms != restart_stability
            || before.genderless != restart_genderless;
        if restart_worthy {
            restart_loop_if_running(app);
        }
        // A language change moves the EFFECTIVE selection (Korean pulls
        // the recognizer set, Japanese+tiny pulls Small — deferred-model-
        // packs ticket 04): the downloads are a consequence of that
        // selection, so they start here too — gated on the change so an
        // unrelated cadence edit never re-hashes every pack.
        if before.source.language != restart_language {
            crate::shell::start_required_pack_downloads(app);
        }
        apply_state(app);
        Ok(())
    })
    .await
}

// ---- Update Check (ticket 28, CONTEXT.md) ----

/// The About tab's "Check for new version" (ticket 28): fetches the
/// maintainer's app list from the configured URL, finds Langception's
/// entry, and compares it against the running version. Every verdict —
/// including every failure — comes back as an [`UpdateCheckOutcome`] the
/// page reports gently; the command itself only errors when the config is
/// broken. Pure HTTP on the async runtime (nothing here touches windows or
/// the main thread), check-only by construction: no download, no install.
#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> Result<UpdateCheckOutcome, String> {
    let config = current_config(&app)?;
    let current = app.package_info().version.to_string();
    let outcome =
        langception_core::update::check_for_updates(&config.update.url, &current).await;
    record_update_check(&app, outcome.clone());
    Ok(outcome)
}

/// Remembers the last completed check (manual or launch-time) so the
/// Settings page can show it on open — a launch check that finished while
/// the window was closed still surfaces — and pushes it to a live settings
/// window through the same event the page listens for.
pub(crate) fn record_update_check(app: &AppHandle, outcome: UpdateCheckOutcome) {
    *app.state::<AppState>()
        .last_update_check
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(outcome.clone());
    // No settings window open = nothing listening; the stored copy serves
    // the next open instead.
    let _ = app.emit_to(SETTINGS_LABEL, "update-check-result", outcome);
}

/// The launch-time auto-check (ticket 28): spawned by the shell's setup
/// after the config loads, so it never blocks startup. Owns the whole
/// enablement decision — a disabled toggle or an empty URL (the feature
/// dormant) skips silently, and failures stay silent too: they land in
/// the recorded outcome and show wherever the Settings page shows checks,
/// never as a toast or warning.
pub(crate) fn spawn_launch_update_check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (url, check_on_launch) = match &*app
            .state::<AppState>()
            .config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            Some(Ok(config)) => (config.update.url.clone(), config.update.check_on_launch),
            _ => return,
        };
        if !check_on_launch || url.trim().is_empty() {
            return;
        }
        let current = app.package_info().version.to_string();
        let outcome = langception_core::update::check_for_updates(&url, &current).await;
        record_update_check(&app, outcome);
    });
}

/// The About tab's "check on launch" toggle (ticket 28): persists
/// immediately, like Start-with-Windows, so the checkbox never lies about
/// what the next launch will do.
#[tauri::command]
pub(crate) async fn set_update_check_on_launch(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        with_config_mut(app, |config| {
            config.update.check_on_launch = enabled;
            Ok(())
        })
    })
    .await
}

/// The new-version result's "Open download page" action (ticket 28): opens
/// the payload's URL in the system browser on the user's click — the app
/// itself never fetches it. A webview window must never become a general
/// URL opener (the open_engine_link rule): HTTPS passes; plain HTTP only
/// for a loopback host, so a dev/mock endpoint's link still opens.
#[tauri::command]
pub(crate) fn open_update_page(url: String) -> Result<(), String> {
    let url = url.trim();
    let openable = url.to_ascii_lowercase().starts_with("https://")
        || langception_core::openai::is_local_url(url);
    if !openable {
        return Err(format!("\"{url}\" is not an HTTPS download page — not opening it"));
    }
    std::process::Command::new("explorer")
        .arg(url)
        .spawn()
        .map_err(|e| format!("cannot open the link: {e}"))?;
    Ok(())
}

// ---- Hotkeys tab (rebinds apply immediately) ----

#[tauri::command]
pub(crate) async fn set_hotkeys(app: AppHandle, hotkeys: HotkeyStatus) -> Result<(), String> {
    run_off_main(app, move |app| {
        let settings = HotkeySettings {
            toggle_translation: hotkeys.toggle.trim().to_string(),
            new_zone: hotkeys.new_zone.trim().to_string(),
            pause_translation: hotkeys.pause.trim().to_string(),
            adjust_mode: hotkeys.adjust.trim().to_string(),
            one_shot_capture: hotkeys.one_shot.trim().to_string(),
        };
        // Every field must be non-empty, parseable, and unambiguous — a
        // rejected edit never touches config.toml.
        let mut seen = std::collections::HashMap::new();
        for (action, accel) in [
            ("toggle", &settings.toggle_translation),
            ("new zone", &settings.new_zone),
            ("pause", &settings.pause_translation),
            ("adjust mode", &settings.adjust_mode),
            ("one-shot", &settings.one_shot_capture),
        ] {
            if accel.is_empty() {
                return Err(format!("{action} hotkey must not be empty"));
            }
            if let Some(other) = seen.get(accel.as_str()) {
                return Err(format!(
                    "\"{accel}\" is bound to both {other} and {action} — pick different keys"
                ));
            }
            seen.insert(accel.as_str(), action);
            accel
                .parse::<Shortcut>()
                .map_err(|e| format!("\"{accel}\" ({action}) is not a valid hotkey: {e}"))?;
        }

        with_config_mut(app, |config| {
            config.hotkeys = settings.clone();
            Ok(())
        })?;
        // Re-register (drops the previous bindings first). A hotkey that is
        // taken by another app fails at the OS level here: a visible
        // warning, and the other hotkeys still work.
        let warnings = crate::shell::register_hotkeys(app, &settings);
        let state = app.state::<AppState>();
        for warning in warnings {
            push_warning(&state, warning);
        }
        apply_state(app);
        Ok(())
    })
    .await
}

// ---- Glossary tab ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntryDto {
    pub term: String,
    pub thai: String,
}

#[tauri::command]
pub(crate) fn list_glossary(app: AppHandle) -> Result<Vec<GlossaryEntryDto>, String> {
    match &*app.state::<AppState>().config.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(Ok(config)) => Ok(config
            .glossary
            .0
            .iter()
            .map(|(term, thai)| GlossaryEntryDto {
                term: term.clone(),
                thai: thai.clone(),
            })
            .collect()),
        Some(Err(message)) => Err(message.clone()),
        None => Err("config not loaded yet".into()),
    }
}

#[tauri::command]
pub(crate) async fn add_glossary_entry(
    app: AppHandle,
    term: String,
    thai: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        let (term, thai) = validate_glossary_pair(&term, &thai)?;
        // Case-insensitive pre-check: matching ignores case, so two entries
        // that differ only by case would fight over the same blocks.
        if current_config(app)?.glossary.has_term_ci(&term) {
            return Err(format!(
                "A term matching \"{term}\" already exists (case-insensitive)"
            ));
        }
        with_config_mut(app, |config| {
            config.glossary.upsert(&term, &thai);
            Ok(())
        })?;
        apply_state(app);
        restart_loop_if_running(app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn update_glossary_entry(
    app: AppHandle,
    term: String,
    new_term: String,
    thai: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        let (new_term, thai) = validate_glossary_pair(&new_term, &thai)?;
        let old_term = term.trim().to_string();
        with_config_mut(app, |config| {
            if !config.glossary.0.contains_key(&old_term) {
                return Err("term not found (was it changed elsewhere?)".to_string());
            }
            // Renaming to a case-variant of the same term is fine (the key
            // gets rewritten); renaming onto a DIFFERENT existing term is not.
            if !new_term.eq_ignore_ascii_case(&old_term) && config.glossary.has_term_ci(&new_term) {
                return Err(format!(
                    "A term matching \"{new_term}\" already exists (case-insensitive)"
                ));
            }
            config.glossary.remove(&old_term);
            config.glossary.upsert(&new_term, &thai);
            Ok(())
        })?;
        apply_state(app);
        restart_loop_if_running(app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_glossary_entry(app: AppHandle, term: String) -> Result<(), String> {
    run_off_main(app, move |app| {
        with_config_mut(app, |config| {
            if !config.glossary.remove(term.trim()) {
                return Err("term not found (was it deleted elsewhere?)".to_string());
            }
            Ok(())
        })?;
        apply_state(app);
        restart_loop_if_running(app);
        Ok(())
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    pub total: usize,
}

/// Imports a glossary JSON file (the format [`Glossary::to_json_string`]
/// writes): `mode` "merge" keeps existing entries (import wins on
/// conflicts), "replace" empties the glossary first.
#[tauri::command]
pub(crate) async fn import_glossary(
    app: AppHandle,
    path: String,
    mode: String,
) -> Result<ImportReport, String> {
    run_off_main(app, move |app| {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let imported = Glossary::parse_json_str(&text)?;
        let stats = with_config_mut(app, |config| {
            match mode.as_str() {
                "merge" => {}
                "replace" => config.glossary.clear(),
                other => return Err(format!("unknown import mode \"{other}\"")),
            }
            Ok(config.glossary.merge(&imported))
        })?;
        apply_state(app);
        restart_loop_if_running(app);
        Ok(ImportReport {
            added: stats.added,
            updated: stats.updated,
            total: stats.added + stats.updated,
        })
    })
    .await
}

/// Writes the glossary as JSON to the path the save dialog picked. Returns
/// the number of exported entries.
#[tauri::command]
pub(crate) async fn export_glossary(app: AppHandle, path: String) -> Result<usize, String> {
    run_off_main(app, move |app| {
        let config = current_config(app)?;
        let json = config.glossary.to_json_string();
        std::fs::write(&path, json).map_err(|e| format!("cannot write {path}: {e}"))?;
        Ok(config.glossary.0.len())
    })
    .await
}

/// Shared add/update validation: both sides trimmed, both non-empty.
fn validate_glossary_pair(term: &str, thai: &str) -> Result<(String, String), String> {
    let term = term.trim().to_string();
    let thai = thai.trim().to_string();
    if term.is_empty() {
        return Err("Term must not be empty".into());
    }
    if thai.is_empty() {
        return Err("Thai translation must not be empty".into());
    }
    Ok((term, thai))
}

// ---- Profiles tab (ticket 07) ----

/// A Profile as the Settings → Profiles tab edits it. The id, Glossary
/// layer and Capture Zones are managed by their own commands, not this DTO.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub name: String,
    pub exe: String,
    pub language: String,
    /// The Profile's engine choice; `None`/empty = inherit the global one.
    pub engine: Option<String>,
    /// "inherit" | "on" | "off".
    pub genderless: String,
    /// The Profile's System Prompt Preset binding (ticket 43); `None`/empty
    /// = inherit the global selection.
    #[serde(default)]
    pub prompt_preset: Option<String>,
}

/// Runs one edit against one stored Profile. Keeps id/glossary/zones
/// unless the edit says otherwise; the serialized config must still parse
/// and validate (with_config_mut's rule), so a rejected edit lands nowhere.
/// Ticket 40: also the shell's Adjust Mode drop path (adjusting a Profile
/// zone persists through here, so saving stays single-shaped).
pub(crate) fn with_profile_mut<T>(
    app: &AppHandle,
    profile_id: &str,
    edit: impl FnOnce(&mut Profile) -> Result<T, String>,
) -> Result<T, String> {
    with_config_mut(app, |config| {
        let profile = config
            .profiles
            .iter_mut()
            .find(|p| p.id == profile_id)
            .ok_or_else(|| "profile not found (was it deleted elsewhere?)".to_string())?;
        edit(profile)
    })
}

// pub(crate): the Results window's set_source_language (ticket 61, the
// titlebar language dropdown) shares the General tab's accepted spellings
// and error wording through this one match.
pub(crate) fn parse_language(code: &str) -> Result<SourceLanguage, String> {
    match code {
        "en" => Ok(SourceLanguage::En),
        "ja" => Ok(SourceLanguage::Ja),
        "zh" => Ok(SourceLanguage::Zh),
        "ko" => Ok(SourceLanguage::Ko),
        other => Err(format!("unknown source language \"{other}\"")),
    }
}

fn parse_genderless(text: &str) -> Result<GenderlessOverride, String> {
    match text {
        "inherit" => Ok(GenderlessOverride::Inherit),
        "on" => Ok(GenderlessOverride::On),
        "off" => Ok(GenderlessOverride::Off),
        other => Err(format!("unknown genderless override \"{other}\"")),
    }
}

fn parse_engine(engine: Option<&str>) -> Result<Option<String>, String> {
    let engine = engine
        .map(str::trim)
        .filter(|engine| !engine.is_empty())
        .map(str::to_string);
    // Unknown names are rejected by the config validation on commit; the
    // message there lists the accepted engines.
    if let Some(engine) = &engine {
        if !langception_core::config::TRANSLATE_ENGINES.contains(&engine.to_lowercase().as_str()) {
            return Err(format!(
                "unknown engine \"{engine}\" — engines: {}",
                langception_core::config::TRANSLATE_ENGINES.join(", ")
            ));
        }
    }
    Ok(engine)
}

/// The Profile's System Prompt Preset binding (ticket 43): trimmed, empty
/// = `None` (inherit the global selection). Unlike the engine list, the
/// accepted names depend on the config (the saved presets), so the check
/// runs against it — with the same clear error the config's own validation
/// gives, minus the misleading "config.toml is invalid" framing.
fn parse_prompt_preset(value: Option<&str>, config: &AppConfig) -> Result<Option<String>, String> {
    let preset = value.map(str::trim).filter(|preset| !preset.is_empty());
    if let Some(preset) = &preset {
        if config.prompt_preset_texts(preset).is_none() {
            return Err(format!(
                "unknown prompt preset \"{preset}\" — presets: {} (or choose inherit)",
                langception_core::prompt_preset::accepted_preset_names(config).join(", ")
            ));
        }
    }
    Ok(preset.map(str::to_string))
}

fn parse_profile_dto(
    dto: ProfileDto,
) -> Result<
    (
        String,
        String,
        SourceLanguage,
        Option<String>,
        GenderlessOverride,
    ),
    String,
> {
    let name = dto.name.trim().to_string();
    if name.is_empty() {
        return Err("Profile name must not be empty".into());
    }
    let exe = dto.exe.trim().to_string();
    if exe.is_empty() {
        return Err("Profile must target an executable (e.g. \"game.exe\")".into());
    }
    let language = parse_language(&dto.language)?;
    let engine = parse_engine(dto.engine.as_deref())?;
    let genderless = parse_genderless(&dto.genderless)?;
    Ok((name, exe, language, engine, genderless))
}

/// The app picker's options: every visible top-level window grouped by
/// executable, so a Profile can bind an app that is actually running.
/// Langception's own windows are excluded (its windows are never
/// translation sources, per ADR-0003).
#[tauri::command]
pub(crate) async fn list_running_apps() -> Result<Vec<crate::foreground::RunningApp>, String> {
    tauri::async_runtime::spawn_blocking(crate::foreground::running_apps)
        .await
        .map_err(|e| format!("background task failed: {e}"))
}

/// Installed font family names for the Text Style selector (ticket 15).
/// DirectWrite enumeration is a COM call, so it stays off the IPC thread.
#[tauri::command]
pub(crate) async fn list_fonts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(crate::fonts::font_families)
        .await
        .map_err(|e| format!("background task failed: {e}"))
}

#[tauri::command]
pub(crate) async fn create_profile(app: AppHandle, profile: ProfileDto) -> Result<String, String> {
    let prompt_preset = profile.prompt_preset.clone();
    let (name, exe, language, engine, genderless) = parse_profile_dto(profile)?;
    run_off_main(app, move |app| {
        let id = format!(
            "p{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        with_config_mut(app, |config| {
            // The binding resolves against the config being edited (the
            // saved presets live there).
            let prompt_preset = parse_prompt_preset(prompt_preset.as_deref(), config)?;
            config.profiles.push(Profile {
                id: id.clone(),
                name,
                exe,
                language,
                engine,
                genderless,
                prompt_preset,
                glossary: Glossary::default(),
                zones: Vec::new(),
            });
            Ok(())
        })?;
        apply_state(app);
        Ok(id)
    })
    .await
}

#[tauri::command]
pub(crate) async fn update_profile(
    app: AppHandle,
    id: String,
    profile: ProfileDto,
) -> Result<(), String> {
    let raw_preset = profile.prompt_preset.clone();
    let (name, exe, language, engine, genderless) = parse_profile_dto(profile)?;
    run_off_main(app, move |app| {
        // The binding resolves against the current config (the saved
        // presets live there); the edit itself cannot change that set.
        let prompt_preset = parse_prompt_preset(raw_preset.as_deref(), &current_config(app)?)?;
        with_profile_mut(app, &id, |profile| {
            profile.name = name;
            profile.exe = exe;
            profile.language = language;
            profile.engine = engine;
            profile.genderless = genderless;
            profile.prompt_preset = prompt_preset;
            Ok(())
        })?;
        // While the edited profile is active the loop restarts so the new
        // language/glossary/engine/prompt preset apply at once.
        profile_edited(app, &id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_profile(app: AppHandle, id: String) -> Result<(), String> {
    run_off_main(app, move |app| {
        with_config_mut(app, |config| {
            let before = config.profiles.len();
            config.profiles.retain(|p| p.id != id);
            if config.profiles.len() == before {
                return Err("profile not found (was it deleted elsewhere?)".into());
            }
            Ok(())
        })?;
        profile_edited(app, &id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) fn list_profile_glossary(
    app: AppHandle,
    profile_id: String,
) -> Result<Vec<GlossaryEntryDto>, String> {
    match &*app.state::<AppState>().config.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(Ok(config)) => config
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|profile| {
                profile
                    .glossary
                    .0
                    .iter()
                    .map(|(term, thai)| GlossaryEntryDto {
                        term: term.clone(),
                        thai: thai.clone(),
                    })
                    .collect()
            })
            .ok_or_else(|| "profile not found (was it deleted elsewhere?)".into()),
        Some(Err(message)) => Err(message.clone()),
        None => Err("config not loaded yet".into()),
    }
}

#[tauri::command]
pub(crate) async fn add_profile_glossary_entry(
    app: AppHandle,
    profile_id: String,
    term: String,
    thai: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        let (term, thai) = validate_glossary_pair(&term, &thai)?;
        // Case-insensitive pre-check within the profile's own layer: two
        // entries that differ only by case would fight over the same
        // blocks (the layer is merged over the global one, profile wins).
        let config = current_config(app)?;
        let profile = config
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .ok_or_else(|| "profile not found (was it deleted elsewhere?)".to_string())?;
        if profile.glossary.has_term_ci(&term) {
            return Err(format!(
                "A term matching \"{term}\" already exists (case-insensitive)"
            ));
        }
        with_profile_mut(app, &profile_id, |profile| {
            profile.glossary.upsert(&term, &thai);
            Ok(())
        })?;
        profile_edited(app, &profile_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn update_profile_glossary_entry(
    app: AppHandle,
    profile_id: String,
    term: String,
    new_term: String,
    thai: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        let (new_term, thai) = validate_glossary_pair(&new_term, &thai)?;
        let old_term = term.trim().to_string();
        with_profile_mut(app, &profile_id, |profile| {
            if !profile.glossary.0.contains_key(&old_term) {
                return Err("term not found (was it changed elsewhere?)".to_string());
            }
            // Renaming to a case-variant of the same term is fine (the key
            // gets rewritten); renaming onto a DIFFERENT existing term is
            // not — same rule as the global glossary.
            if !new_term.eq_ignore_ascii_case(&old_term) && profile.glossary.has_term_ci(&new_term)
            {
                return Err(format!(
                    "A term matching \"{new_term}\" already exists (case-insensitive)"
                ));
            }
            profile.glossary.remove(&old_term);
            profile.glossary.upsert(&new_term, &thai);
            Ok(())
        })?;
        profile_edited(app, &profile_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_profile_glossary_entry(
    app: AppHandle,
    profile_id: String,
    term: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        with_profile_mut(app, &profile_id, |profile| {
            if !profile.glossary.remove(term.trim()) {
                return Err("term not found (was it deleted elsewhere?)".to_string());
            }
            Ok(())
        })?;
        profile_edited(app, &profile_id);
        Ok(())
    })
    .await
}

/// Exports one Profile's glossary layer as JSON to the path the save
/// dialog picked (ticket 33): the exact format [`Glossary::to_json_string`]
/// writes for the global Glossary — one format, files move either
/// direction. Returns the number of exported entries (an empty layer
/// exports as an empty set and reports 0).
#[tauri::command]
pub(crate) async fn export_profile_glossary(
    app: AppHandle,
    profile_id: String,
    path: String,
) -> Result<usize, String> {
    run_off_main(app, move |app| {
        let config = current_config(app)?;
        let profile = config
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .ok_or_else(|| "profile not found (was it deleted elsewhere?)".to_string())?;
        let json = profile.glossary.to_json_string();
        std::fs::write(&path, json).map_err(|e| format!("cannot write {path}: {e}"))?;
        Ok(profile.glossary.0.len())
    })
    .await
}

/// Imports a glossary JSON file into one Profile's glossary layer (ticket
/// 33) — same format and modes as [`import_glossary`]: "merge" keeps
/// existing entries (import wins on conflicts), "replace" empties the
/// layer first. The file is fully parsed before anything is touched, so a
/// malformed or wrong-shaped file leaves the profile unchanged.
#[tauri::command]
pub(crate) async fn import_profile_glossary(
    app: AppHandle,
    profile_id: String,
    path: String,
    mode: String,
) -> Result<ImportReport, String> {
    run_off_main(app, move |app| {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let imported = Glossary::parse_json_str(&text)?;
        let stats = with_profile_mut(app, &profile_id, |profile| {
            match mode.as_str() {
                "merge" => {}
                "replace" => profile.glossary.clear(),
                other => return Err(format!("unknown import mode \"{other}\"")),
            }
            Ok(profile.glossary.merge(&imported))
        })?;
        profile_edited(app, &profile_id);
        Ok(ImportReport {
            added: stats.added,
            updated: stats.updated,
            total: stats.added + stats.updated,
        })
    })
    .await
}

/// Deletes one window-relative Capture Zone from a Profile (Settings →
/// Profiles). Adding zones goes through the picker (`open_zone_picker`
/// with mode `profile:<id>`), which stores them window-relative.
#[tauri::command]
pub(crate) async fn delete_profile_zone(
    app: AppHandle,
    profile_id: String,
    zone_id: String,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        with_profile_mut(app, &profile_id, |profile| {
            let before = profile.zones.len();
            profile.zones.retain(|zone| zone.id != zone_id);
            if profile.zones.len() == before {
                return Err("zone not found (was it deleted elsewhere?)".into());
            }
            Ok(())
        })?;
        // While the profile is active the loop restart in profile_edited
        // drops the zone's pills; a stopped loop has nothing to clear.
        profile_edited(app, &profile_id);
        Ok(())
    })
    .await
}

// ---- Model Pack tests (deferred-model-packs ticket 04) ----

#[cfg(test)]
mod pack_tests {
    use super::*;
    use serde_json::json;

    /// A valid minimal manifest over the launch-contract file names (fake
    /// hashes; the snapshot reads pins, never verifies bytes against a
    /// real bundle).
    fn test_manifest() -> langception_core::model_packs::Manifest {
        let hex_a = "aa".repeat(32);
        let file = |name: &str, size: u64| {
            format!("{{\"name\": \"{name}\", \"sha256\": \"{hex_a}\", \"size\": {size}}}")
        };
        let pack = |name: &str, url: &str, files: &[String]| {
            format!(
                "\"{name}\": {{\"version\": \"1\", \"urls\": [\"{url}\"], \"files\": [{}]}}",
                files.join(", ")
            )
        };
        let json = format!(
            r#"{{"manifest_version": 1,
                "bundled": {{"files": [{}]}},
                "packs": {{{}, {}, {}}}}}"#,
            file("pp-ocrv6_tiny_det.onnx", 1),
            pack(
                "small",
                "https://example/small.zip",
                &[
                    file("pp-ocrv6_small_det.onnx", 10),
                    file("pp-ocrv6_small_rec.onnx", 20),
                    file("ppocrv6_dict.txt", 5),
                ],
            ),
            pack(
                "medium",
                "https://example/medium.zip",
                &[
                    file("pp-ocrv6_medium_det.onnx", 30),
                    file("pp-ocrv6_medium_rec.onnx", 40),
                    file("ppocrv6_dict.txt", 5),
                ],
            ),
            pack(
                "korean",
                "https://example/korean.zip",
                &[
                    file("korean_pp-ocrv5_mobile_rec.onnx", 7),
                    file("ppocrv5_korean_dict.txt", 4),
                ],
            ),
        );
        langception_core::model_packs::Manifest::parse(&json).unwrap()
    }

    /// A downloader over the test manifest with a transport the tests
    /// never call (no download is started — only `state()` is read).
    fn test_downloader(
        active_dir: &std::path::Path,
    ) -> std::sync::Arc<langception_core::pack_downloader::PackDownloader> {
        struct NeverTransport;
        #[async_trait::async_trait]
        impl langception_core::pack_downloader::PackTransport for NeverTransport {
            async fn fetch(
                &self,
                _url: &str,
            ) -> Result<
                langception_core::pack_downloader::PackResponse,
                langception_core::pack_downloader::PackFetchError,
            > {
                unreachable!("no download runs in this test")
            }
        }
        std::sync::Arc::new(langception_core::pack_downloader::PackDownloader::new(
            std::sync::Arc::new(NeverTransport),
            std::sync::Arc::new(test_manifest()),
            active_dir,
        ))
    }

    fn snapshot_for(
        active_dir: &std::path::Path,
        engine: &str,
        language: SourceLanguage,
    ) -> PackStatesDto {
        let downloader = test_downloader(active_dir);
        pack_states_snapshot(Some(&downloader), active_dir, engine, language)
    }

    #[test]
    fn the_snapshot_reports_disk_required_and_totals_per_pack() {
        let root = std::env::temp_dir().join(format!("pack-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let active = root.join("active");
        std::fs::create_dir_all(&active).unwrap();

        // Small fully present but wrong: the pins' sizes so the size
        // check passes and the hash decides, with one file's bytes wrong
        // — stale. The other two packs are untouched (missing).
        std::fs::write(active.join("pp-ocrv6_small_det.onnx"), b"0123456789").unwrap();
        std::fs::write(active.join("pp-ocrv6_small_rec.onnx"), vec![0u8; 20]).unwrap();
        std::fs::write(active.join("ppocrv6_dict.txt"), b"12345").unwrap();

        let dto = snapshot_for(&active, "pp-ocrv6-small", SourceLanguage::En);
        assert!(dto.managed, "a downloader means managed");
        let by_pack: std::collections::BTreeMap<String, &PackStateDto> = dto
            .packs
            .iter()
            .map(|p| (p.pack.as_str().to_string(), p))
            .collect();
        assert_eq!(by_pack.len(), 3, "every manifest pack is reported");
        let small = by_pack["small"];
        assert_eq!(
            small.disk, "stale",
            "every file present, one hash-mismatching — missing wins only over absence"
        );
        let medium = by_pack["medium"];
        assert_eq!(medium.disk, "missing");
        assert_eq!(medium.total_bytes, 75, "the pinned total for the reclaim note");
        assert!(small.required, "the selected tier's pack is required");
        assert!(!medium.required);
        assert!(
            !by_pack["korean"].required,
            "English never needs the recognizer set"
        );
        assert_eq!(
            small.download,
            langception_core::pack_downloader::PackDownloadState::NotDownloading,
            "no download in flight"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_snapshot_follows_the_effective_selection() {
        let root =
            std::env::temp_dir().join(format!("pack-snapshot-required-{}", std::process::id()));
        let active = root.join("active");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&active).unwrap();

        let required = |engine: &str, language: SourceLanguage| {
            let dto = snapshot_for(&active, engine, language);
            dto.packs
                .iter()
                .filter(|p| p.required)
                .map(|p| p.pack.as_str().to_string())
                .collect::<Vec<_>>()
        };
        // English + tiny: the bundled baseline covers it.
        assert!(required("pp-ocrv6-tiny", SourceLanguage::En).is_empty());
        // Japanese + tiny: the substitution runs Small's files.
        assert_eq!(required("pp-ocrv6-tiny", SourceLanguage::Ja), vec!["small"]);
        // Korean + tiny: the recognizer set only (no tier detection file).
        assert_eq!(required("pp-ocrv6-tiny", SourceLanguage::Ko), vec!["korean"]);
        // Korean + medium: the tier's set plus the recognizer set.
        assert_eq!(
            required("pp-ocrv6-medium", SourceLanguage::Ko),
            vec!["medium", "korean"]
        );
        // Windows OCR: nothing of this engine.
        assert!(required("windows", SourceLanguage::Ko).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_snapshot_without_a_downloader_is_unmanaged() {
        let dto = pack_states_snapshot(
            None,
            std::path::Path::new("."),
            "pp-ocrv6-small",
            SourceLanguage::En,
        );
        assert!(!dto.managed);
        assert!(dto.packs.is_empty());
    }

    #[test]
    fn the_snapshot_wire_shape_is_camel_case_with_kebab_download_states() {
        let dto = pack_states_snapshot(
            None,
            std::path::Path::new("."),
            "pp-ocrv6-small",
            SourceLanguage::En,
        );
        let unmanaged = serde_json::to_value(&dto).unwrap();
        assert_eq!(unmanaged, json!({ "managed": false, "packs": [] }));

        let root =
            std::env::temp_dir().join(format!("pack-snapshot-wire-{}", std::process::id()));
        let active = root.join("active");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&active).unwrap();
        let dto = snapshot_for(&active, "pp-ocrv6-small", SourceLanguage::En);
        let managed = serde_json::to_value(&dto).unwrap();
        let small = &managed["packs"][0];
        assert_eq!(small["pack"], "small");
        assert_eq!(small["displayName"], "Small set");
        assert_eq!(small["disk"], "missing");
        assert_eq!(small["download"], json!({ "state": "not-downloading" }));
        assert_eq!(small["required"], json!(true));
        assert_eq!(small["totalBytes"], json!(35));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pack_ids_parse_by_manifest_spelling_and_unknown_values_are_named_errors() {
        for id in PackId::ALL {
            assert_eq!(parse_pack_id(id.as_str()).unwrap(), id);
        }
        let message = parse_pack_id("tiny").unwrap_err();
        assert!(message.contains("unknown pack \"tiny\""), "{message}");
        assert!(message.contains("small, medium, korean"), "{message}");
    }

    #[test]
    fn disk_codes_map_the_three_statuses() {
        assert_eq!(disk_code(&PackStatus::Ready), "ready");
        assert_eq!(
            disk_code(&PackStatus::Missing { files: vec![] }),
            "missing"
        );
        assert_eq!(disk_code(&PackStatus::Stale { files: vec![] }), "stale");
    }
}
