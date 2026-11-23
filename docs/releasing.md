# Releasing Langception

How the public repo is published, and how a release is cut.

## One-time: publish the public repo

The GitHub repo is created by a human (it is an outward-facing action under
their account; `gh` may not be installed locally):

1. **Strip the local issue tracker from history** (decision 2026-11-23:
   `.scratch/` never goes public). The tracker has been committed since
   the very first commit, so a `.gitignore` entry alone would still
   expose all of it in the early public history. This must happen
   **before** the first push — afterwards the history is public and
   effectively frozen. Commit all pending work first (`git filter-repo`
   discards uncommitted changes), and back the tracker up out of the
   worktree (the rewrite's `reset --hard` deletes tracked files that the
   rewritten history no longer contains):

   ```bash
   pip install git-filter-repo
   cp -r .scratch ../langception-tracker-backup
   git filter-repo --force --invert-paths --path .scratch
   ```

   Repair the tracker's internal commit citations — filter-repo rewrites
   every SHA, and its map (`.git/filter-repo/commit-map`, one
   `<old-sha> <new-sha>` per line) drives the fixup. Commits that only
   touched the tracker are pruned from public history and map to
   all-zeros — skip those; their old citations simply dangle:

   ```bash
   while read -r old new; do
     case "$new" in 0000*) continue ;; esac
     sed -i "s/${old:0:7}/${new:0:7}/g" ../langception-tracker-backup/langception/issues/*.md
   done < .git/filter-repo/commit-map
   ```

   Re-land the tracker as untracked and keep it out forever:

   ```sh
   mv ../langception-tracker-backup .scratch
   echo ".scratch/" >> .gitignore
   git add .gitignore
   git commit -m "Keep the local issue tracker out of the public repo"
   ```

2. Create a **public** repo on GitHub (e.g. `<user>/langception`), empty —
   no README/LICENSE init, so this history pushes cleanly.
3. Add the remote and push the full history:

   ```sh
   git remote add origin git@github.com:<user>/langception.git
   git push -u origin main
   ```

   This push also puts ticket 28's version-check endpoint live (decided
   2026-11-23): follow `docs/update-check-endpoint.md`'s § Publishing
   steps in the same move — `latest.json` at the repo root, and the
   `DEFAULT_UPDATE_URL` flip (two lines: the const in
   `crates/langception-core/src/update.rs` plus the matching `url` line
   in the config template, kept honest by the
   `template_url_tracks_the_default_const` test).

4. Confirm the Actions tab shows the CI workflow green on `main`.
5. Optional: add a CI badge to the README top with the real repo URL, and a
   `repository` field in both `Cargo.toml` files. Also enable
   Settings → Actions → General → allow read/write for the release job's
   GITHUB_TOKEN (default on for new repos unless restricted by org policy).

## Cutting a release

A release is a tag push; CI does the rest
(`.github/workflows/release.yml`):

1. Bump the version in **both** places (they must stay identical):

   - `src-tauri/tauri.conf.json` → `version` (drives the installer, the
     exe's version resource, and the About tab)
   - `Cargo.toml` → `[workspace.package] version` (both member crates
     inherit it via `version.workspace = true`)

2. Update `Cargo.lock` and commit:

   ```sh
   cargo update --workspace -v   # or just build once; commit the lock
   git commit -am "vX.Y.Z"
   ```

3. Tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   The tag must be `v` + the config version — the release workflow checks
   this first and fails fast on a mismatch.

4. Watch the **Release** workflow run: tests → `cargo tauri build` →
   portable zip assembly → publish. When green, the tag's GitHub Release
   has both artifacts:

   - `Langception_X.Y.Z_x64-setup.exe` — NSIS installer (currentUser
     install; downloads WebView2 at install time when missing)
   - `Langception_X.Y.Z_x64-portable.zip` — bare exe + README + LICENSE

   Release notes are auto-generated from commits since the previous tag.

## Artifact details

| Decision | Where | Why |
| --- | --- | --- |
| WebView2: `downloadBootstrapper` (silent) | `bundle.windows.webviewInstallMode` | Clean Windows 10 machines without the runtime get it at install time (needs internet). Windows 11 always has it preinstalled. `offlineInstaller` would make the installer huge to cover an edge case. |
| `installMode: currentUser` | `bundle.windows.nsis` | No admin prompt; installs under `%LOCALAPPDATA%\Programs\Langception` — appropriate for a per-user tool. |
| Installer languages `["English", "Thai"]` | `bundle.windows.nsis` | The NSIS UI follows the OS language; Thai users get a Thai installer shell. Tauri logs "Custom tauri messages for Thai are not translated" — the untranslated strings (install-mode chooser, language-selector dialog) never render under `currentUser` + no selector, so no `customLanguageFiles` needed. |
| Portable zip assembled by hand | release.yml | Tauri has no portable target; the exe embeds `ui/` (static frontendDist), so the bare exe is the portable app. |

## Post-release manual checks

Run `docs/manual-tests/13-release-packaging-ci.md` on a real machine
(ideally a clean Windows 10 VM without WebView2 to prove the bootstrapper
path, and any Windows 11 machine for the normal path).
