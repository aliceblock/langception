//! The user's config file (`config.toml` in the app-data directory). It is
//! the hand-editing surface: Capture Zones drawn in the app are persisted
//! here automatically, and hotkeys are rebindable here. The file is written
//! once on first launch, then kept in sync by the app.

use crate::glossary::Glossary;
use crate::ports::{ExcludedBlock, Region, SourceLanguage, Zone};
use crate::profile::Profile;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config.toml is invalid: {0}")]
    Parse(String),
    #[error("config.toml is invalid: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSettings {
    pub language: SourceLanguage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSettings {
    #[serde(default = "default_cadence_ms")]
    pub cadence_ms: u32,
    /// How long a Text Block whose pixels changed must hold them stable
    /// before it is OCR'd and translated (ticket 05; debounces animations,
    /// whose pixels never stop changing). 0 translates every change on the
    /// detecting pass.
    #[serde(default = "default_stability_ms")]
    pub stability_ms: u32,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            cadence_ms: default_cadence_ms(),
            stability_ms: default_stability_ms(),
        }
    }
}

fn default_cadence_ms() -> u32 {
    500
}

fn default_stability_ms() -> u32 {
    400
}

/// How translated text is drawn on the In-place Overlay (CONTEXT.md):
/// font, colors, outline, and the background pill. `font_family` is any
/// font name the system or the bundled fonts resolve (Noto Sans Thai ships
/// with the app so Thai glyphs always render); everything else is clamped
/// by [`AppConfig::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub font_family: String,
    pub font_size: f32,
    pub color: String,
    pub outline_width: f32,
    pub outline_color: String,
    pub background: bool,
    pub background_opacity: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            color: "#ffffff".to_string(),
            outline_width: 2.0,
            outline_color: "#000000".to_string(),
            background: true,
            background_opacity: 0.6,
        }
    }
}

/// Bundled with the app (`ui/fonts/`) and always in the overlay's font
/// fallback chain, so Thai renders even when the chosen font has no Thai.
pub const DEFAULT_FONT_FAMILY: &str = "Noto Sans Thai";

/// `#rgb` / `#rrggbb` (leading `#` optional) — everything the overlay's
/// CSS needs for text/outline colors.
pub fn is_valid_hex_color(value: &str) -> bool {
    let body = value.strip_prefix('#').unwrap_or(value);
    (body.len() == 3 || body.len() == 6) && body.chars().all(|c| c.is_ascii_hexdigit())
}

impl TextStyle {
    /// Range/shape checks with `[text_style]`-prefixed messages, shared by
    /// config loading and the settings surface's live edits.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.font_family.trim().is_empty() {
            return Err(ConfigError::Validation(
                "[text_style] font_family must not be empty".into(),
            ));
        }
        if !(6.0..=96.0).contains(&self.font_size) {
            return Err(ConfigError::Validation(
                "[text_style] font_size must be between 6 and 96".into(),
            ));
        }
        if !(0.0..=8.0).contains(&self.outline_width) {
            return Err(ConfigError::Validation(
                "[text_style] outline_width must be between 0 and 8".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.background_opacity) {
            return Err(ConfigError::Validation(
                "[text_style] background_opacity must be between 0 and 1".into(),
            ));
        }
        for (field, value) in [
            ("color", &self.color),
            ("outline_color", &self.outline_color),
        ] {
            if !is_valid_hex_color(value) {
                return Err(ConfigError::Validation(format!(
                    "[text_style] {field} = \"{value}\" is not a hex color like \"#ffffff\""
                )));
            }
        }
        Ok(())
    }
}

/// Global hotkeys (spec: toggle / zone / adjust / pause / one-shot),
/// rebindable here. Values are accelerator strings like "Alt+T" or
/// "Ctrl+Shift+Z"; parsing happens where they are registered (the shell, at
/// startup), so only non-emptiness is validated here. Changes apply on app
/// restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub toggle_translation: String,
    pub new_zone: String,
    pub pause_translation: String,
    pub adjust_mode: String,
    pub one_shot_capture: String,
}

/// What the loop watches (CONTEXT.md "Capture Target": exactly one is
/// active at a time): the Capture Zones, or full-screen mode (every Text
/// Block on one monitor). Per-app Profiles arrive in ticket 07.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    Zones,
    FullScreen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeSettings {
    pub target: CaptureMode,
    /// Monitor full-screen mode watches, by system monitor name. Empty =
    /// the primary monitor (the default; resolved at runtime by the shell).
    pub fullscreen_monitor: String,
}

