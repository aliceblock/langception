//! The Tauri shell: tray-first control (start/stop/quit), the Results
//! Window event bridge, and the config file lifecycle. Thin adapter —
//! exercised by the manual smoke checklist.

use langception_core::config::AppConfig;
use langception_core::glossary::Glossary;
use langception_core::google::GoogleTranslateEngine;
use langception_core::orchestrator::{LoopConfig, Orchestrator, SystemClock};
use langception_core::ports::{Region, TARGET_LANGUAGE};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

/// What the Results Window needs to show: loop state plus the effective
/// config (slice 1's only settings surface is the config file).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub config_error: Option<String>,
    pub language: String,
    pub region: String,
    pub cadence_ms: u32,
    pub glossary_terms: usize,
    pub target_language: &'static str,
    pub config_path: String,
}

struct RunningLoop {
    orchestrator: tauri::async_runtime::JoinHandle<()>,
    forwarder: tauri::async_runtime::JoinHandle<()>,
}

struct AppState {
    /// `Some(Err)` = the config file is broken; surfaced in the Results
    /// Window, never silently.
    config: Mutex<Option<Result<AppConfig, String>>>,
    config_path: PathBuf,
    running: Mutex<Option<RunningLoop>>,
    toggle_item: Mutex<Option<MenuItem<tauri::Wry>>>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            config: Mutex::new(None),
            config_path: config_file_path(),
            running: Mutex::new(None),
            toggle_item: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            start_translation,
            stop_translation,
            get_status
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Load (or first-run create) the config; a broken file must show
            // up in the Results Window, not vanish.
            let state = handle.state::<AppState>();
            let config_path = state.config_path.clone();
            *state.config.lock().unwrap() = Some(load_config(&config_path));

            build_tray(&handle)?;

            if let Some(window) = handle.get_webview_window("results") {
                let _ = window.show();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Langception");
}

fn config_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("langception")
        .join("config.toml")
}

/// Reads the config file, writing the annotated default template on first
/// launch. Errors are returned as display strings — a broken config is a
/// visible condition, never a silent one.
fn load_config(path: &std::path::Path) -> Result<AppConfig, String> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create config folder {}: {e}", parent.display()))?;
        }
        std::fs::write(path, AppConfig::DEFAULT_TEMPLATE)
            .map_err(|e| format!("cannot write default config {}: {e}", path.display()))?;
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    AppConfig::from_toml_str(&text).map_err(|e| {
        format!(
            "{} is broken — fix it, then stop/start translation:\n{e}",
            path.display()
        )
    })
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Start Translation", true, None::<&str>)?;
    let open_config =
        MenuItem::with_id(app, "open-config", "Open Config Folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &open_config, &quit])?;

    *app.state::<AppState>().toggle_item.lock().unwrap() = Some(toggle);

    // Tray menu events are handled here only (not via Builder::on_menu_event)
    // so each item fires exactly once.
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().expect("bundled window icon").clone())
        .tooltip("Langception — stopped")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                let _ = toggle_translation(app);
            }
            "open-config" => open_config_folder(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left click on the tray icon focuses the Results Window.
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("results") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
}

fn status_for(app: &AppHandle) -> Status {
    let state = app.state::<AppState>();
    let config_guard = state.config.lock().unwrap();
    let running = state.running.lock().unwrap().is_some();
    let config_path = state.config_path.display().to_string();

    match &*config_guard {
        Some(Ok(config)) => Status {
            running,
            config_error: None,
            language: config.source.language.code().to_string(),
            region: format!(
                "{},{} {}×{}",
                config.region.x, config.region.y, config.region.width, config.region.height
            ),
            cadence_ms: config.capture.cadence_ms,
            glossary_terms: config.glossary.0.len(),
            target_language: TARGET_LANGUAGE,
            config_path,
        },
        Some(Err(message)) => Status {
            running: false,
            config_error: Some(message.clone()),
            language: "-".into(),
            region: "-".into(),
            cadence_ms: 0,
            glossary_terms: 0,
            target_language: TARGET_LANGUAGE,
            config_path,
        },
        None => Status {
            running: false,
            config_error: Some("config not loaded yet".into()),
            language: "-".into(),
            region: "-".into(),
            cadence_ms: 0,
            glossary_terms: 0,
            target_language: TARGET_LANGUAGE,
            config_path,
        },
    }
}

