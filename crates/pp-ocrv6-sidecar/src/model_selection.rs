//! Model selection (ticket 02): the complete (Source Language, Model Tier)
//! → model-files mapping, as pure logic over file names relative to the
//! models directory.
//!
//! The mapping is the spec's language→model rule made executable:
//! EN/JA/ZH read the selected tier's PP-OCRv6 detection + recognition
//! models (per-size dictionaries), and Korean always reads the official
//! Korean PP-OCRv5 recognition model over the tier's v6 detection —
//! tier-independent recognition, because PP-OCRv6's vocabulary has no
//! hangul. The tiny + Japanese unsupported pairing is handled upstream (the
//! engine substitutes Small and fires a notice, ticket 05), so this module
//! maps exactly what it is told to.
//!
//! The detection thresholds are the official PP-OCRv6 per-size defaults
//! published by PaddleOCR and mirrored by oar-ocr's `pp_ocrv6` preset
//! (score 0.2, box 0.45 — 0.4 for tiny —, unclip 1.4, up to 3000
//! candidates). They are redeclared here because the pipeline crate's
//! preset constructor only resolves registry names, not a models directory.

use std::str::FromStr;

/// A Source Language, from the `-language=` launch argument. The values
/// mirror the app's `ports::SourceLanguage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Ja,
    Zh,
    Ko,
}

/// A Model Tier, from the `-tier=` launch argument: which PP-OCRv6 model
/// size loads. Mirrors the app's `Model Tier` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Tiny,
    Small,
    Medium,
}

/// The three model files one pipeline runs on, as file names relative to
/// the models directory (the launch contract's layout — see the ticket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSelection {
    /// The text detection ONNX model.
    pub detection: &'static str,
    /// The text recognition ONNX model.
    pub recognition: &'static str,
    /// The recognition dictionary (character list).
    pub dictionary: &'static str,
}

/// The official PP-OCRv6 DB detection thresholds for one tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionThresholds {
    /// Minimum detection score for a candidate region.
    pub score: f32,
    /// Minimum box score after polygon scoring.
    pub box_score: f32,
    /// Polygon expansion ratio around a detected region.
    pub unclip: f32,
    /// Maximum candidate regions per image.
    pub max_candidates: usize,
}

/// The tier's PP-OCRv6 detection + recognition models. Tiny ships its own
/// reduced dictionary; small and medium share the full one.
pub fn selection_for(language: Language, tier: Tier) -> ModelSelection {
    let detection = det_model(tier);
    match language {
        // Korean: PP-OCRv6's vocabulary has no hangul, so recognition is
        // the official Korean PP-OCRv5 model at every tier — a
        // tier-independent mapping by definition, not a fallback.
        Language::Ko => ModelSelection {
            detection,
            recognition: "korean_pp-ocrv5_mobile_rec.onnx",
            dictionary: "ppocrv5_korean_dict.txt",
        },
        Language::En | Language::Ja | Language::Zh => ModelSelection {
            detection,
            recognition: rec_model(tier),
            dictionary: dict_for_tier(tier),
        },
    }
}

/// The official PP-OCRv6 detection thresholds for the tier.
pub fn thresholds_for(tier: Tier) -> DetectionThresholds {
    DetectionThresholds {
        score: 0.2,
        box_score: match tier {
            Tier::Tiny => 0.4,
            Tier::Small | Tier::Medium => 0.45,
        },
        unclip: 1.4,
        max_candidates: 3000,
    }
}

fn det_model(tier: Tier) -> &'static str {
    match tier {
        Tier::Tiny => "pp-ocrv6_tiny_det.onnx",
        Tier::Small => "pp-ocrv6_small_det.onnx",
        Tier::Medium => "pp-ocrv6_medium_det.onnx",
    }
}

fn rec_model(tier: Tier) -> &'static str {
    match tier {
        Tier::Tiny => "pp-ocrv6_tiny_rec.onnx",
        Tier::Small => "pp-ocrv6_small_rec.onnx",
        Tier::Medium => "pp-ocrv6_medium_rec.onnx",
    }
}

fn dict_for_tier(tier: Tier) -> &'static str {
    match tier {
        Tier::Tiny => "ppocrv6_tiny_dict.txt",
        Tier::Small | Tier::Medium => "ppocrv6_dict.txt",
    }
}

impl Language {
    /// The value the launch argument carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ja => "ja",
            Language::Zh => "zh",
            Language::Ko => "ko",
        }
    }
}

impl Tier {
    /// The value the launch argument carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Tiny => "tiny",
            Tier::Small => "small",
            Tier::Medium => "medium",
        }
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Values are exact lowercase (the app controls them; strictness
        // catches contract bugs).
        match s {
            "en" => Ok(Language::En),
            "ja" => Ok(Language::Ja),
            "zh" => Ok(Language::Zh),
            "ko" => Ok(Language::Ko),
            _ => Err(format!(
                "unknown language {s:?} (expected: en, ja, zh, ko)"
            )),
        }
    }
}

