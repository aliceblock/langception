# 61: Restyle the UI following Gemini's redesign.html guidelines — manual tests

The restyle is CSS/markup/JS presentation plus the small backend seams the
mock's elements need (`set_source_language`, `set_results_on_top`, the
footer's status fields); the automated suite pins the i18n seams (key
parity, markup key scan) and the restart rule. Everything visual and every
interaction below needs eyes and hands. (Reopened pass: the mock's compact
type scale, icons everywhere, the interactive language dropdown, the pin,
the collapsible config section, the log toolbar, the Adjust banner, the
footer status bar, the Settings profile strip + glossary filter, and the
narrow-width overflow fix.)

## Design-token sweep (all windows)

- [ ] Results, Settings, Debug Log share the new ramp: near-black blue-grey
      surfaces, deep amber accent (`#f59e0b`, hover `#fbbf24`), rose errors,
      emerald success. No leftover yellow (`#ffc91f`) anywhere except the
      kept warning/privacy/glossary amber-yellow (`--glossary`) and the logo
      artwork itself.
- [ ] The compact scale is consistent: 12px labels/controls, 14px window
      titles, 16px Settings page titles — nothing still at the old 16-18px
      body sizes.
- [ ] Inline Lucide icons render everywhere the mock has them (Results
      header buttons, segmented control, config actions, zone trash,
      exclusion ✕, toolbar, empty-state glyph, toast, Settings sidebar);
      nothing shows a blank box.
- [ ] Scrollbars in every window are thin (6px) rounded pills; hovering a
      thumb brightens it.
- [ ] Latin text renders in Inter (the bundled variable font), Thai in the
      bundled Noto Sans Thai, both real weights (no faux bold on labels —
      check a bold Thai label like the Settings sidebar's active item
      against ไทย text).
- [ ] Keyboard focus still shows the amber ring (`:focus-visible`) on every
      control — Tab through Results and Settings.

## Results window

- [ ] Titlebar at 800px, stopped: logo, "Langception", Stopped badge, the
      sky-tinted `EN → TH` dropdown button, then Start (amber, play icon),
      Adjust, Settings (icons + labels), divider, pin, minimize, close.
      Nothing overlaps; the row stays 52px.
- [ ] Language dropdown: clicking opens the "Translation Pair" menu with the
      four native-name pairs and mono codes, the current pair's code in
      amber. Picking a different pair persists it (config) and restarts the
      loop if it was running; the chip and the menu's amber mark follow.
      Click-away closes it.
- [ ] Pin: click toggles amber (always-on-top applies immediately and
      survives reopen — it writes the same config flag as Settings →
      General's "Keep Results window on top", which reflects the change).
- [ ] Start → badge flips to Running (emerald), Start hides, amber Pause
      and rose Stop appear; the whole row still fits 800px with no
      overlap. Pause → badge Paused, the button swaps to Resume (play
      icon); Resume and Stop behave.
- [ ] Config section top bar: "Capture Mode:" label, segmented control
      (icons, amber checked side, live zone-count badge), "New Zone"
      (zones mode) / "Exclude Area" (full-screen mode), "Hide"/"Show"
      collapse toggle that rotates its chevron.
- [ ] Zones view: two-line bordered cards (name over mono dims) in a
      two-column grid with amber dots; unplugged monitor zones grey +
      struck; trash-icon delete and click-to-rename still work; the empty
      hint names the LIVE hotkey as keycaps.
- [ ] Full-screen view: "Target Display:" + monitor dropdown + the hint on
      the right; "Excluded Blocks (N)" with the shield-off icon; rose
      chips with ✕ removal; the empty line reads "No excluded areas
      (e.g. the Taskbar, …)". Exclude Area enters Adjust Mode (the real
      exclusion flow) and the amber banner appears.
- [ ] Log toolbar: once a translation arrives it reads "Translation
      History (N)"; Clear wipes the feed client-side (loop keeps running)
      and the empty-state card returns.
- [ ] Empty state: amber icon tile, "Ready to translate text on your
      screen" + lede, five keycap shortcut rows matching the LIVE hotkeys,
      "+ Draw Capture Zone" and "Start Now" (disabled while running).
- [ ] A translation card arrives with the slide-in animation, amber zone
      badge, mono timestamp, hover copy affordance (whole card still
      copies → dark toast bottom-right with the info glyph), and carries
      the amber edge while newest.
- [ ] Footer status bar: "OCR Engine: …" (Windows OCR / PP-OCRv6 tier /
      Vision model in fused reading), "Overlay: Visible/Hidden" following
      the loop, "v… • Tauri" on the right, and the ☁ cloud badge while a
      cloud engine translates.
- [ ] Window drag (titlebar), double-click maximize, minimize and
      close-to-tray still work.

## Settings window

- [ ] The nav is a left sidebar (icons + labels, "Engines & OCR" wording);
      the active tab is the amber-tinted segment; About sits pinned at the
      bottom. Clicking through all seven tabs swaps the pages.
- [ ] Profiles: "+ New Profile" is the amber primary in the page head; the
      selector strip holds the styled dropdown (white medium text) and the
      rose Delete. A long profile name ellipsizes; at the 620px window
      minimum nothing overflows the frame (the reported bug).
- [ ] Glossary: the filter box under the import/export row narrows the
      list on term or Thai text; an active filter with no matches shows
      "No terms match the filter." (not "No terms yet.").
- [ ] Cards: every section reads as a rounded (12px) bordered card; engine
      cards collapse/expand; the wizard shows the same card look.
- [ ] Sticky save bar: edit any tracked field — the bar appears with a
      pulsing amber dot and "You have unsaved changes", Discard and "Save
      Changes" on the right, amber-tinted top edge. Discard still restores
      every snapshot in one press.
- [ ] Hotkey inputs render as centered mono amber boxes with the mock's
      fixed width; editing and saving them works.
- [ ] Thai UI (`ui_language = "th"`): the whole window renders Thai —
      sidebar labels ("เอนจินและ OCR"), "คุณมีการแก้ไขที่ยังไม่ได้บันทึก",
      "บันทึกการเปลี่ยนแปลง" — and the Results window carries the mock's
      own Thai sentences ("พร้อมแปลข้อความบนหน้าจอ",
      "แปลทุกข้อความบนหน้าจอ ยกเว้นพื้นที่ใน Excluded Blocks") with no
      tofu and no English fallbacks.

## Windows not depicted by the redesign (adopted tokens only)

- [ ] Debug Log: layout unchanged, colors follow the new ramp, Inter body
      font; export/clear/read-trace controls work.
- [ ] Zone picker: the selection rectangle border is the new amber; the
      instruction pill sits on the new dark ground.
- [ ] In-place Overlay: translation pills unchanged (user-styled);
      Adjust-mode rectangles keep the cyan identity; zone labels still
      render Thai bold correctly.

## Regression sweep

- [ ] `cargo test --workspace` green (the i18n key-parity test covers the
      new footer/dropdown/toolbar/banner keys and the removed
      mode_panel/zones_title/empty_hint keys in both dictionaries).
- [ ] A source-language pick from the Results titlebar while the loop runs
      restarts it exactly once (the General-tab restart rule) — watch the
      badge blink stopped→running.
- [ ] The wizard path on a fresh config still shows first, and Skip setup
      lands on the tabbed shell.
