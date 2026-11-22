# Product

## Register

product

## Users

Windows users who play games or watch EN/JA/ZH/KO content full-screen and
want Thai translation without leaving it. They run Langception beside a
fullscreen app for long stretches; the Results Window and tray are glanced
at, not studied. Many use per-app Profiles (e.g. one pinned to a specific
game exe) so their setup is mostly "configure once, then forget".

## Product Purpose

Langception watches the screen — Capture Zones, a full monitor, or a
per-app Profile — OCRs the text and shows Thai translation in place over
the original or in an always-on-top Results Window. Success is invisible
plumbing: the right Thai text appears where the original was, and the
control surfaces stay quiet until something needs attention.

## Brand Personality

Sharp, quiet, alert. A focused tool that lives next to a game: dark
surfaces, one loud yellow accent, signals only. Calm when idle, unmissable
when something is wrong.

## Anti-references

- SaaS dashboard clichés: stat cards, icon+title filler grids, glassmorphism,
  hero-metric blocks.
- Plain-text noise: long comma-separated status sentences in the header
  (the ticket-18 mistake — info belongs in Settings, signals belong in the
  title bar).
- Gamer-RGB excess: neon glows, animated gradients, HUD clutter.

## Design Principles

1. **Signals only** — every visible element is either actionable or an
   active state; if it's settings-shaped, it lives in Settings.
2. **Quiet surfaces, one loud accent** — near-black panels; yellow marks
   the special/primary state, cyan marks interactive geometry, red is
   reserved for errors.
3. **Never steal attention from the game** — the overlay and windows must
   not compete with fullscreen content; motion is rare and purposeful.
4. **Truthful UI** — chips and pills reflect what the translation loop is
   actually doing right now (active Profile, current reading mode).
5. **Readable in the dark** — AA contrast on near-black, dark-only by
   design, keyboard focus always visible.

## Accessibility & Inclusion

AA contrast on dark surfaces (≥4.5:1 body, ≥3:1 large/bold); visible
`:focus-visible` rings everywhere; color is never the only signal (chips
carry text, errors carry messages); the app is dark-only by design, with
`color-scheme: dark` so native controls agree. No motion exists today; any
future motion needs a reduced-motion fallback.
