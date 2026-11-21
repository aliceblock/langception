# Manual smoke checklist — Ticket 04 (Adjust Mode + Excluded Blocks + full-screen mode)

Adjust Mode interactivity, zone drag/resize feel, exclusion clicks, monitor
selection, and unplugged-monitor behavior live outside automated tests; run
this checklist once on a real Windows display (ideally with two monitors).
Everything else (exclusion matching, skip + re-translate semantics, config
persistence, the vanished-block fix) is covered by `cargo test` — see the
ticket comment for what was already verified offline.

## Setup

1. `cargo run` from the repo root. Have some non-Thai text on screen (a
   browser page or editor works; a game is the real target).
2. Create a Capture Zone over text (Alt+Z) or keep one from `config.toml`.

## Checklist

### Adjust Mode (Alt+A)

- [ ] **Enter/exit toggles cleanly**: Alt+A → the instruction bar appears at
      the top of each monitor, zone rectangles render on their monitor with
      a name label and 8 handles; Alt+A again → everything disappears and
      click-through is fully restored (click apps under the overlay to
      prove it). Esc also exits Adjust Mode.
- [ ] **Click-through off while adjusting**: during Adjust Mode, clicks land
      on zones/pills (the overlay is interactive); after exit they land in
      the app underneath again.
- [ ] **Zone drag**: drag a zone rect by its interior → it follows the
      mouse; on release it persists (check `[[zones]]` in
      `%APPDATA%\langception\config.toml`) and translations re-target to
      the new region within one tick (translation keeps running during the
      drag).
- [ ] **Zone resize**: drag each of the 8 handles (corners + edges) → the
      rect resizes, never below a tiny minimum; release persists. Note the
      resize happens in CSS px — on a scaled monitor the saved physical
      region is the scaled equivalent.
- [ ] **Drag round-trip**: hand-edit a zone's `region` in config.toml
      (bigger numbers), restart, enter Adjust Mode → the rect appears where
      the config says.
- [ ] **Adjust while stopped**: stop translation (Alt+T), press Alt+A →
      overlays appear with zone rects; drag/resize works; the Results
      Window's zones list updates. Exit Adjust → overlays hide again (no
      loop → nothing to show).
- [ ] **Picker interplay**: enter Adjust Mode, then Alt+Z → Adjust Mode
      exits and the picker opens (the two must not fight over the screen);
      Esc → back to normal.

### Full-screen mode

- [ ] **Switch targets**: Results Window → Capture Target → "Full screen".
      The loop re-targets live: pills appear for every Text Block on the
      selected monitor (menus, HUD, whatever is visible), zone pills clear.
      The tray tooltip shows "· full screen"; the tray "Full-Screen Mode"
      checkmark matches. Switch back to "Capture Zones" → zone watching
      resumes, full-screen pills clear.
- [ ] **Monitor selection**: with two monitors, pick the second one in the
      Monitor dropdown → blocks from THAT monitor are translated, not the
      primary. The choice persists across restart (`[mode]` in config.toml).
- [ ] **Exclusion click**: in full-screen mode press Alt+A → hover a Thai
      pill (dashed outline + pointer cursor) → click → the pill disappears
      immediately and the Results Window's "Excluded Blocks" panel gains an
      entry labeled with the original text. The position is never
      translated again while the entry exists.
- [ ] **Exclusion persists**: restart the app, full-screen mode again → the
      excluded position stays untranslated (config `[[excluded_blocks]]`).
- [ ] **Clearing restores**: delete the entry (✕ in the Excluded Blocks
      panel) → the very next pass re-translates the position (the exclusion
      dropped its dedup state while active — no stale-silence bug).
- [ ] **Exclusions are full-screen-only**: switch to zone mode → a zone
      covering the excluded position translates it normally (exclusions are
      consulted in full-screen mode per CONTEXT.md). A one-shot pass honors
      exclusions only while full-screen mode is the target.
- [ ] **Jitter tolerance**: let the game re-render the same text a few
      pixels off → it stays excluded (matching tolerates small movement);
      a different line a row below does not get caught by the exclusion.

### Monitors

- [ ] **Zones remember their monitor**: draw a zone (primary picker) → its
      config entry carries `monitor = "\\\\.\\DISPLAY1"`-style name; the
      zones list shows it.
- [ ] **Unplugged monitor disables its zones**: (two monitors) hand-edit a
      zone's `monitor` to a non-connected name (e.g. `\\.\DISPLAY9`) → the
      zones list greys it out with "(monitor unplugged)"; translation keeps
      running for the healthy zones. Restore the name → enabled again.
      (Real hotplug testing needs a stop/start to re-enumerate — known
      slice limitation from ticket 03.)
- [ ] **Legacy zones keep working**: a config written by ticket 02/03 (no
      `monitor` key on zones) still parses; such zones follow the primary
      monitor and stay enabled.

### Misc

- [ ] **Adjust hotkey is rebindable**: set `adjust_mode = "F7"` in
      `[hotkeys]`, restart → F7 toggles Adjust Mode, Alt+A does not. The
      hotkey hint in the empty state updates.
- [ ] **Invalid mode surfaces**: set `target = "window"` in `[mode]` →
      Results Window shows the config validation error on restart; fix it.
- [ ] **One-shot excludes too** (full-screen target): Alt+Q over the
      excluded area → no entry for that position.

## Known slice-4 limitations (not bugs)

- Zone drag/resize is primary-monitor-picker-only for CREATION (carried
  from ticket 02); dragging existing zones works on any monitor.
- Monitor hotplug mid-run needs stop/start translation (carried from
  ticket 03); unplugged-monitor disabling is evaluated when zones are
  (re)sent and in the status snapshot.
- Adjust Mode renders zone rects per monitor; a zone spanning two monitors
  shows only on the monitor it belongs to, possibly clipped.
- Exclusions are positional: if the game moves the block far, the position
  re-translates (by design — the exclusion is a position, not a text).
- The mode/excluded panels are deliberately minimal; the full settings
  window with tabs comes with ticket 06.
