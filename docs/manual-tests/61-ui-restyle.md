# 61: Restyle the UI following Gemini's redesign.html guidelines — manual tests

The restyle is CSS/markup/JS presentation; the automated suite pins the
i18n seams (key parity, markup key scan). Everything visual and every
interaction below needs eyes and hands.

## Design-token sweep (all windows)

- [ ] Results, Settings, Debug Log share the new ramp: near-black blue-grey
      surfaces, deep amber accent (`#f59e0b`, hover `#fbbf24`), rose errors,
      emerald success. No leftover yellow (`#ffc91f`) anywhere except the
      kept warning/privacy/glossary amber-yellow (`--glossary`) and the logo
      artwork itself.
- [ ] Scrollbars in every window are thin (6px) rounded pills; hovering a
      thumb brightens it.
- [ ] Latin text renders in Inter (the bundled variable font), Thai in the
      bundled Noto Sans Thai, both real weights (no faux bold on labels —
      check a bold Thai label like the Settings sidebar's active item
      against ไทย text).
- [ ] Keyboard focus still shows the amber ring (`:focus-visible`) on every
      control — Tab through Results and Settings.

## Results window

- [ ] Titlebar: the status badge reads Stopped (grey dot + pill); Start is
      the solid amber button with dark ink text.
- [ ] Start → badge flips to Translating (emerald), Start becomes a
      rose-tinted Stop, Pause turns into its amber-tinted enabled look.
      Pause → badge flips to amber "Paused"; Resume and Stop behave.
- [ ] Capture Target panel: the two radios render as the segmented control
      — the checked side amber-tinted, the idle side grey. Keyboard arrow
      keys switch the segments; clicking works. Full-screen reveals the
      Monitor dropdown; the dropdown keeps its dark native styling.
- [ ] Capture Zones: rows are bordered cards in a two-column grid, each
      with an amber dot; a zone on an unplugged monitor shows grey + struck
      through at reduced opacity. Long regions ellipsize. Rename (click the
      zone name) and delete (✕) still work.
- [ ] Excluded Blocks: entries are rose-tinted chips that wrap; ✕ removes.
- [ ] Empty state: the centered card shows the amber icon tile, the
      "No translations yet." title, five shortcut rows with pressed-key
      keycaps (`Alt`+`Shift`+`Z` …) matching the LIVE hotkey bindings after
      rebinding in Settings → Hotkeys, and the two quick actions.
      "+ New Zone" opens the picker; "Start now" starts the loop (the card
      disappears on the first translation).
- [ ] A translation card arrives with the slide-in animation and carries
      the amber edge while it is the newest; an older card keeps the plain
      border. Clicking a card copies the Thai and shows the Copied ✓ toast
      (dark ink on emerald now).
- [ ] Window drag (titlebar), double-click maximize, minimize and
      close-to-tray still work (52px titlebar).

## Settings window

- [ ] The nav is a left sidebar (icons + labels); the active tab is the
      amber-tinted segment; hovering an idle tab brightens it; About sits
      pinned at the bottom under a divider line. Clicking through all seven
      tabs swaps the pages.
- [ ] Cards: every section reads as a rounded (12px) bordered card on the
      dark ground; engine cards collapse/expand (selected engine expanded);
      the wizard (About → Run setup wizard) shows the same card look.
- [ ] Sticky save bar: edit any tracked field (e.g. Cadence) — the bar
      appears with a pulsing amber dot and "You have unsaved changes",
      Discard and Save changes on the right, amber-tinted top edge.
- [ ] Discard: with edits on several tabs (General + Text Style preview
      showing its "unsaved" chip), press Discard once — every field snaps
      back to its saved snapshot, the Text Style preview re-renders the
      saved style, and the bar hides. The config file on disk is untouched.
- [ ] Save still works per-area (invalid hotkey still rejects with its
      toast and the fields snap back).
- [ ] Hotkey inputs render in the mono face with amber text; editing and
      saving them works.
- [ ] Thai UI (`ui_language = "th"`): the whole window renders Thai
      including the new strings — sidebar labels, "You have unsaved
      changes", "Discard", "Start now", and the five shortcut labels in the
      Results empty state — with no tofu and no English fallbacks.

## Windows not depicted by the redesign (adopted tokens only)

- [ ] Debug Log: layout unchanged, colors follow the new ramp, Inter body
      font; export/clear/read-trace controls work.
- [ ] Zone picker: the selection rectangle border is the new amber; the
      instruction pill sits on the new dark ground.
- [ ] In-place Overlay: translation pills unchanged (user-styled);
      Adjust-mode rectangles keep the cyan identity; zone labels still
      render Thai bold correctly.

## Regression sweep

- [ ] `cargo test --workspace` green (the i18n key-parity test now covers
      the swapped shortcut keys and the new footer/CTA keys in both
      dictionaries).
- [ ] The wizard path on a fresh config still shows first, and Skip setup
      lands on the tabbed shell.
