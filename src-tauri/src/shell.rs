//! The Tauri shell: tray-first control, global hotkeys (rebindable live from
//! Settings → Hotkeys, or via config), the Capture Zone picker overlay
//! (Alt+Z / Alt+Q), Adjust Mode (Alt+A: zone drag/resize + block exclusion,
//! ticket 04), full-screen mode with monitor selection, the per-monitor
//! In-place Overlay (ticket 03), the Results Window event bridge, the
//! config file lifecycle, single-instance behavior, and the cloud-engine
//! privacy badge (ticket 06). Thin adapter — exercised by the manual smoke
//! checklist.

use langception_core::config::{AppConfig, CaptureMode, HotkeySettings, TextStyle};
use langception_core::glossary::Glossary;
use langception_core::google::GoogleTranslateEngine;
use langception_core::orchestrator::{LoopConfig, LoopEvent, Orchestrator, SystemClock};
use langception_core::ports::{ExcludedBlock, Region, SourceLanguage, Zone, TARGET_LANGUAGE};
use langception_core::profile::{GenderlessOverride, Profile, ProfileZone};
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
use tauri_plugin_autostart::ManagerExt as _;
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
#[derive(Debug, Clone, PartialEq)]
enum PickerMode {
    /// Drag creates a persistent Capture Zone.
    Zone,
    /// Drag runs a single OCR+translate pass (One-shot Capture).
    OneShot,
    /// Drag adds a window-relative Capture Zone to the named Profile's app
    /// (ticket 07): the rectangle is stored relative to the app's window,
    /// so it follows the window around the screen.
    ProfileZone { profile_id: String },
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
    /// How long a changed block must hold stable before it is translated
    /// (ticket 05).
    pub stability_ms: u32,
    /// The Genderless Mode global toggle (ticket 08); a Profile's override
    /// (Settings → Profiles) layers on top of this per app.
    pub genderless: bool,
    pub glossary_terms: usize,
    pub hotkeys: HotkeyStatus,
    pub text_style: TextStyleDto,
    pub target_language: &'static str,
    pub config_path: String,
    /// The active Translate Engine while a pipeline is in flight, but only
    /// when it sends screen text off-machine (`is_cloud`). Drives the
    /// privacy badge in the Results Window and the tray tooltip (ticket 06).
    pub privacy_engine: Option<String>,
    /// True on the very first launch (no config file existed when the app
    /// started): the settings window opens with the setup wizard.
    pub first_run: bool,
    /// Whether Langception launches at Windows sign-in (OS-managed, not in
    /// config.toml).
    pub start_with_windows: bool,
    /// The per-app Profiles (ticket 07), with an `active` flag on each.
    pub profiles: Vec<ProfileStatus>,
    /// The id of the Profile currently matched to the foreground app, or
    /// `None` while the global fallback target applies.
    pub active_profile_id: Option<String>,
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

/// One per-app Profile (ticket 07), as the Settings → Profiles tab reads
/// and writes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    pub id: String,
    pub name: String,
    pub exe: String,
    pub language: String,
    /// The Profile's engine choice; `None` = inherit the global engine.
    pub engine: Option<String>,
    /// "inherit" | "on" | "off".
    pub genderless: String,
    pub glossary_terms: usize,
    pub zones: Vec<ProfileZoneStatus>,
    /// Whether this Profile is active right now (its app owns the
    /// foreground window).
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileZoneStatus {
    pub id: String,
    pub name: String,
    /// Window-relative physical px ("x,y width×height").
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub(crate) struct RunningLoop {
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

/// Shared app state. `pub(crate)` fields: the settings module (ticket 06)
/// mutates config and reads runtime flags through the shell's helpers.
pub(crate) struct AppState {
    /// `Some(Err)` = the config file is broken; surfaced in the Results
    /// Window, never silently.
    pub(crate) config: Mutex<Option<Result<AppConfig, String>>>,
    pub(crate) config_path: PathBuf,
    pub(crate) running: Mutex<Option<RunningLoop>>,
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
    pub(crate) warnings: Mutex<Vec<String>>,
    registered: Mutex<RegisteredHotkeys>,
    toggle_item: Mutex<Option<MenuItem<Wry>>>,
    pause_item: Mutex<Option<MenuItem<Wry>>>,
    adjust_item: Mutex<Option<MenuItem<Wry>>>,
    fullscreen_item: Mutex<Option<CheckMenuItem<Wry>>>,
    tray: Mutex<Option<TrayIcon<Wry>>>,
    /// `Some` while the picker overlay is open, recording why.
    picker_mode: Mutex<Option<PickerMode>>,
    /// The active pipeline's cloud Translate Engine display name (`None` =
    /// no pipeline in flight, or the engine is local). Set when the loop or
    /// a one-shot starts, cleared when they stop — the privacy badge reads
    /// this (ticket 06).
    pub(crate) cloud_engine: Mutex<Option<String>>,
    /// True when no config file existed at startup (first launch): the
    /// settings window opens straight into the setup wizard.
    pub(crate) first_run: bool,
    /// The Profile currently matched to the foreground app, by id (`None` =
    /// the global fallback target). Maintained by the profile watcher and
    /// recomputed wherever the loop target is pushed; the Status snapshot
    /// and the profile commands read it.
    pub(crate) active_profile: Mutex<Option<String>>,
    /// The active Profile app's foreground window rect as of the last
    /// target computation — the watcher compares against it to follow
    /// window moves and resizes.
    active_window: Mutex<Option<Region>>,
}

pub fn run() {
    let (zones_tx, _) = watch::channel(Vec::new());
    let (paused_tx, _) = watch::channel(false);
    let (exclusions_tx, _) = watch::channel(Vec::new());

    // First launch = no config file yet, detected BEFORE load_config writes
    // the template: the setup wizard shows exactly once.
    let config_path = config_file_path();
    let first_run = !config_path.exists();

    tauri::Builder::default()
        // Single-instance MUST be the first registered plugin: a second
        // launch never reaches Builder::run — it hands off here and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Surface the running instance: focus the Results Window when it
            // exists, otherwise open the settings window.
            let app = app.clone();
            spawn_off_main(&app, |app| {
                if let Some(window) = app.get_webview_window("results") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                } else {
                    crate::settings::open_settings(app)?;
                }
                Ok(())
            });
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState {
            config: Mutex::new(None),
            config_path,
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
            cloud_engine: Mutex::new(None),
            first_run,
            active_profile: Mutex::new(None),
            active_window: Mutex::new(None),
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
            set_text_style,
            crate::settings::open_settings_window,
            crate::settings::get_ocr_pack_status,
            crate::settings::open_language_pack_settings,
            crate::settings::get_about,
            crate::settings::set_general,
            crate::settings::set_start_with_windows,
            crate::settings::set_hotkeys,
            crate::settings::list_glossary,
            crate::settings::add_glossary_entry,
            crate::settings::update_glossary_entry,
            crate::settings::delete_glossary_entry,
            crate::settings::import_glossary,
            crate::settings::export_glossary,
            crate::settings::list_running_apps,
            crate::settings::create_profile,
            crate::settings::update_profile,
            crate::settings::delete_profile,
            crate::settings::list_profile_glossary,
            crate::settings::add_profile_glossary_entry,
            crate::settings::update_profile_glossary_entry,
            crate::settings::delete_profile_glossary_entry,
            crate::settings::delete_profile_zone,
            open_config_folder
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
            // The settings window hides instead of closing (tray-first: the
            // app keeps running); reopening is instant and keeps page state.
            // The hide itself runs off the main thread — window manipulation
            // from inside an event handler can deadlock (see run_off_main).
            if window.label() == crate::settings::SETTINGS_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let app = window.app_handle().clone();
                    spawn_off_main(&app, |app| {
                        if let Some(window) =
                            app.get_webview_window(crate::settings::SETTINGS_LABEL)
                        {
                            let _ = window.hide();
                        }
                        Ok(())
                    });
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // Load (or first-run create) the config; a broken file must show
            // up in the Results Window, not vanish. First launch (no config
            // file yet) opens straight into the setup wizard.
            let state = handle.state::<AppState>();
            let config_path = state.config_path.clone();
            *state.config.lock().unwrap() = Some(load_config(&config_path));

            build_tray(&handle)?;
            install_hotkeys(&handle)?;
            spawn_profile_watcher(&handle);

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

            if state.first_run {
                // Window creation runs off the main thread (run_off_main's
                // rule); the page decides via status.firstRun to show the
                // wizard view first.
                spawn_off_main(&handle, crate::settings::open_settings);
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
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
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
            &settings_item,
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
            "settings" => spawn_off_main(app, crate::settings::open_settings),
            "open-config" => open_config_folder_inner(app),
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

    for warning in register_hotkeys(app, &hotkeys) {
        state.warnings.lock().unwrap().push(warning);
    }
    Ok(())
}

/// Binds every configured hotkey, after dropping all previous bindings (so
/// a live rebind from Settings → Hotkeys stops firing the old accelerator).
/// Returns one warning per hotkey that failed to parse or register; the
/// others keep working.
pub(crate) fn register_hotkeys(app: &AppHandle, hotkeys: &HotkeySettings) -> Vec<String> {
    let mut warnings = Vec::new();
    let _ = app.global_shortcut().unregister_all();

    let mut registered = RegisteredHotkeys::default();
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
    *app.state::<AppState>().registered.lock().unwrap() = registered;
    warnings
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
pub(crate) fn apply_state(app: &AppHandle) {
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
        // Privacy badge (ticket 06): whenever a pipeline is in flight and
        // its Translate Engine sends screen text off-machine, the tooltip
        // says so — the user never has to open a window to know.
        let privacy_note = match &*state.cloud_engine.lock().unwrap() {
            Some(engine) if running => format!(" · ☁ {engine}"),
            _ => String::new(),
        };
        let tooltip = if !running {
            "Langception — stopped"
        } else if paused {
            "Langception — paused"
        } else {
            "Langception — running"
        };
        let _ = tray.set_tooltip(Some(format!("{tooltip}{mode_note}{privacy_note}")));
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
        stability_ms,
        genderless,
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
            config.capture.stability_ms,
            config.genderless,
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
            false,
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
            false,
            0,
            default_hotkeys,
            TextStyleDto::default(),
        ),
    };

    let privacy_engine = state.cloud_engine.lock().unwrap().clone();
    let start_with_windows = app.autolaunch().is_enabled().unwrap_or(false);
    let active_profile_id = state.active_profile.lock().unwrap().clone();
    let profiles = match &*state.config.lock().unwrap() {
        Some(Ok(config)) => config
            .profiles
            .iter()
            .map(|profile| ProfileStatus {
                id: profile.id.clone(),
                name: profile.name.clone(),
                exe: profile.exe.clone(),
                language: profile.language.code().to_string(),
                engine: profile.engine.clone(),
                genderless: genderless_code(profile.genderless).to_string(),
                glossary_terms: profile.glossary.0.len(),
                zones: profile
                    .zones
                    .iter()
                    .map(|zone| ProfileZoneStatus {
                        id: zone.id.clone(),
                        name: zone.name.clone(),
                        region: format!(
                            "{},{} {}×{}",
                            zone.region.x, zone.region.y, zone.region.width, zone.region.height
                        ),
                    })
                    .collect(),
                active: active_profile_id.as_deref() == Some(profile.id.as_str()),
            })
            .collect(),
        Some(Err(_)) | None => Vec::new(),
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
        stability_ms,
        genderless,
        glossary_terms,
        hotkeys,
        text_style,
        target_language: TARGET_LANGUAGE,
        config_path,
        privacy_engine,
        first_run: state.first_run,
        start_with_windows,
        profiles,
        active_profile_id,
    }
}

/// The config-file spelling of [`GenderlessOverride`].
fn genderless_code(override_: GenderlessOverride) -> &'static str {
    match override_ {
        GenderlessOverride::Inherit => "inherit",
        GenderlessOverride::On => "on",
        GenderlessOverride::Off => "off",
    }
}

/// What the loop should watch RIGHT NOW: the active Profile's app-window
/// target when one matches the foreground window's executable, the global
/// capture target (zones / full-screen monitor) otherwise. Everything the
/// pipeline reads at start follows it: Source Language, Glossary layer,
/// engine choice, Genderless override, Excluded Blocks.
struct EffectiveTarget {
    profile_id: Option<String>,
    /// The matched app's foreground window (screen px); `Some` iff a
    /// Profile matched.
    window: Option<Region>,
    zones: Vec<Zone>,
    exclusions: Vec<ExcludedBlock>,
    language: SourceLanguage,
    glossary: Glossary,
    /// The Profile's engine choice (`None` = the global engine).
    engine: Option<String>,
    /// The effective Genderless Mode (ticket 08): the Profile's override,
    /// or the global toggle — the orchestrator's Thai scrubber consumes it
    /// via the loop config.
    genderless: bool,
}

/// Computes the target for the current foreground app against `config`.
fn effective_target(app: &AppHandle, config: &AppConfig) -> EffectiveTarget {
    let foreground = crate::foreground::foreground_app();
    let profile = foreground
        .as_ref()
        .and_then(|fg| Profile::resolve(&config.profiles, &fg.exe));
    if let (Some(profile), Some(fg)) = (profile, foreground) {
        return EffectiveTarget {
            profile_id: Some(profile.id.clone()),
            window: Some(fg.rect),
            zones: profile.capture_zones(fg.rect),
            // Excluded Blocks are a full-screen-mode concept (CONTEXT.md);
            // a Profile's window target has none.
            exclusions: Vec::new(),
            language: profile.language,
            glossary: profile.effective_glossary(&config.glossary),
            engine: profile.engine.clone(),
            genderless: profile.effective_genderless(config.genderless),
        };
    }
    EffectiveTarget {
        profile_id: None,
        window: None,
        zones: active_zones(app, config),
        exclusions: active_exclusions(config),
        language: config.source.language,
        glossary: config.glossary.clone(),
        engine: None,
        genderless: config.genderless,
    }
}

/// The real Windows pipeline for translation passes: adapters wired to the
/// effective Source Language and the global cadence, plus the effective
/// Glossary layer.
struct Pipeline {
    config: LoopConfig,
    glossary: Glossary,
    capture: Arc<dyn langception_core::ports::CaptureSource>,
    ocr: Arc<dyn langception_core::ports::OcrEngine>,
    translate: Arc<dyn langception_core::ports::TranslateEngine>,
}

fn build_pipeline(config: &AppConfig, target: &EffectiveTarget) -> Result<Pipeline, String> {
    Ok(Pipeline {
        config: LoopConfig {
            source_language: target.language,
            cadence: Duration::from_millis(config.capture.cadence_ms as u64),
            stability: Duration::from_millis(config.capture.stability_ms as u64),
            genderless: target.genderless,
        },
        glossary: target.glossary.clone(),
        capture: Arc::new(crate::capture::GdiCaptureSource::new()),
        ocr: Arc::new(crate::ocr::WindowsOcrEngine::new(target.language)),
        // The Profile's engine choice (None = inherit global). This build
        // offers only the keyless Google engine (ADR-0002); tickets 09-11
        // extend the match. Config validation rejects unknown names, so
        // they cannot reach here from a valid config.
        translate: match target.engine.as_deref().map(str::trim) {
            None | Some("google") => Arc::new(
                GoogleTranslateEngine::new()
                    .map_err(|e| format!("failed to initialize Google engine: {e}"))?,
            ),
            Some(other) => return Err(format!("unknown engine \"{other}\"")),
        },
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

    // A fresh loop starts unpaused, watching the effective target's regions
    // (the active Profile's app window, or the global zones / full-screen
    // monitor) and the Excluded Blocks in force. Subscribe BEFORE sending: a
    // watch::send with zero receivers does not store the value, so a later
    // subscriber would see an empty list.
    let target = effective_target(app, &config);
    *state.active_profile.lock().unwrap() = target.profile_id.clone();
    *state.active_window.lock().unwrap() = target.window;
    let zones_rx = state.zones_tx.subscribe();
    let paused_rx = state.paused_tx.subscribe();
    let excluded_rx = state.exclusions_tx.subscribe();
    let _ = state.paused_tx.send(false);
    *state.paused.lock().unwrap() = false;
    let pipeline = build_pipeline(&config, &target)?;
    let _ = state.zones_tx.send(target.zones);
    let _ = state.exclusions_tx.send(target.exclusions);
    // Privacy badge (ticket 06): record whether the active engine sends
    // screen text off-machine while the loop runs.
    *state.cloud_engine.lock().unwrap() = if pipeline.translate.is_cloud() {
        Some(pipeline.translate.name().to_string())
    } else {
        None
    };
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    // Per-monitor overlays show the translations in place. Creation failure
    // never stops the loop: the Results Window still works, and the warning
    // stays visible.
    let overlays = match crate::overlay::ensure_overlays(app) {
        Ok(overlays) => overlays,
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
    // Nothing translates after this: the privacy badge goes away with the
    // pipeline.
    *state.cloud_engine.lock().unwrap() = None;
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
pub(crate) fn push_warning(state: &AppState, message: String) {
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

/// The About tab's button and the tray item share this.
#[tauri::command]
fn open_config_folder(app: AppHandle) {
    open_config_folder_inner(&app);
}

fn open_config_folder_inner(app: &AppHandle) {
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
        if let Some(monitor) = monitors
            .iter()
            .find(|m| m.name().map(|n| n.as_str()) == Some(wanted))
        {
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
                    let _ =
                        app.emit_to(&label, "overlay-adjust", &adjust_payload_for(app, &monitor));
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
            .push(format!(
                "could not exclude the picker from capture: {message}"
            ));
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

/// Runs one edit against the in-memory config and persists it. The edit
/// applies to a clone first; the result is only committed to memory AND
/// file when the serialized form still parses and validates — a rejected
/// edit (bad cadence, duplicate name, …) never lands anywhere, and the
/// caller's error is shown in whichever surface asked.
pub(crate) fn with_config_mut<T>(
    app: &AppHandle,
    edit: impl FnOnce(&mut AppConfig) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let mut guard = state.config.lock().unwrap();
    let config = match &mut *guard {
        Some(Ok(config)) => config,
        Some(Err(message)) => return Err(message.clone()),
        None => return Err("config not loaded yet".into()),
    };
    let mut edited = config.clone();
    let value = edit(&mut edited)?;
    let text = edited
        .to_toml_string()
        .map_err(|e| format!("edit rejected: {e}"))?;
    AppConfig::from_toml_str(&text).map_err(|e| format!("edit rejected: {e}"))?;
    std::fs::write(&state.config_path, &text)
        .map_err(|e| format!("cannot write {}: {e}", state.config_path.display()))?;
    *config = edited;
    Ok(value)
}

/// A clone of the current valid config (`Err` when broken/not yet loaded).
pub(crate) fn current_config(app: &AppHandle) -> Result<AppConfig, String> {
    match &*app.state::<AppState>().config.lock().unwrap() {
        Some(Ok(config)) => Ok(config.clone()),
        Some(Err(message)) => Err(message.clone()),
        None => Err("config not loaded yet".into()),
    }
}

/// Applies an edit that the running loop only reads at start (source
/// language, cadence, stability, glossary): restarts the loop so the change
/// visibly applies, resetting per-block tracking (a language change rebuilds
/// the OCR engine anyway). No-op while stopped.
pub(crate) fn restart_loop_if_running(app: &AppHandle) {
    let running = app.state::<AppState>().running.lock().unwrap().is_some();
    if !running {
        return;
    }
    stop_loop(app);
    if let Err(message) = start_loop(app) {
        push_warning(
            &app.state::<AppState>(),
            format!("could not restart translation after a settings change: {message}"),
        );
    }
}

/// How often the profile watcher polls the foreground window. GetForegroundWindow
/// plus a limited-info process query cost microseconds — 4× per second is
/// far under any perceptible delay for focus switches.
const PROFILE_POLL_MS: u64 = 250;

/// Spawns the Profile watcher (ticket 07): it runs for the whole app
/// lifetime, matching the foreground window's executable against the
/// configured Profiles. A match change restarts the running loop (the
/// pipeline's language/glossary/engine and the watched regions all follow
/// the Profile — and stop_loop clears the previous app's overlays, so no
/// stale pills linger); a same-Profile window move re-pushes the
/// window-relative zones. While translation is stopped it only updates the
/// status's active-Profile id.
fn spawn_profile_watcher(app: &AppHandle) {
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(PROFILE_POLL_MS)).await;
            let tick_app = task_app.clone();
            // The tick queries windows and (on a match change) restarts the
            // loop — both dispatch through the main event loop and block
            // until it pumps, so they must run off the main thread (see
            // run_off_main).
            if let Err(message) =
                tauri::async_runtime::spawn_blocking(move || profile_watch_tick(&tick_app))
                    .await
                    .map_err(|e| format!("background task failed: {e}"))
                    .and_then(|r| r)
            {
                let state = task_app.state::<AppState>();
                push_warning(&state, message);
            }
        }
    });
}

/// One watcher pass: resolve the foreground app against the Profiles and
/// apply what changed. Failures surface as warnings, never silently.
fn profile_watch_tick(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let foreground = crate::foreground::foreground_app();

    // Which profile matches the foreground app right now (None = global
    // fallback target). A broken or not-yet-loaded config matches nothing.
    let matched: Option<(String, Region, Vec<Zone>)> = {
        let guard = state.config.lock().unwrap();
        match (&*guard, &foreground) {
            (Some(Ok(config)), Some(fg)) => Profile::resolve(&config.profiles, &fg.exe)
                .map(|profile| (profile.id.clone(), fg.rect, profile.capture_zones(fg.rect))),
            _ => None,
        }
    };
    let new_id = matched.as_ref().map(|(id, _, _)| id.clone());

    let changed = {
        let mut active = state.active_profile.lock().unwrap();
        let changed = *active != new_id;
        if changed {
            *active = new_id;
        }
        changed
    };

    if changed {
        // Activation/deactivation swaps everything the pipeline reads at
        // start (language, glossary, engine, zones): restart the running
        // loop — start_loop recomputes the effective target itself, so a
        // foreground change between here and there still lands correctly.
        restart_loop_if_running(app);
        apply_state(app);
        return Ok(());
    }

    // Same profile (or still none): while a profile is active, follow its
    // window when it moves or resizes — its window-relative zones re-resolve
    // against the new rect, and pills at the old position must not linger.
    if let Some((_, rect, zones)) = matched {
        let mut window = state.active_window.lock().unwrap();
        if *window != Some(rect) {
            *window = Some(rect);
            drop(window);
            crate::overlay::clear_all(app);
            let _ = state.zones_tx.send(zones);
        }
    }
    Ok(())
}

/// Recomputes and pushes what the loop should watch right now — the active
/// Profile's window target when one matches the foreground app, the global
/// capture target otherwise. The single push path for every config edit
/// that can change zones or Excluded Blocks; the loop picks the new list
/// up on its next tick (per-zone tracking state survives where the regions
/// do). Zones that disappeared or moved have their pills cleared; zones
/// that are untouched keep theirs (adding a zone must not blank the rest).
fn push_active_target(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = current_config(app)?;
    let target = effective_target(app, &config);
    *state.active_profile.lock().unwrap() = target.profile_id.clone();
    *state.active_window.lock().unwrap() = target.window;
    let old = state.zones_tx.borrow().clone();
    for zone in &old {
        if !target
            .zones
            .iter()
            .any(|z| z.id == zone.id && z.region == zone.region)
        {
            crate::overlay::clear_zone(app, &zone.id);
        }
    }
    let _ = state.zones_tx.send(target.zones);
    let _ = state.exclusions_tx.send(target.exclusions);
    Ok(())
}

/// Applies an edit to one Profile: while that profile is active the loop
/// restarts so its pipeline and watched zones follow (the pipeline's
/// language/glossary are read at start); a status refresh happens either
/// way. Shared by every profile-editing command.
pub(crate) fn profile_edited(app: &AppHandle, profile_id: &str) {
    let state = app.state::<AppState>();
    let is_active = *state.active_profile.lock().unwrap() == Some(profile_id.to_string());
    if is_active {
        restart_loop_if_running(app);
    }
    apply_state(app);
}

// ---- Commands invoked from the Results Window and the picker ----

/// Runs a shell operation on a blocking worker thread and awaits its
/// result. Window creation and window manipulation dispatch through the
/// main event loop and block the caller until that loop pumps — calling
/// them from a synchronous command deadlocks forever: sync commands run
/// ON the main thread, `WebviewWindowBuilder::build()` waits for the very
/// loop that is blocked, the invoke never settles, nothing errors, and
/// the button that issued it stays frozen. Every command path that can
/// create or move the overlay/picker/settings windows goes through here
/// (or [`spawn_off_main`] when no answer is needed).
pub(crate) async fn run_off_main<T, F>(app: AppHandle, f: F) -> Result<T, String>
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
    with_config_mut(&app, |config| {
        let zone = config
            .zones
            .iter_mut()
            .find(|z| z.id == id)
            .ok_or_else(|| "zone not found (was it deleted elsewhere?)".to_string())?;
        zone.name = name;
        Ok(())
    })?;
    push_active_target(&app)?;
    apply_state(&app);
    Ok(())
}

#[tauri::command]
fn delete_zone(app: AppHandle, id: String) -> Result<(), String> {
    with_config_mut(&app, |config| {
        let before = config.zones.len();
        config.zones.retain(|z| z.id != id);
        if config.zones.len() == before {
            return Err("zone not found (was it deleted elsewhere?)".into());
        }
        Ok(())
    })?;
    push_active_target(&app)?;
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
        // "profile:<id>" — the drag becomes a window-relative Capture Zone
        // on the named Profile's app (Settings → Profiles).
        other => match other.strip_prefix("profile:") {
            Some(profile_id) if !profile_id.is_empty() => PickerMode::ProfileZone {
                profile_id: profile_id.to_string(),
            },
            _ => return Err(format!("unknown picker mode \"{other}\"")),
        },
    };
    run_off_main(app, move |app| open_picker(app, mode)).await
}

/// The picker overlay asks once it has loaded what session it is serving.
#[tauri::command]
fn get_picker_mode(app: AppHandle) -> String {
    match &*app.state::<AppState>().picker_mode.lock().unwrap() {
        Some(PickerMode::Zone) => "zone",
        Some(PickerMode::OneShot) => "oneshot",
        Some(PickerMode::ProfileZone { .. }) => "profile",
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
/// zone branches may start/restart the loop (which creates overlay
/// windows) — all off the main thread. A failed commit keeps the session
/// open: the picker page shows the error and the next drag commits.
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

    let result = commit_picked_region(app, &mode, x, y, width, height);
    if result.is_err() {
        *state.picker_mode.lock().unwrap() = Some(mode);
    }
    result
}

/// The committed rectangle (CSS px relative to the primary monitor) for the
/// session's mode. Consumes the session on success (the picker closes); on
/// failure the caller restores it.
fn commit_picked_region(
    app: &AppHandle,
    mode: &PickerMode,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let monitor = primary_monitor(app)?;
    let region = region_from_css(&monitor, x, y, width, height);

    match mode {
        PickerMode::Zone => {
            // Persist the zone first so a failed save keeps the overlay open
            // with the error visible in it. The picker covers the primary
            // monitor, so the new zone remembers it.
            let name = monitor_name(&monitor);
            with_config_mut(app, |config| {
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
            push_active_target(app)?;
            close_picker(app);
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
            // closes, so failures stay visible in it. The pass follows the
            // effective target: an active Profile's language and Glossary
            // layer, its exclusions (none) — the global settings otherwise.
            let config = match &*state.config.lock().unwrap() {
                Some(Ok(config)) => config.clone(),
                Some(Err(message)) => return Err(message.clone()),
                None => return Err("config not loaded yet".into()),
            };
            let target = effective_target(app, &config);
            let pipeline = build_pipeline(&config, &target)?;
            let exclusions = target.exclusions;
            close_picker(app);
            run_one_shot(app, pipeline, region, exclusions);
            Ok(())
        }
        PickerMode::ProfileZone { profile_id } => {
            // Fallible parts resolve before the picker closes, so failures
            // stay visible in it (it stays open for another drag).
            let profile_exe = {
                let guard = state.config.lock().unwrap();
                match &*guard {
                    Some(Ok(config)) => config
                        .profiles
                        .iter()
                        .find(|p| p.id == *profile_id)
                        .map(|p| p.exe.clone()),
                    _ => None,
                }
            }
            .ok_or_else(|| "profile not found (was it deleted?)".to_string())?;
            let window = crate::foreground::app_window_rect(&profile_exe).ok_or_else(|| {
                format!(
                    "\"{profile_exe}\" has no visible window right now — start the app (not minimized), then draw the zone"
                )
            })?;
            let rel = region_in_window(window, region)?;
            let zone_id = format!(
                "pz{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            with_config_mut(app, |config| {
                let profile = config
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == *profile_id)
                    .ok_or_else(|| "profile not found (was it deleted?)".to_string())?;
                profile.zones.push(ProfileZone {
                    id: zone_id.clone(),
                    name: next_profile_zone_name(profile),
                    region: rel,
                });
                Ok(())
            })?;
            close_picker(app);
            profile_edited(app, profile_id);
            Ok(())
        }
    }
}

/// Converts a drawn screen region into window-relative physical px for
/// storing in a Profile: relative to the app window's top-left, clamped
/// inside it. Errors when the rectangle does not intersect the window.
fn region_in_window(window: Region, region: Region) -> Result<Region, String> {
    let left = region.x.max(window.x);
    let top = region.y.max(window.y);
    let right = (region.x + region.width as i32).min(window.x + window.width as i32);
    let bottom = (region.y + region.height as i32).min(window.y + window.height as i32);
    let width = right - left;
    let height = bottom - top;
    if width <= 0 || height <= 0 {
        return Err("the drawn rectangle is entirely outside the app's window".into());
    }
    Ok(Region {
        x: left - window.x,
        y: top - window.y,
        width: width as u32,
        height: height as u32,
    })
}

/// The next free "Zone N" name within one Profile (existing names win).
fn next_profile_zone_name(profile: &Profile) -> String {
    for n in 1.. {
        let name = format!("Zone {n}");
        if !profile
            .zones
            .iter()
            .any(|z| z.name.trim().eq_ignore_ascii_case(&name))
        {
            return name;
        }
    }
    unreachable!()
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
    run_off_main(app, move |app| {
        set_capture_mode_inner(app, target, Some(monitor))
    })
    .await
}

/// Shared by the tray item (which passes no monitor — it only flips the
/// target) and the [`set_capture_mode`] command.
fn set_capture_mode_inner(
    app: &AppHandle,
    target: CaptureMode,
    monitor: Option<String>,
) -> Result<(), String> {
    with_config_mut(app, |config| {
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
    // Pills from the previous target must not linger: a zone pill and a
    // full-screen pill for the same text are different display entries.
    // push_active_target clears every zone that left the watched set (when
    // a Profile is active the global target switch changes nothing it
    // watches, and its pills legitimately survive).
    push_active_target(app)?;
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
        with_config_mut(app, |config| {
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
        push_active_target(app)?;
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
    with_config_mut(&app, |config| {
        config.excluded_blocks.push(exclusion.clone());
        Ok(())
    })?;
    // Active immediately in full-screen mode (the orchestrator stops
    // translating the position on its next pass).
    push_active_target(&app)?;
    apply_state(&app);
    Ok(())
}

/// Clears one exclusion from the Results Window's list: the block at that
/// position is translated again (its dedup state was dropped while
/// excluded, so the very next pass re-translates it).
#[tauri::command]
fn delete_excluded_block(app: AppHandle, id: String) -> Result<(), String> {
    with_config_mut(&app, |config| {
        let before = config.excluded_blocks.len();
        config.excluded_blocks.retain(|e| e.id != id);
        if config.excluded_blocks.len() == before {
            return Err("excluded block not found (was it cleared elsewhere?)".into());
        }
        Ok(())
    })?;
    push_active_target(&app)?;
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

    // A one-shot also sends screen text to the engine: the privacy badge
    // covers it too (cleared once the pass completes).
    *app.state::<AppState>().cloud_engine.lock().unwrap() = if pipeline.translate.is_cloud() {
        Some(pipeline.translate.name().to_string())
    } else {
        None
    };
    apply_state(app);

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

    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        orchestrator.capture_once(region).await;
        // events_tx drops with the orchestrator: the forwarder drains and exits.
        // Clear the privacy badge only if the loop is not running — a loop
        // started during the pass owns the badge now (stop_loop clears it).
        let state = task_app.state::<AppState>();
        if state.running.lock().unwrap().is_none() {
            *state.cloud_engine.lock().unwrap() = None;
            apply_state(&task_app);
        }
    });
    // One-shot events never reach the overlay (Results Window only), so the
    // forwarder needs no overlay list.
    spawn_forwarder(app, events_rx, Vec::new());
}
