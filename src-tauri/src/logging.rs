//! Persistent diagnostics (ticket 27): a per-launch app log and a panic
//! hook writing crash logs, both under a `logs` folder next to the config
//! file. Hand-rolled on purpose — the contract is one small file writer,
//! and diagnostics must work exactly when everything else breaks (release
//! builds have no stderr: the window subsystem detaches it), so logging
//! itself must never panic, lock up, or fail loudly. Every write failure
//! degrades to a dropped line; the app never notices.
//!
//! The pure helpers (names, prune policy, line format) are unit-tested at
//! the bottom; the file-touching paths are exercised by launching the app.

use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many `app-*.log` / `crash-*.log` files retention keeps per kind:
/// old launches are pruned at startup so the folder cannot grow forever.
const KEEP_PER_KIND: usize = 20;

/// The one open app-log file (`None` before init or when disabled).
static APP_LOG: Mutex<Option<File>> = Mutex::new(None);

/// The `logs` folder: beside the config file (the config directory the app
/// already resolves), falling back to the temp dir when the config path has
/// no parent.
pub fn logs_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|dir| dir.join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("langception").join("logs"))
}

/// Datetime-stamped log file name, e.g. `app-2026-11-22-15-04-05.log`.
/// Zero-padded fields make names sort chronologically, which the retention
/// pass relies on.
fn file_name(kind: &str, at: chrono::DateTime<Local>) -> String {
    format!("{kind}-{}.log", at.format("%Y-%m-%d-%H-%M-%S"))
}

/// One log line: `2026-11-22 15:04:05.123 [loop] message`.
fn format_line(at: chrono::DateTime<Local>, tag: &str, message: &str) -> String {
    format!(
        "{} [{tag}] {message}",
        at.format("%Y-%m-%d %H:%M:%S%.3f")
    )
}

/// Retention policy: names sort chronologically (datetime-stamped), so the
/// oldest-first beyond the cap are the ones to delete.
fn prune_beyond(names: &mut Vec<String>, keep: usize) -> Vec<String> {
    names.sort();
    let excess = names.len().saturating_sub(keep);
    names.drain(..excess).collect()
}

/// Opens this launch's app log (`app-<datetime>.log`), prunes old logs
/// beyond the cap, and writes the launch line. Failures here leave logging
/// disabled for the session (every later [`log`] call becomes a no-op) —
/// the app must run even when the disk refuses.
pub fn init(config_path: &Path, version: &str) {
    let dir = logs_dir(config_path);
    // The panic hook must resolve the dir without Tauri state (it can fire
    // before the builder runs and on any thread), so init remembers it.
    *APP_LOGS_DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir.clone());
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    prune(&dir, "app", KEEP_PER_KIND);
    prune(&dir, "crash", KEEP_PER_KIND);

    let now = Local::now();
    let path = dir.join(file_name("app", now));
    let mut guard = APP_LOG.lock().unwrap_or_else(|p| p.into_inner());
    // No file, no logging; nothing else in the app depends on it.
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
        *guard = Some(file);
        drop(guard);
        let config = config_path.display();
        log(
            "app",
            format!("Langception {version} started (config: {config})"),
        );
    }
}

/// Deletes the oldest `logs/<kind>-*.log` files beyond `keep`. Failure to
/// prune (locked files, permission errors) is silently ignored — the next
/// launch retries, and log files never block the app.
fn prune(dir: &Path, kind: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&format!("{kind}-")) && name.ends_with(".log")
        })
        // File names are datetime-stamped; the modified time would follow
        // renames and clock changes, the name is the launch identity.
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for name in prune_beyond(&mut names, keep) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Appends one timestamped line to the app log. Best-effort: an unwritable
/// sink drops the line and the caller carries on (write failures must never
/// block or crash the translation loop — ticket 27).
pub fn log(tag: &str, message: impl AsRef<str>) {
    let line = format_line(Local::now(), tag, message.as_ref());
    let mut guard = APP_LOG.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(file) = guard.as_mut() {
        // Errors are ignored by contract: a broken sink (full disk, deleted
        // folder) disables the log silently instead of disturbing the app.
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Installs the crash-log panic hook: every panic writes a
/// `crash-<datetime>.log` (message, location, thread, backtrace) before the
/// previous hook — normally stderr's — runs, so a crash leaves evidence even
/// when stderr is detached in release builds. The hook itself never panics
/// (no unwraps; every file op is ignored on failure), and panics that happen
/// while writing the log still reach the previous hook.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_crash_log(info);
        previous(info);
    }));
}

fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) {
    let dir = match app_logs_dir() {
        Some(dir) => dir,
        None => return,
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Millis in the name: two panics in the same second must not fight over
    // one file (each append-open would interleave, making both unreadable).
    let now = Local::now();
    let path = dir.join(format!("crash-{}.log", now.format("%Y-%m-%d-%H-%M-%S-%3f")));

    let thread = std::thread::current();
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".into());

    let text = format!(
        "{}\nthread: {}\nlocation: {}\n{}\n",
        format_line(now, "crash", message),
        thread.name().unwrap_or("<unnamed>"),
        location,
        std::backtrace::Backtrace::force_capture(),
    );
    let _ = std::fs::write(path, text);
}

/// The logs dir of the RUNNING app, stored by [`init`] (from the same
/// config path the shell computes at startup) before any panic could
/// happen. The hook cannot consult `AppState` — it must work before
/// Tauri's state exists and on any thread.
static APP_LOGS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The running app's logs dir (`None` before [`init`] ran).
fn app_logs_dir() -> Option<PathBuf> {
    APP_LOGS_DIR
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> chrono::DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(y, m, d, h, min, s).unwrap()
    }

    #[test]
    fn file_name_matches_ticket_format() {
        assert_eq!(
            file_name("app", at(2026, 11, 22, 15, 4, 5)),
            "app-2026-11-22-15-04-05.log"
        );
    }

    #[test]
    fn file_names_sort_chronologically() {
        let early = file_name("app", at(2026, 1, 2, 3, 4, 5));
        let late = file_name("app", at(2027, 12, 31, 23, 59, 59));
        assert!(early < late, "{early} must sort before {late}");
    }

    #[test]
    fn line_has_timestamp_tag_and_message() {
        let line = format_line(at(2026, 11, 22, 15, 4, 5), "loop", "started");
        assert_eq!(line, "2026-11-22 15:04:05.000 [loop] started");
    }

    #[test]
    fn prune_keeps_the_newest() {
        let mut names = vec![
            "app-2026-11-20-10-00-00.log".into(),
            "app-2026-11-22-15-04-05.log".into(),
            "app-2026-11-21-12-30-00.log".into(),
        ];
        let deleted = prune_beyond(&mut names, 1);
        assert_eq!(
            deleted,
            vec!["app-2026-11-20-10-00-00.log", "app-2026-11-21-12-30-00.log"]
        );
        assert_eq!(names, vec!["app-2026-11-22-15-04-05.log"]);
    }

    #[test]
    fn prune_under_cap_deletes_nothing() {
        let mut names = vec!["app-2026-11-22-15-04-05.log".into()];
        assert!(prune_beyond(&mut names, 20).is_empty());
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn prune_is_stable_across_kinds() {
        // Crash names carry millis; they must sort within their own kind.
        let mut names = vec![
            "crash-2026-11-22-15-04-05-100.log".into(),
            "crash-2026-11-22-15-04-05-050.log".into(),
            "crash-2026-11-22-15-04-05-999.log".into(),
        ];
        let deleted = prune_beyond(&mut names, 2);
        assert_eq!(deleted, vec!["crash-2026-11-22-15-04-05-050.log"]);
    }

    #[test]
    fn logs_dir_sits_beside_the_config() {
        let config = Path::new("C:\\Users\\x\\AppData\\Roaming\\langception\\config.toml");
        assert_eq!(
            logs_dir(config),
            PathBuf::from("C:\\Users\\x\\AppData\\Roaming\\langception\\logs")
        );
    }

    /// Serializes the tests that touch the process-global logger state and
    /// real files (`init` / `log` / the panic hook).
    static FS_LOCK: Mutex<()> = Mutex::new(());

    fn temp_config_dir(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("langception-log-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("config.toml"), dir)
    }

    fn list_logs(config: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(logs_dir(config))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn init_opens_a_fresh_log_and_prunes_old_ones() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (config, dir) = temp_config_dir("init");

        // 25 old launches: retention keeps the newest KEEP_PER_KIND.
        let logs = logs_dir(&config);
        std::fs::create_dir_all(&logs).unwrap();
        for day in 1..=25 {
            std::fs::write(logs.join(format!("app-2026-11-{day:02}-10-00-00.log")), "old").unwrap();
        }

        init(&config, "0.0.0");
        log("loop", "started — reading=ocr engine=Google");

        let app_logs: Vec<String> = list_logs(&config)
            .into_iter()
            .filter(|n| n.starts_with("app-"))
            .collect();
        // The cap plus this launch's fresh file.
        assert_eq!(app_logs.len(), KEEP_PER_KIND + 1, "{app_logs:?}");

        // The fresh log carries the launch line and the logged line (the
        // old logs hold only "old").
        let texts: Vec<String> = app_logs
            .iter()
            .map(|name| std::fs::read_to_string(logs_dir(&config).join(name)).unwrap())
            .collect();
        assert_eq!(
            texts.iter().filter(|t| t.contains("Langception 0.0.0 started")).count(),
            1,
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("[loop] started — reading=ocr engine=Google")),
            "{texts:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panic_hook_writes_a_crash_log() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (config, dir) = temp_config_dir("panic");
        init(&config, "0.0.0");
        install_panic_hook();

        // The panic is contained; the hook still ran (it fires before the
        // unwinding payload reaches catch_unwind).
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panic!("boom for the crash log");
        }));
        assert!(result.is_err());

        let crashes: Vec<String> = list_logs(&config)
            .into_iter()
            .filter(|n| n.starts_with("crash-"))
            .collect();
        assert_eq!(crashes.len(), 1, "{crashes:?}");
        let text = std::fs::read_to_string(dir.join("logs").join(&crashes[0])).unwrap();
        assert!(text.contains("boom for the crash log"), "{text}");
        assert!(text.contains("location: "), "{text}");
        assert!(text.contains("thread: "), "{text}");
        // The backtrace capture (frame lines, e.g. "   0: …") — its exact
        // shape varies by platform and build, so only frames are asserted.
        assert!(text.contains(" 0: "), "{text}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
