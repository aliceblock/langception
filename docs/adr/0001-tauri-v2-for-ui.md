# Tauri v2 for the UI stack

Langception needs transparent click-through overlay windows, an always-on-top results window, a rich settings UI, system tray, and user-styled translated text (font size, color, outline). We decided on Tauri v2: its overlay recipe (`transparent` + `alwaysOnTop` + `setIgnoreCursorEvents`) is confirmed working on Windows, and HTML/CSS makes text styling and the settings UI nearly free. Considered alternatives: egui (single binary, but text-outline quality and rich settings UI are significantly harder) and Slint (less mature multi-window overlay story).

## Consequences

- Every window is a WebView: memory-heavy, so there is **one overlay window per monitor**, never one per text block.
- Windows distribution uses WebView2 (preinstalled on Windows 10/11).
- Per-pixel click-through has an open upstream gap ([tauri#13070]); we use region-level click-through (whole window passes events or not) plus Adjust Mode to work around it.
