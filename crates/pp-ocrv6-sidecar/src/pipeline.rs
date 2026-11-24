//! The OCR pipeline adapter: builds the oar-ocr det+rec pipeline from the
//! models directory and runs single images through it. This is the only
//! module that touches the pipeline crate's runtime models; the
//! region→reply mapping is pure and unit-tested below (no process, no
//! model files), while building/running a pipeline needs real models and
//! is exercised by launching the binary by hand (ticket 02's demo) —
//! deliberately not by CI.

use std::path::Path;

use oar_ocr::domain::tasks::TextDetectionConfig;
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::processors::Point;
use oar_ocr::utils::image::load_image_from_memory;

use crate::model_selection::{DetectionThresholds, ModelSelection};
use crate::protocol::ReplyRegion;

/// Builds the det+rec pipeline for one (Source Language, Model Tier)
/// launch. The three files come from the models directory by the launch
/// contract's fixed names; the tier's tuned detection thresholds arrive
/// through [`DetectionThresholds`] (the pipeline crate's preset constructor
/// only resolves registry names, not a directory).
pub fn build_pipeline(
    models_dir: &Path,
    selection: &ModelSelection,
    thresholds: DetectionThresholds,
) -> Result<OAROCR, String> {
    let detection = models_dir.join(selection.detection);
    let recognition = models_dir.join(selection.recognition);
    let dictionary = models_dir.join(selection.dictionary);
    for file in [&detection, &recognition, &dictionary] {
        if !file.is_file() {
            return Err(format!("model file missing: {}", file.display()));
        }
    }
    OAROCRBuilder::new(detection, recognition, dictionary)
        .text_detection_config(TextDetectionConfig {
            score_threshold: thresholds.score,
            box_threshold: thresholds.box_score,
            unclip_ratio: thresholds.unclip,
            max_candidates: thresholds.max_candidates,
            ..TextDetectionConfig::default()
        })
        .build()
        .map_err(|e| format!("failed to load the OCR pipeline: {e}"))
}

/// Runs one task image (PNG/JPEG bytes) through the pipeline and returns
/// its recognized regions in reply shape. Regions whose recognition
/// produced no text are skipped — an empty result surfaces as the
/// code-101 empty reply.
pub fn run_once(pipeline: &OAROCR, image: &[u8]) -> Result<Vec<ReplyRegion>, String> {
    let image = load_image_from_memory(image)
        .map_err(|e| format!("failed to decode the task image: {e}"))?;
    let results = pipeline
        .predict(vec![image])
        .map_err(|e| format!("OCR failed: {e}"))?;
    let regions = results
        .into_iter()
        .next()
        .map(|page| page.text_regions)
        .unwrap_or_default();
    Ok(regions
        .iter()
        .filter_map(|region| {
            region_to_reply(
                &region.bounding_box.points,
                region.text.as_deref(),
                region.confidence,
            )
        })
        .collect())
}

/// Maps one pipeline text region into reply shape — pure logic: no text
/// (recognition failed or was confidence-filtered) and blank text are
/// skipped, an empty polygon is skipped, and polygon points are rounded
/// to whole capture-space pixels (the pipeline returns source-pixel
/// coordinates; the client re-hulls them anyway).
fn region_to_reply(
    points: &[Point],
    text: Option<&str>,
    confidence: Option<f32>,
) -> Option<ReplyRegion> {
    let text = text?.trim();
    if text.is_empty() || points.is_empty() {
        return None;
    }
    Some(ReplyRegion {
        box_points: points
            .iter()
            .map(|p| [p.x.round() as i32, p.y.round() as i32])
            .collect(),
        score: confidence,
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_maps_to_a_rounded_reply_block() {
        let reply = region_to_reply(
            &[
                Point::new(13.2, 5.0),
                Point::new(160.6, 5.0),
                Point::new(161.0, 26.7),
                Point::new(13.0, 27.0),
            ],
            Some("飞舞的因果交流"),
            Some(0.954),
        )
        .unwrap();
        assert_eq!(reply.box_points, vec![[13, 5], [161, 5], [161, 27], [13, 27]]);
        assert_eq!(reply.text, "飞舞的因果交流");
        assert_eq!(reply.score, Some(0.954));
    }

    #[test]
    fn regions_without_usable_text_are_skipped() {
        // Recognition failed (None) or produced only whitespace — both
        // must vanish so the empty result becomes the code-101 reply.
        assert!(region_to_reply(&[Point::new(0.0, 0.0)], None, Some(0.9)).is_none());
        assert!(region_to_reply(&[Point::new(0.0, 0.0)], Some("   "), Some(0.9)).is_none());
    }

    #[test]
    fn an_empty_polygon_is_skipped() {
        assert!(region_to_reply(&[], Some("text"), None).is_none());
    }

    #[test]
    fn a_missing_confidence_still_replies_with_the_text() {
        // The score field only rides along when the pipeline supplies it.
        let reply = region_to_reply(&[Point::new(1.0, 2.0)], Some("t"), None).unwrap();
        assert_eq!(reply.score, None);
        assert_eq!(reply.text, "t");
    }
}
