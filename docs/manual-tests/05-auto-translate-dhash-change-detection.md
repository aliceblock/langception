# Manual smoke checklist — Ticket 05 (Auto-translate dHash change detection + cadence)

The frame-checksum gate, per-block dHash dedup, and the stability window are
fully covered by offline seam tests (`cargo test`, fake clock + fake ports).
This checklist covers what only a real screen can show: that an idle screen
is genuinely cheap, that animations debounce into silence, that real text
changes still translate promptly, and that pause/resume behaves. Run once on
a real Windows display; a game with an animated HUD is the ideal target.

## Setup

1. `cargo run` from the repo root. Have text on screen over an ANIMATED
   background (a game UI, a video, or a page with CSS animation) and some
   static text elsewhere.
2. Create a Capture Zone covering the animated text (Alt+Z) — or use
   full-screen mode.

## Checklist

### Idle cost (the point of the ticket)

- [ ] **Unchanged screen → no OCR churn**: with translation running and
      NOTHING changing inside a zone, CPU stays near zero (Task Manager:
      the Langception process idles). Every tick is capture + hash only;
      OCR must not run. Leave it a minute — no re-translations appear in
      the Results Window.
- [ ] **Unrelated change does not retranslate**: move the MOUSE through the
      zone (cursor pixels change) → OCR may run, but existing pills do not
      flicker/re-render and no new Results Window entries appear (unchanged
      blocks are skipped by their dHash even when the frame changed).
- [ ] **Status line shows both knobs**: the Results Window footer reads
      "every 500ms · hold 400ms" (from `[capture]`).

### Debounce (stability window)

- [ ] **Animation jitter never triggers**: over an animated background
      (video, particles, pulsing highlight), text that never actually
      changes produces exactly ONE translation (its first appearance, after
      the window) and never re-translates — no spam entries in the Results
      Window while the animation runs. If the text appeared over a CONSTANTLY
      animating surface and never settles, it may never translate (by
      design — animations are noise).
- [ ] **A stable change fires once, promptly**: change the text (e.g. a
      dialog line advances) → the new Thai appears within about a second
      (change detected at tick N, fired at N+1 once the 400 ms window has
      elapsed with the default cadence) and fires exactly ONCE — no
      duplicate entries.
- [ ] **A flash never translates**: brief text that appears and vanishes
      within ~half a second (a tooltip flash, a hit-point popup) produces
      no Results Window entry and no flickering pill.
- [ ] **stability_ms is honored**: set `stability_ms = 2000` in
      `[capture]`, restart, change text → the translation now waits ~2 s
      of stability; set `stability_ms = 0` → every pixel change translates
      on the detecting pass (animation backgrounds now DO spam — that is
      the knob doing its job). Restore `stability_ms = 400`.
- [ ] **absurd stability is rejected**: `stability_ms = 20000` → the
      Results Window shows the config validation error on restart; fix it.

### Cadence

- [ ] **cadence_ms is honored**: set `cadence_ms = 50` (the minimum) → text
      changes show almost instantly; set `cadence_ms = 2000` → visibly
      slower. Restore `cadence_ms = 500`.
- [ ] **cadence floor enforced**: `cadence_ms = 1` → config validation
      error on restart (must be at least 50).

### Pause (Alt+P)

- [ ] **Pause stops everything**: Alt+P with a running loop → tray tooltip
      "paused"; CPU drops to zero (no capture, no OCR — not just no
      translation); changing text on screen does nothing.
- [ ] **Resume does not retranslate**: Alt+P again after a minute of paused
      stillness → unchanged text produces no new Results Window entries
      (tracking state survived the pause); changed text translates.
- [ ] **Stop clears**: Alt+T → pills disappear; Start → the screen
      re-translates from scratch (fresh state — expected after stop, unlike
      pause).

### Full-screen mode

- [ ] **Full-screen mode stays cheap**: switch the Capture Target to "Full
      screen" on a mostly static desktop → idle CPU stays low (the checksum
      covers the whole monitor but OCR only runs when something changed).
- [ ] **Animated wallpaper caveat**: on a monitor with continuously moving
      content (video wallpaper), OCR runs each tick (pixels always change)
      but unchanged text blocks still do not re-translate — the dHash
      dedup holds. CPU is higher, as expected; that is the documented
      behavior, not a bug.

## Known slice-5 limitations (not bugs)

- A block over a CONTINUOUSLY animating surface that has never held stable
  will never translate (the debounce deliberately trades it away). If that
  hurts a specific game, lower `stability_ms` for that session.
- dHash dedup is per BLOCK box: a change confined to sub-cell gradients of
  a huge block could in theory hash equal — real text changes repaint
  enough pixels that this has not been observed; the frame checksum still
  guarantees OCR sees the change.
- The budget test (hash a full-HD frame well under 50 ms) runs offline; a
  4K full-screen zone costs proportionally more but stays well under the
  budget on any modern machine.
