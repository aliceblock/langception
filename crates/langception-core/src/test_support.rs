//! Test-only fixture builders shared by unit tests across modules
//! (compiled only under `cfg(test)`; never part of the shipped crate).

use crate::google::{LegResponse, LegTransport};
use crate::ports::{Clock, TranslationGuidance};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A [`TranslationGuidance`] from `(term, thai)` pairs plus the genderless
/// flag — the guidance shape every LLM-adapter test builds.
pub(crate) fn guided(entries: &[(&str, &str)], genderless: bool) -> TranslationGuidance {
    TranslationGuidance {
        glossary: entries
            .iter()
            .map(|(term, thai)| (term.to_string(), thai.to_string()))
            .collect(),
        genderless,
        custom_base: None,
        preset: None,
        recent_translations: Vec::new(),
    }
}

// ---- the Google leg-chain harness (ticket 02) ----

/// A wall clock the tests move by hand, so cool-down windows run
/// instantly instead of waiting real minutes.
pub(crate) struct FakeClock(Mutex<u64>);

impl FakeClock {
    pub(crate) fn at(ms: u64) -> Arc<Self> {
        Arc::new(Self(Mutex::new(ms)))
    }

    pub(crate) fn advance(&self, ms: u64) {
        *self.0.lock().unwrap() += ms;
    }
}

#[async_trait]
impl Clock for FakeClock {
    async fn sleep(&self, _duration: Duration) {}
    fn now_ms(&self) -> u64 {
        *self.0.lock().unwrap()
    }
}

/// HTTP responses served in script order against the leg endpoints. A
/// call past the script panics naming the leg that ran out; `contacted`
/// records the leg short-name per request, in call order, so tests can
/// assert WHICH legs a request touched.
pub(crate) struct ScriptedTransport {
    steps: Mutex<VecDeque<Result<LegResponse, String>>>,
    contacted: Mutex<Vec<&'static str>>,
}

impl ScriptedTransport {
    pub(crate) fn new(steps: Vec<Result<LegResponse, String>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            contacted: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn contacted(&self) -> Vec<&'static str> {
        self.contacted.lock().unwrap().clone()
    }
}

#[async_trait]
impl LegTransport for ScriptedTransport {
    async fn get(
        &self,
        url: &str,
        _headers: &[(&str, &str)],
        _query: &[(&str, &str)],
    ) -> Result<LegResponse, String> {
        let leg = if url.contains("translate_a/single") {
            "gtx"
        } else if url.contains("translate_a/t") {
            "chrome-ex"
        } else if url.contains("dictionaryExtensionData") {
            "dictext"
        } else {
            panic!("unexpected leg url: {url}");
        };
        self.contacted.lock().unwrap().push(leg);
        match self.steps.lock().unwrap().pop_front() {
            Some(step) => step,
            None => {
                let log = self.contacted.lock().unwrap().clone();
                panic!("script exhausted — unplanned request to {leg}; contacted so far: {log:?}")
            }
        }
    }
}

/// A healthy gtx answer (the recorded golden fixture).
pub(crate) fn ok_gtx() -> Result<LegResponse, String> {
    Ok(LegResponse {
        status: 200,
        body: include_str!("../fixtures/google/en_single_segment.json").into(),
    })
}

/// A healthy dictionary-extension answer (the recorded golden fixture).
pub(crate) fn ok_dictext() -> Result<LegResponse, String> {
    Ok(LegResponse {
        status: 200,
        body: include_str!("../fixtures/google/dictionaryextension.json").into(),
    })
}

/// A healthy chrome-ex answer (the recorded golden fixture).
pub(crate) fn ok_chrome_ex() -> Result<LegResponse, String> {
    Ok(LegResponse {
        status: 200,
        body: include_str!("../fixtures/google/chrome_ex.json").into(),
    })
}

/// Any non-success HTTP status — the body never parses.
pub(crate) fn status(code: u16) -> Result<LegResponse, String> {
    Ok(LegResponse {
        status: code,
        body: "blocked".into(),
    })
}

/// The rate-limit shape.
pub(crate) fn rate_limited() -> Result<LegResponse, String> {
    status(429)
}

/// The outage shape.
pub(crate) fn server_error() -> Result<LegResponse, String> {
    status(500)
}

/// The dictionary extension's documented healthy-but-empty shape.
pub(crate) fn no_translation_body() -> Result<LegResponse, String> {
    Ok(LegResponse {
        status: 200,
        body: "{\"status\": 404}".into(),
    })
}