impl Default for ModeSettings {
    fn default() -> Self {
        Self {
            target: CaptureMode::Zones,
            fullscreen_monitor: String::new(),
        }
    }
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            toggle_translation: DEFAULT_TOGGLE_HOTKEY.to_string(),
            new_zone: DEFAULT_NEW_ZONE_HOTKEY.to_string(),
            pause_translation: DEFAULT_PAUSE_HOTKEY.to_string(),
            adjust_mode: DEFAULT_ADJUST_HOTKEY.to_string(),
            one_shot_capture: DEFAULT_ONE_SHOT_HOTKEY.to_string(),
        }
    }
}

pub const DEFAULT_TOGGLE_HOTKEY: &str = "Alt+T";
pub const DEFAULT_NEW_ZONE_HOTKEY: &str = "Alt+Z";
pub const DEFAULT_PAUSE_HOTKEY: &str = "Alt+P";
pub const DEFAULT_ADJUST_HOTKEY: &str = "Alt+A";
pub const DEFAULT_ONE_SHOT_HOTKEY: &str = "Alt+Q";

/// The Translate Engines this build offers, by config name — the accepted
/// values for a Profile's `engine` choice (and, from ticket 09 on, for the
/// global engine setting). An unknown name in a hand-edited config must be
/// a visible validation error, never a silent fallback.
pub const TRANSLATE_ENGINES: [&str; 1] = ["google"];

