# Manual smoke checklist — Ticket 07 (Per-app Profiles)

Profile matching/zones/glossary-merge/override logic, config validation
(duplicate exes, unknown engines, bad zones) and the window-relative
rectangle math are covered by offline tests (`cargo test`). This checklist
covers what only a real Windows session can show: actual foreground-window
detection, activation/deactivation while the loop runs, overlay hygiene
across focus switches, the app picker, and window-follow. Run once on a
real machine.

## Setup

1. `cargo run` from the repo root. You need two apps to switch between:
   ideally one game-like window (borderless or full-screen) plus any
   normal app (Notepad works). Note each app's executable name (Task
   Manager → Details → Name) for the profile's exe field.
2. Have a Source Language pack installed for whichever language the test
   app displays (usually English).

## Checklist

### Creating a Profile (Settings → Profiles)

- [ ] **New profile drafts from running apps**: "New profile…" scans
      visible windows; the picker lists real apps (title — exe), NOT
      Langception's own windows, NOT closed/minimized-to-tray apps.
- [ ] **Name/exe/language editable**: change the exe via the dropdown
      (↻ re-scans), set Source Language, engine "Inherit", Genderless
      "Inherit" → Save profile → values survive reopening Settings and an
      app restart; config.toml gains the `[[profiles]]` table.
- [ ] **Rejections are visible**: empty name / empty exe → a toast names
      the problem, nothing saved. (Duplicate exe can only be forced by
      hand-editing config.toml — it is a validation error there.)
- [ ] **Delete**: two-click confirm ("Really delete?") removes the
      profile; config.toml loses the table; an active profile's deletion
      returns the loop to the global target.

### Activation on foreground

- [ ] **Activates**: with translation RUNNING and the profile's app
      focused, the Results Window status line shows `profile "<name>"`,
      and Settings → Profiles shows the green "active" chip on it. The
      profile's Source Language replaces the global one (status line
      language changes when they differ).
- [ ] **Deactivates**: focus another app → within ~a second the status
      line loses `profile "…"`, the language falls back to the global
      one, the chip disappears.
- [ ] **Global fallback**: with NO profile matching (focus on an
      unlisted app), the global Capture Target (zones or full-screen) is
      watched exactly as before this feature.
- [ ] **Stopped loop**: with translation STOPPED, focus switches still
      flip the active chip/status line (the watcher updates state), but
      nothing is captured.

### Capture scopes to the window

- [ ] **Whole-window watch** (no profile zones): while the profile is
      active, text inside its window is translated (Results Window
      entries appear); text in OTHER apps is not, even where global zones
      overlap them.
- [ ] **Window-relative zones**: "Draw zone on screen…" opens the picker
      ("Profile Capture Zone — drag inside the app's window"). Draw over
      part of the app's window → the zone list shows it as
      `(window-relative)`; ONLY that part of the window is watched.
- [ ] **Zones follow the window**: move the app's window to another
      screen position → within ~a second the same relative area is
      watched at the new position (pills from the old position clear).
      Resize the window → zones stay pinned to the same window-relative
      spot, clamped inside the new bounds.
- [ ] **Minimized/target gone**: minimize the app → nothing sensible is
      watched (the loop idles or surfaces capture errors visibly — never
      silently translates the wrong thing); restoring the app resumes.

### Focus switching is clean

- [ ] **No stale overlays**: get some pills showing for the profile's
      app, then focus a non-profile app → the profile app's pills
      disappear (within the restart moment), and the global target's
      zones take over. Switch back → the app is re-captured fresh.
- [ ] **Rapid switching**: Alt-Tab back and forth a few times quickly →
      no duplicated pills, no mixed languages on screen, no errors
      beyond (at most) transient capture notes.
- [ ] **Profile ↔ global targets coexist**: set up a global zone AND an
      active profile; focusing in/out of the profile app swaps which
      regions are watched; the global zone's pills come back when the
      profile deactivates.

### Glossary layer

- [ ] **Profile term wins**: global glossary has `Vertin = "global-value"`,
      the profile's layer has `Vertin = "เวอร์ติน"`; with the profile
      active, a block reading exactly `Vertin` shows `เวอร์ติน`. With the
      profile inactive (same screen text in another app... or after
      deactivation) the global value applies.
- [ ] **Profile-only term**: a term only in the profile's layer matches
      while active, does nothing while inactive.
- [ ] **Editor**: add/edit/delete terms in the profile's layer
      (case-insensitive duplicate rejected); deleting the profile
      removes its layer from config.toml.

### Genderless override plumbing

- [ ] **Config plumbing**: set a profile's Genderless to "On"/"Off" →
      the value lands in config.toml as `genderless = "on"/"off"`. (The
      visible scrubbing behavior itself is ticket 08 — here it just must
      not break anything.)

### Config robustness

- [ ] **Hand-edited profile**: add a second `[[profiles]]` by hand
      (different exe), restart → both profiles appear, both activate on
      their apps. Two profiles with the SAME exe → config error surfaces
      in the Results Window (never silent).
- [ ] **Old config**: a pre-profiles config.toml still loads (profiles
      default to empty) — nothing about zones/glossary/modes changes.

### One-shot with profile active

- [ ] **One-shot follows the active profile**: with the profile active
      (its app focused), Alt+Q → the pass uses the profile's language
      and glossary layer (a glossary term on screen shows the profile's
      Thai in the Results Window).
