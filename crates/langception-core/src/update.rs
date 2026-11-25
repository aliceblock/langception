//! The Update Check (ticket 28, CONTEXT.md): the client half of the
//! maintainer's published app list — fetch it, find Langception's entry by
//! name, compare versions, and classify the outcome for the Settings
//! surface. Check-only by design: nothing here downloads or installs
//! anything, and the request is a bare GET (no key, no cookies, no
//! identifying headers — the fetch itself is the whole request).
//!
//! The endpoint (a `latest.json` at the root of the public GitHub repo, per
//! `docs/update-check-endpoint.md`) does not exist yet, so every failure
//! path — empty URL, network failure, non-200, malformed JSON — degrades
//! into an honest [`UpdateCheckOutcome`] the UI reports gently. Payload
//! parsing and version comparison are pure and unit-tested offline; the
//! fetch is exercised against a localhost mock server in the tests.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::Duration;

/// This app's slug in the published app list; entries are matched against
/// it case-insensitively so `Langception` and `LANGCEPTION` both find it.
pub const APP_NAME: &str = "langception";

/// The update URL release builds check by default — the raw `latest.json`
/// at the root of the public repo, flipped at go-public (tickets 13/28) so
/// release builds work with zero per-user setup. The payload shape lives
/// in `docs/update-check-endpoint.md` § The payload; the config template's
/// `url` line must match this (a unit test pins the two together).
pub const DEFAULT_UPDATE_URL: &str =
    "https://raw.githubusercontent.com/intception-app/langception/main/latest.json";

/// The `[update]` config section: where to check, and whether launches
/// check by themselves. Both keys predate the feature for configs written
/// before ticket 28 via the container default — an old config.toml loads
/// with the check disabled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// The app-list JSON's URL. Empty (the default) disables the feature
    /// entirely: no fetch is ever made and the button reports the
    /// unavailable state.
    pub url: String,
    /// Check automatically at every launch (Settings → About's toggle).
    /// Off by default — opt-in, since a launch check contacts the URL
    /// without being asked. A launch check never blocks startup and its
    /// failures stay silent.
    pub check_on_launch: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            url: DEFAULT_UPDATE_URL.to_string(),
            check_on_launch: false,
        }
    }
}

/// One version check's verdict, exactly what the Settings surface reports.
/// Every failure is an honest variant, never a crash: `Unavailable` (the
/// feature is off), `NotListed` (endpoint reachable, no Langception
/// entry), or `Error` (network failure, non-200, malformed JSON — with a
/// plain-language reason).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UpdateCheckOutcome {
    /// The URL is empty — the feature is disabled in this install.
    Unavailable,
    /// The published version is not newer than the running one.
    UpToDate,
    /// A newer version exists. `released` (the payload's `latest_update`)
    /// and `download_url` are shown/offered only when the payload carries
    /// them.
    #[serde(rename_all = "camelCase")]
    NewVersion {
        version: String,
        released: Option<String>,
        download_url: Option<String>,
    },
    /// The endpoint answered with a well-formed app list that has no
    /// entry for [`APP_NAME`].
    NotListed,
    /// The check could not complete (network failure, non-200, malformed
    /// JSON); the message is display-ready and blame-free.
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateCheckError {
    /// `url` is empty — the feature is off in this install.
    Unavailable,
    /// The fetch failed (DNS, refused, timeout — anything below HTTP).
    Network(String),
    /// The server answered, but not with 200.
    Status(u16),
    /// The body is not the expected JSON shape.
    Malformed(String),
}

impl std::fmt::Display for UpdateCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("no update URL is configured"),
            Self::Network(e) => write!(f, "cannot reach the update server: {e}"),
            Self::Status(code) => write!(f, "the update server answered HTTP {code}"),
            Self::Malformed(detail) => write!(f, "the update information is malformed: {detail}"),
        }
    }
}