/// Everything the app reads from the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub source: SourceSettings,
    /// Slice 1 watched a single `[region]`. Capture Zones (ticket 02)
    /// replaced it; a config that still carries one is migrated into a zone
    /// on load — never silently dropped.
    #[serde(default)]
    pub region: Option<Region>,
    #[serde(default)]
    pub capture: CaptureSettings,
    #[serde(default)]
    pub mode: ModeSettings,
    #[serde(default)]
    pub hotkeys: HotkeySettings,
    #[serde(default)]
    pub text_style: TextStyle,
    #[serde(default)]
    pub glossary: Glossary,
    #[serde(default)]
    pub zones: Vec<Zone>,
    /// Excluded Blocks (ticket 04): Text Block positions the user marked as
    /// not-to-be-translated in full-screen mode. Clicking a block in Adjust
    /// Mode appends here; clearing an entry makes the block translate again.
    #[serde(default)]
    pub excluded_blocks: Vec<ExcludedBlock>,
    /// Per-app Profiles (ticket 07): each binds one executable with its own
    /// Source Language, engine choice, window-relative Capture Zones,
    /// Glossary layer and Genderless Mode override. The Profile whose app is
    /// the foreground window is active; the settings above are the global
    /// fallback when none matches.
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl AppConfig {
    pub fn from_toml_str(toml_text: &str) -> Result<Self, ConfigError> {
        let config: AppConfig =
            toml::from_str(toml_text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Slice-1 configs watched a single `[region]`. Move it into a named
    /// Capture Zone (in memory — the caller persists); returns whether
    /// anything changed. A config that already has zones keeps its legacy
    /// region untouched (zones win).
    pub fn migrate_legacy_region(&mut self) -> bool {
        if !self.zones.is_empty() {
            return false;
        }
        let Some(region) = self.region.take() else {
            return false;
        };
        self.zones.push(Zone {
            id: "legacy-region".to_string(),
            name: "Configured region".to_string(),
            // Drawn before zones remembered their monitor; the shell's
            // monitor migration fills this from the region's position.
            monitor: String::new(),
            region,
        });
        true
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(region) = &self.region {
            if region.width == 0 || region.height == 0 {
                return Err(ConfigError::Validation(
                    "[region] width and height must be greater than zero".into(),
                ));
            }
        }
        if self.capture.cadence_ms < 50 {
            return Err(ConfigError::Validation(
                "[capture] cadence_ms must be at least 50".into(),
            ));
        }
        if self.capture.stability_ms > 10_000 {
            return Err(ConfigError::Validation(
                "[capture] stability_ms must be at most 10000".into(),
            ));
        }
        self.text_style.validate()?;
        for (action, key) in [
            ("toggle_translation", &self.hotkeys.toggle_translation),
            ("new_zone", &self.hotkeys.new_zone),
            ("pause_translation", &self.hotkeys.pause_translation),
            ("one_shot_capture", &self.hotkeys.one_shot_capture),
        ] {
            if key.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "[hotkeys] {action} must not be empty"
                )));
            }
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for zone in &self.zones {
            let name = zone.name.trim();
            if name.is_empty() {
                return Err(ConfigError::Validation(
                    "every zone needs a non-empty name".into(),
                ));
            }
            if zone.region.width == 0 || zone.region.height == 0 {
                return Err(ConfigError::Validation(format!(
                    "zone \"{name}\" must have a width and height greater than zero"
                )));
            }
            if !ids.insert(zone.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate zone id \"{id}\"",
                    id = zone.id
                )));
            }
            let name_key = name.to_lowercase();
            if !names.insert(name_key.clone()) {
                return Err(ConfigError::Validation(format!(
                    "two zones are named \"{name}\" — zone names must be unique"
                )));
            }
        }
        let mut exclusion_ids = HashSet::new();
        for exclusion in &self.excluded_blocks {
            if exclusion.id.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "every excluded block needs a non-empty id".into(),
                ));
            }
            if exclusion.region.width == 0 || exclusion.region.height == 0 {
                return Err(ConfigError::Validation(format!(
                    "excluded block \"{id}\" must have a width and height greater than zero",
                    id = exclusion.id
                )));
            }
            if !exclusion_ids.insert(exclusion.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate excluded block id \"{id}\"",
                    id = exclusion.id
                )));
            }
        }
        let mut profile_ids = HashSet::new();
        let mut profile_names = HashSet::new();
        let mut profile_exes = HashSet::new();
        for profile in &self.profiles {
            let name = profile.name.trim();
            if name.is_empty() {
                return Err(ConfigError::Validation(
                    "every profile needs a non-empty name".into(),
                ));
            }
            if profile.exe.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "profile \"{name}\" needs the executable it targets (e.g. \"game.exe\")"
                )));
            }
            if !profile_ids.insert(profile.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate profile id \"{id}\"",
                    id = profile.id
                )));
            }
            if !profile_names.insert(name.to_lowercase()) {
                return Err(ConfigError::Validation(format!(
                    "two profiles are named \"{name}\" — profile names must be unique"
                )));
            }
            if !profile_exes.insert(profile.exe.trim().to_lowercase()) {
                return Err(ConfigError::Validation(format!(
                    "two profiles target the same executable \"{exe}\" — one profile per app",
                    exe = profile.exe.trim()
                )));
            }
            if let Some(engine) = &profile.engine {
                if !TRANSLATE_ENGINES.contains(&engine.trim().to_lowercase().as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "profile \"{name}\" engine \"{engine}\" is unknown — engines: {}",
                        TRANSLATE_ENGINES.join(", ")
                    )));
                }
            }
            let mut zone_ids = HashSet::new();
            let mut zone_names = HashSet::new();
            for zone in &profile.zones {
                let zone_name = zone.name.trim();
                if zone_name.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "profile \"{name}\" zone needs a non-empty name"
                    )));
                }
                if zone.region.width == 0 || zone.region.height == 0 {
                    return Err(ConfigError::Validation(format!(
                        "profile \"{name}\" zone \"{zone_name}\" must have a width and height greater than zero"
                    )));
                }
                if !zone_ids.insert(zone.id.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "profile \"{name}\" has two zones with id \"{id}\"",
                        id = zone.id
                    )));
                }
                if !zone_names.insert(zone_name.to_lowercase()) {
                    return Err(ConfigError::Validation(format!(
                        "profile \"{name}\" has two zones named \"{zone_name}\" — zone names must be unique"
                    )));
                }
            }
        }
        Ok(())
    }

    /// The file written on first launch. Zones drawn with Alt+Z are appended
    /// automatically as `[[zones]]` tables, Excluded Blocks (Adjust Mode,
    /// full-screen mode) as `[[excluded_blocks]]`; the shape is documented
    /// here so the file stays hand-editable.
    pub const DEFAULT_TEMPLATE: &str = r##"# Langception configuration.
# Capture Zones you draw (Alt+Z) and Excluded Blocks you mark (Alt+A) are
# saved into this file automatically. You can also edit it by hand; restart
# the app after changing [hotkeys].

[source]
# Language Langception expects to read: "en" | "ja" | "zh" | "ko"
language = "en"

[capture]
# How often each Capture Zone is re-checked, in milliseconds.
cadence_ms = 500
# How long a Text Block whose pixels changed must hold them stable before
# it is translated (debounces animations, whose pixels never stop
# changing). 0 translates every change on the pass that detects it.
stability_ms = 400

[mode]
# What the loop watches: "zones" (the Capture Zones below) or "fullscreen"
# (every Text Block on one monitor, minus Excluded Blocks).
target = "zones"
# Monitor full-screen mode watches, by system name. Empty = the primary
# monitor. Find the names in the Results Window's mode panel.
fullscreen_monitor = ""