impl FromStr for Tier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tiny" => Ok(Tier::Tiny),
            "small" => Ok(Tier::Small),
            "medium" => Ok(Tier::Medium),
            _ => Err(format!(
                "unknown tier {s:?} (expected: tiny, small, medium)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(language: Language, tier: Tier) -> ModelSelection {
        selection_for(language, tier)
    }

    #[test]
    fn en_ja_zh_read_their_tiers_v6_models() {
        // The tier's det + rec pair, with the tier's dictionary — for
        // every one of the three v6 languages.
        for language in [Language::En, Language::Ja, Language::Zh] {
            assert_eq!(
                selection(language, Tier::Tiny),
                ModelSelection {
                    detection: "pp-ocrv6_tiny_det.onnx",
                    recognition: "pp-ocrv6_tiny_rec.onnx",
                    dictionary: "ppocrv6_tiny_dict.txt",
                }
            );
            assert_eq!(
                selection(language, Tier::Small),
                ModelSelection {
                    detection: "pp-ocrv6_small_det.onnx",
                    recognition: "pp-ocrv6_small_rec.onnx",
                    dictionary: "ppocrv6_dict.txt",
                }
            );
            assert_eq!(
                selection(language, Tier::Medium),
                ModelSelection {
                    detection: "pp-ocrv6_medium_det.onnx",
                    recognition: "pp-ocrv6_medium_rec.onnx",
                    dictionary: "ppocrv6_dict.txt",
                }
            );
        }
    }

    #[test]
    fn korean_reads_the_v5_korean_recognizer_at_every_tier() {
        // Recognition and dictionary are tier-independent; detection
        // follows the selected tier's v6 model.
        let tier_det = [
            (Tier::Tiny, "pp-ocrv6_tiny_det.onnx"),
            (Tier::Small, "pp-ocrv6_small_det.onnx"),
            (Tier::Medium, "pp-ocrv6_medium_det.onnx"),
        ];
        for (tier, det) in tier_det {
            assert_eq!(
                selection(Language::Ko, tier),
                ModelSelection {
                    detection: det,
                    recognition: "korean_pp-ocrv5_mobile_rec.onnx",
                    dictionary: "ppocrv5_korean_dict.txt",
                }
            );
        }
    }

    #[test]
    fn detection_thresholds_are_the_official_v6_defaults() {
        // PaddleOCR's published PP-OCRv6 DB defaults; tiny relaxes the box
        // threshold (its documented 0.4 vs 0.45). Mirrors oar-ocr's preset.
        for tier in [Tier::Tiny, Tier::Small, Tier::Medium] {
            let t = thresholds_for(tier);
            assert_eq!(t.score, 0.2);
            assert_eq!(t.unclip, 1.4);
            assert_eq!(t.max_candidates, 3000);
        }
        assert_eq!(thresholds_for(Tier::Tiny).box_score, 0.4);
        assert_eq!(thresholds_for(Tier::Small).box_score, 0.45);
        assert_eq!(thresholds_for(Tier::Medium).box_score, 0.45);
    }

    #[test]
    fn languages_and_tiers_parse_from_their_launch_values() {
        assert_eq!("en".parse::<Language>().unwrap(), Language::En);
        assert_eq!("ja".parse::<Language>().unwrap(), Language::Ja);
        assert_eq!("zh".parse::<Language>().unwrap(), Language::Zh);
        assert_eq!("ko".parse::<Language>().unwrap(), Language::Ko);
        assert_eq!("tiny".parse::<Tier>().unwrap(), Tier::Tiny);
        assert_eq!("small".parse::<Tier>().unwrap(), Tier::Small);
        assert_eq!("medium".parse::<Tier>().unwrap(), Tier::Medium);
        assert_eq!(Language::Ko.as_str(), "ko");
        assert_eq!(Tier::Small.as_str(), "small");
    }

    #[test]
    fn unknown_values_are_rejected_with_the_accepted_list() {
        let e = "fr".parse::<Language>().unwrap_err();
        assert!(e.contains("fr") && e.contains("en, ja, zh, ko"), "{e}");
        let e = "".parse::<Language>().unwrap_err();
        assert!(e.contains("expected"), "{e}");
        let e = "large".parse::<Tier>().unwrap_err();
        assert!(e.contains("large") && e.contains("tiny, small, medium"), "{e}");
    }

    #[test]
    fn values_are_case_sensitive() {
        assert!("EN".parse::<Language>().is_err());
        assert!("Small".parse::<Tier>().is_err());
    }
}
