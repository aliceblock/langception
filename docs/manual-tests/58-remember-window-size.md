# Manual tests — Ticket 58: Remember app window size across restarts

The per-label scope is covered offline by `cargo test`
(`window_state_tracks_exactly_the_three_resizable_windows`: Results,
Settings, Debug Log in; the picker, every overlay index, and unknown
labels out). The restore/save mechanics themselves are the plugin's
(`tauri-plugin-window-state`): its restore skips a saved position no
monitor intersects, and its cache never holds a label the filter
rejected. This checklist exercises what only a live desktop shows:
geometry actually surviving a restart, and neighbors that must not care.

## Setup

1. Build and run the dev app: `cargo run -p langception` (or your usual
   `tauri dev` flow). Note: fresh config + no state file = the
   first-run checks below; delete
   `%APPDATA%\app.langception\window-state.json` to reset the saved
   geometry without touching `config.toml`.
2. Have a second monitor available for the move-across-monitors items
   if you can (skip them if not — the single-monitor path is covered by
   the rest).

## Checklist

- [ ] **Fresh install = today's defaults**: with
      `window-state.json` deleted, launch — the Results Window opens at
      its usual 800×720, the Settings window at 740×640 (centered), and
      the tray's Debug Log Window… at 780×540. Nothing opens sized or
      placed oddly.
- [ ] **Settings remembers**: resize and move the Settings window,
      quit (tray → Quit), relaunch, open Settings — it opens at the
      same size and position.
- [ ] **Results Window remembers**: same for the Results Window —
      resize + move, quit, relaunch — it comes back where you left it
      (still frameless, still pinned per `results_on_top`).
- [ ] **Debug Log remembers**: open the Debug Log window, resize +
      move it, quit, relaunch, open it from the tray — it opens at the
      remembered size + position, with no flash of the default size
      first.
- [ ] **Hide (tray) keeps the size within the session**: resize the
      Results Window, hide it (its close button), show it again from
      the tray — same size. Same for Settings and Debug Log. Quit +
      relaunch — the sizes come back (hidden windows are saved too,
      because the state is written at exit, not at close).
- [ ] **Overlays and picker never move**: with a zone running and
      overlays showing, quit + relaunch (and with a stale
      `window-state.json` present) — the overlays still hug their
      monitors and the picker still blankets the screen edge to edge;
      nothing restores a stale rect onto them. The state file only ever
      contains the labels `results`, `settings`, `debug`.
- [ ] **Off-screen geometry cannot strand a window**: resize + move a
      window onto your second monitor, quit, unplug that monitor,
      relaunch — the window opens somewhere visible (OS default
      placement), not trapped off-screen. Restore the monitor and quit +
      relaunch again — remembering resumes.
- [ ] **config.toml untouched**: after all of the above,
      `config.toml` has no size/position keys — the `[windows]` section
      still holds only `results_on_top` / `settings_on_top` (and the
      capture-visibility key), and both pin toggles still work as
      before.
- [ ] **No maximize memory** (out of scope, must not break): maximize
      the Results Window, quit, relaunch — it comes back as a normal
      window at its pre-maximize size (never a stuck maximized frame).

## Cleanup

- Quit the app. Delete `%APPDATA%\app.langception\window-state.json` if
  you want the machine back to fresh-install behavior; restore any
  changed `config.toml`.
