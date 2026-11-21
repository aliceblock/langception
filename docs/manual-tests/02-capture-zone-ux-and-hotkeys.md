# Manual tests — Ticket 02: Capture Zone UX + hotkeys

Run on a machine with a live desktop session (Windows). Start with the app
built (`cargo run -p langception`) and `%APPDATA%\langception\config.toml`
present. Scripted verification already covered by the agent: boot to tray,
legacy `[region]` → zone migration, Alt+Z picker open/Esc-cancel, Alt+T start
→ zone translations in the Results Window, Alt+P pause/resume, Alt+Q one-shot
drag → "one-shot" badge entry, zone persistence across restarts. The items
below are the ones a human must eyeball.

## Alt+Z → dim → drag → zone saved; ESC cancels

- [ ] Press Alt+Z: the screen dims (content visible through the tint) and an
      instruction pill appears: "Drag to draw a Capture Zone · Esc to cancel".
- [ ] Press Esc: the dimming disappears instantly, nothing is saved.
- [ ] Alt+Z again, click without dragging (or a tiny drag): the session
      cancels (standard snip behavior), nothing is saved.
- [ ] Alt+Z while the overlay is already open: it closes (toggle).
- [ ] Alt+Z, drag a rectangle over some text: overlay closes and a zone named
      "Zone N" appears in the Results Window's "Capture Zones" panel with the
      correct pixel geometry.
- [ ] If translation was stopped, it starts automatically after the first
      zone is created (pill turns "Running").

## Multiple zones tracked independently

- [ ] Create two zones over two different text areas. Both produce
      translations; entries are badged with their zone name.
- [ ] Change the text in only one zone: only that zone produces a new entry
      (the other stays deduplicated).
- [ ] Two zones over the same text at the same relative position do not
      collide (each translates its own copy).

## Zones persist; management list

- [ ] Quit the app (tray → Quit) and relaunch: the drawn zones are still
      listed and translations resume for them after Start.
- [ ] Click a zone name in the panel: it becomes an inline edit field.
      Enter commits the rename (badge and list update); Esc cancels.
- [ ] Renaming to an already-used name is rejected with a visible error.
- [ ] The ✕ button deletes the zone; it disappears from the panel and stops
      producing translations (after stop/start is NOT needed — it applies
      while running).
- [ ] Deleting all zones leaves the loop running but idle (no events, no
      errors).

## Alt+T / Alt+P and tray state

- [ ] Alt+T starts translation; Alt+T again stops it (same as the tray's
      Start/Stop item).
- [ ] Alt+P while running: the Results Window pill shows amber "Paused", the
      pause button reads "Resume", and the tray tooltip reads
      "Langception — paused". Text on screen does not get re-translated.
- [ ] Alt+P while paused: resumes; unchanged text is NOT re-translated
      (dedup survived the pause).
- [ ] Alt+P while stopped: no effect; the tray's Pause item is disabled
      while stopped.

## Alt+Q One-shot Capture

- [ ] Alt+Q: the picker opens with "One-shot Capture — drag a rectangle to
      translate it once · Esc to cancel".
- [ ] Drag over text while translation is STOPPED: exactly one pass runs and
      the entry appears with a purple "one-shot" badge — no zone is created
      (zone count unchanged).
- [ ] Esc cancels the one-shot picker without side effects.

## Hotkeys rebindable via config

- [ ] In config.toml set `new_zone = "F9"` under `[hotkeys]`, restart the
      app: F9 now opens the zone picker and Alt+Z does nothing.
- [ ] Set a deliberately invalid value (`pause_translation = "Not+A+Key"`):
      the app still starts, the Results Window shows a yellow warning line
      naming the bad binding, and the other hotkeys keep working.
- [ ] Bind two actions to the same key: the second registration produces a
      visible warning; the first binding keeps working.

## Known limitations (expected, not bugs)

- The zone picker covers the **primary monitor only** (multi-monitor zones
  are a later ticket, spec story 32).
- Hotkey changes in config.toml apply on **app restart** (settings UI and
  live re-registration come with ticket 06).
- Zone operations (draw/rename/delete) rewrite `config.toml` from the app's
  in-memory state; hand edits made while the app runs can be overwritten.
- Over exclusive-fullscreen games the picker/dim may not render above the
  game (documented spec limitation of always-on-top windows; borderless
  windowed works).
- Alt+T / Alt+P / Alt+Z / Alt+Q are global: they fire even when Langception
  is not focused, and can clash with other apps' bindings (rebind via
  config).
