# Manual tests — Ticket 04 (pp-ocrv6-sidecar): the four OCR engine options

The new engine's lifecycle (lazy spawn, reuse, restart on model change,
crash recovery, timeouts) is covered offline by `cargo test` against the
scripted stub (`ppocr_sidecar_tests`); this checklist exercises the REAL
sidecar and the packaging — everything that needs a live desktop and the
fetched models (the installer/portable artifact checks live in
`31-bundle-verification.md`).

## Setup

1. The bundle (git-ignored) must be in place at
   `src-tauri/resources/pp-ocrv6/`:
   `powershell -File packaging/fetch-pp-ocrv6.ps1` fetches and
   SHA-256-verifies all ten model files (into a work dir under
   `packaging/stage/`), stages the tiny-only bundle — the three Tiny
   files plus the generated `models-manifest.json` — into the bundle
   dir, and builds the three Model Pack archives into `packaging/`.
   (Since the deferred-model-packs feature the bundle is tiny-only:
   selecting small/medium downloads its pack on first use — this
   checklist's full download-state pass is that feature's ticket 06.)
   The sidecar exe comes from
   `cargo build -p pp-ocrv6-sidecar` (the workspace target dir is one of
   the app's resolution candidates in dev).
   Optional headless check (live desktop needed):
   `cargo test -p langception --test manual_smoke pp_ocrv6_smoke -- --ignored --nocapture`
   proves spawn → handshake → base64 PNG → parsed blocks through the NEW
   engine (passes: en fresh, en reuse, ja restart).
2. Fresh config or your usual `config.toml`, `cargo run`. Display some
   text on screen (any window).

## Checklist

- [ ] **Dropdown shows the four options**: Settings → Engines → OCR engine
      lists Windows OCR (selected by default), PP-OCRv6 tiny, PP-OCRv6
      small, PP-OCRv6 medium — each label names its tier.
- [ ] **Legacy "paddle" config maps to small**: hand-edit `config.toml`'s
      `[ocr] engine = "paddle"`, start translation — the log line says
      `ocr=pp-ocrv6-small` and the new sidecar runs. Open Settings →
      Engines — the dropdown displays PP-OCRv6 small; saving writes
      `engine = "pp-ocrv6-small"`.
- [ ] **Default untouched (story 17)**: with Windows OCR selected,
      everything behaves exactly as before this feature — no sidecar
      process, the launch log says `ocr=windows`.
- [ ] **PP-OCRv6-small runs the loop end-to-end**: select PP-OCRv6 small,
      start translation (Alt+Shift+T) — the first pass loads models
      (~1–3 s; later passes are fast), the text is read and translated,
      and the launch log says `ocr=pp-ocrv6-small`.
- [ ] **Tier change restarts the sidecar**: with the loop running, switch
      small → medium and save — the next capture reloads models (slower
      first pass) and the log names the restart; Task Manager shows one
      `pp-ocrv6-sidecar.exe` at a time, replaced on the switch.
- [ ] **Language switch restarts too**: cycle the Source Language
      en → ja → ko — each restarts the sidecar on that language's models
      and reads its text (Korean reads through the official Korean
      PP-OCRv5 model at every tier — same behavior whatever the tier).
- [ ] **Korean is tier-independent (story 7)**: with ko as the Source
      Language, switch tiers tiny → small → medium — Korean keeps
      working identically (the sidecar launch line names the same Korean
      recognizer each time) and fires no notice.
- [ ] **tiny + Japanese surfaces the substitution notice (ticket 05,
      story 10)**: select PP-OCRv6 tiny and set the Source Language to
      ja — the app log (Debug Log Window) gets one `ocr` line saying
      tiny cannot read Japanese (no kana) and the small model set runs
      instead; kana text reads correctly. One line per substituted
      session: switching to en and back to ja produces a second line
      (each transition is its own incident), and so does saving any
      setting that rebuilds the loop. A failing pass stays silent here —
      the fallback warning covers broken bundles. en/zh with tiny fire
      no line at all.
- [ ] **No console flash**: the sidecar runs hidden — no terminal window
      appears with the app.
- [ ] **Crash → surfaced, then recovery**: kill `pp-ocrv6-sidecar.exe` in
      Task Manager. No hang, no app crash, and the death is visible: the
      app log gets an `ocr` line ("exited unexpectedly — restarting")
      and the next pass respawns it. One line per incident, not one per
      pass.
- [ ] **Missing bundle → fallback**: rename `src-tauri/resources/pp-ocrv6/`
      away and start translation with PP-OCRv6 selected — every pass
      still reads text (Windows OCR fallback) and a warning says
      PP-OCRv6 could not run and why.
- [ ] **Debug Log Window (story 12)**: per-pass OCR timing still shows;
      the engine/tier is visible from the selection (the launch line)
      with no new field.
- [ ] **Wizard untouched**: the first-run wizard still flows through the
      Windows-OCR language-pack check; it never mentions tiers.
- [ ] **Config roundtrip**: switch tiers in Settings → Engines, save,
      reopen — the choice persisted in `[ocr] engine`; an `[ocr]` section
      missing entirely (pre-ticket-31 config) still loads as windows.
