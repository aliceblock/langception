//! The Tauri shell: tray-first control, global hotkeys (rebindable via
//! config), the Capture Zone picker overlay (Alt+Z / Alt+Q), Adjust Mode
//! (Alt+A: zone drag/resize + block exclusion, ticket 04), full-screen mode
//! with monitor selection, the per-monitor In-place Overlay (ticket 03), the
//! Results Window event bridge, and the config file lifecycle. Thin adapter —
//! exercised by the manual smoke checklist.

use langception_core::config::{AppConfig, CaptureMode, HotkeySettings, TextStyle};
use langception_core::glossary::Glossary;
use langception_core::google::GoogleTranslateEngine;
use langception_core::orchestrator::{LoopConfig, LoopEvent, Orchestrator, SystemClock};
use langception_core::ports::{ExcludedBlock, Region, Zone, TARGET_LANGUAGE};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, Position, Size,
    WebviewUrl, WebviewWindowBuilder, Wry,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};
use tokio::sync::{mpsc, watch};

const PICKER_LABEL: &str = "zone-picker";
/// Drags smaller than this (CSS px) count as a click: the session cancels.
const MIN_DRAG_CSS_PX: i32 = 8;

/// Full-screen mode watches one synthetic "zone" spanning the selected
/// monitor; its id keys overlay routing and the Results Window badge.
const FULLSCREEN_ZONE_ID: &str = "fullscreen";
const FULLSCREEN_ZONE_NAME: &str = "Full screen";

/// What the picker overlay was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerMode {
    /// Drag creates a persistent Capture Zone.
    Zone,
    /// Drag runs a single OCR+translate pass (One-shot Capture).
    OneShot,
}

/// What the Results Window needs to show: loop state, the capture target
/// (zones vs full-screen), the zone list (the minimal management UI), the
/// Excluded Block list, the effective hotkeys, and any warnings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub paused: bool,
    pub adjust_mode: bool,
    pub config_error: Option<String>,
    pub warnings: Vec<String>,
    pub language: String,
    /// The active capture target: "zones" | "fullscreen".
    pub mode: String,
    /// The monitor full-screen mode watches ("" = primary).
    pub fullscreen_monitor: String,
    pub monitors: Vec<MonitorStatus>,
    pub zones: Vec<ZoneStatus>,
    pub excluded_blocks: Vec<ExcludedStatus>,
    pub cadence_ms: u32,
    pub glossary_terms: usize,
    pub hotkeys: HotkeyStatus,
    pub text_style: TextStyleDto,
    pub target_language: &'static str,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStatus {
    pub name: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneStatus {
    pub id: String,
    pub name: String,
    pub region: String,
    /// The monitor the zone was drawn on ("" = primary).
    pub monitor: String,
    /// False when the zone's monitor is unplugged: the zone stays in the
    /// config but is not watched.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedStatus {
    pub id: String,
    pub monitor: String,
    pub label: String,
    pub region: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub toggle: String,
    pub new_zone: String,
    pub pause: String,
    pub adjust: String,
    pub one_shot: String,
}

/// The overlay's text style, as the settings surface reads and writes it
/// (camelCase JSON). The config file keeps its own snake_case TOML shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextStyleDto {
    pub font_family: String,
    pub font_size: f32,
    pub color: String,
    pub outline_width: f32,
    pub outline_color: String,
    pub background: bool,
    pub background_opacity: f32,
}

impl From<&TextStyle> for TextStyleDto {
    fn from(style: &TextStyle) -> Self {
        Self {
            font_family: style.font_family.clone(),
            font_size: style.font_size,
            color: style.color.clone(),
            outline_width: style.outline_width,
            outline_color: style.outline_color.clone(),
            background: style.background,
            background_opacity: style.background_opacity,
        }
    }
}

impl From<TextStyleDto> for TextStyle {
    fn from(dto: TextStyleDto) -> Self {
        Self {
            font_family: dto.font_family,
            font_size: dto.font_size,
            color: dto.color,
            outline_width: dto.outline_width,
            outline_color: dto.outline_color,
            background: dto.background,
            background_opacity: dto.background_opacity,
        }
    }
}

impl Default for TextStyleDto {
    fn default() -> Self {
        Self::from(&TextStyle::default())
    }
}

impl Default for HotkeyStatus {
    fn default() -> Self {
        let defaults = HotkeySettings::default();
        Self {
            toggle: defaults.toggle_translation,
            new_zone: defaults.new_zone,
            pause: defaults.pause_translation,
            adjust: defaults.adjust_mode,
            one_shot: defaults.one_shot_capture,
        }
    }
}

struct RunningLoop {
    orchestrator: tauri::async_runtime::JoinHandle<()>,
    forwarder: tauri::async_runtime::JoinHandle<()>,
}

/// The shortcut currently bound to each action (parsed at startup); the
/// global-shortcut handler matches pressed keys against these.
#[derive(Default)]
struct RegisteredHotkeys {
    toggle: Option<Shortcut>,
    new_zone: Option<Shortcut>,
    pause: Option<Shortcut>,
    adjust: Option<Shortcut>,
    one_shot: Option<Shortcut>,
}

struct AppState {
    /// `Some(Err)` = the config file is broken; surfaced in the Results
    /// Window, never silently.
    config: Mutex<Option<Result<AppConfig, String>>>,
    config_path: PathBuf,
    running: Mutex<Option<RunningLoop>>,
    paused: Mutex<bool>,
    /// Adjust Mode (Alt+A): click-through off, zones draggable/resizable,
    /// full-screen blocks clickable for exclusion.
    adjust_mode: Mutex<bool>,
    /// The shell pushes the active zone list here; a running loop picks it
    /// up on its next tick without losing per-zone tracking state. In
    /// full-screen mode this is one synthetic zone spanning the monitor.
    zones_tx: watch::Sender<Vec<Zone>>,
    paused_tx: watch::Sender<bool>,
    /// The Excluded Blocks currently in force (all of them in full-screen
    /// mode; empty in zones mode — exclusions are consulted there only).
    exclusions_tx: watch::Sender<Vec<ExcludedBlock>>,
    warnings: Mutex<Vec<String>>,
    registered: Mutex<RegisteredHotkeys>,
    toggle_item: Mutex<Option<MenuItem<Wry>>>,
    pause_item: Mutex<Option<MenuItem<Wry>>>,
    adjust_item: Mutex<Option<MenuItem<Wry>>>,
    fullscreen_item: Mutex<Option<CheckMenuItem<Wry>>>,
    tray: Mutex<Option<TrayIcon<Wry>>>,
    /// `Some` while the picker overlay is open, recording why.
    picker_mode: Mutex<Option<PickerMode>>,
}

