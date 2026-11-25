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

- [ ] Titlebar at 800px: the mock's amber "LC" gradient tile (no app name
      — the reporter removed it), the Stopped badge, the sky-tinted
      `EN → TH` dropdown, Start (amber, play icon), Adjust, divider, pin,
      minimize, close. Nothing overlaps; the row stays 52px.
- [ ] The status bar's right end holds "v… • Tauri" and the ICON-ONLY
      Settings gear; clicking it opens/focuses the Settings window (the
      titlebar has no Settings button anymore).
- [ ] Empty state: the card hugs its content (no full-height sheet) and
      centers in the workspace.
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

- [ ] Titlebar: the gear mark only — no app name, no ☁ cloud chip (the
      privacy signal lives in the Results footer and the tray). The
      config-error chip still appears on a broken config.
- [ ] Visible hints are gone from the tab pages; the moved details live in
      tooltips: UI-Language limitation (label), OCR-engine tiers (label),
      Translation-engine ☁/Profiles note (label), Prompts (advanced) h3,
      Genderless "applies on restart" (label), screen-reading prompt
      (label), Glossary matching rule (h2), Profiles full explanation
      (h2) and the foreground rule (Profile card h3), Hotkeys format (h2),
      Updates check-only note (h3), Text Style per-row hints (labels).
      Hover each to confirm the wording (English and Thai).
- [ ] Engines lede and Glossary/Profiles ledes read as the mock's one
      lines; the packs card is the mock's shape — "Windows OCR Language
      Packs" with Re-check on the head row, list rows "English (en-US)" +
      installed/missing pill right, the how-to-install text as a row
      tooltip, "Open Windows language settings" as a quiet ghost button
      under the list.
- [ ] Glossary: the filter's magnifier icon no longer sits under the
      typed text; filtering still narrows on term or Thai and the
      no-match line reads "No terms match the filter.".
- [ ] Text Style: the LIVE PREVIEW head with Dark / Light / Game Scene
      switcher repaints only the backdrop (dark default); Font Size is
      the mock's slider with a live "(Npx)" label in the row label; the
      corner "unsaved" chip still tracks form edits.
- [ ] Hotkeys: the boxes are recorders — click ("Press keys…"), press a
      combination (e.g. Ctrl+Shift+K) and it lands in the box and flips
      the save bar; Esc cancels; clicking another box abandons the first.
      Saving still rejects taken bindings with the fields snapping back.
- [ ] Profiles: "+ New Profile" is the amber primary in the page head; the
      selector strip holds the styled dropdown (white medium text) and the
      rose Delete. A long profile name ellipsizes; at the 620px window
      minimum nothing overflows the frame.
- [ ] Cards: every section reads as a rounded (12px) bordered card; engine
      cards collapse/expand; the wizard shows the same card look.
- [ ] Sticky save bar: edit any tracked field — the bar appears with a
      pulsing amber dot and "You have unsaved changes", Discard and "Save
      Changes" on the right, amber-tinted top edge. Discard still restores
      every snapshot in one press.
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
      new footer/dropdown/toolbar/banner/tooltip keys and the removed
      hint keys in both dictionaries).
- [ ] A source-language pick from the Results titlebar while the loop runs
      restarts it exactly once (the General-tab restart rule) — watch the
      badge blink stopped→running.
- [ ] The wizard path on a fresh config still shows first, and Skip setup
      lands on the tabbed shell.

## Debug Log white window (reporter's bug — diagnosis recorded)

- [ ] With the CURRENT build: open Settings, close it (✕ hides), open the
      Debug Log from the tray — the window must show its real dark page
      ("waiting for the first pass…"). Reproduced clean on 2026-11-25
      (automated open/close/probe); see the ticket for the full
      diagnosis. The white window reported is WebView2's
      "can't reach this page" error sheet — it appears when the running
      exe navigates to a dead page origin (e.g. a `tauri dev` dev server
      that is no longer running); already-loaded windows keep their
      rendered content, so only freshly opened windows (the Debug Log)
      show it. If it reproduces on a fresh `cargo build` exe, capture the
      window with the PrintWindow probe and record the served origin.

### Addendum (2026-11-25, recurrence re-diagnosed)

