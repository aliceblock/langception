# Manual smoke checklist — Ticket 13 (Release packaging: CI + NSIS + portable zip)

Everything mechanical about the pipeline is checked where it can be: the
workflow files are linted by GitHub's parser when the workflows run, the
tag/version match check runs inside the release job, and `cargo tauri
build` was run locally to produce both artifacts before the release CI
existed. This checklist covers what only a real machine can show: the
artifacts install/extract/run correctly, and the release workflow publishes
end-to-end. Run once per release on (ideally) two machines: a **clean
Windows 10** VM without WebView2 (worst case) and a **Windows 11** machine
(normal case).

## Setup

1. A machine without WebView2 and without Langception (clean VM or a
   user profile that never installed either).
2. The release's two artifacts downloaded from the GitHub Release (not
   from a local build output).
3. For the CI leg: the repo pushed to GitHub with Actions enabled.

## Checklist

### Release pipeline (GitHub)

- [ ] Pushing a `vX.Y.Z` tag (matching `tauri.conf.json`'s version) starts
      the **Release** workflow on windows-latest and it goes green.
- [ ] The Release page for the tag carries both artifacts:
      `Langception_X.Y.Z_x64-setup.exe` and
      `Langception_X.Y.Z_x64-portable.zip`, plus auto-generated notes.
- [ ] A tag that does NOT match the config version (e.g. `v9.9.9`) fails
      fast in the tag/version check step, before any build.
- [ ] CI (`.github/workflows/ci.yml`) runs clippy + tests green on pushes
      to main and on PRs.

### NSIS installer — clean Windows 10 without WebView2

- [ ] SmartScreen may warn ("Windows protected your PC") on first run of
      the installer — More info → Run anyway proceeds.
- [ ] The installer runs **without an admin prompt** (currentUser
      install), shows English or Thai UI matching the OS language.
- [ ] On the machine without WebView2, the installer downloads and
      installs the WebView2 Runtime silently, then completes.
- [ ] After install: Start-menu shortcut exists; launching it opens the
      Results Window + tray icon; the setup wizard appears on first launch
      (fresh profile).
- [ ] Windows Settings → Apps shows "Langception" with the release
      version and "The Langception Authors" as publisher; uninstall
      removes the app, shortcuts, and (if offered) leaves the config
      folder in place.

### Portable zip

- [ ] The zip extracts to a `Langception/` folder containing
      `Langception.exe`, `README.txt`, `LICENSE.txt` — nothing else, no
      installer.
- [ ] Extracted to a different location, `Langception.exe` launches to
      tray + Results Window on the WebView2-equipped machine.
- [ ] Running it a second time shows the single-instance behavior (focus
      of the existing window, no second app).
- [ ] The zip's README mentions the WebView2 requirement and the config
      folder location.

### Version metadata

- [ ] The installed app's About tab (Settings → About) reports the release
      version and the config path.
- [ ] The exe file's Properties → Details shows File version =
      release version, product name Langception.
- [ ] The two artifacts' filenames carry the same version as the tag.

### README accuracy (follow it literally as a new user would)

- [ ] Install → Releases table matches the actual artifact names.
- [ ] First-run section matches what actually happens (wizard appears,
      skippable, zero-config works).
- [ ] OCR pack section: on a machine missing a pack, Settings → Engines
      lists the gap and the jump button opens Windows' Language & region
      page.
- [ ] Engine key setup: each engine's Settings card has the same signup
      link the README lists; pasting a key + Test works for one keyed
      engine.
- [ ] LM Studio guide: following it verbatim (start server, Check,
      pick model, set engine) produces working local translation.
- [ ] The anti-cheat caution section is present and honest.

## Notes

- The WebView2 bootstrapper needs internet at install time. If a truly
  offline clean-machine install must ever be supported, switch
  `bundle.windows.webviewInstallMode` to `offlineInstaller` (large
  installer, no internet needed).
- Known non-goals: code signing (unsigned → SmartScreen warning,
  documented in the README), MSI bundle, auto-update channel.
