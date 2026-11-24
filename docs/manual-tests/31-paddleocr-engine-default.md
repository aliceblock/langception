# Manual tests — Ticket 31: PaddleOCR engine (bundled) as the default OCR engine

> **Ticket 55 note:** the default OCR engine is now **Windows OCR**; Paddle
> stays bundled and selectable. The checklist below still applies, but
> "fresh config" no longer means paddle — select Paddle OCR in
> Settings → Engines where an item says so.

The sidecar protocol (reply parsing, request encoding) and the process state
machine (lazy spawn, reuse, model switch, crash recovery, kill-on-drop) are
covered offline by `cargo test` (fixtures + `paddle_stub`). This checklist
exercises the REAL sidecar and the packaging — everything that needs a live
desktop and the ~300 MB bundle.

## Setup

1. Fetch the sidecar (git-ignored, one command):
   `powershell -File packaging/fetch-paddleocr.ps1` — or point
   `LANGCEPTION_PADDLEOCR_DIR` at an extracted release archive.
   Optional headless check:
   `cargo test -p langception --test manual_smoke paddle_ocr_smoke -- --ignored --nocapture`
   proves spawn → handshake → base64 PNG → parsed blocks against the real
   exe (two passes, second must reuse the process).
2. Fresh config (delete `%APPDATA%\langception\config.toml` to simulate a
   first launch), `cargo run`. Display some text on screen (any window).

## Checklist

- [ ] **New default (ticket 55)**: fresh install/config starts on Windows
      OCR — Settings → Engines shows "Windows OCR" selected, the log line
      says `ocr=windows`, and no `PaddleOCR-json.exe` appears while the
      loop runs.
- [ ] **Paddle out of the box when selected**: with Paddle OCR selected in
      Settings → Engines (the ticket-31 default behavior), NO Windows OCR
      language packs are needed — starting translation (Alt+T) reads the
      text. The sidecar spawns on the first capture (first pass takes
      ~1–3s to load models; later passes are fast). Log line says
      `ocr=paddle`.
- [ ] **No console flash**: the sidecar runs hidden — no terminal window
      appears with the app.
- [ ] **Sidecar is reused**: watch the process list (Task Manager →
      "PaddleOCR-json.exe") — exactly one while the loop runs; it survives
      many passes (not one per capture).
- [ ] **All four Source Languages**: Settings → General cycles en/ja/zh/ko;
      each reads its language correctly (the sidecar restarts on the
      switch — first pass after a switch is slower again). A per-app
      Profile with a different Source Language gets its language's model
      when its app is foreground.
- [ ] **Config roundtrip**: switch OCR engine in Settings → Engines, save,
      reopen — the choice persisted. Check `config.toml` gained
      `[ocr] engine = "…"`; a config WITHOUT the `[ocr]` section (written
      by an older build) still loads and means the current default
      (windows) — while a saved `engine = "paddle"` keeps paddle across
      the ticket-55 flip, no migration, no silent switch.
- [ ] **Live switch**: with the loop running, switch the OCR engine in
      Settings → Engines and save — the next capture uses the other engine
      (paddle: one `PaddleOCR-json.exe` appears; windows: none), no app
      restart.
- [ ] **Crash → surfaced, then recovery**: kill `PaddleOCR-json.exe` in
      Task Manager. No hang, no app crash, and the death is visible: killed
      between passes, the app log gets an `ocr` line ("exited unexpectedly
      — restarting") and the next pass respawns it (slower first pass);
      killed mid-pass, the Results Window shows a warning that PaddleOCR
      could not run and Windows OCR covered that pass. One line/warning
      per incident, not one per pass.
- [ ] **Missing bundle → fallback**: remove/rename `src-tauri/resources/`
      (or unset the env override) and start translation with paddle
      selected — every pass still reads text (Windows OCR fallback) and a
      warning says PaddleOCR could not run and why.
- [ ] **Clean exit, no orphan**: quit via tray → `PaddleOCR-json.exe`
      disappears from the process list (also check after closing the app
      from the taskbar while a pass is in flight).
- [ ] **Privacy surfaces**: with Paddle OCR running, the tray tooltip and
      Results Window show the ☁ badge only for cloud TRANSLATE engines —
      never for Paddle OCR (it is fully offline).
- [ ] **Installer/portable bundling** (needs `tauri build` with the
      resources fetched): the NSIS installer installs `resources/paddleocr/`
      beside the exe and the portable zip carries the same folder; both
      builds pass the first checklist item on a machine without OCR
      language packs.