/// One app's entry in the published list. Unknown fields anywhere in the
/// payload (here, on sibling entries, or at the top level) are ignored by
/// serde's default, so the maintainer can add `notes`, `min_version`, or
/// more apps later without an app rebuild.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AppListEntry {
    pub name: String,
    /// Latest released version, bare `x.y.z` (a leading `v` is tolerated
    /// by [`compare_versions`]).
    pub version: String,
    /// Release date (ISO 8601), informational only.
    pub latest_update: Option<String>,
    /// Where the new version is obtained — opened in the system browser
    /// only on the user's click, never fetched by the app.
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppListPayload {
    #[serde(default)]
    apps: Vec<AppListEntry>,
}

/// Parses the published payload into its app entries. Malformed JSON (an
/// HTML error page, a truncated file) is an [`UpdateCheckError::Malformed`],
/// not a panic; a JSON body without an `apps` array simply has no entries.
pub fn parse_app_list(body: &str) -> Result<Vec<AppListEntry>, UpdateCheckError> {
    serde_json::from_str::<AppListPayload>(body)
        .map(|payload| payload.apps)
        .map_err(|e| UpdateCheckError::Malformed(e.to_string()))
}

/// Finds this app's entry — name matched case-insensitively per the
/// payload contract.
pub fn find_app_entry<'a>(entries: &'a [AppListEntry], name: &str) -> Option<&'a AppListEntry> {
    entries
        .iter()
        .find(|entry| entry.name.trim().eq_ignore_ascii_case(name))
}