pub fn run() {
    let (zones_tx, _) = watch::channel(Vec::new());
    let (paused_tx, _) = watch::channel(false);
    let (exclusions_tx, _) = watch::channel(Vec::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            config: Mutex::new(None),
            config_path: config_file_path(),
            running: Mutex::new(None),
            paused: Mutex::new(false),
            adjust_mode: Mutex::new(false),
            zones_tx,
            paused_tx,
            exclusions_tx,
            warnings: Mutex::new(Vec::new()),
            registered: Mutex::new(RegisteredHotkeys::default()),
            toggle_item: Mutex::new(None),
            pause_item: Mutex::new(None),
            adjust_item: Mutex::new(None),
            fullscreen_item: Mutex::new(None),
            tray: Mutex::new(None),
            picker_mode: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            start_translation,
            stop_translation,
            get_status,
            toggle_pause_translation,
            toggle_adjust_mode,
            set_capture_mode,
            rename_zone,
            delete_zone,
            update_zone_region,
            open_zone_picker,
            get_picker_mode,
            commit_zone_drag,
            cancel_zone_drag,
            exclude_block,
            delete_excluded_block,
            get_text_style,
            set_text_style
        ])
        .on_window_event(|window, event| {
            // The picker overlay is ephemeral; however it dies, the session
            // ends with it.
            if window.label() == PICKER_LABEL && matches!(event, tauri::WindowEvent::Destroyed) {
                *window
                    .app_handle()
                    .state::<AppState>()
                    .picker_mode
                    .lock()
                    .unwrap() = None;
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // Load (or first-run create) the config; a broken file must show
            // up in the Results Window, not vanish.
            let state = handle.state::<AppState>();
            let config_path = state.config_path.clone();
            *state.config.lock().unwrap() = Some(load_config(&config_path));

            build_tray(&handle)?;
            install_hotkeys(&handle)?;

            if let Some(window) = handle.get_webview_window("results") {
                // Langception's own windows are never translation sources
                // (ADR-0003): the Results Window displays Thai, and a zone
                // overlapping it would translate its own entries forever.
                if let Err(message) = crate::overlay::exclude_from_capture(&window) {
                    state.warnings.lock().unwrap().push(format!(
                        "could not exclude the Results Window from capture: {message}"
                    ));
                }
                let _ = window.show();
            }
            apply_state(&handle);

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
fn load_config(path: &Path) -> Result<AppConfig, String> {
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
    let mut config = AppConfig::from_toml_str(&text).map_err(|e| {
        format!(
            "{} is broken — fix it, then stop/start translation:\n{e}",
            path.display()
        )
    })?;
    // Slice-1 configs watched a single [region]; carry it over as a zone.
    if config.migrate_legacy_region() {
        if let Ok(serialized) = config.to_toml_string() {
            let _ = std::fs::write(path, serialized);
        }
    }
    Ok(config)
}

/// The monitor's system name, owned (`Monitor::name` borrows).
fn monitor_name(monitor: &Monitor) -> String {
    monitor.name().cloned().unwrap_or_default()
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Start Translation", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause Translation", false, None::<&str>)?;
    let adjust = MenuItem::with_id(app, "adjust", "Enter Adjust Mode", true, None::<&str>)?;
    // Full-screen mode is a capture target, so its tray entry shows the
    // current state with a checkmark. Monitor choice lives in the Results
    // Window's mode panel (a dropdown, bad tray material).
    let fullscreen = CheckMenuItem::with_id(
        app,
        "fullscreen",
        "Full-Screen Mode",
        true,
        false,
        None::<&str>,
    )?;
    let new_zone = MenuItem::with_id(app, "new-zone", "New Capture Zone", true, None::<&str>)?;
    let one_shot = MenuItem::with_id(app, "one-shot", "One-shot Capture", true, None::<&str>)?;
    let open_config =
        MenuItem::with_id(app, "open-config", "Open Config Folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &pause,
            &adjust,
            &separator,
            &fullscreen,
            &separator,
            &new_zone,
            &one_shot,
            &open_config,
            &quit,
        ],
    )?;

    let state = app.state::<AppState>();
    *state.toggle_item.lock().unwrap() = Some(toggle);
    *state.pause_item.lock().unwrap() = Some(pause);
    *state.adjust_item.lock().unwrap() = Some(adjust);
    *state.fullscreen_item.lock().unwrap() = Some(fullscreen);

    // Tray menu events are handled here only (not via Builder::on_menu_event)
    // so each item fires exactly once.
    let tray = TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .expect("bundled window icon")
                .clone(),
        )
        .tooltip("Langception — stopped")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            // Every item that reaches the overlay/picker windows runs off the
            // main thread (see run_off_main); failures surface as warnings.
            "toggle" => spawn_off_main(app, toggle_translation),
            "pause" => toggle_pause(app),
            "adjust" => spawn_off_main(app, |app| {
                let next = !*app.state::<AppState>().adjust_mode.lock().unwrap();
                set_adjust_mode(app, next);
                Ok(())
            }),
            "fullscreen" => {
                // The checkmark flips to the other target.
                spawn_off_main(app, |app| {
                    let current = match &*app.state::<AppState>().config.lock().unwrap() {
                        Some(Ok(config)) => config.mode.target,
                        _ => CaptureMode::Zones,
                    };
                    let next = match current {
                        CaptureMode::Zones => CaptureMode::FullScreen,
                        CaptureMode::FullScreen => CaptureMode::Zones,
                    };
                    set_capture_mode_inner(app, next, None)
                });
            }
            "new-zone" => spawn_off_main(app, |app| open_picker(app, PickerMode::Zone)),
            "one-shot" => spawn_off_main(app, |app| open_picker(app, PickerMode::OneShot)),
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
                let app = tray.app_handle().clone();
                spawn_off_main(&app, |app| {
                    if let Some(window) = app.get_webview_window("results") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                    Ok(())
                });
            }
        })
        .build(app)?;

    *state.tray.lock().unwrap() = Some(tray);
    Ok(())
}

