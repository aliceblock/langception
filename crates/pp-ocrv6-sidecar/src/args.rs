//! Launch arguments (ticket 02): the sidecar's launch contract, parsed as
//! pure logic. The binary takes exactly three `-key=value` (or
//! `--key=value`) arguments — the models directory, the Source Language,
//! and the Model Tier — mirroring the bundled engine's `-key=value` style:
//!
//! ```text
//! pp-ocrv6-sidecar.exe -models_path=<dir> -language=<en|ja|zh|ko> -tier=<tiny|small|medium>
//! ```

use std::path::PathBuf;

use crate::model_selection::{Language, Tier};

/// Everything the sidecar needs before it can load its pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    /// Directory containing the model files (layout in the ticket).
    pub models_path: PathBuf,
    /// The Source Language whose model mapping applies.
    pub language: Language,
    /// The Model Tier to load.
    pub tier: Tier,
}

/// Parses launch arguments (everything after argv[0]). Unknown keys,
/// missing keys, and bad values are errors naming the offender.
pub fn parse_args<I>(args: I) -> Result<LaunchArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let mut models_path: Option<String> = None;
    let mut language: Option<Language> = None;
    let mut tier: Option<Tier> = None;

    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| format!("expected -key=value, got {arg:?}"))?;
        let key = key
            .strip_prefix("--")
            .or_else(|| key.strip_prefix('-'))
            .ok_or_else(|| format!("expected -key=value, got {arg:?}"))?;
        if value.is_empty() {
            return Err(format!("-{key}= needs a value"));
        }
        match key {
            "models_path" => models_path = Some(value.to_string()),
            "language" => language = Some(value.parse()?),
            "tier" => tier = Some(value.parse()?),
            other => {
                return Err(format!(
                    "unknown argument -{other}= (expected: models_path, language, tier)"
                ))
            }
        }
    }

    Ok(LaunchArgs {
        models_path: PathBuf::from(models_path.ok_or("missing -models_path=<dir>")?),
        language: language.ok_or("missing -language=<en|ja|zh|ko>")?,
        tier: tier.ok_or("missing -tier=<tiny|small|medium>")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<LaunchArgs, String> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    fn valid() -> LaunchArgs {
        LaunchArgs {
            models_path: PathBuf::from("resources/pp-ocrv6"),
            language: Language::Ko,
            tier: Tier::Small,
        }
    }

    #[test]
    fn the_three_keys_define_the_launch_contract() {
        assert_eq!(
            args(&[
                "-models_path=resources/pp-ocrv6",
                "-language=ko",
                "-tier=small"
            ])
            .unwrap(),
            valid()
        );
        // Double dashes are accepted for humans demoing the binary.
        assert_eq!(
            args(&[
                "--models_path=resources/pp-ocrv6",
                "--language=ko",
                "--tier=small"
            ])
            .unwrap(),
            valid()
        );
    }

    #[test]
    fn missing_keys_are_errors_naming_the_key() {
        let e = args(&["-language=en", "-tier=tiny"]).unwrap_err();
        assert!(e.contains("models_path"), "{e}");
        let e = args(&["-models_path=m", "-tier=tiny"]).unwrap_err();
        assert!(e.contains("language"), "{e}");
        let e = args(&["-models_path=m", "-language=en"]).unwrap_err();
        assert!(e.contains("tier"), "{e}");
        assert!(args(&[]).unwrap_err().contains("models_path"));
    }

    #[test]
    fn unknown_and_malformed_arguments_are_errors() {
        let e = args(&["-models_path=m", "-language=en", "-tier=tiny", "-gpu=on"])
            .unwrap_err();
        assert!(e.contains("gpu"), "{e}");
        let e = args(&["models_path"]).unwrap_err();
        assert!(e.contains("-key=value"), "{e}");
        let e = args(&["key=value"]).unwrap_err();
        assert!(e.contains("-key=value"), "{e}");
        let e = args(&["-language="]).unwrap_err();
        assert!(e.contains("needs a value"), "{e}");
    }

    #[test]
    fn bad_values_surface_the_language_or_tier_error() {
        let e = args(&["-models_path=m", "-language=fr", "-tier=tiny"]).unwrap_err();
        assert!(e.contains("fr"), "{e}");
        let e = args(&["-models_path=m", "-language=en", "-tier=xl"]).unwrap_err();
        assert!(e.contains("xl"), "{e}");
    }
}