/// Compares two dotted version strings numerically, answering whether
/// `latest` is newer than `current`: `Less` = update available, `Equal` =
/// same, `Greater` = running ahead of the published entry. A leading `v` /
/// `V` is tolerated on either side; components compare as numbers, so
/// `0.2.10 > 0.2.9`; a missing component counts as `0`, so `1.0` equals
/// `1.0.0`; a component without leading digits counts as `0`.
pub fn compare_versions(current: &str, latest: &str) -> Ordering {
    let numeric_components = |version: &str| -> Vec<u64> {
        version
            .trim()
            .trim_start_matches(['v', 'V'])
            .split('.')
            .map(|component| {
                component
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let current = numeric_components(current);
    let latest = numeric_components(latest);
    for index in 0..current.len().max(latest.len()) {
        // A missing component is 0, so `1.0` == `1.0.0`.
        let a = current.get(index).copied().unwrap_or(0);
        let b = latest.get(index).copied().unwrap_or(0);
        if a != b {
            return a.cmp(&b);
        }
    }
    Ordering::Equal
}

/// How long the fetch may take. A version check must never hold the
/// Settings surface (or a launch) hostage: the payload is a tiny static
/// file, so ten seconds is generous and still bounds a hung connection.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetches the app list at `url` and classifies it against
/// `current_version`. The one network call the feature makes: a bare GET —
/// no key, no cookies, no custom headers (the fetch itself is the whole
/// request, per the ticket's privacy note). Every failure comes back as an
/// honest [`UpdateCheckError`], never a panic.
pub async fn fetch_update_info(url: &str) -> Result<Vec<AppListEntry>, UpdateCheckError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(UpdateCheckError::Unavailable);
    }
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| UpdateCheckError::Network(format!("failed to build HTTP client: {e}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| UpdateCheckError::Network(e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(UpdateCheckError::Status(status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| UpdateCheckError::Network(e.to_string()))?;
    parse_app_list(&body)
}

/// The whole check as the Tauri command surfaces it: fetch, find, compare
/// — mapped onto [`UpdateCheckOutcome`] so every path, success or failure,
/// is a gentle report.
pub async fn check_for_updates(url: &str, current_version: &str) -> UpdateCheckOutcome {
    let entries = match fetch_update_info(url).await {
        Ok(entries) => entries,
        Err(error) => {
            return match error {
                UpdateCheckError::Unavailable => UpdateCheckOutcome::Unavailable,
                other => UpdateCheckOutcome::Error {
                    message: other.to_string(),
                },
            }
        }
    };
    let Some(entry) = find_app_entry(&entries, APP_NAME) else {
        return UpdateCheckOutcome::NotListed;
    };
    match compare_versions(current_version, &entry.version) {
        Ordering::Less => UpdateCheckOutcome::NewVersion {
            version: entry.version.clone(),
            released: entry.latest_update.clone(),
            download_url: entry.download_url.clone(),
        },
        // Equal (same version) or Greater (the running build is newer than
        // the published entry — dev builds, a rolled-back payload) are both
        // "nothing to do".
        _ => UpdateCheckOutcome::UpToDate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ---- Version comparison ----

    #[test]
    fn equal_versions_compare_equal() {
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.0.0", "0"), Ordering::Equal);
    }

    #[test]
    fn older_and_newer_order_numerically() {
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("0.2.0", "0.1.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.9.9", "2.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
    }

    #[test]
    fn multi_digit_components_beat_lexicographic_order() {
        // The whole point of a numeric comparator: string order would call
        // "0.2.10" older than "0.2.9".
        assert_eq!(compare_versions("0.2.9", "0.2.10"), Ordering::Less);
        assert_eq!(compare_versions("0.2.10", "0.2.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.10", "0.2.11"), Ordering::Less);
    }

    #[test]
    fn leading_v_is_tolerated_on_either_side() {
        assert_eq!(compare_versions("v0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.0", "V0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("v1.2.3", "V1.2.4"), Ordering::Less);
    }

    #[test]
    fn missing_components_count_as_zero() {
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2", "1.2.1"), Ordering::Less);
        assert_eq!(compare_versions("2", "1.9.9"), Ordering::Greater);
    }

    #[test]
    fn whitespace_and_non_numeric_components_do_not_panic() {
        assert_eq!(compare_versions(" 0.1.0 ", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.0-beta", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("garbage", "0.0.1"), Ordering::Less);
        assert_eq!(compare_versions("", "0.0.1"), Ordering::Less);
    }

    // ---- Payload parsing ----

    const PAYLOAD: &str = r#"{
        "apps": [
            { "name": "other-app", "version": "9.9.9" },
            { "name": "Langception", "version": "0.2.0",
              "latest_update": "2026-11-20",
              "download_url": "https://github.com/me/langception/releases/latest" }
        ]
    }"#;

    #[test]
    fn finds_langception_case_insensitively_among_several_apps() {
        let entries = parse_app_list(PAYLOAD).unwrap();
        assert_eq!(entries.len(), 2);
        let entry = find_app_entry(&entries, APP_NAME).unwrap();
        assert_eq!(entry.version, "0.2.0");
        assert_eq!(entry.latest_update.as_deref(), Some("2026-11-20"));
        assert_eq!(
            entry.download_url.as_deref(),
            Some("https://github.com/me/langception/releases/latest")
        );
    }

    #[test]
    fn unknown_fields_anywhere_are_ignored() {
        // Extra keys at the top level, on the matched entry, and on a
        // sibling entry — the maintainer can enrich the payload freely.
        let body = r#"{
            "schema": 2,
            "notes": "hello",
            "apps": [
                { "name": "langception", "version": "1.2.3",
                  "min_version": "0.1.0", "codename": "flamingo",
                  "latest_update": "2026-01-02",
                  "download_url": "https://example.com/dl" },
                { "name": "friend-app", "version": "0.0.1", "extra": [1, 2] }
            ]
        }"#;
        let entries = parse_app_list(body).unwrap();
        let entry = find_app_entry(&entries, APP_NAME).unwrap();
        assert_eq!(entry.version, "1.2.3");
        assert_eq!(entry.latest_update.as_deref(), Some("2026-01-02"));
    }

    #[test]
    fn missing_optional_fields_stay_none() {
        let body = r#"{ "apps": [ { "name": "langception", "version": "1.0.0" } ] }"#;
        let entries = parse_app_list(body).unwrap();
        let entry = find_app_entry(&entries, APP_NAME).unwrap();
        assert!(entry.latest_update.is_none());
        assert!(entry.download_url.is_none());
    }

    #[test]
    fn missing_entry_is_none() {
        let entries = parse_app_list(PAYLOAD).unwrap();
        assert!(find_app_entry(&entries, "something-else").is_none());
    }

    #[test]
    fn payload_without_apps_key_has_no_entries() {
        // A valid JSON body that is not the app-list envelope (for
        // instance the superseded single-object contract) parses to an
        // empty list — the check reports NotListed, not a crash.
        let entries = parse_app_list(r#"{"version": "1.2.3"}"#).unwrap();
        assert!(entries.is_empty());
        assert!(find_app_entry(&entries, APP_NAME).is_none());
    }

    #[test]
    fn malformed_json_is_a_malformed_error() {
        for body in ["", "<html>blocked</html>", "{not json", "{\"apps\": 5}"] {
            let error = parse_app_list(body).unwrap_err();
            assert!(
                matches!(error, UpdateCheckError::Malformed(_)),
                "was: {error:?}"
            );
        }
    }

    #[test]
    fn outcome_json_shape_is_what_the_settings_page_reads() {
        // The Settings page switches on `kind` (kebab-case variants) and
        // reads camelCase fields — pin the wire shape so a rename cannot
        // silently blank the result area.
        let json = serde_json::to_string(&UpdateCheckOutcome::NewVersion {
            version: "0.2.0".into(),
            released: Some("2026-11-20".into()),
            download_url: Some("https://example.com/dl".into()),
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["kind"], "new-version");
        assert_eq!(value["version"], "0.2.0");
        assert_eq!(value["released"], "2026-11-20");
        assert_eq!(value["downloadUrl"], "https://example.com/dl");
        for (outcome, kind) in [
            (UpdateCheckOutcome::Unavailable, "unavailable"),
            (UpdateCheckOutcome::UpToDate, "up-to-date"),
            (UpdateCheckOutcome::NotListed, "not-listed"),
        ] {
            let value: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(&outcome).unwrap()).unwrap();
            assert_eq!(value["kind"], kind);
        }
        let value: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&UpdateCheckOutcome::Error {
                message: "no".into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(value["kind"], "error");
        assert_eq!(value["message"], "no");
    }

    #[test]
    fn outcome_maps_every_payload_path() {
        // Pure path through check_for_updates' classification, with the
        // network mocked out at the parse seam.
        let classify = |body: &str, current: &str| match parse_app_list(body) {
            Ok(entries) => match find_app_entry(&entries, APP_NAME) {
                Some(entry) => match compare_versions(current, &entry.version) {
                    Ordering::Less => UpdateCheckOutcome::NewVersion {
                        version: entry.version.clone(),
                        released: entry.latest_update.clone(),
                        download_url: entry.download_url.clone(),
                    },
                    _ => UpdateCheckOutcome::UpToDate,
                },
                None => UpdateCheckOutcome::NotListed,
            },
            Err(_) => UpdateCheckOutcome::Error {
                message: "malformed".into(),
            },
        };
        assert_eq!(classify(PAYLOAD, "0.2.0"), UpdateCheckOutcome::UpToDate);
        assert_eq!(
            classify(PAYLOAD, "0.1.0"),
            UpdateCheckOutcome::NewVersion {
                version: "0.2.0".into(),
                released: Some("2026-11-20".into()),
                download_url: Some("https://github.com/me/langception/releases/latest".into()),
            }
        );
        assert_eq!(
            classify(r#"{"apps":[{"name":"other","version":"1.0.0"}]}"#, "0.1.0"),
            UpdateCheckOutcome::NotListed
        );
    }

    // ---- Fetch against a mock server ----

    /// A localhost HTTP server answering every GET with `status` +
    /// `body`, recording each request's method and headers so the tests
    /// can prove the fetch sends nothing beyond the GET itself. Each
    /// response carries `Connection: close` so every request is its own
    /// connection (same trick as the orchestrator's fake engine).
    async fn fake_endpoint(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    // Read to the end of the headers (a GET has no body).
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let n = socket.read(&mut chunk).await.expect("read request");
                        buffer.extend_from_slice(&chunk[..n]);
                        if n == 0 || buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    recorded
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&buffer).into_owned());
                    let reason = if status == 200 { "OK" } else { "Not Found" };
                    let reply = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    socket.write_all(reply.as_bytes()).await.ok();
                    socket.shutdown().await.ok();
                });
            }
        });
        (format!("http://{addr}/latest.json"), requests)
    }

    // The mock bodies (leaked so the server task can hold &'static str).
    static BODY_NEWER: &str = r#"{ "apps": [ { "name": "langception", "version": "99.0.0", "latest_update": "2026-11-20", "download_url": "http://localhost/releases/latest" } ] }"#;
    static BODY_CURRENT: &str = r#"{ "apps": [ { "name": "langception", "version": "0.1.0" } ] }"#;
    static BODY_NO_ENTRY: &str = r#"{ "apps": [ { "name": "other-app", "version": "1.0.0" } ] }"#;
    static BODY_MALFORMED: &str = "<html>502 behind me</html>";

    #[tokio::test]
    async fn newer_entry_classifies_as_new_version_with_date_and_link() {
        let (url, _requests) = fake_endpoint(200, BODY_NEWER).await;
        let outcome = check_for_updates(&url, "0.1.0").await;
        assert_eq!(
            outcome,
            UpdateCheckOutcome::NewVersion {
                version: "99.0.0".into(),
                released: Some("2026-11-20".into()),
                download_url: Some("http://localhost/releases/latest".into()),
            }
        );
    }

    #[tokio::test]
    async fn current_entry_classifies_as_up_to_date() {
        let (url, _requests) = fake_endpoint(200, BODY_CURRENT).await;
        assert_eq!(
            check_for_updates(&url, "0.1.0").await,
            UpdateCheckOutcome::UpToDate
        );
    }

    #[tokio::test]
    async fn reachable_endpoint_without_our_entry_is_not_listed() {
        let (url, _requests) = fake_endpoint(200, BODY_NO_ENTRY).await;
        assert_eq!(
            check_for_updates(&url, "0.1.0").await,
            UpdateCheckOutcome::NotListed
        );
    }

    #[tokio::test]
    async fn non_200_is_a_gentle_error_naming_the_code() {
        let (url, _requests) = fake_endpoint(404, "not found").await;
        let outcome = check_for_updates(&url, "0.1.0").await;
        let UpdateCheckOutcome::Error { message } = outcome else {
            panic!("was: {outcome:?}");
        };
        assert!(message.contains("404"), "was: {message}");
    }

    #[tokio::test]
    async fn malformed_body_is_a_gentle_error() {
        let (url, _requests) = fake_endpoint(200, BODY_MALFORMED).await;
        let outcome = check_for_updates(&url, "0.1.0").await;
        let UpdateCheckOutcome::Error { message } = outcome else {
            panic!("was: {outcome:?}");
        };
        assert!(message.contains("malformed"), "was: {message}");
    }

    #[tokio::test]
    async fn refused_connection_is_a_gentle_error() {
        // Port 1 on the loopback interface: nothing listens there, so the
        // connect fails fast without touching the network.
        let outcome = check_for_updates("http://127.0.0.1:1/latest.json", "0.1.0").await;
        let UpdateCheckOutcome::Error { message } = outcome else {
            panic!("was: {outcome:?}");
        };
        assert!(
            message.contains("cannot reach the update server"),
            "was: {message}"
        );
    }

    #[test]
    fn empty_url_is_unavailable_without_any_fetch() {
        // No server, no async runtime needed: the guard answers before a
        // connection could even be attempted.
        let outcome = futures_block(check_for_updates("", "0.1.0"));
        assert_eq!(outcome, UpdateCheckOutcome::Unavailable);
        assert_eq!(outcome, futures_block(check_for_updates("   ", "0.1.0")));
    }

    /// Minimal block-on for one future (tokio's test runtime is already
    /// multi-thread; this keeps the pure guard test synchronous).
    fn futures_block<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[tokio::test]
    async fn the_fetch_sends_nothing_beyond_the_get() {
        // Privacy criterion: no Authorization, no Cookie, no custom
        // identifying headers — a bare GET, nothing else.
        let (url, requests) = fake_endpoint(200, BODY_CURRENT).await;
        let _ = check_for_updates(&url, "0.1.0").await;
        let recorded = requests.lock().unwrap();
        assert_eq!(recorded.len(), 1, "one fetch, one request");
        let request = &recorded[0];
        assert!(request.starts_with("GET "), "was: {request}");
        assert!(
            request.to_lowercase().contains("latest.json"),
            "was: {request}"
        );
        let lower = request.to_lowercase();
        assert!(!lower.contains("authorization:"), "was: {request}");
        assert!(!lower.contains("cookie:"), "was: {request}");
    }
}
