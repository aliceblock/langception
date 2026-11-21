# Manual smoke checklist — Ticket 01 (Spine)

The capture, Windows OCR, and Tauri shell adapters are deliberately outside
automated tests (spec: Testing Decisions). Run this checklist once per
release on a real Windows display. Everything else (orchestrator, glossary,
Google parsing, config) is covered by `cargo test`.

## Setup

1. `cargo install tauri-cli --version ^2` (only needed for the dev icon/build
   conveniences; `cargo run` alone works because the frontend is static).
2. First-run sanity check of the thin adapters (optional but recommended):
   `cargo test -p langception --test manual_smoke -- --ignored --nocapture`
   — proves GDI capture and the Windows OCR path work on this display.
3. `cargo run` from the repo root. First launch creates
   `%APPDATA%\langception\config.toml` with an annotated default template.
4. Edit the config: point `[region]` at a spot on screen with visible text
   (physical pixels — mind display scaling), pick `[source].language`.

## Checklist

- [ ] **Launch to tray**: app shows the Results Window ("Langception — Results",
      always on top) and a tray icon; no console window (release build).
- [ ] **Tray menu**: right-click shows *Start Translation* / *Open Config
      Folder* / *Quit*; left-click focuses the Results Window.
- [ ] **Start**: click *Start Translation* (or the Start button in the window).
      Pill turns "Running"; button flips to "Stop".
- [ ] **Translation flows**: put text in the configured region → within ~1s a
      new entry appears: original (muted) → Thai (large), "engine" badge, timestamp.
- [ ] **Newest first**: a second, different text appears above the first entry.
- [ ] **Dedup**: leave the text unchanged for a few seconds → no new entries.
- [ ] **Change detection**: change the text → exactly one new entry for it.
- [ ] **Glossary short-circuit**: add `Hello = "สวัสดี"` under `[glossary]`,
      stop/start translation, put exactly `hello` (any case) in the region →
      entry with the "glossary" badge and no engine round-trip (appears instantly).
- [ ] **Click-to-copy**: click an entry → Thai text lands on the clipboard
      (paste somewhere to verify), entry flashes green + "Copied ✓" toast.
- [ ] **Stop**: *Stop Translation* → pill "Stopped"; no new entries arrive.
- [ ] **Errors are visible, never silent**:
  - set `[capture] width = 0` → restart translation → red banner shows a
    config validation error.
  - point `[region]` fully off-screen → red banner shows the BitBlt capture
    error within ~1 tick.
  - set `language = "ko"` with no Korean OCR pack installed → red banner shows
    the OCR language-pack error listing installed packs.
  - (optional, flaky by nature) block network → translate a *new* text →
    red banner shows the Google endpoint error.
- [ ] **Restart keeps working**: Quit → relaunch → Start → translation works.
- [ ] **Config folder shortcut**: tray → *Open Config Folder* opens
      `%APPDATA%\langception`.

## Known slice-1 limitations (not bugs)

- Config is file-only (settings UI arrives with ticket 06); region is
  physical pixels; restarting the loop re-translates previously seen text
  (dedup state lives per run); a failed block is not retried until its text
  changes or the loop restarts.
