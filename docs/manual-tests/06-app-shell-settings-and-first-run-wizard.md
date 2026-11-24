# Manual smoke checklist — Ticket 06 (App shell: settings window + first-run wizard)

The glossary JSON import/export format, merge/upsert/case-normalization rules,
and editor primitives are covered by offline tests (`cargo test`, `glossary`
module). This checklist covers what only a real Windows session can show: the
first-run wizard on a fresh config, real OCR-pack enumeration, native file
dialogs, live hotkey rebinding, the privacy badge, single-instance focus, and
the Windows autostart registry behavior. Run once on a real machine.

## Setup

1. `cargo run` from the repo root. For the first-run wizard steps you need a
   CLEAN state: quit Langception, delete
   `%APPDATA%\langception\config.toml`, and start again (back the file up if
   you have zones/glossary worth keeping — or just let the app recreate a
   default and re-draw a zone after).
2. Note which OCR language packs your machine actually has (Settings →
   Time & Language → Language & region) so the wizard's verdict can be
   checked against reality.

## Checklist

### First-run wizard

- [ ] **Auto-opens on first launch**: with no config file, the app starts
      with the Settings window open on the wizard view (tabs hidden). The
      Results Window also appears (as it always does).
- [ ] **Pack detection is truthful**: each of EN/JA/ZH/KO shows
      "installed" + the pack tag, or "missing" with install instructions —
      matching what Windows Settings shows. "Re-check" re-runs the probe
      without reopening the window.
- [ ] **New default stated up front (ticket 55)**: the OCR step says
      Windows OCR is the default and that it needs a language pack for
      the Source Language, and names the bundled Paddle OCR as the
      no-packs alternative in Settings → Engines — visible before any
      translation runs, not only after a failed read.
- [ ] **"Open Windows language settings"** opens Time & Language →
      Language & region.
- [ ] **Skippable**: "Skip setup" and "Get started" both reveal the six
      tabs; nothing is required. Translation works immediately afterward
      (draw a zone with Alt+Shift+Z → translation appears in the Results Window)
      with zero configuration.
- [ ] **Shows exactly once**: quit and relaunch — the app starts with the
      tabs, not the wizard. (It returns only if the config file is deleted
      again, or via About → "Run setup wizard".)

### Settings window shell

- [ ] **Tray menu**: "Settings…" opens/focuses the window; the window is
      NOT always-on-top and NOT click-through-ghostly (normal window).
- [ ] **Hide-on-close**: closing the settings window hides it (app stays in
      the tray); tray → Settings… reopens it with the same tab selected.
- [ ] **Never a capture source**: with translation running in full-screen
      mode, the settings window's text is not translated and does not
      trigger OCR churn (ADR-0003 exclusion; with Capture Visibility ON,
      ticket 46, protection is masking instead — same guarantee).
- [ ] **Capture Visibility applies live (ticket 46, ADR-0004)**: in
      Settings → General, "Show Langception in screen captures" is OFF by
      default. With it off, Win+Shift+S / Snipping Tool / OBS / an RDP
      session show NO Langception window. Turn it on and Save → the same
      tools immediately show the Settings window and Results Window (and
      the zone picker's dim while it is open — see ticket 02's checklist;
      the In-place Overlay stays invisible in both modes). Turn it off
      again → windows vanish from captures just as immediately. Restart
      the app while hidden-to-tray windows exist → each window comes back
      with the correct mode.
- [ ] **Cloud captures stay clean (ticket 46)**: with a fused/cloud engine
      and Capture Visibility ON, a zone overlapping the visible Results
      Window or Settings window produces no entries sourced from their
      pixels — those windows' rects are masked out of every frame before
      OCR, before the cloud upload (verify via `LANGCEPTION_SMOKE_DUMP`
      frames if needed: the window areas are black).
- [ ] **Results Window entry point**: the "Settings" button in the Results
      Window header opens/focuses the same window. The Results Window no
      longer has a Text Style panel (it moved to Settings → Text Style).

### General tab

- [ ] **Source language switch**: set language to a language whose pack is
      MISSING → if translation is running it restarts and OCR errors become
      visible ("No Windows OCR language pack…"); set it back to a working
      language → translation resumes.
- [ ] **Cadence/stability persist**: change cadence to 250 and stability to
      0, Save → the Results Window status line shows "every 250ms · hold
      0ms" (after the automatic restart). Values survive an app restart.
- [ ] **Rejection is visible**: enter cadence 10 → the toast shows the
      validation error ("cadence_ms must be at least 50"); config.toml is
      unchanged.
