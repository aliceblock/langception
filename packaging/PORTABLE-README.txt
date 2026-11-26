Langception (portable)
======================

This is the portable build: no installer, nothing is added to your Start
menu or registry. Run Langception.exe and quit by right-clicking the tray
icon (near the clock) > Quit. To remove Langception, delete this folder
plus the config folder (see below).

Requirements
------------
- Windows 10 or 11 (64-bit).
- WebView2 Runtime — preinstalled on Windows 11 and on up-to-date
  Windows 10. If the app fails to open a window saying WebView2 is
  missing, install it once from:
  https://developer.microsoft.com/microsoft-edge/webview2/
  (the "Evergreen Bootstrapper" is enough; it needs internet access).
- OCR: the default Windows OCR engine needs an OCR language pack for
  your source language (English, Japanese, Chinese, or Korean) — without
  it nothing is read. Settings > Engines shows what is installed, with a
  jump to Windows' language settings. If you cannot install packs,
  switch to PP-OCRv6 tiny there: it ships with the app (in the
  "resources" folder next to Langception.exe — keep the two together
  when moving the folder) and works fully offline with no language
  packs. The larger tiers (small, medium) and the Korean recognizer are
  Model Packs: selecting one downloads it on first use, with progress
  shown in Settings > Engines. See the project README and the section
  below.

Where settings live
-------------------
- Config file: %APPDATA%\langception\config.toml
  (paste that path into Explorer's address bar; the tray menu's
  "Open Config Folder" opens it too)
- Keys and glossaries are stored only in that file on your machine.

Downloaded OCR models
---------------------
Selecting a PP-OCRv6 tier that does not ship with the app downloads its
Model Pack on first use — nothing downloads unless you select it. In
the portable build the download creates a "models" folder next to
Langception.exe and keeps the packs there, so the models travel with
the app: keep that folder together with the exe when moving or copying
Langception's folder. Nothing deletes a pack on its own either —
Settings > Engines has a "Delete unused packs" button when you want
the disk space back, and deleting the "models" folder yourself is safe
(the app rebuilds what ships with the app and re-downloads a pack when
you select its tier again).

First steps
-----------
1. Start Langception.exe — the tray icon appears and the setup wizard
   opens on the very first launch (skippable; the app works with zero
   configuration).
2. Choose how text is shown: draw a Capture Zone (Alt+Shift+Z) over the
   text you want translated, or use full-screen mode.
3. Press Alt+Shift+T to start translation, or "Start Translation" in the
   tray menu.

Full documentation: see the project's README.md on GitHub.

Caution with online games
-------------------------
Langception draws over game windows and captures screen content. Some
anti-cheat systems are hostile to exactly that. See "Anti-cheat and
overlay caution" in the project README before using Langception with an
online game you care about.
