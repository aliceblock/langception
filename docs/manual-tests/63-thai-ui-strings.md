# Manual tests — Ticket 63: Thai UI strings (full ไทย dictionary + typography)

The dictionary contract is covered offline by `cargo test`
(`i18n_dictionaries_are_flat_maps_in_full_key_parity`: th.json holds
exactly the en.json key set with every `{placeholder}` and inline tag
preserved; `tray_text_thai_strings_are_pinned`,
`the_notices_seam_resolves_the_wording_per_language`, and
`the_substitution_notice_names_both_tiers` pin the Rust-side Thai
byte-for-byte). What only a live desktop shows is whether the Thai
actually renders — real bold from the bundled Noto Sans Thai Bold, no
tofu or fallback-system-font look in any window — and when each surface
picks the language up.

## Setup

1. Build and run the dev app: `cargo run -p langception`.
2. Switch the UI language: Settings → General → **UI Language** → ไทย →
   **Save changes** (this is the acceptance path; hand-editing
   `[ui] ui_language = "th"` in `config.toml` is the fallback and is
   covered by the restart item below).

## Checklist

- [ ] **Settings applies Thai live**: saving with ไทย selected re-renders
      the open Settings window in Thai immediately (tabs, General labels,
      engine cards and their rich hints, glossary, profiles, prompts, text
      style, hotkeys, About) — no reopen needed. The select itself keeps
      showing native names (English / ไทย) in both languages.
- [ ] **Tray menu relabels live, no restart**: right after the same save,
      the tray menu shows Thai (แสดงหน้าต่างผลลัพธ์ / เริ่มการแปล /
      โซนจับภาพใหม่ / จับภาพครั้งเดียว / ตั้งค่า… / ออกจากโปรแกรม…), and
      the tooltip states carry the Thai wording (Langception —
      กำลังทำงาน). This works because every `apply_state` rebuilds the
      whole menu from `tray_text`, and a General save ends in
      `apply_state` — no restart or next-build wait.
- [ ] **Other windows pick Thai up on next show**: close the Results
      Window and reopen it from the tray — status pills, buttons, panel
      titles, empty-state hints (with the configured hotkeys interpolated)
      are Thai. Same for the Debug Log window and the wizard (About →
      เรียกใช้ตัวช่วยตั้งค่า).
- [ ] **Restart = Thai from the start**: quit (tray → ออกจากโปรแกรม) and
      relaunch with `ui_language = "th"` — every window opens in Thai at
      first paint with no English flash (windows are created hidden and
      shown only after their dictionaries resolve).
- [ ] **Picker and overlays**: trigger a One-shot Capture and a New Zone
      draw — the bar text (จับภาพครั้งเดียว — ลากกรอบเพื่อแปลครั้งเดียว ·
      Esc เพื่อยกเลิก) is Thai; with a loop running, enter Adjust Mode —
      the overlay notes and the Saving… indicator are Thai.
- [ ] **Emitted notices in Thai**: with Thai active, start the loop with
      nothing watched — the WATCHING_NOTHING warning in the Results Window
      is the Thai wording. Select engine PP-OCRv6-tiny + source ญี่ปุ่น and
      run a pass — the OCR log carries the Thai substitution line naming
      PP-OCRv6-tiny and PP-OCRv6-small (tier names stay Latin). Note: a
      warning pushed in one language keeps its wording until it clears —
      switching languages mid-warning does not reword the existing notice
      (the hint in Settings says the same).
- [ ] **Thai bold is real bold**: headings, section labels, and status
      chips (weight 600/700) render with the bundled Noto Sans Thai Bold —
      clean thick strokes, not the smeared synthetic thickening Chromium
      paints when only a Regular face is declared. Compare a Thai heading
      against Thai body text: the contrast should match the Latin
      heading/body contrast.
- [ ] **No tofu anywhere**: sweep every window for missing-glyph boxes —
      including user data: a zone named in Thai on the overlay, a Thai
      glossary term in the Settings rows, a Thai preset name in the
      selects. Everything renders in the bundled face (window stacks are
      "Segoe UI", "Noto Sans Thai" — Latin stays Segoe UI, Thai falls
      through).
- [ ] **English unchanged**: switch back to English and save — Settings
      and the tray relabel live again, and every string reads exactly as
      before ticket 63 (the English arm is pinned byte-for-byte by test;
      en.json is untouched by this ticket).
