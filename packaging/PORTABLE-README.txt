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
- OCR language packs for your source language (English, Japanese,
  Chinese, or Korean): Settings > Engines shows what is installed, with
  a jump to Windows' language settings. See the project README.

Where settings live
-------------------
- Config file: %APPDATA%\langception\config.toml
  (paste that path into Explorer's address bar; the tray menu's
  "Open Config Folder" opens it too)
- Keys and glossaries are stored only in that file on your machine.

First steps
-----------
1. Start Langception.exe — the tray icon appears and the setup wizard
   opens on the very first launch (skippable; the app works with zero
   configuration).
2. Choose how text is shown: draw a Capture Zone (Alt+Z) over the text
   you want translated, or use full-screen mode.
3. Press Alt+T to start translation, or "Start Translation" in the tray
   menu.

Full documentation: see the project's README.md on GitHub.

Caution with online games
-------------------------
Langception draws over game windows and captures screen content. Some
anti-cheat systems are hostile to exactly that. See "Anti-cheat and
overlay caution" in the project README before using Langception with an
online game you care about.
