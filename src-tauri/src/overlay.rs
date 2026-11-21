//! The In-place Overlay (CONTEXT.md): one transparent, always-on-top,
//! click-through window per monitor that draws each Text Block's Thai
//! translation at the block's screen position (ticket 03). Windows follow
//! the ADR-0001 recipe (`transparent` + `always_on_top` +
//! `set_ignore_cursor_events`); the `ui/overlay.html` page does the actual
//! rendering and receives events routed by [`OverlayRouter`].

use langception_core::config::TextStyle;
use langception_core::orchestrator::EventOrigin;
use langception_core::ports::TextBlock;
use serde::Serialize;
use std::collections::HashMap;
use tauri::{
    AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, Position, Size,
    WebviewUrl, WebviewWindowBuilder,
};

/// Overlay windows are named `overlay-0`, `overlay-1`, … — one per monitor,
/// indexed in `available_monitors()` order.
pub const OVERLAY_LABEL_PREFIX: &str = "overlay-";

/// Creates any missing per-monitor overlay windows, repositions them all to
/// their monitors (the layout may have changed since creation) and shows
/// them. Returns each window's label with its monitor, in the same order,
/// for an [`OverlayRouter`].
pub fn ensure_overlays(app: &AppHandle) -> Result<Vec<(String, Monitor)>, String> {
    let monitors = app
        .available_monitors()
        .map_err(|e| format!("cannot enumerate monitors: {e}"))?;
    let mut overlays = Vec::with_capacity(monitors.len());
    for (index, monitor) in monitors.iter().enumerate() {
        let label = format!("{OVERLAY_LABEL_PREFIX}{index}");
        let window = match app.get_webview_window(&label) {
            Some(existing) => existing,
            None => create_overlay(app, &label, monitor)?,
        };
        window
            .set_position(Position::Physical(PhysicalPosition::new(
                monitor.position().x,
                monitor.position().y,
            )))
            .map_err(|e| format!("cannot place {label}: {e}"))?;
        window
            .set_size(Size::Physical(PhysicalSize::new(
                monitor.size().width,
                monitor.size().height,
            )))
            .map_err(|e| format!("cannot size {label}: {e}"))?;
        window
            .show()
            .map_err(|e| format!("cannot show {label}: {e}"))?;
        overlays.push((label, monitor.clone()));
    }
    Ok(overlays)
}

fn create_overlay(
    app: &AppHandle,
    label: &str,
    monitor: &Monitor,
) -> Result<tauri::WebviewWindow, String> {
    let monitor_name = monitor
        .name()
        .map(|n| n.as_str())
        .unwrap_or("unnamed monitor");
    // Created hidden and positioned before showing, like the zone picker,
    // so it never flashes at the wrong place. `focused(false)` keeps the
    // user's app (e.g. a game) in focus while the overlay appears.
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("overlay.html".into()))
        .title("Langception Overlay")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .shadow(false)
        .visible(false)
        .build()
        .map_err(|e| format!("cannot create the overlay for {monitor_name}: {e}"))?;

    window
        .set_position(Position::Physical(PhysicalPosition::new(
            monitor.position().x,
            monitor.position().y,
        )))
        .map_err(|e| format!("cannot place the overlay for {monitor_name}: {e}"))?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            monitor.size().width,
            monitor.size().height,
        )))
        .map_err(|e| format!("cannot size the overlay for {monitor_name}: {e}"))?;
    // The whole window passes mouse events through: underlying apps receive
    // every click (region-level click-through per ADR-0001).
    window
        .set_ignore_cursor_events(true)
        .map_err(|e| format!("cannot make the overlay for {monitor_name} click-through: {e}"))?;
    exclude_from_capture(&window)
        .map_err(|e| format!("cannot exclude the overlay for {monitor_name} from capture: {e}"))?;
    window
        .show()
        .map_err(|e| format!("cannot show the overlay for {monitor_name}: {e}"))?;
    Ok(window)
}

/// Excludes a Langception window from screen capture via
/// `SetWindowDisplayAffinity` (`WDA_EXCLUDEFROMCAPTURE`): the window stays
/// fully visible on screen but capture pipelines skip it. Applied to every
/// window that displays Langception's own Thai output — the per-monitor
/// overlays (which sit over the watched regions; without this the loop
/// would OCR its own translations forever, see ADR-0003) and the Results
/// Window (whose Thai entries would feed the loop whenever a zone overlaps
/// it). With the plain-`SRCCOPY` capture, content *behind* an excluded
/// window still composes through, so watching a region under the Results
/// Window translates that underlying app, not the results list.
///
/// Alternatives that do NOT work for a WebView2 (DirectComposition) window:
/// `WS_EX_LAYERED` (DComp content bypasses the layered redirection surface
/// and still appears in captures), and plain "not layered" semantics.
#[cfg(windows)]
pub(crate) fn exclude_from_capture(window: &tauri::WebviewWindow) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    let hwnd = HWND(
        window
            .hwnd()
            .map_err(|e| format!("no native window handle: {e}"))?
            .0,
    );
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
        .map_err(|e| format!("SetWindowDisplayAffinity failed: {e}"))
}

#[cfg(not(windows))]
pub(crate) fn exclude_from_capture(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(()) // no-op off Windows; the overlay recipe is Windows-first anyway
}

pub(crate) fn overlay_windows(app: &AppHandle) -> Vec<tauri::WebviewWindow> {
    let mut windows: Vec<_> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(OVERLAY_LABEL_PREFIX))
        .map(|(_, window)| window)
        .collect();
    windows.sort_by_key(|w| w.label().to_string());
    windows
}

