//! The OCR engine fallback (ticket 31): when the configured engine — in
//! practice the bundled PaddleOCR sidecar — cannot start or dies mid-pass,
//! the pass is covered by Windows OCR instead, with a visible notice about
//! which engine actually ran. The choice itself never silently swaps: the
//! primary engine is retried every pass (a sidecar crash recovers by
//! restart), so the notice marks a transition, not a permanent switch.

use langception_core::ports::{CapturedImage, OcrEngine, OcrError, SourceLanguage, TextBlock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Called on a healthy→fallback transition. The shell's implementation
/// lands the notice in the Results Window's warnings and the app log.
pub(crate) type Notify = Arc<dyn Fn(String) + Send + Sync>;

pub(crate) struct FallbackOcrEngine {
    primary: Arc<dyn OcrEngine>,
    primary_name: &'static str,
    fallback: Arc<dyn OcrEngine>,
    fallback_name: &'static str,
    notify: Notify,
    /// Sticky until the primary succeeds again: one notice per incident,
    /// never one per failing pass.
    notified: AtomicBool,
}

impl FallbackOcrEngine {
    pub(crate) fn new(
        primary: Arc<dyn OcrEngine>,
        primary_name: &'static str,
        fallback: Arc<dyn OcrEngine>,
        fallback_name: &'static str,
        notify: Notify,
    ) -> Self {
        Self {
            primary,
            primary_name,
            fallback,
            fallback_name,
            notify,
            notified: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl OcrEngine for FallbackOcrEngine {
    async fn recognize(
        &self,
        image: &CapturedImage,
        source: SourceLanguage,
    ) -> Result<Vec<TextBlock>, OcrError> {
        match self.primary.recognize(image, source).await {
            Ok(blocks) => {
                // The primary is back (if it was away): the next failure
                // announces itself again instead of hiding behind the old
                // notice.
                self.notified.store(false, Ordering::Relaxed);
                Ok(blocks)
            }
            Err(primary_error) => {
                let first_failure = !self.notified.swap(true, Ordering::Relaxed);
                if first_failure {
                    (self.notify)(format!(
                        "{primary} could not run ({primary_error}) — this pass used {fallback} instead; \
                         {primary} is retried every pass.",
                        primary = self.primary_name,
                        fallback = self.fallback_name,
                    ));
                }
                self.fallback.recognize(image, source).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct Scripted {
        results: StdMutex<Vec<Result<Vec<TextBlock>, OcrError>>>,
    }

    // Pop answers in the scripted order, then repeat the last one.
    #[async_trait::async_trait]
    impl OcrEngine for Scripted {
        async fn recognize(
            &self,
            _image: &CapturedImage,
            _source: SourceLanguage,
        ) -> Result<Vec<TextBlock>, OcrError> {
            let mut results = self.results.lock().unwrap();
            if results.len() == 1 {
                return results[0].clone();
            }
            results.remove(0)
        }
    }

    fn block(text: &str) -> TextBlock {
        TextBlock {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            text: text.to_string(),
        }
    }

    fn engine(
        primary_results: Vec<Result<Vec<TextBlock>, OcrError>>,
        fallback_results: Vec<Result<Vec<TextBlock>, OcrError>>,
        notices: Arc<StdMutex<Vec<String>>>,
    ) -> FallbackOcrEngine {
        FallbackOcrEngine::new(
            Arc::new(Scripted {
                results: StdMutex::new(primary_results),
            }),
            "PaddleOCR",
            Arc::new(Scripted {
                results: StdMutex::new(fallback_results),
            }),
            "Windows OCR",
            Arc::new(move |message| notices.lock().unwrap().push(message)),
        )
    }

    fn image() -> CapturedImage {
        CapturedImage {
            width: 8,
            height: 8,
            bgra: vec![0; 8 * 8 * 4],
        }
    }

    fn ocr_error(part: &str) -> OcrError {
        OcrError::new(format!("primary broke: {part}"))
    }

    #[tokio::test]
    async fn a_primary_failure_covers_the_pass_and_notices_once() {
        let notices = Arc::new(StdMutex::new(Vec::new()));
        let engine = engine(
            vec![Err(ocr_error("missing")), Err(ocr_error("missing"))],
            vec![Ok(vec![block("windows text")])],
            notices.clone(),
        );

        let first = engine.recognize(&image(), SourceLanguage::En).await.unwrap();
        let second = engine.recognize(&image(), SourceLanguage::En).await.unwrap();

        assert_eq!(first[0].text, "windows text");
        assert_eq!(second[0].text, "windows text");
        let notices = notices.lock().unwrap();
        assert_eq!(
            notices.len(),
            1,
            "one notice per incident, not per failing pass: {notices:?}"
        );
        assert!(notices[0].contains("PaddleOCR"));
        assert!(notices[0].contains("Windows OCR"));
        assert!(notices[0].contains("missing"), "the cause is included");
    }

    #[tokio::test]
    async fn a_recovered_primary_resets_the_notice_state() {
        let notices = Arc::new(StdMutex::new(Vec::new()));
        let engine = engine(
            vec![
                Err(ocr_error("crashed")),
                Ok(vec![block("paddle text")]),
                Err(ocr_error("crashed again")),
            ],
            vec![Ok(vec![block("windows text")])],
            notices.clone(),
        );

        assert_eq!(engine.recognize(&image(), SourceLanguage::En).await.unwrap()[0].text, "windows text");
        assert_eq!(engine.recognize(&image(), SourceLanguage::En).await.unwrap()[0].text, "paddle text");
        assert_eq!(engine.recognize(&image(), SourceLanguage::En).await.unwrap()[0].text, "windows text");

        let notices = notices.lock().unwrap();
        assert_eq!(notices.len(), 2, "each incident gets its own notice: {notices:?}");
        assert!(notices[1].contains("crashed again"));
    }

    #[tokio::test]
    async fn a_failing_fallback_surfaces_its_own_error() {
        let notices = Arc::new(StdMutex::new(Vec::new()));
        let engine = engine(
            vec![Err(ocr_error("missing"))],
            vec![Err(OcrError::new("windows broke too"))],
            notices,
        );
        let error = engine.recognize(&image(), SourceLanguage::En).await.unwrap_err();
        assert!(error.message.contains("windows broke too"));
    }
}