/// Registers the global shortcut plugin and binds every configured hotkey.
/// A hotkey that cannot be parsed or registered (e.g. taken by another app)
/// becomes a visible warning, never a silent no-op.
fn install_hotkeys(app: &AppHandle) -> tauri::Result<()> {
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(dispatch_shortcut)
            .build(),
    )?;

    let state = app.state::<AppState>();
    let (hotkeys, config_broken) = match &*state.config.lock().unwrap() {
        Some(Ok(config)) => (config.hotkeys.clone(), false),
        _ => (HotkeySettings::default(), true),
    };
    if config_broken {
        state
            .warnings
            .lock()
            .unwrap()
            .push("config is broken — default hotkeys are active".into());
    }

    let mut registered = RegisteredHotkeys::default();
    let mut warnings = state.warnings.lock().unwrap();
    for (action, bind) in [
        ("toggle", (&hotkeys.toggle_translation, Bind::Toggle)),
        ("new_zone", (&hotkeys.new_zone, Bind::NewZone)),
        ("pause", (&hotkeys.pause_translation, Bind::Pause)),
        ("adjust_mode", (&hotkeys.adjust_mode, Bind::Adjust)),
        ("one_shot", (&hotkeys.one_shot_capture, Bind::OneShot)),
    ] {
        let (accel, slot) = bind;
        match accel.parse::<Shortcut>() {
            Ok(shortcut) => match app.global_shortcut().register(shortcut) {
                Ok(()) => match slot {
                    Bind::Toggle => registered.toggle = Some(shortcut),
                    Bind::NewZone => registered.new_zone = Some(shortcut),
                    Bind::Pause => registered.pause = Some(shortcut),
                    Bind::Adjust => registered.adjust = Some(shortcut),
                    Bind::OneShot => registered.one_shot = Some(shortcut),
                },
                Err(e) => warnings.push(format!(
                    "hotkey \"{accel}\" for {action} could not be registered: {e} (in use by another app?)"
                )),
            },
            Err(e) => warnings.push(format!(
                "[hotkeys] {action} = \"{accel}\" is not a valid hotkey: {e}"
            )),
        }
    }
    drop(warnings);
    *state.registered.lock().unwrap() = registered;
    Ok(())
}

enum Bind {
    Toggle,
    NewZone,
    Pause,
    Adjust,
    OneShot,
}

/// Fires on every registered hotkey press, wherever the focus is.
fn dispatch_shortcut(app: &AppHandle, pressed: &Shortcut, event: ShortcutEvent) {
    if !matches!(event.state(), ShortcutState::Pressed) {
        return; // key release: nothing to do
    }
    let state = app.state::<AppState>();
    let registered = state.registered.lock().unwrap();
    let action = if registered.toggle.as_ref() == Some(pressed) {
        Some(Bind::Toggle)
    } else if registered.new_zone.as_ref() == Some(pressed) {
        Some(Bind::NewZone)
    } else if registered.pause.as_ref() == Some(pressed) {
        Some(Bind::Pause)
    } else if registered.adjust.as_ref() == Some(pressed) {
        Some(Bind::Adjust)
    } else if registered.one_shot.as_ref() == Some(pressed) {
        Some(Bind::OneShot)
    } else {
        None
    };
    drop(registered);

    match action {
        // Hotkeys fire on the main thread; anything that reaches the
        // overlay/picker windows runs off it (see run_off_main).
        Some(Bind::Toggle) => spawn_off_main(app, toggle_translation),
        Some(Bind::Pause) => toggle_pause(app),
        Some(Bind::NewZone) => spawn_off_main(app, |app| open_picker(app, PickerMode::Zone)),
        Some(Bind::Adjust) => spawn_off_main(app, |app| {
            let next = !*app.state::<AppState>().adjust_mode.lock().unwrap();
            set_adjust_mode(app, next);
            Ok(())
        }),
        Some(Bind::OneShot) => spawn_off_main(app, |app| open_picker(app, PickerMode::OneShot)),
        None => {}
    }
}

/// One refresh path for every state change: tray labels/tooltip + the
/// Results Window's status snapshot.
fn apply_state(app: &AppHandle) {
    let state = app.state::<AppState>();
    let running = state.running.lock().unwrap().is_some();
    let paused = *state.paused.lock().unwrap();
    let adjusting = *state.adjust_mode.lock().unwrap();
    let fullscreen = match &*state.config.lock().unwrap() {
        Some(Ok(config)) => matches!(config.mode.target, CaptureMode::FullScreen),
        _ => false,
    };

    if let Some(item) = state.toggle_item.lock().unwrap().as_ref() {
        let _ = item.set_text(if running {
            "Stop Translation"
        } else {
            "Start Translation"
        });
    }
    if let Some(item) = state.pause_item.lock().unwrap().as_ref() {
        let _ = item.set_enabled(running);
        let _ = item.set_text(if paused {
            "Resume Translation"
        } else {
            "Pause Translation"
        });
    }
    if let Some(item) = state.adjust_item.lock().unwrap().as_ref() {
        let _ = item.set_text(if adjusting {
            "Exit Adjust Mode"
        } else {
            "Enter Adjust Mode"
        });
    }
    if let Some(item) = state.fullscreen_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(fullscreen);
    }
    if let Some(tray) = state.tray.lock().unwrap().as_ref() {
        let mode_note = if fullscreen { " · full screen" } else { "" };
        let tooltip = if !running {
            "Langception — stopped"
        } else if paused {
            "Langception — paused"
        } else {
            "Langception — running"
        };
        let _ = tray.set_tooltip(Some(format!("{tooltip}{mode_note}")));
    }
    let _ = app.emit("engine-state", &status_for(app));
}

fn status_for(app: &AppHandle) -> Status {
    let state = app.state::<AppState>();
    let running = state.running.lock().unwrap().is_some();
    let paused = *state.paused.lock().unwrap();
    let adjust_mode = *state.adjust_mode.lock().unwrap();
    let warnings = state.warnings.lock().unwrap().clone();
    let config_path = state.config_path.display().to_string();

    // Monitors drive the full-screen dropdown and the zones' enabled flags.
    // On a (rare) enumeration failure the list is empty but every zone stays
    // reported as enabled — see zone_monitor_present.
    let monitors = app.available_monitors().unwrap_or_default();
    let primary = app.primary_monitor().ok().flatten();
    // Monitor has no is_primary(); position equality identifies it (monitors
    // never overlap).
    let is_primary = |m: &Monitor| {
        primary
            .as_ref()
            .is_some_and(|p| p.position() == m.position())
    };

    let default_hotkeys = HotkeyStatus::default();
    let (
        config_error,
        language,
        mode,
        fullscreen_monitor,
        zones,
        excluded_blocks,
        cadence_ms,
        glossary_terms,
        hotkeys,
        text_style,
    ) = match &*state.config.lock().unwrap() {
        Some(Ok(config)) => (
            None,
            config.source.language.code().to_string(),
            match config.mode.target {
                CaptureMode::Zones => "zones",
                CaptureMode::FullScreen => "fullscreen",
            }
            .to_string(),
            config.mode.fullscreen_monitor.clone(),
            config
                .zones
                .iter()
                .map(|zone| ZoneStatus {
                    id: zone.id.clone(),
                    name: zone.name.clone(),
                    region: format!(
                        "{},{} {}×{}",
                        zone.region.x, zone.region.y, zone.region.width, zone.region.height
                    ),
                    monitor: zone.monitor.clone(),
                    enabled: zone_monitor_present(&monitors, primary.as_ref(), &zone.monitor),
                })
                .collect(),
            config
                .excluded_blocks
                .iter()
                .map(|e| ExcludedStatus {
                    id: e.id.clone(),
                    monitor: e.monitor.clone(),
                    label: e.label.clone(),
                    region: format!(
                        "{},{} {}×{}",
                        e.region.x, e.region.y, e.region.width, e.region.height
                    ),
                })
                .collect(),
            config.capture.cadence_ms,
            config.glossary.0.len(),
            HotkeyStatus {
                toggle: config.hotkeys.toggle_translation.clone(),
                new_zone: config.hotkeys.new_zone.clone(),
                pause: config.hotkeys.pause_translation.clone(),
                adjust: config.hotkeys.adjust_mode.clone(),
                one_shot: config.hotkeys.one_shot_capture.clone(),
            },
            TextStyleDto::from(&config.text_style),
        ),
        Some(Err(message)) => (
            Some(message.clone()),
            "-".into(),
            "zones".into(),
            String::new(),
            Vec::new(),
            Vec::new(),
            0,
            0,
            default_hotkeys,
            TextStyleDto::default(),
        ),
        None => (
            Some("config not loaded yet".into()),
            "-".into(),
            "zones".into(),
            String::new(),
            Vec::new(),
            Vec::new(),
            0,
            0,
            default_hotkeys,
            TextStyleDto::default(),
        ),
    };

    Status {
        running,
        paused,
        adjust_mode,
        config_error,
        warnings,
        language,
        mode,
        fullscreen_monitor,
        monitors: monitors
            .iter()
            .map(|m| MonitorStatus {
                name: monitor_name(m),
                primary: is_primary(m),
            })
            .collect(),
        zones,
        excluded_blocks,
        cadence_ms,
        glossary_terms,
        hotkeys,
        text_style,
        target_language: TARGET_LANGUAGE,
        config_path,
    }
}