[text_style]
# How Thai text is drawn on the In-place Overlay, in place over the
# original. font_family may be any installed font; "Noto Sans Thai" is
# bundled with the app and always tried as a fallback, so Thai glyphs
# render even when the chosen font lacks them.
font_family = "Noto Sans Thai"
font_size = 16.0
color = "#ffffff"
# Outline around each glyph; 0 disables it.
outline_width = 2.0
outline_color = "#000000"
# Background pill behind each translation, with its opacity (0-1).
background = true
background_opacity = 0.6

[hotkeys]
# Global hotkeys, rebindable. Format: modifiers + key, e.g. "Alt+T",
# "Ctrl+Shift+Z", "F9". They apply system-wide. Rebind them in
# Settings → Hotkeys (applies immediately); hand edits apply after the
# app restarts.
toggle_translation = "Alt+T"
new_zone = "Alt+Z"
pause_translation = "Alt+P"
adjust_mode = "Alt+A"
one_shot_capture = "Alt+Q"

[glossary]
# Terms that pin a fixed Thai translation. A block whose ENTIRE text
# matches a term (trimmed, case-insensitive) shows the Thai below
# without calling the translation engine. Manage entries in
# Settings → Glossary (add/edit/delete, JSON import/export).
# Vertin = "เวอร์ติน"

# Capture Zones are appended here automatically. A zone remembers the
# screen rectangle it covers, in PHYSICAL pixels (origin = top-left of
# the primary monitor), and the monitor it was drawn on:
# [[zones]]
# id = "z1700000000000000000"
# name = "Chat box"
# monitor = "\\\\.\\DISPLAY1"
#
# [zones.region]
# x = 100
# y = 100
# width = 640
# height = 180

# Excluded Blocks are appended here automatically when you click a Text
# Block in Adjust Mode (full-screen mode only). The block at this position
# is never translated again until the entry is cleared (Results Window).
# [[excluded_blocks]]
# id = "x1700000000000000000"
# monitor = "\\\\.\\DISPLAY1"
# label = "Start"
#
# [excluded_blocks.region]
# x = 1200
# y = 340
# width = 90
# height = 24

