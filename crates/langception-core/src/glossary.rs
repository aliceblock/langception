//! Glossary: a user-defined mapping from a source-language term to a fixed
//! Thai translation that takes precedence over Translate Engine output.
//!
//! Matching rule (spec): trimmed, case-insensitive, **whole-block exact
//! match** short-circuits the engine entirely. Sub-sentence replacement is
//! explicitly out of scope (cross-script replacement is unreliable).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Whether `needle` (lowercased) occurs in `haystack` (lowercased chars) at
/// a word edge — the injection rule of [`Glossary::relevant_entries`]. A
/// match glued to an ASCII letter/digit/underscore is inside a different
/// word and does not count; anything else (space, punctuation, start/end,
/// or a character from a script without word separation) does.
fn contains_at_word_edge(haystack: &[char], needle: &str) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for start in 0..=haystack.len().saturating_sub(needle.len()) {
        if haystack[start..start + needle.len()] != needle[..] {
            continue;
        }
        let glued_before = start > 0 && is_word_char(haystack[start - 1]);
        let end = start + needle.len();
        let glued_after = end < haystack.len() && is_word_char(haystack[end]);
        if !glued_before && !glued_after {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Glossary(#[serde(default)] pub BTreeMap<String, String>);

/// What an import changed: new terms vs overwritten values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeStats {
    pub added: usize,
    pub updated: usize,
}

impl Glossary {
    /// Whole-block lookup: `block_text` is trimmed and compared
    /// case-insensitively against every term (also trimmed).
    pub fn lookup(&self, block_text: &str) -> Option<String> {
        let text = block_text.trim();
        if text.is_empty() {
            return None;
        }
        self.0
            .iter()
            .filter(|(term, _)| !term.trim().is_empty())
            .find(|(term, _)| term.trim().eq_ignore_ascii_case(text))
            .map(|(_, thai)| thai.clone())
    }

    /// Whether `term` collides with an existing entry under the lookup rule
    /// (trimmed, case-insensitive) — the check every editor/import path runs
    /// so the map never holds two entries that match the same block text.
    pub fn has_term_ci(&self, term: &str) -> bool {
        let wanted = term.trim().to_string();
        !wanted.is_empty()
            && self
                .0
                .keys()
                .any(|key| key.trim().eq_ignore_ascii_case(&wanted))
    }

    /// The entries whose source term occurs anywhere in `block_text`
    /// (trimmed, case-insensitive) — the LLM prompt injection set (ticket
    /// 11): when an LLM engine translates the block, these ride in the
    /// prompt so in-sentence names come back per the Glossary even though
    /// the block is not an exact match (whole-block matches never reach
    /// here; they short-circuit the engine entirely). Ordered by term (the
    /// map's own order), so prompts are deterministic.
    ///
    /// Matching is at word edges for space-delimited scripts: a match
    /// glued to ASCII letters/digits is part of a DIFFERENT word ("Son"
    /// inside "Person" must not inject a pinned translation into an
    /// unrelated word). Scripts without word separation (Thai, Japanese,
    /// Chinese — the Source Languages) match as substrings: their
    /// characters are morphemes, there are no spaces to bound with, and a
    /// substring hit is the word.
    pub fn relevant_entries(&self, block_text: &str) -> Vec<(String, String)> {
        let haystack: Vec<char> = block_text.to_lowercase().chars().collect();
        self.0
            .iter()
            .filter(|(term, _)| {
                let term = term.trim();
                !term.is_empty() && contains_at_word_edge(&haystack, &term.to_lowercase())
            })
            .map(|(term, thai)| (term.trim().to_string(), thai.clone()))
            .collect()
    }

    /// Inserts or updates one entry (both sides trimmed). Returns whether
    /// the map changed (a value update also counts as changed; re-inserting
    /// the identical pair does not).
    pub fn upsert(&mut self, term: &str, thai: &str) -> bool {
        let term = term.trim();
        let thai = thai.trim();
        if term.is_empty() || thai.is_empty() {
            return false;
        }
        // Case-insensitive hits normalize onto the existing key, so lookup
        // ("VERTIN" matches "Vertin") always finds exactly one entry.
        let key = match self.0.iter().find(|(key, _)| key.trim().eq_ignore_ascii_case(term)) {
            Some((key, _)) => key.clone(),
            None => term.to_string(),
        };
        match self.0.get(&key) {
            Some(existing) if existing == thai => false,
            _ => {
                self.0.insert(key, thai.to_string());
                true
            }
        }
    }

    /// Removes one entry by exact key. Returns whether it existed.
    pub fn remove(&mut self, term: &str) -> bool {
        self.0.remove(term).is_some()
    }

    /// Merges `other` in: new terms are added, existing ones take `other`'s
    /// Thai value (import wins — the user just chose it). Counted in
    /// [`MergeStats`].
    pub fn merge(&mut self, other: &Glossary) -> MergeStats {
        let mut stats = MergeStats { added: 0, updated: 0 };
        for (term, thai) in &other.0 {
            let existed = self.has_term_ci(term);
            if self.upsert(term, thai) {
                if existed {
                    stats.updated += 1;
                } else {
                    stats.added += 1;
                }
            }
        }
        stats
    }

    /// Drops every entry, keeping the container.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Serializes as a plain JSON object (`{"term": "thai"}`) — the backup /
    /// share format for import/export (spec: "import/export the Glossary as
    /// JSON"). Pretty-printed so exported files diff nicely.
    pub fn to_json_string(&self) -> String {
        let map: &BTreeMap<String, String> = &self.0;
        serde_json::to_string_pretty(map).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parses the import format written by [`Glossary::to_json_string`]:
    /// a JSON object of `term → Thai` strings. Every error names the
    /// offending entry so a bad file is diagnosable without a debugger.
    pub fn parse_json_str(text: &str) -> Result<Glossary, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "expected a JSON object like {\"term\": \"คำแปล\"}".to_string())?;
        let mut glossary = Glossary::default();
        for (term, thai) in object {
            let Some(thai) = thai.as_str() else {
                return Err(format!(
                    "entry \"{term}\" must map to a string, found {}",
                    if thai.is_null() { "null" } else { "a number/bool/array" }
                ));
            };
            if term.trim().is_empty() {
                return Err("a term is empty or whitespace-only".to_string());
            }
            if thai.trim().is_empty() {
                return Err(format!("term \"{term}\" has an empty translation"));
            }
            glossary.upsert(term, thai);
        }
        Ok(glossary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary(entries: &[(&str, &str)]) -> Glossary {
        Glossary(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn exact_match_returns_thai() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("Vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn match_is_case_insensitive() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("VERTIN"), Some("เวอร์ติน".to_string()));
        assert_eq!(g.lookup("vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn match_trims_surrounding_whitespace() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("  Vertin  "), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn term_with_surrounding_whitespace_still_matches() {
        let g = glossary(&[("  Vertin  ", "เวอร์ติน")]);
        assert_eq!(g.lookup("Vertin"), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn partial_match_does_not_short_circuit() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        // Whole-block exact match only — a sentence containing the term is
        // left to the engine.
        assert_eq!(g.lookup("Vertin appears here"), None);
        assert_eq!(g.lookup("Vert"), None);
    }

    #[test]
    fn whitespace_only_block_never_matches() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(g.lookup("   "), None);
        assert_eq!(g.lookup(""), None);
    }

    #[test]
    fn non_latin_term_matches_exactly() {
        let g = glossary(&[("勇者", "ผู้กล้า")]);
        assert_eq!(g.lookup("勇者"), Some("ผู้กล้า".to_string()));
        assert_eq!(g.lookup(" 勇者 "), Some("ผู้กล้า".to_string()));
    }

    #[test]
    fn serializes_as_plain_toml_table() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        let toml = toml::to_string(&g).unwrap();
        assert!(toml.contains("Vertin = \"เวอร์ติน\""), "toml was: {toml}");
    }

    // ---- Editor primitives (settings window, ticket 06) ----

    #[test]
    fn upsert_adds_and_updates() {
        let mut g = Glossary::default();
        assert!(g.upsert("Vertin", "เวอร์ติน"));
        assert_eq!(g.lookup("VERTIN"), Some("เวอร์ติน".to_string()));
        assert!(g.upsert("Vertin", "เวอร์ตินทัน"), "value change counts");
        assert_eq!(g.lookup("vertin"), Some("เวอร์ตินทัน".to_string()));
        assert!(!g.upsert("Vertin", "เวอร์ตินทัน"), "no-op reinsert");
        assert_eq!(g.0.len(), 1);
    }

    #[test]
    fn upsert_normalizes_case_onto_existing_key() {
        let mut g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert!(g.upsert("  VERTIN  ", "ผู้กล้า"));
        assert_eq!(g.0.len(), 1, "no second key that matches the same text");
        assert_eq!(g.lookup("Vertin"), Some("ผู้กล้า".to_string()));
    }

    #[test]
    fn upsert_rejects_empty_sides() {
        let mut g = Glossary::default();
        assert!(!g.upsert("   ", "เวอร์ติน"));
        assert!(!g.upsert("Vertin", "  "));
        assert!(g.0.is_empty());
    }

    #[test]
    fn remove_removes_only_exact_key() {
        let mut g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert!(!g.remove("vertin"), "lookup is case-insensitive, remove is not");
        assert!(g.remove("Vertin"));
        assert!(g.0.is_empty());
    }

    #[test]
    fn has_term_ci_finds_case_variants() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert!(g.has_term_ci("vertin"));
        assert!(g.has_term_ci(" Vertin "));
        assert!(!g.has_term_ci("Other"));
        assert!(!g.has_term_ci("  "));
    }

    #[test]
    fn merge_counts_added_and_updated() {
        let mut g = glossary(&[("Vertin", "old"), ("Keep", "keep")]);
        let stats = g.merge(&glossary(&[
            ("VERTIN", "new"), // case-insensitive hit → update
            ("New", "added"),  // new term → add
            ("Keep", "keep"),  // identical → neither
        ]));
        assert_eq!(stats, MergeStats { added: 1, updated: 1 });
        assert_eq!(g.lookup("vertin"), Some("new".to_string()));
        assert_eq!(g.lookup("keep"), Some("keep".to_string()));
        assert_eq!(g.0.len(), 3);
    }

    // ---- In-sentence relevance (LLM prompt injection, ticket 11) ----

    #[test]
    fn relevant_entries_find_terms_inside_a_sentence() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(
            g.relevant_entries("Vertin picks up the case"),
            vec![("Vertin".to_string(), "เวอร์ติน".to_string())]
        );
    }

    #[test]
    fn relevant_entries_match_case_insensitively() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert_eq!(
            g.relevant_entries("VERTIN PICKS UP THE CASE").len(),
            1,
            "case differences must not hide a relevant term"
        );
    }

    #[test]
    fn relevant_entries_return_only_the_terms_that_appear() {
        let g = glossary(&[("Vertin", "เวอร์ติน"), ("Regulus", "เรกุลุส"), ("Sonetto", "โซเน็ตโต")]);
        assert_eq!(
            g.relevant_entries("Regulus glares at Sonetto"),
            vec![
                ("Regulus".to_string(), "เรกุลุส".to_string()),
                ("Sonetto".to_string(), "โซเน็ตโต".to_string()),
            ],
            "map order (alphabetical) keeps prompts deterministic"
        );
    }

    #[test]
    fn block_without_any_term_has_no_relevant_entries() {
        let g = glossary(&[("Vertin", "เวอร์ติน")]);
        assert!(g.relevant_entries("Pay 500 gold").is_empty());
        assert!(g.relevant_entries("").is_empty());
    }

    #[test]
    fn relevant_entries_skip_empty_terms_and_trim_themselves() {
        let g = glossary(&[("  Vertin  ", "เวอร์ติน")]);
        assert_eq!(
            g.relevant_entries("well, Vertin again"),
            vec![("Vertin".to_string(), "เวอร์ติน".to_string())],
            "the prompt sees the trimmed term, never padded whitespace"
        );
    }

    #[test]
    fn non_latin_terms_are_found_in_sentence() {
        let g = glossary(&[("勇者", "ผู้กล้า")]);
        assert_eq!(
            g.relevant_entries("その勇者は強い"),
            vec![("勇者".to_string(), "ผู้กล้า".to_string())]
        );
    }

    #[test]
    fn terms_glued_into_a_larger_word_do_not_inject() {
        // "Son" inside "Person" is a different word — injecting a pinned
        // translation for it would corrupt the sentence.
        let g = glossary(&[("Son", "ลูกชาย")]);
        assert!(g.relevant_entries("Person arrives").is_empty());
        assert!(g.relevant_entries("Sonetto picks up the case").is_empty());
        assert!(g.relevant_entries("the Sonnet").is_empty());
        // Numbers and underscore glue too ("V2" inside "V25").
        assert!(glossary(&[("V2", "วีสอง")])
            .relevant_entries("route V25")
            .is_empty());
    }

    #[test]
    fn the_standalone_word_still_injects() {
        let g = glossary(&[("Son", "ลูกชาย")]);
        assert_eq!(
            g.relevant_entries("Son speaks"),
            vec![("Son".to_string(), "ลูกชาย".to_string())]
        );
        // Punctuation counts as an edge.
        assert_eq!(
            g.relevant_entries("(Son), please"),
            vec![("Son".to_string(), "ลูกชาย".to_string())]
        );
    }

    #[test]
    fn thai_terms_match_as_substrings_no_word_edges_exist() {
        // Thai writes clauses fused: a Thai term inside Thai text cannot be
        // boundary-checked (no spaces), so the substring IS the match.
        let g = glossary(&[("ไปกิน", "ไปกินข้าว")]);
        assert_eq!(
            g.relevant_entries("ฉันไปกินแล้ว"),
            vec![("ไปกิน".to_string(), "ไปกินข้าว".to_string())]
        );
    }

    // ---- JSON import/export (spec: backup and share term lists) ----

    #[test]
    fn json_round_trips() {
        let g = glossary(&[("Vertin", "เวอร์ติน"), ("勇者", "ผู้กล้า")]);
        let json = g.to_json_string();
        let reparsed = Glossary::parse_json_str(&json).unwrap();
        assert_eq!(g, reparsed);
    }

    #[test]
    fn empty_glossary_exports_and_imports() {
        let json = Glossary::default().to_json_string();
        assert_eq!(Glossary::parse_json_str(&json).unwrap(), Glossary::default());
    }

    #[test]
    fn json_parses_a_shared_file() {
        let g = Glossary::parse_json_str(r#"{"Vertin": "เวอร์ติน", "勇者": "ผู้กล้า"}"#).unwrap();
        assert_eq!(g.lookup(" vertin "), Some("เวอร์ติน".to_string()));
    }

    #[test]
    fn json_rejects_non_objects() {
        for text in ["[]", "42", "\"x\"", "null", "{", "not json at all"] {
            let err = Glossary::parse_json_str(text).unwrap_err();
            assert!(!err.is_empty(), "{text} should fail with a message");
        }
    }

    #[test]
    fn json_rejects_non_string_values_and_empty_sides() {
        let err = Glossary::parse_json_str(r#"{"Vertin": 3}"#).unwrap_err();
        assert!(err.contains("Vertin"), "was: {err}");
        let err = Glossary::parse_json_str(r#"{"Vertin": null}"#).unwrap_err();
        assert!(err.contains("Vertin"), "was: {err}");
        let err = Glossary::parse_json_str(r#"{"   ": "x"}"#).unwrap_err();
        assert!(err.contains("empty"), "was: {err}");
        let err = Glossary::parse_json_str(r#"{"Vertin": "  "}"#).unwrap_err();
        assert!(err.contains("Vertin"), "was: {err}");
    }

    #[test]
    fn json_import_survives_duplicate_and_trimmed_keys() {
        // serde_json's map keeps the last duplicate; trimming is applied by
        // upsert. Either way the result holds exactly one usable entry.
        let g = Glossary::parse_json_str(r#"{" Vertin ": "one", "VERTIN": "two"}"#).unwrap();
        assert_eq!(g.0.len(), 1, "case variants normalize to one entry");
        assert_eq!(g.lookup("Vertin"), Some("two".to_string()));
    }
}