pub fn hide_all(app: &AppHandle) {
    for window in overlay_windows(app) {
        let _ = window.hide();
    }
}

pub fn show_all(app: &AppHandle) {
    for window in overlay_windows(app) {
        let _ = window.show();
    }
}

/// Clears every overlay (all blocks, every zone) — used when the loop stops.
pub fn clear_and_hide(app: &AppHandle) {
    clear_all(app);
    hide_all(app);
}

/// Clears every overlay's rendered blocks but keeps the windows visible.
pub fn clear_all(app: &AppHandle) {
    let payload = OverlayClear { origin_id: None };
    for window in overlay_windows(app) {
        let _ = app.emit_to(window.label(), "overlay-clear", &payload);
    }
}

/// Clears one zone's blocks (its Capture Zone was deleted while running).
pub fn clear_zone(app: &AppHandle, zone_id: &str) {
    let payload = OverlayClear {
        origin_id: Some(zone_id.to_string()),
    };
    for window in overlay_windows(app) {
        let _ = app.emit_to(window.label(), "overlay-clear", &payload);
    }
}

/// Pushes new style settings to every overlay so already-rendered blocks
/// restyle immediately (no re-translation needed).
pub fn broadcast_style(app: &AppHandle, style: &TextStyle) {
    let payload = StylePayload::from(style);
    for window in overlay_windows(app) {
        let _ = app.emit_to(window.label(), "overlay-style", &payload);
    }
}

// ---- Events the overlays receive (camelCase JSON) ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayUpdate {
    origin_id: String,
    key: String,
    /// Block rect in PHYSICAL px relative to the overlay's monitor.
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    translation: String,
    /// The block's source text (the overlay page labels an Excluded Block
    /// with it when the user clicks the pill in Adjust Mode).
    text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayRemove {
    origin_id: String,
    keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayClear {
    /// `None` clears every zone; `Some(id)` clears just that zone's blocks.
    origin_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StylePayload<'a> {
    font_family: &'a str,
    font_size: f32,
    color: &'a str,
    outline_width: f32,
    outline_color: &'a str,
    background: bool,
    background_opacity: f32,
}

impl<'a> From<&'a TextStyle> for StylePayload<'a> {
    fn from(style: &'a TextStyle) -> Self {
        Self {
            font_family: &style.font_family,
            font_size: style.font_size,
            color: &style.color,
            outline_width: style.outline_width,
            outline_color: &style.outline_color,
            background: style.background,
            background_opacity: style.background_opacity,
        }
    }
}

/// Routes loop events to the right overlay window and remembers which
/// monitor each displayed block landed on (needed to route removals).
/// One router per loop run; the forwarder task owns it.
pub struct OverlayRouter {
    /// (label, monitor) per overlay, from [`ensure_overlays`].
    overlays: Vec<(String, Monitor)>,
    /// (zone id, block key) → overlay label.
    placed: HashMap<(String, String), String>,
}

impl OverlayRouter {
    pub fn new(overlays: Vec<(String, Monitor)>) -> Self {
        Self {
            overlays,
            placed: HashMap::new(),
        }
    }

    /// Shows one translated block on the overlay of the monitor its center
    /// falls in. One-shot events are ignored: they belong to the Results
    /// Window (their pass is a single preview, not an ongoing translation).
    pub fn show_translation(
        &mut self,
        app: &AppHandle,
        origin: &EventOrigin,
        key: &str,
        block: &TextBlock,
        translation: &str,
    ) {
        let EventOrigin::Zone { id, .. } = origin else {
            return;
        };
        if translation.trim().is_empty() {
            return;
        }
        let Some((label, monitor)) = self.monitor_containing(block) else {
            return;
        };
        let monitor_origin = monitor.position();
        let payload = OverlayUpdate {
            origin_id: id.clone(),
            key: key.to_string(),
            x: block.x - monitor_origin.x,
            y: block.y - monitor_origin.y,
            width: block.width,
            height: block.height,
            translation: translation.to_string(),
            text: block.text.clone(),
        };
        let _ = app.emit_to(label, "overlay-update", &payload);
        self.placed
            .insert((id.clone(), key.to_string()), label.to_string());
    }

    /// Clears the translations of blocks that disappeared from a zone's
    /// latest pass. Unknown keys (never shown, or already cleared) are
    /// ignored so the overlay and this map can drift harmlessly.
    pub fn hide_blocks(&mut self, app: &AppHandle, origin: &EventOrigin, keys: &[String]) {
        let EventOrigin::Zone { id, .. } = origin else {
            return;
        };
        let stale: Vec<String> = self
            .placed
            .keys()
            .filter(|(zone_id, key)| zone_id == id && keys.contains(key))
            .map(|(_, key)| key.clone())
            .collect();
        let mut by_label: HashMap<String, Vec<String>> = HashMap::new();
        for key in stale {
            if let Some((_, label)) = self.placed.remove_entry(&(id.clone(), key.clone())) {
                by_label.entry(label).or_default().push(key);
            }
        }
        for (label, keys) in by_label {
            let payload = OverlayRemove {
                origin_id: id.clone(),
                keys,
            };
            let _ = app.emit_to(label, "overlay-remove", &payload);
        }
    }

    fn monitor_containing(&self, block: &TextBlock) -> Option<&(String, Monitor)> {
        let center_x = block.x + block.width as i32 / 2;
        let center_y = block.y + block.height as i32 / 2;
        self.overlays.iter().find(|(_, monitor)| {
            let origin = monitor.position();
            let size = monitor.size();
            center_x >= origin.x
                && center_x < origin.x + size.width as i32
                && center_y >= origin.y
                && center_y < origin.y + size.height as i32
        })
    }
}
