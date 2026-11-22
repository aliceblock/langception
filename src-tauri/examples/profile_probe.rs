//! Throwaway diagnostic probe for the "profile never activates" bug report:
//! replays the shell's exact decision chain against the REAL config file and
//! the REAL foreground window — the two links the automated tests cannot
//! reach (Win32 adapter + on-disk config).
//!
//! Run it, then focus the game within the sample window:
//! `cargo run -p langception --example profile_probe -- [--seconds 25]`
//!
//! Red = "no profile matches" while the game is focused; green = a match
//! with the zones that would be watched. Prints WHY on every change of the
//! foreground window (exe/title/rect), so a wrong exe name is visible
//! immediately.

use langception::foreground::foreground_app;
use langception_core::config::AppConfig;
use langception_core::profile::Profile;

fn config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("langception")
        .join("config.toml")
}

fn main() {
    let seconds: u64 = std::env::args()
        .skip_while(|a| a != "--seconds")
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);

    // 1. The on-disk config, parsed exactly like the app parses it.
    let path = config_path();
    let text = std::fs::read_to_string(&path).expect("read config.toml");
    let config = AppConfig::from_toml_str(&text).expect("parse config.toml");
    println!("config: {}", path.display());
    if config.profiles.is_empty() {
        println!("!! config has NO profiles — that alone is the bug");
        return;
    }
    for p in &config.profiles {
        println!(
            "  profile {:?}: exe={:?} language={:?} zones={}",
            p.name,
            p.exe,
            p.language.code(),
            p.zones.len()
        );
    }

    // 2. Poll the foreground window, resolving against the profiles on every
    // change — the same query `profile_watch_tick` runs every 250 ms.
    println!(
        "\nfocusing the game now — sampling the foreground window for {seconds}s …"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut last_key = String::new();
    let mut matched_once = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let Some(fg) = foreground_app() else {
            if last_key != "<none>" {
                println!("foreground: <none>");
                last_key = "<none>".into();
            }
            continue;
        };
        let key = format!("{}|{}|{}|{}|{}", fg.exe, fg.rect.x, fg.rect.y, fg.rect.width, fg.rect.height);
        if key == last_key {
            continue;
        }
        last_key = key.clone();
        println!(
            "foreground: exe={:?} title={:?} rect=({},{}) {}x{}",
            fg.exe, fg.title, fg.rect.x, fg.rect.y, fg.rect.width, fg.rect.height
        );
        match Profile::resolve(&config.profiles, &fg.exe) {
            Some(profile) => {
                let zones = profile.capture_zones(fg.rect);
                matched_once = true;
                println!(
                    "  => MATCH {:?}: {} zone(s) would be watched: {:?}",
                    profile.name,
                    zones.len(),
                    zones
                        .iter()
                        .map(|z| (z.id.as_str(), z.region.x, z.region.y, z.region.width, z.region.height))
                        .collect::<Vec<_>>()
                );
                if zones.is_empty() {
                    println!("  !! all profile zones fell outside the window — nothing to watch");
                }
            }
            None => println!("  => NO MATCH — the global capture target would apply"),
        }
    }

    if matched_once {
        println!("\nRESULT: profile matching WORKS at the OS level — bug is downstream.");
    } else {
        println!("\nRESULT: the profile never matched while sampling — reproduced.");
    }
}
