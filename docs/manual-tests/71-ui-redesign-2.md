# 71: Settings follows the reworked redesign.html (Engines & OCR mock) — manual tests

The second redesign pass: the Settings window's shared chrome and the
Engines & OCR tab follow the reworked Gemini mock (redesign.html at the
repo root, committed alongside as the primary source like 61's and 70's).
Presentation only — every id, command, save path, and the ticket-04 pack
state machine are unchanged; the automated suite pins the i18n seams
(parity + literal-key scan) and the whole behavior surface stays covered
by its existing tests. Everything visual and every interaction below
needs eyes and hands unless marked verified. Verified-via-harness items
were driven against the real `ui/settings.html` + `settings.js` through
`.scratch/deferred-model-packs/qa/server.js` with a stubbed `__TAURI__`.

## Titlebar

- [ ] The mock's center filter box sits between the gear and the window
      buttons: magnifier icon left, "Ctrl K" keycap right. Below ~700px of
      window width it hides (the 620px minimum keeps the bar uncramped).
- [ ] Typing narrows the cards: any card (section card, engine accordion,
      profile strip) whose text does not contain the query hides; clearing
      the box (select-all + Delete) brings everything back. Cards keep
      their state — a hidden panel's unsaved edits still count for the
      save bar, and state-hidden panels (the pack sub-card on Windows OCR
      with nothing on disk) never reappear through the filter.
- [ ] Ctrl+K (Cmd+K) focuses the box from anywhere and selects its content,
      so a new query replaces the old.
- [verified-harness] Filter on "azure": the pipeline card, the Azure card,
      and the profile editor section (its engine dropdown names Azure)
      stay; 16 other cards hide. Ctrl+K focuses; select-all + Backspace
      restores all cards.

## Sidebar

- [ ] "Preferences" group label (tiny uppercase) sits above the nav items;
      About pins to the bottom WITH the mock's version chip on its right,
      filled from the real version once About's data lands (no chip before
      that — never a "—" badge).
- [ ] The active tab wears the amber tint + soft amber glow; idle tabs
      hover to a subtle white step on the darker rail.

## Page heads (every tab)

- [ ] Every page opens with the same head: 20px title + one-line lede over
      a hairline. New ledes: General ("App-wide defaults…"), Hotkeys
      ("Global shortcuts…"), About (hero card instead — below). The
      Engines title reads "Engines & OCR".
- [verified-harness] The Engines head's live pill shows the SAVED
      selection ("Active: PP-OCRv6 small + Google Translate (unofficial)"),
      emerald with a pulsing dot; it reads the dirty tracker's snapshot —
      never the live selects — so an unsaved edit (or a live UI-language
      re-render) cannot make it pretend an edit is active; the save bar
      owns that signal.

## Engines & OCR tab

- [ ] The primary card "Translation Pipeline" carries the mock's mini flow
      breadcrumb (OCR engine → Translation engine → Fallback translation,
      amber/sky/muted with arrows) — the same three words as the form rows
      below. The mock lays it beside the title in its ~1150px frame; this
      page's 680px column wraps it below the title as its own row (the
      mock's own stacked fallback), from the 620px minimum on up.
- [ ] The Processing row now has its own (?) tooltip (classic vs vision);
      the other three rows keep theirs.
- [ ] The Model Pack sub-card is the mock's nested inset: amber package
      icon tile, "OCR Model Packs" + lede, then the ticket-04 rows
      unchanged (name + size + state pill + row actions + progress bar),
      then the mock's maintenance footer — "Storage & cache maintenance"
      left, "Delete unused packs" (folder-x icon) right, the note beside.
      The clear-cache row stays OUTSIDE the sub-card with its trash icon —
      on a build without a models manifest the whole sub-card hides and
      cache clearing must survive that.
- [verified-harness] ja + tiny seeds the pill "Active: PP-OCRv6 tiny …",
      the tier options carry their state suffixes, and the required Small
      set row shows "downloading 50%" with a half-filled amber bar and
      Cancel.
- [ ] Download states still work end to end on a real build (manual 32's
      arms): first-use download, cancel, failed + Retry, update-pending,
      delete-unused with the confirm dialog.

## Provider Configurations accordion

- [ ] The section label (tiny uppercase) + "Click a provider to expand
      settings" hint sit above the five cards. Each card's head — title,
      chips (☁ cloud amber / Local · offline emerald), one-line
      description, chevron in its square box — stays visible while
      collapsed; fields live under a hairline in the body.
- [ ] The OPEN card wears the amber border + soft glow and its chevron
      points UP (the mock's rotate-180); closed cards point down.
- [ ] The selected Translation engine's card is the open one (ticket 54
      rule); a head click toggles any card; switching the selection (or a
      rejected save's revert, or Discard) moves the expansion.
- [ ] Google Translate is NOT an accordion item — the keyless default has
      nothing to configure, so it renders as the always-open card (title +
      ☁ cloud + Default chips + description, no chevron, no body). With
      Google selected no accordion item is open.
- [verified-harness] Clicking the Azure head opens it (aria-expanded
      true, body visible, amber border, chevron up); the other four stay
      collapsed.

## About tab

- [ ] The mock's hero leads the tab: amber scan-text tile with glow,
      product name, version in amber mono, one-line description — all
      filled from the real about payload; the info grid, folder actions,
      privacy hint, and the Updates section follow unchanged.
- [verified-harness] Hero shows the stub's "Langception" / "v0.0.0-qa";
      the sidebar chip carries the same version.

## Regression sweep

- [ ] Toasts land bottom-right now (the mock's corner) and still clear the
      save bar.
- [ ] Sticky save bar unchanged: edit → pulsing-dot bar, Discard restores
      every snapshot in one press (verified-harness: cadence 900 → 999 →
      Discard → 900, bar hides).
- [ ] Thai UI: switch UI Language — page ledes, the flow breadcrumb's
      words, the pack sub-card's title/lede, the search placeholder, the
      sidebar group label, and the ACTIVE PILL re-render in Thai (the pill
      re-bakes with the labels; this was caught and fixed in QA — the
      first cut left it English after a live switch). Pack names stay
      Latin ("Model Pack ของ OCR").
- [ ] `cargo test --workspace` green; the i18n parity test covers the 16
      new keys + the reworded Engines title/lede in both dictionaries
      (driven red → green: en first FAILED the parity test on the missing
      th set, th landed green); the literal-key scan covers every new
      data-i18n/t() reference; `settings.engines.translation_h3` and
      `settings.about.title` left the dictionaries as dead keys. The
      Engines lede says "OCR engine", not the glossary's avoided "reader".
- [ ] Window drag from the titlebar still works — the search box and its
      children opt out by simply not carrying the drag attribute.
- [ ] The wizard (first run / About → Run setup wizard) still shows over
      the tabbed shell with its engine cards reading the new card look.
