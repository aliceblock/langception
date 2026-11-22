//! Profiles (CONTEXT.md): a per-app configuration — which app it targets
//! (by executable file name), the Source Language, the engine choice, the
//! Capture Zones inside that app's window, an optional Glossary layer, and
//! a Genderless Mode override.
//!
//! Activation is a shell concern (it needs the OS foreground window), but
//! everything that decides what an active Profile *means* lives here and is
//! tested offline: which Profile matches an executable, what the loop
//! watches while it is active, and how its layers override the global
//! settings. The global settings are the fallback whenever no Profile
//! matches the foreground app.

use crate::glossary::Glossary;
use crate::ports::{Region, SourceLanguage, Zone};
use serde::{Deserialize, Serialize};

/// The Genderless Mode setting for a Profile: [`GenderlessOverride::Inherit`]
/// follows the global toggle (ticket 08 wires the toggle itself), the other
/// two override it regardless of the global value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenderlessOverride {
    #[default]
    Inherit,
    On,
    Off,
}

/// A Capture Zone that belongs to a Profile. Its region is in PHYSICAL px
/// relative to the app window's top-left, so it follows the window around
/// the screen — capture scopes to that window's bounds, and the zones
/// restrict what inside it is watched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileZone {
    pub id: String,
    pub name: String,
    pub region: Region,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// The app this Profile targets, by executable file name ("game.exe").
    /// Matching is trimmed and case-insensitive.
    pub exe: String,
    /// The Profile's Source Language (CONTEXT.md: fixed per Profile).
    pub language: SourceLanguage,
    /// Translate Engine choice; `None` inherits the global engine. Only
    /// engines that exist in this build validate (`config::TRANSLATE_ENGINES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Genderless Mode per-Profile override.
    #[serde(default)]
    pub genderless: GenderlessOverride,
    /// The Profile's Glossary layer: merged over the global Glossary while
    /// the Profile is active, with the Profile's value winning on conflict —
    /// game-specific names ("Vertin" → "เวอร์ติน") live here.
    #[serde(default)]
    pub glossary: Glossary,
    /// Capture Zones INSIDE the app's window (window-relative). When empty,
    /// the whole window is watched.
    #[serde(default)]
    pub zones: Vec<ProfileZone>,
}

/// The id of the synthetic zone watched when an active Profile has no
/// explicit Capture Zones: it spans the app's whole window. Stable per
/// profile, so the loop keeps tracking state across ticks while the
/// profile stays active (and starts fresh when it changes).
pub fn window_zone_id(profile_id: &str) -> String {
    format!("window:{profile_id}")
}

impl Profile {
    /// Whether this Profile targets the app behind `exe` (trimmed,
    /// case-insensitive — Windows executable names are case-preserving but
    /// matching should not be).
    pub fn matches_exe(&self, exe: &str) -> bool {
        let wanted = exe.trim();
        !self.exe.trim().is_empty()
            && !wanted.is_empty()
            && self.exe.trim().eq_ignore_ascii_case(wanted)
    }

    /// The Profile active for `exe`, if any: the FIRST match in config
    /// order wins (validation forbids two profiles for the same
    /// executable anyway; first-match keeps hand-edited configs sane).
    pub fn resolve<'a>(profiles: &'a [Profile], exe: &str) -> Option<&'a Profile> {
        profiles.iter().find(|profile| profile.matches_exe(exe))
    }

    /// What the loop watches while this Profile is active and the app's
    /// window occupies `window` (screen px): the Profile's Capture Zones
    /// converted to screen coordinates and clamped inside the window —
    /// or, when the Profile has no zones, one synthetic zone spanning the
    /// whole window. Zones entirely outside the window are dropped (the
    /// window may have shrunk since the zone was drawn).
    pub fn capture_zones(&self, window: Region) -> Vec<Zone> {
        if self.zones.is_empty() {
            return vec![Zone {
                id: window_zone_id(&self.id),
                name: self.name.clone(),
                monitor: String::new(),
                region: window,
            }];
        }
        self.zones
            .iter()
            .filter_map(|zone| {
                zone_in_window(window, zone.region).map(|region| Zone {
                    id: zone.id.clone(),
                    name: zone.name.clone(),
                    monitor: String::new(),
                    region,
                })
            })
            .collect()
    }

    /// The Glossary in force while the Profile is active: the global
    /// entries plus the Profile's layer, with the Profile's value winning
    /// on conflict (the same rule as [`Glossary::merge`] — the user just
    /// chose it).
    pub fn effective_glossary(&self, global: &Glossary) -> Glossary {
        let mut merged = global.clone();
        merged.merge(&self.glossary);
        merged
    }

    /// The Genderless Mode in force while the Profile is active.
    pub fn effective_genderless(&self, global: bool) -> bool {
        match self.genderless {
            GenderlessOverride::Inherit => global,
            GenderlessOverride::On => true,
            GenderlessOverride::Off => false,
        }
    }
}

