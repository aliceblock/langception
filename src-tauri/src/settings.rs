//! The settings window (ticket 06): one surface for every config the app
//! keeps, doubling as the first-run wizard host. The Results Window stays
//! the live dashboard; this window owns the *editing*: General (language,
//! cadence, stability, start-with-Windows), Engines (engine card + OCR
//! language-pack detection), Glossary (full management + JSON import/
//! export), Text Style, Hotkeys (rebinds apply live), About.
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
    apply_state, current_config, push_warning, restart_loop_if_running, run_off_main,
    with_config_mut, AppState, HotkeyStatus,
};
use langception_core::config::HotkeySettings;
use langception_core::glossary::Glossary;
use langception_core::ports::{SourceLanguage, TARGET_LANGUAGE};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
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
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        SETTINGS_LABEL,
        WebviewUrl::App("settings.html".into()),
    )
    .title("Langception — Settings")
    .inner_size(740.0, 640.0)
    .min_inner_size(560.0, 440.0)
    .resizable(true)
    .center()
    .visible(false)
    .build()
    .map_err(|e| format!("cannot open the settings window: {e}"))?;

    // Same rule as the Results Window (ADR-0003): Langception's own windows
    // are never translation sources.
    if let Err(message) = crate::overlay::exclude_from_capture(&window) {
        push_warning(
            &app.state::<AppState>(),
            format!("could not exclude the settings window from capture: {message}"),
        );
    }
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct About {
    pub product_name: String,
    pub version: String,
    pub config_path: String,
    pub target_language: &'static str,
}

#[tauri::command]
pub(crate) fn get_about(app: AppHandle) -> About {
    let state = app.state::<AppState>();
    About {
        product_name: app.package_info().name.clone(),
        version: app.package_info().version.to_string(),
        config_path: state.config_path.display().to_string(),
        target_language: TARGET_LANGUAGE,
    }
}

// ---- General tab ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettingsDto {
    pub language: String,
    pub cadence_ms: u32,
    pub stability_ms: u32,
}

#[tauri::command]
pub(crate) async fn set_general(app: AppHandle, settings: GeneralSettingsDto) -> Result<(), String> {
    let language = match settings.language.as_str() {
        "en" => SourceLanguage::En,
        "ja" => SourceLanguage::Ja,
        "zh" => SourceLanguage::Zh,
        "ko" => SourceLanguage::Ko,
        other => return Err(format!("unknown source language \"{other}\"")),
    };
    run_off_main(app, move |app| {
        with_config_mut(app, |config| {
            config.source.language = language;
            config.capture.cadence_ms = settings.cadence_ms;
            config.capture.stability_ms = settings.stability_ms;
            Ok(())
        })?;
        // Language/cadence/stability are read when the loop starts: restart
        // it so the change visibly applies (a language change must reset
        // tracking anyway — the OCR engine is rebuilt).
        restart_loop_if_running(app);
        apply_state(app);
        Ok(())
    })
    .await
}

/// Optional start-with-Windows (managed by the OS — the Run registry key on
/// Windows — not by config.toml). Returns the resulting state so the toggle
/// always reflects reality.
#[tauri::command]
pub(crate) async fn set_start_with_windows(app: AppHandle, enabled: bool) -> Result<bool, String> {
    run_off_main(app, move |app| {
        let manager = app.autolaunch();
        if enabled {
            manager
                .enable()
                .map_err(|e| format!("cannot enable start-with-Windows: {e}"))?;
        } else {
            manager
                .disable()
                .map_err(|e| format!("cannot disable start-with-Windows: {e}"))?;
        }
        manager
            .is_enabled()
            .map_err(|e| format!("cannot query start-with-Windows: {e}"))
    })
    .await
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
    match &*app.state::<AppState>().config.lock().unwrap() {
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
pub(crate) async fn add_glossary_entry(app: AppHandle, term: String, thai: String) -> Result<(), String> {
    run_off_main(app, move |app| {
        let (term, thai) = validate_glossary_pair(&term, &thai)?;
        // Case-insensitive pre-check: matching ignores case, so two entries
        // that differ only by case would fight over the same blocks.
        if current_config(app)?.glossary.has_term_ci(&term) {
            return Err(format!("A term matching \"{term}\" already exists (case-insensitive)"));
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
            if !new_term.eq_ignore_ascii_case(&old_term)
                && config.glossary.has_term_ci(&new_term)
            {
                return Err(format!("A term matching \"{new_term}\" already exists (case-insensitive)"));
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
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
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
