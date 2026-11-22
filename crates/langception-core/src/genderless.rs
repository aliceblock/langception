//! Genderless Mode (CONTEXT.md): the rule-based Thai post-processor that
//! scrubs speaker gender from Translate Engine output — sentence-final
//! politeness particles (ครับ/ค่ะ/คะ/จ้ะ/จ้า/นะครับ/นะคะ) and gendered
//! first-person pronouns (ผม/ฉัน/หนู → เรา). Pure logic; the pipeline
//! seam tests it end to end with a fake engine (ticket 08). LLM engines
//! replace this with a prompt-level variant (ticket 11).
//!
//! Rule-based means deliberately conservative:
//! - Particles strip only at the END of the text — that is where they are
//!   politeness markers, and omitting them is natural written Thai.
//! - Pronouns replace only at token starts (string start or after
//!   whitespace/punctuation), where Thai clauses put the subject
//!   ("ฉันไม่รู้" → "เราไม่รู้"). A pronoun fused into other letters is
//!   part of another word: the ผม of "ทรงผม" (hairstyle) is not a speaker.
//! - ผม also means "hair" and หนู also means "mouse"; a clause-initial use
//!   of those senses is neutralized anyway. That is the accepted cost of
//!   having no parser — blind substring replacement would corrupt far more
//!   text than this saves.
//! - Text that is nothing but particles must not scrub to empty (an empty
//!   translation would strand a blank overlay pill), so the engine's
//!   original comes back instead.

/// Sentence-final gendered politeness particles (spec), LONGEST first so
/// the นะ- compounds strip before their bare ค- suffixes — matching bare
/// "ครับ" inside "นะครับ" would strand a dangling "นะ".
const PARTICLES: [&str; 7] = ["นะครับ", "นะคะ", "ครับ", "จ้ะ", "จ้า", "ค่ะ", "คะ"];

/// Sentence punctuation a particle may precede at the very end ("ดีครับ.");
/// Thai MT output usually drops it, but OCR'd game strings keep it.
const TRAILING_PUNCT: [char; 5] = ['.', '!', '?', '~', '…'];

/// Gendered first-person pronouns (spec) — replaced with [`NEUTRAL`].
const PRONOUNS: [&str; 3] = ["ผม", "ฉัน", "หนู"];

/// The gender-neutral first-person pronoun pronouns are replaced with.
const NEUTRAL: &str = "เรา";

/// Whether `c` can separate a pronoun from the word before it: whitespace
/// or sentence punctuation. Thai writes clauses fused (no spaces), so a
/// pronoun directly attached to other letters belongs to another word.
fn starts_token(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '.' | '!' | '?' | ',' | ':' | ';' | '-' | '~' | '…' | '(' | ')' | '"' | '\''
        )
}

/// The Genderless Mode scrubber: engine output in, genderless Thai out.
/// Strips sentence-final gendered particles, then neutralizes gendered
/// first-person pronouns. Text without either passes through untouched.
pub fn scrub(text: &str) -> String {
    let scrubbed = neutralize_pronouns(&strip_final_particles(text));
    if scrubbed.trim().is_empty() && !text.trim().is_empty() {
        // The whole text was particles ("ครับ"): scrubbing would show an
        // empty pill, so the engine's original stands.
        text.to_string()
    } else {
        scrubbed
    }
}

/// Repeatedly strips a gendered particle off the end, tolerating trailing
/// whitespace/punctuation between repetitions ("ครับครับ", "ดีครับ.").
/// Punctuation only goes away when a particle under it actually strips.
fn strip_final_particles(text: &str) -> String {
    let mut rest = text;
    loop {
        let body =
            rest.trim_end_matches(|c: char| c.is_whitespace() || TRAILING_PUNCT.contains(&c));
        let Some(base) = PARTICLES.iter().find_map(|&p| body.strip_suffix(p)) else {
            return rest.to_string();
        };
        rest = base;
    }
}

