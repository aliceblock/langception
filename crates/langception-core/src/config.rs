//! The user's config file (`config.toml` in the app-data directory). It is
//! the hand-editing surface: Capture Zones drawn in the app are persisted
//! here automatically, and hotkeys are rebindable here. The file is written
//! once on first launch, then kept in sync by the app.

use crate::glossary::Glossary;
use crate::ports::{Region, SourceLanguage, Zone};
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
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            cadence_ms: default_cadence_ms(),
        }
    }
}

fn default_cadence_ms() -> u32 {
    500
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

/// Global hotkeys (spec: toggle / zone / pause / one-shot), rebindable here.
/// Values are accelerator strings like "Alt+T" or "Ctrl+Shift+Z"; parsing
/// happens where they are registered (the shell, at startup), so only
/// non-emptiness is validated here. Changes apply on app restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub toggle_translation: String,
    pub new_zone: String,
    pub pause_translation: String,
    pub one_shot_capture: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            toggle_translation: DEFAULT_TOGGLE_HOTKEY.to_string(),
            new_zone: DEFAULT_NEW_ZONE_HOTKEY.to_string(),
            pause_translation: DEFAULT_PAUSE_HOTKEY.to_string(),
            one_shot_capture: DEFAULT_ONE_SHOT_HOTKEY.to_string(),
        }
    }
}

pub const DEFAULT_TOGGLE_HOTKEY: &str = "Alt+T";
pub const DEFAULT_NEW_ZONE_HOTKEY: &str = "Alt+Z";
pub const DEFAULT_PAUSE_HOTKEY: &str = "Alt+P";
pub const DEFAULT_ONE_SHOT_HOTKEY: &str = "Alt+Q";

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
    pub hotkeys: HotkeySettings,
    #[serde(default)]
    pub text_style: TextStyle,
    #[serde(default)]
    pub glossary: Glossary,
    #[serde(default)]
    pub zones: Vec<Zone>,
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
        Ok(())
    }

    /// The file written on first launch. Zones drawn with Alt+Z are appended
    /// automatically as `[[zones]]` tables; the shape is documented here so
    /// the file stays hand-editable.
    pub const DEFAULT_TEMPLATE: &str = r##"# Langception configuration.
# Capture Zones you draw (Alt+Z) are saved into this file automatically.
# You can also edit it by hand; restart the app after changing [hotkeys].

[source]
# Language Langception expects to read: "en" | "ja" | "zh" | "ko"
language = "en"

[capture]
# How often each Capture Zone is re-checked, in milliseconds.
cadence_ms = 500

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
# "Ctrl+Shift+Z", "F9". They apply system-wide.
toggle_translation = "Alt+T"
new_zone = "Alt+Z"
pause_translation = "Alt+P"
one_shot_capture = "Alt+Q"

[glossary]
# Terms that pin a fixed Thai translation. A block whose ENTIRE text
# matches a term (trimmed, case-insensitive) shows the Thai below
# without calling the translation engine.
# Vertin = "เวอร์ติน"

# Capture Zones are appended here automatically. A zone remembers the
# screen rectangle it covers, in PHYSICAL pixels (origin = top-left of
# the primary monitor):
# [[zones]]
# id = "z1700000000000000000"
# name = "Chat box"
#
# [zones.region]
# x = 100
# y = 100
# width = 640
# height = 180
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
        assert_eq!(config.text_style, TextStyle::default());
        assert!(config.glossary.0.is_empty());
        assert!(config.zones.is_empty());
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
}
