# Changelog

What each release changes, written for the people who use the app —
grouped under Features / Bug Fixes / Docs / Misc as applicable, not a
commit log. CI lifts a release's section verbatim onto its GitHub
Release (and appends the auto-generated commit list + Full Changelog
link under it); a release with no section here fails the release build.
See `docs/releasing.md` for the cut-a-release flow.

## v0.3.0

2026-11-27

**TL;DR — Langception installs ~88% smaller now: the offline OCR engine
ships with the app only for what you need, and the rest downloads on
demand. The Settings window got a full redesign too.**

### Downloads

| File | Size | What it is |
| --- | --- | --- |
| `Langception_0.3.0_x64-setup.exe` | 17.3 MB | Recommended — installs per-user, no admin prompt |
| `Langception_0.3.0_x64-portable.zip` | 22.7 MB | Run from a folder, nothing installs |
| `pp-ocrv6-small.zip` / `pp-ocrv6-medium.zip` / `pp-ocrv6-korean.zip` | 24.9 / 95.7 / 8.0 MB | OCR Model Packs — **you don't need to grab these by hand**; the app downloads the one a reading language needs, by itself |

### Features

**OCR Model Packs — download only the languages you read.** Earlier
releases carried every offline OCR model in the installer (~143 MB);
v0.3.0 ships the Tiny tier built in and fetches the larger sets only
when a selection needs them:

- Pick a reading language whose pack isn't on disk and the app downloads
  it straight from this release, with live progress and a working Cancel.
- Packs live like app data — a `models/` folder next to the exe for the
  portable zip, `%LOCALAPPDATA%\langception\models` for the installer —
  survive app updates, and are removable any time via
  **Delete unused packs** in Settings → Engines & OCR.
- Honest states everywhere: not downloaded / downloading N% / download
  failed (Retry) / ready / update pending. While a required pack is
  missing, captures wait in a loading state — the app never silently
  falls back to Windows OCR and never pretends to read.
- Packs are shared: switching languages reuses what is already on disk
  (the Korean set, for one, downloads once across tiers).

**A redesigned Settings window.** The new look covers the whole window —
darker obsidian theme, every page opening with a title + one-line
description, toasts bottom-right — with the Engines & OCR tab fully
rebuilt: a `Ctrl+K` settings search in the titlebar that filters every
card, a live "Active: …" pill showing the saved OCR + translation pair,
a Translation Pipeline card with an OCR → translate → fallback
breadcrumb, provider cards that expand in place (the selected
translation engine's card is the open one), the Model Pack manager
nested inside it, and an About page led by a proper hero with the
version chip in the sidebar.

### Bug Fixes

None — everything in this release is new functionality, docs, or
packaging. Existing settings carry over untouched (the config schema
did not change).

### Docs

- README and the portable zip's README rewritten around the Model Pack
  flow: what ships with the app, what downloads, where it lives, how to
  clean it up.
- Manual test guides updated: 31 (bundle verification) and 32 (OCR
  engine options) now walk the full pack flow; a new 71 covers the
  Settings redesign.
- ADR-0008 records the trade honestly: the worst case (every pack
  installed) moves the megabytes out of the installer rather than
  shrinking them.

### Misc

- One run of `packaging/fetch-pp-ocrv6.ps1` now produces the shipped
  bundle, the models manifest, and all three pack archives from the
  same SHA-256-verified bytes; the release workflow builds and attaches
  all five artifacts to the GitHub Release.
- Release notes live in this file from now on — CI lifts the tagged
  section onto the release.