fn set_toggle_label(app: &AppHandle, label: &str) {
    let state = app.state::<AppState>();
    let guard = state.toggle_item.lock().unwrap();
    if let Some(item) = guard.as_ref() {
        let _ = item.set_text(label);
    }
}

/// Starts the translation loop: wires real Windows adapters to the
/// orchestrator and bridges its events to the Results Window. Idempotent.
fn start_loop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();

    let mut running = state.running.lock().unwrap();
    if running.is_some() {
        return Ok(());
    }

    let config_guard = state.config.lock().unwrap();
    let config = match &*config_guard {
        Some(Ok(config)) => config.clone(),
        Some(Err(message)) => return Err(message.clone()),
        None => return Err("config not loaded yet".into()),
    };
    drop(config_guard);

    let loop_config = LoopConfig {
        region: Region {
            x: config.region.x,
            y: config.region.y,
            width: config.region.width,
            height: config.region.height,
        },
        source_language: config.source.language,
        cadence: Duration::from_millis(config.capture.cadence_ms as u64),
    };
    let glossary: Glossary = config.glossary.clone();

    let (events_tx, mut events_rx) = mpsc::unbounded_channel();

    let capture = Arc::new(crate::capture::GdiCaptureSource::new());
    let ocr = Arc::new(crate::ocr::WindowsOcrEngine::new(config.source.language));
    let translate = Arc::new(
        GoogleTranslateEngine::new()
            .map_err(|e| format!("failed to initialize Google engine: {e}"))?,
    );
    let clock = Arc::new(SystemClock);

    let orchestrator =
        Orchestrator::new(capture, ocr, translate, clock, loop_config, glossary, events_tx);
    let orchestrator_task = tauri::async_runtime::spawn(orchestrator.run());

    let bridge_app = app.clone();
    let forwarder_task = tauri::async_runtime::spawn(async move {
        while let Some(event) = events_rx.recv().await {
            if let Some(window) = bridge_app.get_webview_window("results") {
                // Window temporarily unavailable (user hid it): keep the loop.
                let _ = window.emit("loop-event", &event);
            }
        }
    });

    *running = Some(RunningLoop {
        orchestrator: orchestrator_task,
        forwarder: forwarder_task,
    });
    drop(running);

    set_toggle_label(app, "Stop Translation");
    let _ = app.emit("engine-state", &status_for(app));
    Ok(())
}

fn stop_loop(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(running) = state.running.lock().unwrap().take() {
        running.orchestrator.abort();
        running.forwarder.abort();
    }
    set_toggle_label(app, "Start Translation");
    let _ = app.emit("engine-state", &status_for(app));
}

fn toggle_translation(app: &AppHandle) -> Result<(), String> {
    let running = app
        .state::<AppState>()
        .running
        .lock()
        .unwrap()
        .is_some();
    if running {
        stop_loop(app);
        Ok(())
    } else {
        start_loop(app)
    }
}

fn open_config_folder(app: &AppHandle) {
    let state = app.state::<AppState>();
    let path = state.config_path.clone();
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

// ---- Commands invoked from the Results Window ----

#[tauri::command]
fn start_translation(app: AppHandle) -> Result<(), String> {
    start_loop(&app)
}

#[tauri::command]
fn stop_translation(app: AppHandle) {
    stop_loop(&app)
}

#[tauri::command]
fn get_status(app: AppHandle) -> Status {
    status_for(&app)
}