/// The real Windows pipeline for translation passes: adapters wired to the
/// configured Source Language and cadence, plus the loop's glossary.
struct Pipeline {
    config: LoopConfig,
    glossary: Glossary,
    capture: Arc<dyn langception_core::ports::CaptureSource>,
    ocr: Arc<dyn langception_core::ports::OcrEngine>,
    translate: Arc<dyn langception_core::ports::TranslateEngine>,
}

fn build_pipeline(config: &AppConfig) -> Result<Pipeline, String> {
    Ok(Pipeline {
        config: LoopConfig {
            source_language: config.source.language,
            cadence: Duration::from_millis(config.capture.cadence_ms as u64),
        },
        glossary: config.glossary.clone(),
        capture: Arc::new(crate::capture::GdiCaptureSource::new()),
        ocr: Arc::new(crate::ocr::WindowsOcrEngine::new(config.source.language)),
        translate: Arc::new(
            GoogleTranslateEngine::new()
                .map_err(|e| format!("failed to initialize Google engine: {e}"))?,
        ),
    })
}

/// Bridges orchestrator events to the display layer: the Results Window
/// (translations + errors) and the In-place Overlay (zone translations in
/// place, routed per monitor). Exits on its own when the last event sender
/// drops (loop stopped, or a one-shot pass finished).
fn spawn_forwarder(
    app: &AppHandle,
    mut events_rx: mpsc::UnboundedReceiver<LoopEvent>,
    overlays: Vec<(String, Monitor)>,
) -> tauri::async_runtime::JoinHandle<()> {
    let bridge_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut router = crate::overlay::OverlayRouter::new(overlays);
        while let Some(event) = events_rx.recv().await {
            match &event {
                LoopEvent::Translated {
                    key,
                    block,
                    translation,
                    origin,
                    ..
                } => {
                    router.show_translation(&bridge_app, origin, key, block, translation);
                }
                LoopEvent::BlocksRemoved { origin, keys, .. } => {
                    router.hide_blocks(&bridge_app, origin, keys);
                }
                LoopEvent::Error { .. } => {}
            }
            // The overlay handles its own events; the Results Window is a
            // history log and does not care about removals.
            if matches!(
                event,
                LoopEvent::Translated { .. } | LoopEvent::Error { .. }
            ) {
                if let Some(window) = bridge_app.get_webview_window("results") {
                    // Window temporarily unavailable (user hid it): keep the loop.
                    let _ = window.emit("loop-event", &event);
                }
            }
        }
    })
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

    // A fresh loop starts unpaused, watching the target's regions (zones, or
    // the full-screen monitor) and the Excluded Blocks in force. Subscribe
    // BEFORE sending: a watch::send with zero receivers does not store the
    // value, so a later subscriber would see an empty list.
    let zones_rx = state.zones_tx.subscribe();
    let paused_rx = state.paused_tx.subscribe();
    let excluded_rx = state.exclusions_tx.subscribe();
    let _ = state.paused_tx.send(false);
    *state.paused.lock().unwrap() = false;
    let _ = state.zones_tx.send(active_zones(app, &config));
    let _ = state.exclusions_tx.send(active_exclusions(&config));

    let pipeline = build_pipeline(&config)?;
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    // Per-monitor overlays show the translations in place. Creation failure
    // never stops the loop: the Results Window still works, and the warning
    // stays visible.
    let overlays = match crate::overlay::ensure_overlays(app) {
        Ok(overlays) => {
            overlays
        }
        Err(message) => {
            push_warning(&state, format!("In-place Overlay unavailable: {message}"));
            Vec::new()
        }
    };

    let orchestrator = Orchestrator::new(
        pipeline.capture,
        pipeline.ocr,
        pipeline.translate,
        Arc::new(SystemClock),
        pipeline.config,
        pipeline.glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );
    let orchestrator_task = tauri::async_runtime::spawn(orchestrator.run());
    let forwarder_task = spawn_forwarder(app, events_rx, overlays);

    *running = Some(RunningLoop {
        orchestrator: orchestrator_task,
        forwarder: forwarder_task,
    });
    drop(running);

    apply_state(app);
    Ok(())
}

fn stop_loop(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(running) = state.running.lock().unwrap().take() {
        running.orchestrator.abort();
        running.forwarder.abort();
    }
    // Pause is a sub-state of running: stopping clears it.
    *state.paused.lock().unwrap() = false;
    let _ = state.paused_tx.send(false);
    // Nothing new is translated and stale Thai must not linger on screen.
    // While Adjust Mode is on the overlays stay visible (they carry the
    // draggable zone rectangles), so only their blocks are cleared.
    if *state.adjust_mode.lock().unwrap() {
        crate::overlay::clear_all(app);
    } else {
        crate::overlay::clear_and_hide(app);
    }
    apply_state(app);
}

/// Adds a warning unless the exact same one is already shown (start/stop
/// cycles must not pile up duplicates).
fn push_warning(state: &AppState, message: String) {
    let mut warnings = state.warnings.lock().unwrap();
    if !warnings.iter().any(|w| w == &message) {
        warnings.push(message);
    }
}

fn toggle_translation(app: &AppHandle) -> Result<(), String> {
    let running = app.state::<AppState>().running.lock().unwrap().is_some();
    if running {
        stop_loop(app);
        Ok(())
    } else {
        start_loop(app)
    }
}