/// A window-relative zone's screen rectangle: intersected with `window`,
/// so a zone never captures outside the app's window even after the window
/// moved or shrank. `None` when nothing is left of the zone.
fn zone_in_window(window: Region, rel: Region) -> Option<Region> {
    let left = (window.x + rel.x).max(window.x);
    let top = (window.y + rel.y).max(window.y);
    let right = (window.x + rel.x + rel.width as i32).min(window.right());
    let bottom = (window.y + rel.y + rel.height as i32).min(window.bottom());
    let width = right - left;
    let height = bottom - top;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Region {
        x: left,
        y: top,
        width: width as u32,
        height: height as u32,
    })
}

impl Region {
    /// Exclusive right edge (x + width), the form intersections need.
    fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    /// Exclusive bottom edge (y + height).
    fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str, exe: &str) -> Profile {
        Profile {
            id: id.to_string(),
            name: name.to_string(),
            exe: exe.to_string(),
            language: SourceLanguage::Ja,
            engine: None,
            genderless: GenderlessOverride::Inherit,
            glossary: Glossary::default(),
            zones: Vec::new(),
        }
    }

    fn window() -> Region {
        Region {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        }
    }

    fn rel(x: i32, y: i32, width: u32, height: u32) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    fn glossary(entries: &[(&str, &str)]) -> Glossary {
        Glossary(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    // ---- Exe matching + resolution ----

    #[test]
    fn matches_exe_is_case_insensitive_and_trims() {
        let p = profile("p1", "Game", "Game.exe");
        assert!(p.matches_exe("game.exe"));
        assert!(p.matches_exe("GAME.EXE"));
        assert!(p.matches_exe(" game.exe "));
        assert!(!p.matches_exe("other.exe"));
        assert!(!p.matches_exe("game"));
        assert!(!p.matches_exe(""));
    }

    #[test]
    fn profile_with_empty_exe_never_matches() {
        let p = profile("p1", "Broken", "   ");
        assert!(!p.matches_exe("anything.exe"));
    }

    #[test]
    fn resolve_returns_first_match_in_config_order() {
        let profiles = vec![
            profile("p1", "Game", "game.exe"),
            profile("p2", "Chat", "chat.exe"),
        ];
        assert_eq!(Profile::resolve(&profiles, "GAME.EXE").unwrap().id, "p1");
        assert_eq!(Profile::resolve(&profiles, "chat.exe").unwrap().id, "p2");
        assert!(Profile::resolve(&profiles, "nothing.exe").is_none());
        assert!(Profile::resolve(&[], "game.exe").is_none());
    }

    // ---- Capture zones inside the window ----

    #[test]
    fn profile_without_zones_watches_the_whole_window() {
        let p = profile("p1", "Game", "game.exe");
        let zones = p.capture_zones(window());
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].id, window_zone_id("p1"));
        assert_eq!(zones[0].name, "Game");
        assert_eq!(zones[0].region, window());
    }

    #[test]
    fn window_zone_id_is_stable_per_profile() {
        assert_eq!(window_zone_id("p1"), window_zone_id("p1"));
        assert_ne!(window_zone_id("p1"), window_zone_id("p2"));
        assert_ne!(
            window_zone_id("p1"),
            "fullscreen",
            "must not collide with the shell's full-screen zone id"
        );
    }

    #[test]
    fn zones_are_converted_to_screen_coordinates() {
        let mut p = profile("p1", "Game", "game.exe");
        p.zones.push(ProfileZone {
            id: "z1".into(),
            name: "Chat".into(),
            region: rel(10, 20, 300, 100),
        });
        let zones = p.capture_zones(window());
        assert_eq!(
            zones[0].region,
            Region {
                x: 110,
                y: 220,
                width: 300,
                height: 100
            }
        );
    }

    #[test]
    fn zones_hanging_out_of_the_window_are_clamped() {
        let mut p = profile("p1", "Game", "game.exe");
        // 50px hangs past the window's right edge (window: 100..900).
        p.zones.push(ProfileZone {
            id: "z1".into(),
            name: "Chat".into(),
            region: rel(750, 0, 100, 100),
        });
        let zones = p.capture_zones(window());
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].region.width, 50);
    }

    #[test]
    fn zones_entirely_outside_the_window_are_dropped() {
        let mut p = profile("p1", "Game", "game.exe");
        p.zones.push(ProfileZone {
            id: "z1".into(),
            name: "Gone".into(),
            region: rel(1000, 0, 100, 100), // past the window's right edge
        });
        p.zones.push(ProfileZone {
            id: "z2".into(),
            name: "Here".into(),
            region: rel(0, 0, 100, 100),
        });
        let zones = p.capture_zones(window());
        assert_eq!(zones.len(), 1, "only the inside zone survives");
        assert_eq!(zones[0].id, "z2");
    }

    #[test]
    fn hand_edited_negative_zone_offsets_are_clamped() {
        let mut p = profile("p1", "Game", "game.exe");
        p.zones.push(ProfileZone {
            id: "z1".into(),
            name: "Odd".into(),
            region: rel(-30, -30, 200, 200),
        });
        let zones = p.capture_zones(window());
        assert_eq!(
            zones[0].region,
            Region {
                x: 100,
                y: 200,
                width: 170,
                height: 170
            }
        );
    }

    // ---- Layered settings while active ----

    #[test]
    fn profile_glossary_wins_over_global_on_conflict() {
        let p = profile("p1", "Game", "game.exe");
        let mut p = p;
        p.glossary = glossary(&[("Vertin", "ผู้กล้า"), ("Regulus", "เรกุลุส")]);
        let global = glossary(&[("Vertin", "เวอร์ติน"), ("Keep", "คีป")]);
        let merged = p.effective_glossary(&global);
        assert_eq!(
            merged.lookup("vertin"),
            Some("ผู้กล้า".to_string()),
            "profile wins"
        );
        assert_eq!(
            merged.lookup("regulus"),
            Some("เรกุลุส".to_string()),
            "profile-only term"
        );
        assert_eq!(
            merged.lookup("keep"),
            Some("คีป".to_string()),
            "global-only term kept"
        );
    }

    #[test]
    fn profile_with_empty_glossary_layer_leaves_global_intact() {
        let p = profile("p1", "Game", "game.exe");
        let global = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(p.effective_glossary(&global), global);
    }

    #[test]
    fn empty_global_glossary_gets_the_profile_layer() {
        let mut p = profile("p1", "Game", "game.exe");
        p.glossary = glossary(&[("Vertin", "เวอร์ติน")]);
        let merged = p.effective_glossary(&Glossary::default());
        assert_eq!(merged.lookup("Vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn genderless_override_respected() {
        let mut p = profile("p1", "Game", "game.exe");
        p.genderless = GenderlessOverride::On;
        assert!(p.effective_genderless(false), "On overrides a global off");
        p.genderless = GenderlessOverride::Off;
        assert!(!p.effective_genderless(true), "Off overrides a global on");
        p.genderless = GenderlessOverride::Inherit;
        assert!(p.effective_genderless(true), "Inherit follows global on");
        assert!(!p.effective_genderless(false), "Inherit follows global off");
    }

    #[test]
    fn genderless_override_serializes_lowercase() {
        // As a struct field (the real usage — config TOML), the override is
        // a plain string: "inherit" | "on" | "off".
        #[derive(Debug, Serialize, Deserialize)]
        struct Wrapper {
            genderless: GenderlessOverride,
        }
        for (text, value) in [
            ("inherit", GenderlessOverride::Inherit),
            ("on", GenderlessOverride::On),
            ("off", GenderlessOverride::Off),
        ] {
            let toml = toml::to_string(&Wrapper { genderless: value }).unwrap();
            assert!(
                toml.contains(&format!("genderless = \"{text}\"")),
                "\"{text}\" expected, was: {toml}"
            );
            let parsed: Wrapper = toml::from_str(&format!("genderless = \"{text}\"")).unwrap();
            assert_eq!(parsed.genderless, value);
        }
        // An unknown value is a config error, not a silent default.
        let err = toml::from_str::<Wrapper>("genderless = \"maybe\"").unwrap_err();
        assert!(err.to_string().contains("genderless"), "was: {err}");
    }
}