The reporter hit it again. Re-verified on the current build (with
tickets 62/63/61 landed and the shell.rs WIP hunks in the tree):

- **Live server → green.** Against the running `cargo tauri dev`
  instance the exact sequence (open Settings → WM_CLOSE hide → open
  Debug Log) went GREEN 4× (automated:
  `.scratch/diag-white-window/sdc-cycle.ps1`); the Debug Log rendered
  its dark page every time.
- **Killing the harness kills the app.** Terminating the cargo/
  cargo-tauri chain takes the app down with it — the "orphaned app +
  dead server" state is NOT reachable by closing the dev console. The
  white-window state is reached only by **launching the dev-baked
  `target\debug\langception.exe` directly** with no server.
- **Direct launch → red, reproduced.** With port 1430 free, launching
  the dev exe directly: Settings probes 0.95 white, Debug Log 0.87
  white, stuck at every sample. Screenshot (`sdc-dbg1-001844.png` in
  the diag dir) records the served origin: **http://127.0.0.1:1430 —
  "127.0.0.1 refused to connect", ERR_CONNECTION_REFUSED**.
- **Not the recurrence machine:** no Langception autostart anywhere
  (HKCU Run, HKLM Run, Startup folder all checked) — the recurrence is
  a manual direct launch of the dev exe.
- Prevention stands: drive the app through `cargo tauri dev` (or a
  release build); if white windows appear, the instance is a dev exe
  without its server — quit it from the tray and start
  `cargo tauri dev`. No app code change.

### Addendum 2 (2026-11-25, superseded by ticket 65)

The re-diagnosis continued: the reporter isolated the real trigger —
"Keep Settings window on top" + save + close Settings + open the Debug
Log. The reporter's white window was NOT the dead-origin error sheet: it
was a hidden window's DWM surface composing as a blank white sheet at
its last rect (invisible to window enumeration, which is why the earlier
live probes missed it — they sampled the wrong rect). That is a real
app bug and it is FIXED in ticket 65 (DWM cloak around the frameless
windows' hide/show) — see `docs/manual-tests/65-ghost-white-sheet.md`.
The dead-origin diagnosis above remains valid for the stale-dev-build
case only: a WebView2 "can't reach this page / 127.0.0.1 refused to
connect" sheet means a `cargo tauri dev` build running without its
server, not this bug.

### Addendum 3 (2026-11-25, third feedback round)

- **(?) tooltips**: every tooltip-carrying label/heading shows the
  mock's small "(?)" after its text — UI Language, Translation Context,
  OCR engine, Translation engine, Prompts (advanced), Genderless,
  Screen-reading prompt, Windows OCR Language Packs, Glossary,
  Profiles, Profile card, Font, Auto colors, Background cover source,
  Text alignment, Placement, Hotkeys, Updates, Check on launch.
  Hovering it shows the moved detail (verify in English and Thai);
  inputs/selects keep their titles without a marker.
- **One pin for both windows**: the General tab no longer carries "Keep
  Results window on top" / "Keep Settings window on top" — the Results
  titlebar's pin is the single control. Pinning raises BOTH windows
  above other apps (check the Settings window follows), unpinning drops
  both, and the state survives restart (the same two config flags,
  written together by `set_windows_on_top`). A General save (e.g.
  changing Cadence) must never alter the pin state.

## Addendum 4 — live UI-Language switch reaches every window (67bce44)

1. Launch the app, open Settings and keep the Results window visible.
2. Settings → General → UI Language: switch English → ไทย, Save.
3. Expect: the Settings page flips to Thai immediately (as before), AND the Results window flips the same moment — status pill, Start/Pause labels, config bar, zone cards, history toolbar, footer — no reopen, no restart.
4. Switch back ไทย → English: both windows flip back.
5. Debug Log and the overlays follow too: open the Debug Log while Settings saves a language change — its static labels are already in the new language.
6. Reverse direction: change the translation pair in the Results titlebar while Settings is open — the Settings General tab's Source language select follows immediately (unless you have an unsaved edit in it; your edit wins).
7. Pin interplay: click the Results titlebar pin, then change any General field and Save — the pin state survives the save (the saved on-top values track the pin's broadcast).