/// Alt+P / tray: hold the loop still without resetting per-block tracking.
fn toggle_pause(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.running.lock().unwrap().is_none() {
        return; // pause is a sub-state of running
    }
    let now_paused = {
        let mut paused = state.paused.lock().unwrap();
        *paused = !*paused;
        *paused
    };
    let _ = state.paused_tx.send(now_paused);
    apply_state(app);
}

fn open_config_folder(app: &AppHandle) {
    let state = app.state::<AppState>();
    let path = state.config_path.clone();
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

// ---- Capture target (zones vs full-screen) + Adjust Mode ----

/// Whether a zone's monitor (by system name; empty = the primary, the
/// legacy default) is present. One policy for every caller: when monitor
/// enumeration fails, assume present and let the loop surface the real
/// errors — silently watching nothing would hide the failure.
fn zone_monitor_present(monitors: &[Monitor], primary: Option<&Monitor>, name: &str) -> bool {
    if monitors.is_empty() {
        return true; // enumeration failed: assume present
    }
    if name.is_empty() {
        return primary.is_some();
    }
    monitors
        .iter()
        .any(|m| m.name().map(|n| n.as_str()) == Some(name))
}

/// The zones the loop watches for the config's current capture target:
/// in zones mode, every zone whose monitor is present (unplugged monitors
/// disable their zones without deleting them); in full-screen mode, one
/// synthetic zone spanning the selected monitor.
fn active_zones(app: &AppHandle, config: &AppConfig) -> Vec<Zone> {
    match config.mode.target {
        CaptureMode::Zones => {
            let monitors = app.available_monitors().unwrap_or_default();
            let primary = app.primary_monitor().ok().flatten();
            config
                .zones
                .iter()
                .filter(|zone| zone_monitor_present(&monitors, primary.as_ref(), &zone.monitor))
                .cloned()
                .collect()
        }
        CaptureMode::FullScreen => match fullscreen_watch_monitor(app, config) {
            Ok(monitor) => {
                let origin = monitor.position();
                let size = monitor.size();
                vec![Zone {
                    id: FULLSCREEN_ZONE_ID.to_string(),
                    name: FULLSCREEN_ZONE_NAME.to_string(),
                    monitor: monitor_name(&monitor),
                    region: Region {
                        x: origin.x,
                        y: origin.y,
                        width: size.width,
                        height: size.height,
                    },
                }]
            }
            // No monitor query → nothing to watch; that must not be silent.
            Err(message) => {
                push_warning(
                    &app.state::<AppState>(),
                    format!("full-screen mode watches nothing: {message}"),
                );
                Vec::new()
            }
        },
    }
}

/// The Excluded Blocks in force for the config's current target: all of
/// them in full-screen mode, none in zones mode (CONTEXT.md: exclusions
/// are consulted in full-screen mode).
fn active_exclusions(config: &AppConfig) -> Vec<ExcludedBlock> {
    match config.mode.target {
        CaptureMode::Zones => Vec::new(),
        CaptureMode::FullScreen => config.excluded_blocks.clone(),
    }
}

/// The monitor full-screen mode watches: the configured one by system name,
/// the primary when the config value is empty — and, when the configured
/// monitor is unplugged, the primary with a visible warning (full-screen
/// mode must keep working, not vanish).
fn fullscreen_watch_monitor(app: &AppHandle, config: &AppConfig) -> Result<Monitor, String> {
    let monitors = app
        .available_monitors()
        .map_err(|e| format!("cannot enumerate monitors: {e}"))?;
    let wanted = config.mode.fullscreen_monitor.as_str();
    if !wanted.is_empty() {
        if let Some(monitor) = monitors.iter().find(|m| m.name().map(|n| n.as_str()) == Some(wanted)) {
            return Ok(monitor.clone());
        }
        push_warning(
            &app.state::<AppState>(),
            format!(
                "full-screen monitor \"{wanted}\" is not connected — using the primary monitor"
            ),
        );
    }
    app.primary_monitor()
        .map_err(|e| format!("cannot query the primary monitor: {e}"))?
        .ok_or_else(|| "no primary monitor found".to_string())
}

/// The adjust payload for one overlay's monitor: whether Adjust Mode is on,
/// whether the capture target is full-screen, and the zone rectangles on
/// this monitor (physical px, monitor-relative). In full-screen mode the
/// zones panel stays empty — blocks are excluded by clicking them instead.
fn adjust_payload_for(app: &AppHandle, monitor: &Monitor) -> AdjustPayload {
    let state = app.state::<AppState>();
    let (target, zones) = match &*state.config.lock().unwrap() {
        Some(Ok(config)) => (config.mode.target, config.zones.clone()),
        _ => (CaptureMode::Zones, Vec::new()),
    };
    let fullscreen = matches!(target, CaptureMode::FullScreen);
    let origin = monitor.position();
    let size = monitor.size();
    let name = monitor_name(monitor);

    // A zone shows up on the monitor it belongs to (by name; legacy zones
    // with no monitor follow their region's center). Disabled zones have no
    // monitor to appear on.
    let zone_rects = if fullscreen {
        Vec::new()
    } else {
        zones
            .iter()
            .filter(|zone| {
                if !zone.monitor.is_empty() {
                    zone.monitor == name
                } else {
                    let center_x = zone.region.x + zone.region.width as i32 / 2;
                    let center_y = zone.region.y + zone.region.height as i32 / 2;
                    center_x >= origin.x
                        && center_x < origin.x + size.width as i32
                        && center_y >= origin.y
                        && center_y < origin.y + size.height as i32
                }
            })
            .map(|zone| AdjustZone {
                id: zone.id.clone(),
                name: zone.name.clone(),
                x: zone.region.x - origin.x,
                y: zone.region.y - origin.y,
                width: zone.region.width,
                height: zone.region.height,
            })
            .collect()
    };

    AdjustPayload {
        active: true,
        fullscreen,
        zones: zone_rects,
        origin_x: origin.x,
        origin_y: origin.y,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdjustPayload {
    active: bool,
    fullscreen: bool,
    zones: Vec<AdjustZone>,
    /// This monitor's origin in global physical px, so the page can send
    /// global coordinates back (exclusion rects, adjusted zone regions).
    origin_x: i32,
    origin_y: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdjustZone {
    id: String,
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Sets Adjust Mode on or off: overlay click-through switches off (zones
/// draggable/resizable, full-screen blocks clickable) and back on.
fn set_adjust_mode(app: &AppHandle, active: bool) {
    let state = app.state::<AppState>();
    *state.adjust_mode.lock().unwrap() = active;

    if active {
        // The transient picker must be gone: Adjust Mode owns the overlays'
        // interactivity.
        close_picker(app);
        // The overlays must exist even while translation is stopped (zones
        // are adjustable without the loop running).
        match crate::overlay::ensure_overlays(app) {
            Ok(overlays) => {
                for (label, monitor) in overlays {
                    if let Some(window) = app.get_webview_window(&label) {
                        let _ = window.set_ignore_cursor_events(false);
                    }
                    let _ = app.emit_to(&label, "overlay-adjust", &adjust_payload_for(app, &monitor));
                }
            }
            Err(message) => {
                push_warning(&state, format!("Adjust Mode unavailable: {message}"));
                *state.adjust_mode.lock().unwrap() = false;
            }
        }
    } else {
        for window in crate::overlay::overlay_windows(app) {
            let _ = app.emit_to(
                window.label(),
                "overlay-adjust",
                &AdjustPayload {
                    active: false,
                    fullscreen: false,
                    zones: Vec::new(),
                    origin_x: 0,
                    origin_y: 0,
                },
            );
            let _ = window.set_ignore_cursor_events(true);
        }
        // Adjust Mode created or showed the overlays; without the loop there
        // is nothing to see on them.
        if state.running.lock().unwrap().is_none() {
            crate::overlay::hide_all(app);
        }
    }
    apply_state(app);
}

/// Re-sends the adjust payloads (zone rects moved, target switched) while
/// Adjust Mode is on; no-op otherwise.
fn refresh_adjust_overlays(app: &AppHandle) {
    let adjusting = *app.state::<AppState>().adjust_mode.lock().unwrap();
    if !adjusting {
        return;
    }
    if let Ok(overlays) = crate::overlay::ensure_overlays(app) {
        for (label, monitor) in overlays {
            let _ = app.emit_to(&label, "overlay-adjust", &adjust_payload_for(app, &monitor));
        }
    }
}

// ---- Capture Zone picker overlay ----

/// Opens (or toggles closed) the fullscreen picker overlay.
///
/// Threading: MUST NOT run on the main thread (creates/destroys windows —
/// see `run_off_main`; all callers route through it or `spawn_off_main`).
fn open_picker(app: &AppHandle, mode: PickerMode) -> Result<(), String> {
    // A hotkey or menu item while the overlay is open cancels the session.
    if let Some(existing) = app.get_webview_window(PICKER_LABEL) {
        let _ = existing.destroy();
        return Ok(());
    }

    // The picker owns the overlays' attention while it is open: leave
    // Adjust Mode first (its interactive zone rectangles would fight the
    // dim + drag session).
    if *app.state::<AppState>().adjust_mode.lock().unwrap() {
        set_adjust_mode(app, false);
    }

    // The dim + selection must not fight with already-rendered translations.
    crate::overlay::hide_all(app);

    let monitor = primary_monitor(app)?;
    let size = monitor.size();
    let origin = monitor.position();

    *app.state::<AppState>().picker_mode.lock().unwrap() = Some(mode);

    let window =
        WebviewWindowBuilder::new(app, PICKER_LABEL, WebviewUrl::App("picker.html".into()))
            .title("Langception — Capture Zone")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .visible(false)
            .build()
            .map_err(|e| {
                *app.state::<AppState>().picker_mode.lock().unwrap() = None;
                format!("cannot open the capture overlay: {e}")
            })?;

    window
        .set_position(Position::Physical(PhysicalPosition::new(
            origin.x, origin.y,
        )))
        .map_err(|e| format!("cannot place the capture overlay: {e}"))?;
    window
        .set_size(Size::Physical(PhysicalSize::new(size.width, size.height)))
        .map_err(|e| format!("cannot size the capture overlay: {e}"))?;
    // Same rule as the Results Window: Langception's own windows are never
    // translation sources. The dim must not darken the loop's captures, and
    // a session must not OCR its own selection rectangle.
    if let Err(message) = crate::overlay::exclude_from_capture(&window) {
        app.state::<AppState>()
            .warnings
            .lock()
            .unwrap()
            .push(format!("could not exclude the picker from capture: {message}"));
    }
    window
        .show()
        .map_err(|e| format!("cannot show the capture overlay: {e}"))?;
    let _ = window.set_focus();
    Ok(())
}

fn close_picker(app: &AppHandle) {
    *app.state::<AppState>().picker_mode.lock().unwrap() = None;
    if let Some(window) = app.get_webview_window(PICKER_LABEL) {
        let _ = window.destroy();
    }
    // Back to in-place translations (only meaningful while the loop runs).
    if app.state::<AppState>().running.lock().unwrap().is_some() {
        crate::overlay::show_all(app);
    }
}

fn primary_monitor(app: &AppHandle) -> Result<Monitor, String> {
    app.primary_monitor()
        .map_err(|e| format!("cannot query the primary monitor: {e}"))?
        .ok_or_else(|| "no primary monitor found".to_string())
}

/// Picker viewport coordinates (CSS px) → screen region in physical px.
/// The overlay covers the primary monitor, whose top-left is the region
/// origin; CSS px are scaled by the monitor's DPI factor.
fn region_from_css(monitor: &Monitor, x: i32, y: i32, width: i32, height: i32) -> Region {
    let scale = monitor.scale_factor();
    let origin = monitor.position();
    Region {
        x: origin.x + (f64::from(x) * scale).round() as i32,
        y: origin.y + (f64::from(y) * scale).round() as i32,
        width: ((f64::from(width) * scale).round() as u32).max(1),
        height: ((f64::from(height) * scale).round() as u32).max(1),
    }
}

fn save_config(config: &AppConfig, path: &Path) -> Result<(), String> {
    let text = config.to_toml_string().map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The next free "Zone N" name (existing names win, so deletes never cause
/// collisions).
fn next_zone_name(config: &AppConfig) -> String {
    for n in 1.. {
        let name = format!("Zone {n}");
        if !config
            .zones
            .iter()
            .any(|z| z.name.trim().eq_ignore_ascii_case(&name))
        {
            return name;
        }
    }
    unreachable!()
}

fn with_config_mut(
    app: &AppHandle,
    edit: impl FnOnce(&mut AppConfig) -> Result<(), String>,
) -> Result<AppConfig, String> {
    let state = app.state::<AppState>();
    let mut guard = state.config.lock().unwrap();
    let config = match &mut *guard {
        Some(Ok(config)) => config,
        Some(Err(message)) => return Err(message.clone()),
        None => return Err("config not loaded yet".into()),
    };
    edit(config)?;
    save_config(config, &state.config_path)?;
    Ok(config.clone())
}

// ---- Commands invoked from the Results Window and the picker ----

/// Runs a shell operation on a blocking worker thread and awaits its
/// result. Window creation and window manipulation dispatch through the
/// main event loop and block the caller until that loop pumps — calling
/// them from a synchronous command deadlocks forever: sync commands run
/// ON the main thread, `WebviewWindowBuilder::build()` waits for the very
/// loop that is blocked, the invoke never settles, nothing errors, and
/// the button that issued it stays frozen. Every command path that can
/// create or move the overlay/picker windows goes through here (or
/// [`spawn_off_main`] when no answer is needed).
async fn run_off_main<T, F>(app: AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || f(&app))
        .await
        .map_err(|e| format!("background task failed: {e}"))
        .and_then(|r| r)
}

/// Fire-and-forget variant of [`run_off_main`] for the main-thread event
/// handlers (tray menu, global shortcuts): they cannot await, so failures
/// surface as a visible warning instead of a lost Result.
fn spawn_off_main<F>(app: &AppHandle, f: F)
where
    F: FnOnce(&AppHandle) -> Result<(), String> + Send + 'static,
{
    let task_app = app.clone();
    let warn_app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(message) = tauri::async_runtime::spawn_blocking(move || f(&task_app))
            .await
            .map_err(|e| format!("background task failed: {e}"))
            .and_then(|r| r)
        {
            let state = warn_app.state::<AppState>();
            push_warning(&state, message);
        }
    });
}

#[tauri::command]
async fn start_translation(app: AppHandle) -> Result<(), String> {
    run_off_main(app, start_loop).await
}

#[tauri::command]
async fn stop_translation(app: AppHandle) {
    // stop_loop hides overlay windows — off the main thread, see run_off_main.
    let _ = tauri::async_runtime::spawn_blocking(move || stop_loop(&app)).await;
}

#[tauri::command]
fn get_status(app: AppHandle) -> Status {
    status_for(&app)
}

#[tauri::command]
fn toggle_pause_translation(app: AppHandle) {
    toggle_pause(&app)
}

#[tauri::command]
fn rename_zone(app: AppHandle, id: String, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Zone name must not be empty".into());
    }
    let state = app.state::<AppState>();
    // Pre-check against the current zones so a rejected edit never lands in
    // the in-memory config.
    {
        let guard = state.config.lock().unwrap();
        if let Some(Ok(config)) = &*guard {
            if config
                .zones
                .iter()
                .any(|z| z.id != id && z.name.trim().eq_ignore_ascii_case(&name))
            {
                return Err(format!("A zone named \"{name}\" already exists"));
            }
        }
    }
    let config = with_config_mut(&app, |config| {
        let zone = config
            .zones
            .iter_mut()
            .find(|z| z.id == id)
            .ok_or_else(|| "zone not found (was it deleted elsewhere?)".to_string())?;
        zone.name = name;
        Ok(())
    })?;
    let _ = state.zones_tx.send(active_zones(&app, &config));
    apply_state(&app);
    Ok(())
}

#[tauri::command]
fn delete_zone(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = with_config_mut(&app, |config| {
        let before = config.zones.len();
        config.zones.retain(|z| z.id != id);
        if config.zones.len() == before {
            return Err("zone not found (was it deleted elsewhere?)".into());
        }
        Ok(())
    })?;
    let _ = state.zones_tx.send(active_zones(&app, &config));
    // Its translations must not linger on the overlay either.
    crate::overlay::clear_zone(&app, &id);
    apply_state(&app);
    Ok(())
}

#[tauri::command]
async fn open_zone_picker(app: AppHandle, mode: String) -> Result<(), String> {
    let mode = match mode.as_str() {
        "zone" => PickerMode::Zone,
        "oneshot" => PickerMode::OneShot,
        other => return Err(format!("unknown picker mode \"{other}\"")),
    };
    run_off_main(app, move |app| open_picker(app, mode)).await
}

/// The picker overlay asks once it has loaded what session it is serving.
#[tauri::command]
fn get_picker_mode(app: AppHandle) -> String {
    match *app.state::<AppState>().picker_mode.lock().unwrap() {
        Some(PickerMode::Zone) => "zone",
        Some(PickerMode::OneShot) => "oneshot",
        None => "none",
    }
    .to_string()
}

/// The picker overlay confirms a drag: coordinates are CSS px relative to
/// the overlay's viewport (= the primary monitor's top-left).
#[tauri::command]
async fn commit_zone_drag(
    app: AppHandle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    run_off_main(app, move |app| {
        commit_zone_drag_inner(app, x, y, width, height)
    })
    .await
}

/// Sync body of [`commit_zone_drag`]: closes the picker window, and in the
/// zone branch may start the loop (which creates the overlay windows) —
/// all off the main thread.
fn commit_zone_drag_inner(
    app: &AppHandle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(mode) = state.picker_mode.lock().unwrap().take() else {
        return Err("no capture session is open".into());
    };

    // A click without a drag cancels the session (standard snip behavior).
    if width < MIN_DRAG_CSS_PX || height < MIN_DRAG_CSS_PX {
        close_picker(app);
        return Ok(());
    }
    let monitor = primary_monitor(app)?;
    let region = region_from_css(&monitor, x, y, width, height);

    match mode {
        PickerMode::Zone => {
            // Persist the zone first so a failed save keeps the overlay open
            // with the error visible in it. The picker covers the primary
            // monitor, so the new zone remembers it.
            let name = monitor_name(&monitor);
            let config = with_config_mut(app, |config| {
                config.zones.push(Zone {
                    id: format!(
                        "z{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0)
                    ),
                    name: next_zone_name(config),
                    monitor: name.clone(),
                    region,
                });
                Ok(())
            })?;
            close_picker(app);
            let _ = state.zones_tx.send(active_zones(app, &config));
            // Zones are what the loop watches: creating the first one starts
            // translation (matches the tray's Start).
            if state.running.lock().unwrap().is_none() {
                if let Err(e) = start_loop(app) {
                    state.warnings.lock().unwrap().push(format!(
                        "failed to start translation after creating a zone: {e}"
                    ));
                }
            }
            apply_state(app);
            Ok(())
        }
        PickerMode::OneShot => {
            // Fallible parts (config, engine) resolve before the overlay
            // closes, so failures stay visible in it.
            let config = match &*state.config.lock().unwrap() {
                Some(Ok(config)) => config.clone(),
                Some(Err(message)) => return Err(message.clone()),
                None => return Err("config not loaded yet".into()),
            };
            let pipeline = build_pipeline(&config)?;
            let exclusions = active_exclusions(&config);
            close_picker(app);
            run_one_shot(app, pipeline, region, exclusions);
            Ok(())
        }
    }
}

/// The picker overlay cancelled (Esc / click without drag).
#[tauri::command]
async fn cancel_zone_drag(app: AppHandle) {
    // close_picker destroys the picker window — off the main thread.
    let _ = tauri::async_runtime::spawn_blocking(move || close_picker(&app)).await;
}

/// The overlay page pulls the current style once on load (the push path is
/// `overlay-style` events). A broken config still yields usable defaults —
/// the overlay must never go blank because of config trouble.
#[tauri::command]
fn get_text_style(app: AppHandle) -> TextStyleDto {
    match &*app.state::<AppState>().config.lock().unwrap() {
        Some(Ok(config)) => TextStyleDto::from(&config.text_style),
        _ => TextStyleDto::default(),
    }
}

/// The Results Window's Text Style panel persists a new style and pushes it
/// to the overlays live. Pre-validated against core's rules so a rejected
/// edit never lands in the in-memory config (same pattern as rename_zone).
#[tauri::command]
fn set_text_style(app: AppHandle, style: TextStyleDto) -> Result<(), String> {
    let text_style = TextStyle::from(style);
    text_style.validate().map_err(|e| e.to_string())?;
    with_config_mut(&app, |config| {
        config.text_style = text_style.clone();
        Ok(())
    })?;
    crate::overlay::broadcast_style(&app, &text_style);
    apply_state(&app);
    Ok(())
}

// ---- Adjust Mode, capture target, Excluded Blocks (ticket 04) ----

/// Alt+A / tray / Results Window button.
#[tauri::command]
async fn toggle_adjust_mode(app: AppHandle) {
    // set_adjust_mode creates/moves/hides overlay windows — off the main
    // thread, see run_off_main.
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let active = *app.state::<AppState>().adjust_mode.lock().unwrap();
        set_adjust_mode(&app, !active);
    })
    .await;
}

/// Switches the capture target (and, for full-screen, the watched monitor).
/// While the loop runs it is retargeted live; its old pills are cleared.
#[tauri::command]
async fn set_capture_mode(app: AppHandle, mode: String, monitor: String) -> Result<(), String> {
    let target = match mode.as_str() {
        "zones" => CaptureMode::Zones,
        "fullscreen" => CaptureMode::FullScreen,
        other => return Err(format!("unknown capture mode \"{other}\"")),
    };
    run_off_main(app, move |app| set_capture_mode_inner(app, target, Some(monitor))).await
}

/// Shared by the tray item (which passes no monitor — it only flips the
/// target) and the [`set_capture_mode`] command.
fn set_capture_mode_inner(
    app: &AppHandle,
    target: CaptureMode,
    monitor: Option<String>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = with_config_mut(app, |config| {
        config.mode.target = target;
        // A monitor choice only matters in full-screen mode; switching to
        // zones mode must not wipe the remembered monitor.
        if let Some(monitor) = monitor {
            if matches!(target, CaptureMode::FullScreen) {
                config.mode.fullscreen_monitor = monitor.trim().to_string();
            }
        }
        Ok(())
    })?;
    let _ = state.zones_tx.send(active_zones(app, &config));
    let _ = state.exclusions_tx.send(active_exclusions(&config));
    // Pills from the previous target must not linger: a zone pill and a
    // full-screen pill for the same text are different display entries.
    crate::overlay::clear_all(app);
    // The overlays' adjust view follows the target switch too.
    refresh_adjust_overlays(app);
    apply_state(app);
    Ok(())
}

/// A zone rect dropped in Adjust Mode: global physical px, converted from
/// CSS by the overlay page. The loop picks the new region up on its next
/// tick without losing per-block tracking (same channel as zone creation).
#[tauri::command]
async fn update_zone_region(
    app: AppHandle,
    id: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Zone width and height must be greater than zero".into());
    }
    run_off_main(app, move |app| {
        let state = app.state::<AppState>();
        let config = with_config_mut(app, |config| {
            let zone = config
                .zones
                .iter_mut()
                .find(|z| z.id == id)
                .ok_or_else(|| "zone not found (was it deleted elsewhere?)".to_string())?;
            zone.region = Region {
                x,
                y,
                width,
                height,
            };
            Ok(())
        })?;
        let _ = state.zones_tx.send(active_zones(app, &config));
        // Repositions/re-creates the overlay windows while adjusting.
        refresh_adjust_overlays(app);
        apply_state(app);
        Ok(())
    })
    .await
}

/// The overlay page reports a clicked block (its OCR box, global physical
/// px) as an Excluded Block. Adjust Mode is on and full-screen is the
/// target when the page offers the click; the shell trusts both.
#[tauri::command]
fn exclude_block(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    label: String,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let monitor = monitor_name_at(&app, x + width as i32 / 2, y + height as i32 / 2);
    let exclusion = ExcludedBlock {
        id: format!(
            "x{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ),
        monitor,
        region: Region {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        },
        label: label.trim().to_string(),
    };
    let config = with_config_mut(&app, |config| {
        config.excluded_blocks.push(exclusion.clone());
        Ok(())
    })?;
    // Active immediately in full-screen mode (the orchestrator stops
    // translating the position on its next pass).
    let _ = state.exclusions_tx.send(active_exclusions(&config));
    apply_state(&app);
    Ok(())
}

/// Clears one exclusion from the Results Window's list: the block at that
/// position is translated again (its dedup state was dropped while
/// excluded, so the very next pass re-translates it).
#[tauri::command]
fn delete_excluded_block(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = with_config_mut(&app, |config| {
        let before = config.excluded_blocks.len();
        config.excluded_blocks.retain(|e| e.id != id);
        if config.excluded_blocks.len() == before {
            return Err("excluded block not found (was it cleared elsewhere?)".into());
        }
        Ok(())
    })?;
    let _ = state.exclusions_tx.send(active_exclusions(&config));
    apply_state(&app);
    Ok(())
}

/// The monitor whose bounds contain the point, by name; empty when none
/// matches (the label is display-only).
fn monitor_name_at(app: &AppHandle, x: i32, y: i32) -> String {
    app.available_monitors()
        .unwrap_or_default()
        .into_iter()
        .find(|m| {
            let origin = m.position();
            let size = m.size();
            x >= origin.x
                && x < origin.x + size.width as i32
                && y >= origin.y
                && y < origin.y + size.height as i32
        })
        .and_then(|m| m.name().map(|n| n.to_string()))
        .unwrap_or_default()
}

/// One OCR+translate pass into the Results Window, independent of the
/// running loop (it works while stopped, paused, or running). All fallible
/// setup (config, engine) happens in `commit_zone_drag` before the picker
/// closes, so this only spawns.
fn run_one_shot(
    app: &AppHandle,
    pipeline: Pipeline,
    region: Region,
    exclusions: Vec<ExcludedBlock>,
) {
    // A one-shot pass is outside the zone loop: fresh, empty channels. The
    // exclusions in force match the current capture target, so a full-screen
    // user one-shotting an area gets the same hiding behavior as the loop.
    let (_zones_tx, zones_rx) = watch::channel(Vec::new());
    let (_paused_tx, paused_rx) = watch::channel(false);
    let (_excluded_tx, excluded_rx) = watch::channel(exclusions);
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    let orchestrator = Orchestrator::new(
        pipeline.capture,
        pipeline.ocr,
        pipeline.translate,
        Arc::new(SystemClock),
        pipeline.config,
        pipeline.glossary,
        zones_rx,
        paused_rx,
        excluded_rx,
        events_tx,
    );

    tauri::async_runtime::spawn(async move {
        orchestrator.capture_once(region).await;
        // events_tx drops with the orchestrator: the forwarder drains and exits.
    });
    // One-shot events never reach the overlay (Results Window only), so the
    // forwarder needs no overlay list.
    spawn_forwarder(app, events_rx, Vec::new());
}
