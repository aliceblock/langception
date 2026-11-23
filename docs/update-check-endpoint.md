# Update-check endpoint (ticket 28)

How the app learns a new version exists, and how that information is
published. **Decision (maintainer, 2026-11-23, revised):** the endpoint is
a `latest.json` file at the root of the app's **public GitHub repo**,
fetched via its raw URL — this supersedes the same day's earlier decision
to serve it from intception.app (kept below as a fallback). The payload
contract is unchanged by the hosting move: still the app list, still the
same client behavior.

## The payload (the client contract — stable)

`GET <the latest.json URL>` over HTTPS returns:

```json
{
  "apps": [
    {
      "name": "langception",
      "version": "0.2.0",
      "latest_update": "2026-11-20",
      "download_url": "https://github.com/<owner>/langception/releases/latest"
    }
  ]
}
```

Field semantics (what the app's client does with each):

| Field | Meaning | Client behavior |
| --- | --- | --- |
| `name` | App slug; Langception's is `langception` | Matched case-insensitively to find this app's entry |
| `version` | Latest released version, bare `x.y.z` (a leading `v` is tolerated) | Compared semantically against the running version — numeric components, so `0.2.10 > 0.2.9` |
| `latest_update` | Release date, ISO 8601 (`YYYY-MM-DD`) | Informational only — shown as "Released: …" when present, ignored otherwise |
| `download_url` | Absolute HTTPS URL where the new version is obtained | Offered as an "Open download page" action when a newer version exists; opened in the system browser only on click |

Rules the endpoint should hold:

- Respond `200` with `content-type: application/json`; no auth, no cookies.
- Unknown/extra fields anywhere in the payload are ignored by the client —
  adding fields later (`notes`, `min_version`, more apps) needs no app
  rebuild.
- An entry named `langception` must exist; if the app's entry is missing,
  it reports "no update information" gently.

Adding a second app later: append another object to `apps`. Nothing
changes in Langception.

## Hosting: the public main repo (the decision)

- File: `latest.json` at the repo root.
- URL: `https://raw.githubusercontent.com/<owner>/langception/main/latest.json`
- The repo must be **public** — unauthenticated requests return 404 on
  private repos. Going public is ticket 13's planned human step anyway
  (Apache-2.0 LICENSE is in tree), so one move covers publishing and
  hosting.
- GitHub Releases (same repo) hosts the installers; `download_url` points
  at `https://github.com/<owner>/langception/releases/latest`, which
  GitHub redirects to the newest release.
- Raw files are CDN-cached (~5 minutes) — irrelevant at version-check
  frequency.
- GitHub Pages (`<owner>.github.io/langception/latest.json`) is an
  optional later upgrade: cleaner URL, same file, flipping the default is
  a one-line change.

## Why not the GitHub Releases API as the check source

`api.github.com/repos/<owner>/langception/releases/latest` is the
single-source-of-truth option (cutting a release IS publishing the
version — no second file to keep in sync), and it returns `tag_name`,
`browser_download_url`, and `published_at`. It was rejected as the
primary check for one reason: unauthenticated requests are rate-limited
to **60/hour per IP**, and users behind a shared NAT (office, dorm) can
exhaust that together. It remains a possible fallback — but the client
contract would change (different payload shape), so that would be its own
ticket.

## Publishing steps (human; rides ticket 13's go-public)

1. Create the public repo and push this history (`docs/releasing.md` §
   One-time).
2. Add `latest.json` at the repo root (payload above, `version` set to
   the release number, `download_url` at the Releases page).
3. Flip the client's `DEFAULT_UPDATE_URL` const (empty until now) to the
   raw URL — one line, in the same change. Release builds then check for
   updates with zero per-user setup; development builds simply report
   "update check unavailable" until then.
4. Verify:

   ```sh
   curl https://raw.githubusercontent.com/<owner>/langception/main/latest.json
   ```

Per release afterwards: update `version`, `latest_update`, and
`download_url` in `latest.json`, and make sure the installer is attached
to the Release the URL points at. (A follow-up can automate the
`latest.json` bump in `release.yml` alongside the artifact publish.)

## Fallback hosting (same payload, any plain HTTPS + JSON)

- **intception.app/app_version** — the earlier decision, still fine: any
  route on the webapp serving the same JSON works, and `[update] url` is
  a normal Settings field.
- **Cloudflare Worker** on a custom domain (free tier: 100,000
  requests/day).
- **A raw file in any other public repo or Gist.**

Considered and rejected: DNS TXT via Cloudflare DoH (2026-11-22) — a
different client fetch/parse path; never built. GitHub Releases API as
the primary check — rate limit, above.

## Relationship to the app (client side, ticket 28)

- The check is informational only: no download, no install, no signature
  verification. The auto-update machinery (silent installer runs, helper
  scripts, or `tauri-plugin-updater` with signed manifests) is a possible
  future ticket building on this endpoint — deliberately out of scope.
- SmartScreen / code-signing friction around freshly downloaded
  installers is a release-time concern — covered by ticket 13's manual
  install checklist, not by the version check.
- Failures (endpoint not live yet, network down, bad JSON) are reported
  gently and never block the app; the auto-check (default off) fails
  silently.
- With the endpoint absent, the manual button reports "update check
  unavailable" — exactly ticket 28's shipped behavior.
- Clearing `[update] url` in Settings disables the feature entirely.
