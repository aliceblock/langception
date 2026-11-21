//! The user's config file (`config.toml` in the app-data directory). Slice 1
//! has no settings UI: the file *is* the configuration surface, written once
//! on first launch, then edited by hand.

use crate::glossary::Glossary;
use crate::ports::{Region, SourceLanguage};
use serde::{Deserialize, Serialize};
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

/// Everything slice 1 reads from the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub source: SourceSettings,
    pub region: Region,
    #[serde(default)]
    pub capture: CaptureSettings,
    #[serde(default)]
    pub glossary: Glossary,
}

impl AppConfig {
    pub fn from_toml_str(toml_text: &str) -> Result<Self, ConfigError> {
        let config: AppConfig = toml::from_str(toml_text)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Parse(e.to_string()))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.region.width == 0 || self.region.height == 0 {
            return Err(ConfigError::Validation(
                "[region] width and height must be greater than zero".into(),
            ));
        }
        if self.capture.cadence_ms < 50 {
            return Err(ConfigError::Validation(
                "[capture] cadence_ms must be at least 50".into(),
            ));
        }
        Ok(())
    }

    /// The file written on first launch. Comments document the physical-pixel
    /// region format, since there is no settings UI in slice 1.
    pub const DEFAULT_TEMPLATE: &str = r#"# Langception configuration.
# There is no settings UI yet: edit this file, then restart translation
# (tray menu -> Stop/Start) to apply.

[source]
# Language Langception expects to read: "en" | "ja" | "zh" | "ko"
language = "en"

[region]
# The screen region Langception watches, in PHYSICAL pixels
# (origin = top-left of the primary monitor; check your display scale).
x = 100
y = 100
width = 640
height = 180

[capture]
# How often the watched region is re-checked, in milliseconds.
cadence_ms = 500

[glossary]
# Terms that pin a fixed Thai translation. A block whose ENTIRE text
# matches a term (trimmed, case-insensitive) shows the Thai below
# without calling the translation engine.
# Vertin = "เวอร์ติน"
"#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses() {
        let config = AppConfig::from_toml_str(AppConfig::DEFAULT_TEMPLATE).unwrap();
        assert_eq!(config.source.language, SourceLanguage::En);
        assert_eq!(config.region, Region { x: 100, y: 100, width: 640, height: 180 });
        assert_eq!(config.capture.cadence_ms, 500);
        assert!(config.glossary.0.is_empty());
    }

    #[test]
    fn full_config_roundtrips_through_toml() {
        let text = AppConfig::DEFAULT_TEMPLATE
            .replace("language = \"en\"", "language = \"ja\"")
            .replace("# Vertin = \"เวอร์ติน\"", "Vertin = \"เวอร์ติน\"");
        let config = AppConfig::from_toml_str(&text).unwrap();
        assert_eq!(config.source.language, SourceLanguage::Ja);
        assert_eq!(
            config.glossary.lookup("vertin"),
            Some("เวอร์ติน".to_string())
        );

        let roundtripped = config.to_toml_string().unwrap();
        let reparsed = AppConfig::from_toml_str(&roundtripped).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn missing_glossary_and_capture_sections_get_defaults() {
        let text = r#"
[source]
language = "ko"

[region]
x = 0
y = 0
width = 800
height = 100
"#;
        let config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(config.source.language, SourceLanguage::Ko);
        assert_eq!(config.capture.cadence_ms, 500);
        assert!(config.glossary.0.is_empty());
    }

    #[test]
    fn unknown_language_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("language = \"en\"", "language = \"fr\"");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(err.to_string().contains("language"), "was: {err}");
    }

    #[test]
    fn zero_sized_region_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("width = 640", "width = 0");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)), "was: {err}");
    }

    #[test]
    fn absurd_cadence_is_rejected() {
        let text = AppConfig::DEFAULT_TEMPLATE.replace("cadence_ms = 500", "cadence_ms = 1");
        let err = AppConfig::from_toml_str(&text).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)), "was: {err}");
    }
}