- [ ] **Start with Windows**: toggle on → a `Langception` entry appears in
      Task Manager → Startup apps (or HKCU\...\Run); toggle off → entry
      gone. The checkbox reflects reality after each toggle and after an
      app restart.

### Engines tab

- [ ] **Engine card** shows Google Translate (unofficial) with the ☁ cloud
      chip and the ADR-0002 note.
- [ ] **Pack panel** probes on first open and matches the wizard's result.

### Glossary tab

- [ ] **Add**: add a term (e.g. `Vertin` → `เวอร์ติน`); it appears in the
      list and `config.toml` gains it under `[glossary]`.
- [ ] **Short-circuit still works**: with a zone over a block whose ENTIRE
      text equals the term, the translation appears instantly and matches
      the pinned Thai (Results Window entry, engine not needed offline).
- [ ] **Duplicate rejection**: adding `VERTIN` (different case) is rejected
      with a visible toast ("already exists (case-insensitive)").
- [ ] **Edit inline**: click a term, change it, Enter → the row updates in
      place; an invalid edit (empty Thai, duplicate) shows a toast and the
      row reverts.
- [ ] **Delete**: ✕ removes the row and the config entry.
- [ ] **Export**: Export JSON… → save dialog → pick a path → the file is a
      JSON object `{"term": "thai"}`; a toast reports the count.
- [ ] **Import round-trip**: delete all terms, then Import JSON… with the
      file you exported (merge mode) → all terms return; the toast reports
      added/updated counts. Import again with "replace all" → same result,
      no duplicates.
- [ ] **Bad file is visible**: import a non-JSON file (or an array) → an
      error toast names the problem; the glossary is unchanged.
- [ ] **Live loop pickup**: with translation RUNNING over a glossary term,
      add a new entry → within a moment the block matching it shows the
      pinned Thai (the loop restarted; the overlay briefly clears and
      recaptures — expected).

### Text Style tab

- [ ] **Preview leads the tab** (ticket 56): the exact pill preview is the
      first block, above every row, and repaints live from the FORM's
      current values — unsaved edits included; the amber "unsaved" chip in
      its corner shows while the form differs from the saved style and
      clears on Save.
- [ ] **Explicit Save** (ticket 56): editing rows writes nothing — with
      translation RUNNING, change size/color/outline/background → the
      preview restyles live but the overlays do NOT move until Save; Save
      applies once, toasts, and the overlays restyle immediately (no
      restart; nothing writes config.toml before it). Short-form hex
      colors from config render correctly in the color pickers.
- [ ] **Failed save reverts** (ticket 51's rule): force a rejection (e.g.
      type 200 into Size, press Save) → the toast names the problem and
      every style field snaps back to the saved values; the preview and
      the "unsaved" chip follow them.
- [ ] **Rows** (ticket 56): Outline is a small px number input (default 2,
      0–8, step 0.5) directly after the color rows — the range slider is
      gone; the toggle reads "Background cover source"; the alignment
      selectors appear only while it is checked.
- [ ] **Fresh-config defaults** (ticket 56): with a fresh config, the tab
      opens at 30px, 80% pill opacity, Auto Colors and Background Cover
      Source checked; a config saved by an older build keeps its own
      values.
- [ ] **Persists**: values survive restart; overlays pick them up on load.

### Hotkeys tab

- [ ] **Live rebind WITHOUT restart**: change Toggle translation to `F9`,
      Save → press F9 immediately → translation toggles. Alt+Shift+T no longer
      fires (old binding dropped). The change is in config.toml.
- [ ] **Conflict is visible**: bind two actions to the same key → toast
      "…is bound to both…" and nothing is saved. Bind a key another app
      owns → after save, a warning appears in the Results Window's warning
      line naming the hotkey.
- [ ] **Empty rejected**: clearing a field → toast "must not be empty".

### Privacy badge

- [ ] **Tray tooltip**: hover the tray icon while translation is RUNNING →
      "Langception — running · ☁ Google Translate (unofficial)". When
      stopped → "Langception — stopped" with no ☁ note.
- [ ] **Results Window badge**: the amber "☁ cloud" chip shows in the
      header while translation runs (or a one-shot pass is in flight) and
      disappears when stopped. Tooltip names the engine.
- [ ] **One-shot only**: with translation stopped, run a One-shot Capture
      (Alt+Shift+Q) → the badge appears during the pass and clears after it.

### Single instance + misc

- [ ] **Single instance**: with Langception running, launch it again → the
      second process exits at once; the FIRST instance surfaces (Results
      Window focused, or the settings window if Results was closed).
- [ ] **About tab**: product/version/config path filled; "Open config
      folder" opens the folder containing config.toml; "Run setup wizard"
      re-shows the wizard view.
