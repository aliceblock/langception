# Manual tests — Ticket 28: Check for new version button + auto-check on launch

The mechanics are covered offline by `cargo test`: the version comparator
(equal / older / newer / multi-digit / leading-`v`), the payload parser
(case-insensitive find, unknown fields ignored, missing entry, malformed
JSON), the fetch classification against a mock server (200 / 404 / bad
body / refused connection / empty URL, and the request carrying nothing
beyond the bare GET), the config roundtrip + pre-ticket-28 defaults, and
the outcome JSON shape the Settings page reads. This checklist exercises
what only a live desktop shows: the Settings flow against a real endpoint,
the launch-time check, and the browser handoff.

## Setup

1. A local mock endpoint serving the payload (from the repo root):

   ```sh
   # latest.json — entry version ABOVE the running app version (0.1.0):
   echo '{ "apps": [ { "name": "langception", "version": "99.0.0", "latest_update": "2026-11-20", "download_url": "https://github.com/me/langception/releases/latest" } ] }' > latest.json
   python -m http.server 8765
   ```

2. `config.toml` → `[update] url = "http://127.0.0.1:8765/latest.json"`
   (loopback HTTP, so the download-page action is openable), then start
   the app. `check_on_launch = false` (the default).

## Checklist

- [ ] **Up to date**: set the mock payload's `version` to `0.0.1` (below
      the running version), press **Check for new version** (Settings →
      About) — the result area reports "You are on the latest version",
      no download-page button, nothing crashes.
- [ ] **New version**: set the payload back to `99.0.0`, check again —
      the area shows "New version available: 99.0.0 · Released:
      2026-11-20" and an **Open download page** button appears; clicking
      it opens the system browser at the payload's `download_url`
      (nothing is downloaded by the app itself).
- [ ] **Released date optional**: remove `latest_update` from the
      payload — the new-version line shows no "Released:" part. Remove
      `download_url` too — no download-page button at all.
- [ ] **No update information**: serve a payload without a
      `langception` entry (e.g. only `{"apps":[{"name":"other","version":"1.0.0"}]}`)
      — the area reports gently that no update information is published
      there yet.
- [ ] **Unavailable / gentle failures**, one press each: `url = ""`
      (reports unavailable, and the mock server's log shows NO request
      arrived); server stopped (network failure message); server
      returning 404 (names HTTP 404); server returning an HTML page
      (malformed message). None of these crash or toast — the area says
      "Update check unavailable".
- [ ] **Launch-time check**: set `check_on_launch = true` in Settings →
      About (it persists immediately to config.toml without a Save
      footer) and restart the app with the "newer version" payload
      served — opening Settings → About shows the new-version result
      without pressing anything. With the toggle off (default) a restart
      provably fetches nothing (mock server log stays empty).
- [ ] **Non-blocking, silent**: with `check_on_launch = true` and the
      server stopped, restart the app — startup is instant, no warning
      or toast anywhere; Settings → About later shows the unavailable
      result.
- [ ] **Pre-existing config**: a config.toml written before this ticket
      (no `[update]` section) loads with `check_on_launch` unchecked and
      url empty — nothing breaks.
- [ ] **Refused opener**: with the mock payload's `download_url` set to
      `http://example.com/evil` (non-loopback HTTP), the new-version
      result shows the button but clicking it errors with a refusal
      toast instead of opening the browser.

## Cleanup

Stop the mock server and restore `check_on_launch = false` (or clear the
whole `[update]` section — the defaults load back).