# Profiles (Settings → Profiles) bind one app — by executable file name —
# with its own settings, active while that app is the foreground window.
# Without a matching Profile everything above applies unchanged. A
# Profile's zones are relative to its app's WINDOW (top-left = the
# window's top-left corner, physical px) and move with it; with no zones
# the whole window is watched. Its glossary is merged over the global one
# above (the profile wins on conflict); genderless: "inherit" | "on" |
# "off"; engine: "google" or omit to inherit the global engine.
# [[profiles]]
# id = "p1700000000000000000"
# name = "Reverse: 1999"
# exe = "Game.exe"
# language = "en"
# genderless = "inherit"
#
# [profiles.glossary]
# Vertin = "เวอร์ติน"
#
# [[profiles.zones]]
# id = "z1700000000000000000"
# name = "Chat box"
#
# [profiles.zones.region]
# x = 10
# y = 600
# width = 400
# height = 120
"##;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_with_zone() -> String {
        AppConfig::DEFAULT_TEMPLATE
            .replace("# [[zones]]", "[[zones]]")
            .replace("# id = \"z1700000000000000000\"", "id = \"z1\"")
            .replace("# name = \"Chat box\"", "name = \"Chat box\"")
            .replace("# [zones.region]", "[zones.region]")
            .replace("# x = 100", "x = 100")
            .replace("# y = 100", "y = 100")
            .replace("# width = 640", "width = 640")
            .replace("# height = 180", "height = 180")
    }

    #[test]
    fn default_template_parses() {
        let config = AppConfig::from_toml_str(AppConfig::DEFAULT_TEMPLATE).unwrap();
        assert_eq!(config.source.language, SourceLanguage::En);
        assert_eq!(config.capture.cadence_ms, 500);
        assert_eq!(config.mode, ModeSettings::default());
        assert_eq!(config.text_style, TextStyle::default());
        assert!(config.glossary.0.is_empty());
        assert!(config.zones.is_empty());
        assert!(config.excluded_blocks.is_empty());
        assert!(config.region.is_none());
        assert_eq!(config.hotkeys, HotkeySettings::default());
    }

    #[test]
    fn text_style_defaults_when_section_missing() {
        let text = r#"
[source]
language = "en"
"#;
        let config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(config.text_style, TextStyle::default());
    }

    #[test]
    fn text_style_roundtrips_through_toml() {
        let text = AppConfig::DEFAULT_TEMPLATE
            .replace(
                "font_family = \"Noto Sans Thai\"",
                "font_family = \"Leelawadee UI\"",
            )
            .replace("font_size = 16.0", "font_size = 24.0")
            .replace("color = \"#ffffff\"", "color = \"#0af\"")
            .replace("outline_width = 2.0", "outline_width = 0.0")
            .replace("outline_color = \"#000000\"", "outline_color = \"#123456\"")
            .replace("background = true", "background = false")
            .replace("background_opacity = 0.6", "background_opacity = 0.25");
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(
            config.text_style,
            TextStyle {
                font_family: "Leelawadee UI".into(),
                font_size: 24.0,
                color: "#0af".into(),
                outline_width: 0.0,
                outline_color: "#123456".into(),
                background: false,
                background_opacity: 0.25,
            }
        );

        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn integer_font_size_is_accepted() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("font_size = 16.0", "font_size = 18");
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.text_style.font_size, 18.0);
    }

    #[test]
    fn absurd_text_style_values_are_rejected() {
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "font_family = \"Noto Sans Thai\"",
                "font_family = \"   \"",
                "font_family",
            ),
            ("font_size = 16.0", "font_size = 4.0", "font_size"),
            ("font_size = 16.0", "font_size = 200.0", "font_size"),
            (
                "outline_width = 2.0",
                "outline_width = 9.0",
                "outline_width",
            ),
            (
                "outline_width = 2.0",
                "outline_width = -1.0",
                "outline_width",
            ),
            (
                "background_opacity = 0.6",
                "background_opacity = 1.5",
                "background_opacity",
            ),
            ("color = \"#ffffff\"", "color = \"white\"", "color"),
            (
                "outline_color = \"#000000\"",
                "outline_color = \"#12345\"",
                "outline_color",
            ),
        ];
        for (original, replacement, field) in cases {
            let text = if replacement.is_empty() {
                AppConfig::DEFAULT_TEMPLATE.replace(original, "")
            } else {
                AppConfig::DEFAULT_TEMPLATE.replace(original, replacement)
            };
            let err =
                AppConfig::from_toml_str(&text).expect_err(&format!("{field} should be rejected"));
            assert!(
                err.to_string().contains(field),
                "error should name {field}, was: {err}"
            );
        }
    }

    #[test]
    fn zones_roundtrip_through_toml() {
        let config = AppConfig::from_toml_str(&template_with_zone()).unwrap();
        assert_eq!(config.zones.len(), 1);
        assert_eq!(config.zones[0].id, "z1");
        assert_eq!(config.zones[0].name, "Chat box");
        assert_eq!(
            config.zones[0].region,
            Region {
                x: 100,
                y: 100,
                width: 640,
                height: 180
            }
        );

        let roundtripped = config.to_toml_string().unwrap();
        let reparsed = AppConfig::from_toml_str(&roundtripped).unwrap();
        assert_eq!(config, reparsed);
    }

    fn template_with_second_zone() -> String {
        template_with_zone()
            + "\n[[zones]]\nid = \"z2\"\nname = \"Quest log\"\n\n[zones.region]\nx = 0\ny = 300\nwidth = 200\nheight = 80\n"
    }

    #[test]
    fn multiple_zones_roundtrip_through_toml() {
        let config = AppConfig::from_toml_str(&template_with_second_zone()).unwrap();
        assert_eq!(config.zones.len(), 2);
        assert_eq!(config.zones[1].name, "Quest log");
        let roundtripped = config.to_toml_string().unwrap();
        assert_eq!(config, AppConfig::from_toml_str(&roundtripped).unwrap());
    }

    #[test]
    fn full_config_roundtrips_through_toml() {
        let text = template_with_zone()
            .replace("language = \"en\"", "language = \"ja\"")
            .replace("# Vertin = \"เวอร์ติน\"", "Vertin = \"เวอร์ติน\"")
            .replace(
                "toggle_translation = \"Alt+T\"",
                "toggle_translation = \"F9\"",
            );
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.source.language, SourceLanguage::Ja);
        assert_eq!(config.hotkeys.toggle_translation, "F9");
        assert_eq!(config.glossary.lookup("vertin"), Some("เวอร์ติน".to_string()));

        let roundtripped = config.to_toml_string().unwrap();
        let reparsed = AppConfig::from_toml_str(&roundtripped).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn missing_sections_get_defaults() {
        let text = r#"
[source]
language = "ko"
"#;
        let config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(config.source.language, SourceLanguage::Ko);
        assert_eq!(config.capture.cadence_ms, 500);
        assert_eq!(config.hotkeys, HotkeySettings::default());
        assert!(config.glossary.0.is_empty());
        assert!(config.zones.is_empty());
        assert!(config.region.is_none());
    }

    #[test]
    fn legacy_slice1_config_still_parses_and_migrates() {
        let text = r#"
[source]
language = "en"

[region]
x = 100
y = 100
width = 640
height = 180
"#;
        let mut config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(
            config.region,
            Some(Region {
                x: 100,
                y: 100,
                width: 640,
                height: 180
            })
        );
        assert!(config.migrate_legacy_region());
        assert!(config.region.is_none());
        assert_eq!(config.zones.len(), 1);
        assert_eq!(config.zones[0].name, "Configured region");
        assert_eq!(
            config.zones[0].region,
            Region {
                x: 100,
                y: 100,
                width: 640,
                height: 180
            }
        );

        // Migration is idempotent: a second call changes nothing.
        assert!(!config.migrate_legacy_region());
    }

    #[test]
    fn legacy_region_is_kept_when_zones_already_exist() {
        let mut config = AppConfig::from_toml_str(&template_with_zone()).unwrap();
        config.region = Some(Region {
            x: 1,
            y: 2,
            width: 30,
            height: 40,
        });
        assert!(
            !config.migrate_legacy_region(),
            "zones win over the legacy region"
        );
        assert_eq!(config.zones.len(), 1);
        assert!(config.region.is_some());
    }

    #[test]
    fn unknown_language_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("language = \"en\"", "language = \"fr\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("language"), "was: {err}");
    }

    #[test]
    fn absurd_cadence_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("cadence_ms = 500", "cadence_ms = 1");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)), "was: {err}");
    }

    #[test]
    fn stability_defaults_and_roundtrips() {
        let config = AppConfig::from_toml_str(AppConfig::DEFAULT_TEMPLATE).unwrap();
        assert_eq!(config.capture.stability_ms, 400);

        let text = AppConfig::DEFAULT_TEMPLATE.replace("stability_ms = 400", "stability_ms = 0");
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.capture.stability_ms, 0);
        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn capture_section_without_stability_gets_the_default() {
        // A ticket-01..04 config predates stability_ms.
        let text = AppConfig::DEFAULT_TEMPLATE.replace("\nstability_ms = 400", "");
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.capture.stability_ms, 400);
    }

    #[test]
    fn absurd_stability_is_rejected() {
        let text =
            AppConfig::DEFAULT_TEMPLATE.replace("stability_ms = 400", "stability_ms = 20000");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("stability_ms"), "was: {err}");
    }

    #[test]
    fn zero_sized_zone_is_rejected() {
        let text = template_with_zone().replace("width = 640", "width = 0");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)), "was: {err}");
    }

    #[test]
    fn empty_zone_name_is_rejected() {
        let text = template_with_zone().replace("name = \"Chat box\"", "name = \"   \"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("non-empty name"), "was: {err}");
    }

    #[test]
    fn duplicate_zone_names_are_rejected() {
        let text =
            template_with_second_zone().replace("name = \"Quest log\"", "name = \"Chat box\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("must be unique"), "was: {err}");
    }

    #[test]
    fn duplicate_zone_ids_are_rejected() {
        let text = template_with_second_zone().replace("id = \"z2\"", "id = \"z1\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("duplicate zone id"), "was: {err}");
    }

    #[test]
    fn empty_hotkey_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE
            .replace("pause_translation = \"Alt+P\"", "pause_translation = \" \"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("pause_translation"), "was: {err}");
    }

    #[test]
    fn mode_defaults_when_section_missing() {
        let text = r#"
[source]
language = "en"
"#;
        let config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(config.mode, ModeSettings::default());
        assert_eq!(config.mode.target, CaptureMode::Zones);
        assert_eq!(config.mode.fullscreen_monitor, "");
    }

    #[test]
    fn fullscreen_mode_roundtrips_through_toml() {
        let text = AppConfig::DEFAULT_TEMPLATE
            .replace("target = \"zones\"", "target = \"fullscreen\"")
            .replace(
                "fullscreen_monitor = \"\"",
                "fullscreen_monitor = \"\\\\\\\\.\\\\DISPLAY2\"",
            );
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.mode.target, CaptureMode::FullScreen);
        assert_eq!(config.mode.fullscreen_monitor, "\\\\.\\DISPLAY2");

        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn unknown_mode_target_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("target = \"zones\"", "target = \"window\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("target"), "was: {err}");
    }

    fn template_with_excluded_block() -> String {
        AppConfig::DEFAULT_TEMPLATE
            .replace("# [[excluded_blocks]]", "[[excluded_blocks]]")
            .replace("# id = \"x1700000000000000000\"", "id = \"x1\"")
            .replace(
                "# monitor = \"\\\\\\\\.\\\\DISPLAY1\"",
                "monitor = \"\\\\\\\\.\\\\DISPLAY1\"",
            )
            .replace("# label = \"Start\"", "label = \"เริ่ม\"")
            .replace("# [excluded_blocks.region]", "[excluded_blocks.region]")
            .replace("# x = 1200", "x = 1200")
            .replace("# y = 340", "y = 340")
            .replace("# width = 90", "width = 90")
            .replace("# height = 24", "height = 24")
    }

    #[test]
    fn excluded_blocks_roundtrip_through_toml() {
        let config = AppConfig::from_toml_str(&template_with_excluded_block()).unwrap();
        assert_eq!(config.excluded_blocks.len(), 1);
        assert_eq!(config.excluded_blocks[0].id, "x1");
        assert_eq!(config.excluded_blocks[0].label, "เริ่ม");
        assert_eq!(config.excluded_blocks[0].monitor, "\\\\.\\DISPLAY1");
        assert_eq!(
            config.excluded_blocks[0].region,
            Region {
                x: 1200,
                y: 340,
                width: 90,
                height: 24
            }
        );

        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn duplicate_excluded_block_id_is_rejected() {
        let text = template_with_excluded_block()
            + "\n[[excluded_blocks]]\nid = \"x1\"\nmonitor = \"\"\nlabel = \"Again\"\n\n[excluded_blocks.region]\nx = 10\ny = 20\nwidth = 5\nheight = 5\n";
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(
            err.to_string().contains("duplicate excluded block id"),
            "was: {err}"
        );
    }

    #[test]
    fn zero_sized_excluded_block_is_rejected() {
        let text = template_with_excluded_block().replace("width = 90", "width = 0");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)), "was: {err}");
    }

    #[test]
    fn zone_monitor_field_roundtrips_through_toml() {
        let text = template_with_zone().replace(
            "name = \"Chat box\"",
            "name = \"Chat box\"\nmonitor = \"\\\\\\\\.\\\\DISPLAY2\"",
        );
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.zones[0].monitor, "\\\\.\\DISPLAY2");

        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn old_config_without_monitor_or_adjust_hotkey_gets_defaults() {
        // A ticket-02/03 config: no zone monitor, no adjust_mode hotkey.
        // Slice-3 configs predate both fields; serde fills them from the
        // defaults instead of rejecting the file.
        let legacy = template_with_zone().replace("\nadjust_mode = \"Alt+A\"", "");
        let config = AppConfig::from_toml_str(&legacy).unwrap();
        assert_eq!(config.hotkeys.adjust_mode, DEFAULT_ADJUST_HOTKEY);
        assert_eq!(
            config.zones[0].monitor, "",
            "legacy zones default to empty = primary"
        );

        // An explicit rebind is still honored.
        let text = legacy.replace(
            "pause_translation = \"Alt+P\"",
            "pause_translation = \"Alt+P\"\nadjust_mode = \"Ctrl+Alt+A\"",
        );
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.hotkeys.adjust_mode, "Ctrl+Alt+A");
    }

    // ---- Profiles (ticket 07) ----

    fn template_with_profile() -> String {
        AppConfig::DEFAULT_TEMPLATE
            .replace("# [[profiles]]", "[[profiles]]")
            .replace("# id = \"p1700000000000000000\"", "id = \"p1\"")
            .replace("# name = \"Reverse: 1999\"", "name = \"Reverse: 1999\"")
            .replace("# exe = \"Game.exe\"", "exe = \"Game.exe\"")
            .replace("# language = \"en\"", "language = \"en\"")
            .replace("# genderless = \"inherit\"", "genderless = \"inherit\"")
            .replace("# [profiles.glossary]", "[profiles.glossary]")
            .replace("# Vertin = \"เวอร์ติน\"", "Vertin = \"เวอร์ติน\"")
            .replace("# [[profiles.zones]]", "[[profiles.zones]]")
            .replace("# id = \"z1700000000000000000\"", "id = \"pz1\"")
            .replace("# name = \"Chat box\"", "name = \"Chat box\"")
            .replace("# [profiles.zones.region]", "[profiles.zones.region]")
            // The profile zone's region values: distinct from the global
            // zone example's (`# x = 100` …) so the replaces never collide.
            .replace("# x = 10\n", "x = 10\n")
            .replace("# y = 600", "y = 600")
            .replace("# width = 400", "width = 400")
            .replace("# height = 120", "height = 120")
    }

    #[test]
    fn default_template_has_no_profiles() {
        let config = AppConfig::from_toml_str(AppConfig::DEFAULT_TEMPLATE).unwrap();
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn profile_roundtrips_through_toml() {
        let config = AppConfig::from_toml_str(&template_with_profile()).unwrap();
        assert_eq!(config.profiles.len(), 1);
        let profile = &config.profiles[0];
        assert_eq!(profile.id, "p1");
        assert_eq!(profile.name, "Reverse: 1999");
        assert_eq!(profile.exe, "Game.exe");
        assert_eq!(profile.language, SourceLanguage::En);
        assert_eq!(
            profile.genderless,
            crate::profile::GenderlessOverride::Inherit
        );
        assert_eq!(profile.engine, None, "omitted engine = inherit global");
        assert_eq!(
            profile.glossary.lookup("vertin"),
            Some("เวอร์ติน".to_string())
        );
        assert_eq!(profile.zones.len(), 1);
        assert_eq!(profile.zones[0].id, "pz1");
        assert_eq!(profile.zones[0].name, "Chat box");
        assert_eq!(
            profile.zones[0].region,
            Region {
                x: 10,
                y: 600,
                width: 400,
                height: 120
            }
        );

        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn profile_engine_choice_roundtrips() {
        let text = template_with_profile().replace(
            "genderless = \"inherit\"",
            "genderless = \"on\"\nengine = \"google\"",
        );
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(
            config.profiles[0].engine.as_deref(),
            Some("google"),
            "empty glossary stays a table"
        );
        assert_eq!(
            config.profiles[0].genderless,
            crate::profile::GenderlessOverride::On
        );
        let reparsed = AppConfig::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn ticket06_config_without_profiles_gets_the_default() {
        // A ticket-06 config predates profiles entirely.
        let legacy = template_with_zone();
        let config = AppConfig::from_toml_str(&legacy).unwrap();
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn profile_with_empty_name_is_rejected() {
        let text = template_with_profile().replace("name = \"Reverse: 1999\"", "name = \"  \"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("non-empty name"), "was: {err}");
    }

    #[test]
    fn profile_with_empty_exe_is_rejected() {
        let text = template_with_profile().replace("exe = \"Game.exe\"", "exe = \"\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("executable"), "was: {err}");
    }

    #[test]
    fn duplicate_profile_exe_is_rejected() {
        let text = template_with_profile()
            + "\n[[profiles]]\nid = \"p2\"\nname = \"Same game\"\nexe = \"GAME.EXE\"\nlanguage = \"ja\"\n";
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("same executable"), "was: {err}");
    }

    #[test]
    fn duplicate_profile_name_is_rejected() {
        let text = template_with_profile()
            + "\n[[profiles]]\nid = \"p2\"\nname = \"reverse: 1999\"\nexe = \"Other.exe\"\nlanguage = \"ja\"\n";
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("must be unique"), "was: {err}");
    }

    #[test]
    fn duplicate_profile_id_is_rejected() {
        let text = template_with_profile()
            + "\n[[profiles]]\nid = \"p1\"\nname = \"Other\"\nexe = \"Other.exe\"\nlanguage = \"ja\"\n";
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(
            err.to_string().contains("duplicate profile id"),
            "was: {err}"
        );
    }

    #[test]
    fn unknown_profile_engine_is_rejected() {
        let text = template_with_profile().replace(
            "genderless = \"inherit\"",
            "genderless = \"inherit\"\nengine = \"azure\"",
        );
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("engine \"azure\""), "was: {err}");
    }

    #[test]
    fn unknown_profile_genderless_is_rejected() {
        let text =
            template_with_profile().replace("genderless = \"inherit\"", "genderless = \"maybe\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("genderless"), "was: {err}");
    }

    #[test]
    fn zero_sized_profile_zone_is_rejected() {
        let text = template_with_profile().replace("width = 400", "width = 0");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("greater than zero"), "was: {err}");
    }

    #[test]
    fn duplicate_profile_zone_name_is_rejected() {
        let text = template_with_profile()
            + "\n[[profiles.zones]]\nid = \"pz2\"\nname = \"chat box\"\n\n[profiles.zones.region]\nx = 0\ny = 0\nwidth = 100\nheight = 100\n";
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("must be unique"), "was: {err}");
    }

    #[test]
    fn profile_zones_do_not_collide_with_global_zone_names() {
        // A Profile zone named like a global zone is fine: they are scoped
        // to their profile (and the global zone list, respectively).
        let text = template_with_profile()
            + "\n[[zones]]\nid = \"z1\"\nname = \"Chat box\"\n\n[zones.region]\nx = 1\ny = 1\nwidth = 100\nheight = 100\n";
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.profiles[0].zones.len(), 1);
        assert_eq!(config.profiles[0].zones[0].name, "Chat box");
        assert_eq!(config.zones.len(), 1);
        assert_eq!(config.zones[0].name, "Chat box");
    }
}