/// Replaces every gendered first-person pronoun that starts a token
/// (string start or right after [`starts_token`]) with [`NEUTRAL`].
fn neutralize_pronouns(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut boundary = true; // the string's start is a token start
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if boundary {
            if let Some(remainder) = PRONOUNS.iter().find_map(|&p| rest.strip_prefix(p)) {
                out.push_str(NEUTRAL);
                rest = remainder;
                boundary = false; // a pronoun's end is not a new token start
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
        boundary = starts_token(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Particles ----

    #[test]
    fn strips_sentence_final_particles() {
        assert_eq!(scrub("สวัสดีครับ"), "สวัสดี");
        assert_eq!(scrub("รับทราบค่ะ"), "รับทราบ");
        assert_eq!(scrub("จริงเหรอคะ"), "จริงเหรอ");
        assert_eq!(scrub("ขอบใจจ้า"), "ขอบใจ");
        assert_eq!(scrub("ได้จ้ะ"), "ได้");
    }

    #[test]
    fn strips_na_compounds_before_their_bare_suffixes() {
        // "นะครับ"/"นะคะ" must not strip as "ครับ"/"คะ" (leaving "…นะ").
        assert_eq!(scrub("ได้นะครับ"), "ได้");
        assert_eq!(scrub("ได้นะคะ"), "ได้");
    }

    #[test]
    fn strips_doubled_particles() {
        assert_eq!(scrub("รับทราบค่ะค่ะ"), "รับทราบ");
        assert_eq!(scrub("เข้าใจแล้วครับครับ"), "เข้าใจแล้ว");
    }

    #[test]
    fn tolerates_trailing_space_and_punctuation_after_a_particle() {
        assert_eq!(scrub("ดีครับ "), "ดี");
        assert_eq!(scrub("โอเคครับ."), "โอเค");
        assert_eq!(scrub("ได้ค่ะ! "), "ได้");
    }

    #[test]
    fn punctuation_without_a_particle_stays_untouched() {
        assert_eq!(scrub("สวัสดี!"), "สวัสดี!");
        assert_eq!(scrub("จบบท."), "จบบท.");
    }

    #[test]
    fn mid_sentence_particles_are_not_touched() {
        assert_eq!(scrub("โอเคครับ ไปกันเลย"), "โอเคครับ ไปกันเลย");
    }

    // ---- Pronouns ----

    #[test]
    fn replaces_clause_initial_gendered_pronouns() {
        assert_eq!(scrub("ฉันไม่รู้"), "เราไม่รู้");
        assert_eq!(scrub("ผมจะไป"), "เราจะไป");
        assert_eq!(scrub("หนูอยากกิน"), "เราอยากกิน");
    }

    #[test]
    fn replaces_pronouns_after_a_boundary() {
        assert_eq!(scrub("ถ้าไปได้ ฉันจะไป"), "ถ้าไปได้ เราจะไป");
        assert_eq!(scrub("โอเค, ผมทำเอง"), "โอเค, เราทำเอง");
    }

    #[test]
    fn replaces_every_occurrence() {
        assert_eq!(scrub("ฉันทำ ฉันรับผิดชอบ"), "เราทำ เรารับผิดชอบ");
    }

    #[test]
    fn pronouns_fused_into_other_words_survive() {
        // The ผม of "ทรงผม" (hairstyle) and the หนู of "กระรอกกับหนู"
        // (squirrel and mouse) are not speakers.
        assert_eq!(scrub("ทรงผมดี"), "ทรงผมดี");
        assert_eq!(scrub("กระรอกกับหนู"), "กระรอกกับหนู");
    }

    // ---- Combined + fallbacks ----

    #[test]
    fn scrubs_particle_and_pronoun_together() {
        assert_eq!(scrub("ฉันจะไปครับ"), "เราจะไป");
    }

    #[test]
    fn non_thai_text_passes_through() {
        assert_eq!(scrub("Hello, world!"), "Hello, world!");
        assert_eq!(scrub("123"), "123");
    }

    #[test]
    fn all_particle_text_keeps_the_engine_output() {
        // Scrubbing "ครับครับ" to empty would strand a blank overlay pill;
        // the original stands instead.
        assert_eq!(scrub("ครับ"), "ครับ");
        assert_eq!(scrub("ครับครับ"), "ครับครับ");
        assert_eq!(scrub("ค่ะจ้ะครับ"), "ค่ะจ้ะครับ");
    }
}
