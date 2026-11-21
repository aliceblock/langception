# Manual smoke checklist — Ticket 03 (In-place Overlay + Text Style)

The per-monitor overlay windows, capture-exclusion recipe (ADR-0003), and
the settings panel's feel are outside automated tests; run this checklist
once on a real Windows display. Everything else (orchestrator removals,
text-style config validation) is covered by `cargo test`.

Automated verification already done by the agent (see the ticket comment):
overlay windows count = monitors, capture excludes the overlay (dump + OCR
of a watched region), one-event-per-change stability, click-through to the
app underneath, and the overlay page's render/shrink-to-fit/style push in a
browser mock. This checklist covers what only a human can see and feel.

## Setup

1. `cargo run` from the repo root; create a Capture Zone over some on-screen
   text (Alt+Z) or keep one from `config.toml`.
2. Start translation (Alt+T or the Start button).

## Checklist

- [ ] **Overlay renders in place**: each Text Block's Thai appears exactly
      over the original text, styled per the config's `[text_style]`
      (default: Noto Sans Thai 20px, white, 2px black outline, dark pill at
      60% opacity).
- [ ] **Underlying apps receive mouse input**: click buttons, drag windows,
      and scroll inside the zone region — the overlay never intercepts
      anything (whole-window click-through; per-pixel click-through is a
      known upstream gap, ADR-0001).
- [ ] **In-place updates without flicker**: change the text under a zone
      (e.g. edit a document) → the Thai pill updates in place; no flashing,
      no duplicates, no re-translation churn in the Results Window.
- [ ] **Stale text clears**: make the text disappear (close/hide the
      source) → the Thai pill disappears too, within ~2 ticks (≈1s at the
      default cadence; removals wait out OCR glitches by design).
- [ ] **Auto-shrink-to-fit**: point a zone at short text whose translation
      is much longer (e.g. English "OK") → the Thai shrinks to the block's
      width instead of sprawling; very long Thai wraps to ≤ ~2× the block's
      height rather than covering the screen.
- [ ] **Bundled font**: the Thai renders with correct stacked vowels/tone
      marks even if no Thai-capable system font is installed (Noto Sans Thai
      ships in `ui/fonts/`, license alongside).
- [ ] **One overlay per monitor**: with two monitors, translations appear on
      whichever monitor the block sits on (check a zone on the secondary).
- [ ] **Zone picker interplay**: with translations on screen, press Alt+Z →
      the picker dims cleanly over everything (no stale Thai fighting the
      dim); Esc → translations return.
- [ ] **Text Style panel (Results Window, "Text Style (overlay)")**:
  - [ ] Font size: type 30 → pills visibly grow, live, no restart.
  - [ ] Color: pick e.g. yellow → text recolors live.
  - [ ] Outline: width 0 → outline off; width 4 + red → thick red outline
        (outline paints under the fill; glyphs never get "eaten").
  - [ ] Pill: untick → background gone; opacity slider → pill fades.
  - [ ] Font family: set a font you have (e.g. "Leelawadee UI") → applies;
        set a nonsense name → falls back to Noto Sans Thai (still readable).
  - [ ] Persistence: restart the app → the panel shows the saved values,
        from `[text_style]` in `%APPDATA%\langception\config.toml`.
- [ ] **Style survives a config round-trip**: hand-edit `[text_style]` (e.g.
      `font_size = 12.5`, `background = false`), restart → matches. NOTE:
      the section key must be `[text_style]` (underscore) — TOML sections
      are matched by exact field name and a `[text-style]` table would be
      silently ignored with defaults applied.
- [ ] **Invalid style values surface, never silently**: set
      `font_size = 200` or `color = "white"` → Results Window shows the
      `[text_style]` validation error; overlay keeps the last valid style.
- [ ] **Stop hides the overlay**: Stop translation → pills clear and the
      overlay windows hide; Start → they come back.
- [ ] **One-shot stays out of the overlay**: Alt+Q drag → results only in
      the Results Window (no pills appear for one-shot blocks).

## Known slice-3 limitations (not bugs)

- Click-through is whole-window: the overlay is either fully click-through
  or fully interactive; per-pixel hit-testing lands with Adjust Mode
  (ticket 04) per ADR-0001.
- Overlays are capture-invisible everywhere (OBS, screenshots, DWM
  thumbnails) — that is the mechanism keeping the OCR loop stable
  (ADR-0003), not a bug.
- Zones are primary-monitor-picker-only and hotkey edits need a restart —
  carried over from ticket 02.
- Monitor hotplug while running: overlays are (re)positioned at loop start;
  plug/unplug a monitor mid-run and stop/start translation to re-fit.
- The settings panel is deliberately minimal (font/size/colors/outline/pill);
  the full settings window with tabs comes with ticket 06.
