//! Auto Colors (ticket 14): sample a Text Block's text and pill colors from
//! the captured frame, so a translation dresses like the scene it covers —
//! white source text stays white, a greenish backdrop tints the pill. Pure
//! pixel math over [`CapturedImage`]; the orchestrator calls it per fired
//! block and the shell relays the result to the overlay page.
//!
//! Per block, in three steps:
//!
//! 1. **Interior histogram.** The block's pixels are bucketed on their
//!    4-bit-per-channel color. The most common bucket is one candidate
//!    (usually the block's background), the most common bucket a clear
//!    distance away is the other (usually the text).
//! 2. **Text vs background, disambiguated by the ring.** Dense text can be
//!    the block's MAJORITY color, so "mode = background" is not reliable on
//!    its own. The mode of a ring around the block — the scene continuing
//!    behind the text — catches the inversion: when the distinct color is
//!    near the ring (it plausibly IS the scene) while the mode is far from
//!    it, the two roles swap. Ambiguous cases keep the default reading.
//! 3. **Contrast floor.** The pill takes the ring's color; if the text
//!    then fails a WCAG 4.5:1 contrast check against it, the text is mixed
//!    toward black or white — whichever extreme buys more contrast — just
//!    far enough to pass. A sample that never clears the floor (or never
//!    finds two distinct interior colors at all) yields `None` and the
//!    overlay falls back to the manual Text Style.
//!
//! Runs once per *changed* block (the change-detection gate), on OCR-line-
//! sized rects, so the histogram loops cost well under a millisecond in
//! release builds.

use crate::ports::{CapturedImage, Region};
use serde::Serialize;
use std::collections::HashMap;

/// An 8-bit sRGB color, the unit the sampler works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    fn dist_sq(self, other: Rgb) -> i64 {
        let (dr, dg, db) = (
            i32::from(self.r) - i32::from(other.r),
            i32::from(self.g) - i32::from(other.g),
            i32::from(self.b) - i32::from(other.b),
        );
        i64::from(dr * dr + dg * dg + db * db)
    }

    fn lerp(self, to: Rgb, t: f64) -> Rgb {
        let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
        Rgb {
            r: mix(self.r, to.r),
            g: mix(self.g, to.g),
            b: mix(self.b, to.b),
        }
    }
}

/// `#rrggbb`, lowercase — the form the overlay's inline CSS takes.
pub fn hex(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// The sampled per-block colors, as the orchestrator's `Translated` event
/// and the shell's `overlay-update` payload carry them (camelCase JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampledStyle {
    pub text_color: String,
    pub pill_color: String,
}

impl SampledStyle {
    pub fn from_block(style: &BlockStyle) -> Self {
        Self {
            text_color: hex(style.text),
            pill_color: hex(style.pill),
        }
    }
}

/// The colors sampled for one Text Block: `text` is the dominant
/// high-contrast interior color, `pill` the dominant color around the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStyle {
    pub text: Rgb,
    pub pill: Rgb,
}

/// Histogram bucketing: 4 bits per channel (16 levels) — coarse enough that
/// antialiasing noise collapses into the dominant buckets, fine enough to
/// keep real hues apart.
const QUANT_SHIFT: u32 = 4;

/// Squared RGB distance an interior color needs from the mode before it
/// counts as a distinct cluster (candidate text, not antialiasing noise).
const CLUSTER_DIST_SQ: i64 = 80 * 80;

/// The sampled text must reach this WCAG contrast ratio against the pill.
const CONTRAST_FLOOR: f64 = 4.5;

/// A distinct cluster must hold at least this share of the block's pixels
/// to be the text — stray icons and single-pixel OCR noise stay out —
/// with a small absolute floor for tiny boxes.
const MIN_TEXT_SHARE: f64 = 0.03;
const MIN_TEXT_PIXELS: u32 = 4;

/// The ring needs this many pixels before its mode is trusted; smaller
/// rings (a block flush against the capture edge) fall back to the block's
/// own background color for the pill.
const MIN_RING_PIXELS: u32 = 16;

/// One histogram bucket: pixel count plus per-channel sums, so the bucket's
/// representative color is the average of its raw pixels rather than the
/// quantized grid point.
#[derive(Default)]
struct Bucket {
    count: u32,
    r: u64,
    g: u64,
    b: u64,
}

impl Bucket {
    fn add(&mut self, r: u8, g: u8, b: u8) {
        self.count += 1;
        self.r += u64::from(r);
        self.g += u64::from(g);
        self.b += u64::from(b);
    }

    fn color(&self) -> Rgb {
        Rgb {
            r: (self.r / u64::from(self.count)) as u8,
            g: (self.g / u64::from(self.count)) as u8,
            b: (self.b / u64::from(self.count)) as u8,
        }
    }
}

type Histogram = HashMap<u16, Bucket>;

fn bucket_key(r: u8, g: u8, b: u8) -> u16 {
    ((u16::from(r >> QUANT_SHIFT)) << 8) | ((u16::from(g >> QUANT_SHIFT)) << 4) | u16::from(b >> QUANT_SHIFT)
}

/// Clamps `rect` into the image; `None` when nothing of it remains.
fn clamp_rect(rect: Region, width: u32, height: u32) -> Option<(i32, i32, i32, i32)> {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = (rect.x + rect.width as i32).min(width as i32);
    let bottom = (rect.y + rect.height as i32).min(height as i32);
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

/// Reads the (r, g, b) of one pixel — the capture is BGRA, top-down.
fn pixel(image: &CapturedImage, x: i32, y: i32) -> (u8, u8, u8) {
    let index = ((y * image.width as i32 + x) * 4) as usize;
    let bgra = &image.bgra;
    (bgra[index + 2], bgra[index + 1], bgra[index])
}

fn histogram_for(
    image: &CapturedImage,
    (x, y, w, h): (i32, i32, i32, i32),
    skip: Option<(i32, i32, i32, i32)>,
) -> Histogram {
    let mut hist = Histogram::new();
    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if let Some((sx, sy, sw, sh)) = skip {
                if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
                    continue;
                }
            }
            let (r, g, b) = pixel(image, px, py);
            hist.entry(bucket_key(r, g, b)).or_default().add(r, g, b);
        }
    }
    hist
}

/// The bucket holding the most pixels.
fn mode(hist: &Histogram) -> Option<&Bucket> {
    hist.values().max_by_key(|b| b.count)
}

/// The bucket holding the most pixels among colors a clear distance from
/// `away_from` — the text candidate inside a block whose mode is the
/// background (or vice versa).
fn dominant_distinct(hist: &Histogram, away_from: Rgb, total: u32) -> Option<&Bucket> {
    let min_count = ((total as f64 * MIN_TEXT_SHARE) as u32).max(MIN_TEXT_PIXELS);
    hist.values()
        .filter(|b| b.color().dist_sq(away_from) >= CLUSTER_DIST_SQ)
        .filter(|b| b.count >= min_count)
        .max_by_key(|b| b.count)
}

/// Samples the text and pill colors for one Text Block (region-relative
/// rect) from `image`. `None` = sampling failed (degenerate rect, no two
/// distinct interior colors) — the caller falls back to the manual style.
pub fn sample_block_style(image: &CapturedImage, rect: Region) -> Option<BlockStyle> {
    let inner = clamp_rect(rect, image.width, image.height)?;
    let inner_area = inner.2 as u32 * inner.3 as u32;

    // 1. Interior candidates: the mode (usually the block's backdrop) and
    //    the most common clearly-distinct color (usually the text).
    let inner_hist = histogram_for(image, inner, None);
    let mode_color = mode(&inner_hist)?.color();
    let distinct = dominant_distinct(&inner_hist, mode_color, inner_area)?.color();

    // 2. The ring around the block (the scene continuing behind it) tells
    //    the two apart (module docs): when the distinct color plausibly IS
    //    the scene — near the ring's color — while the mode clearly is not,
    //    dense text has inverted the default reading and the roles swap.
    //    Both-near or both-far keeps the default: a text color that merely
    //    resembles its scene stays a benign miss (the result is still
    //    contrast-checked below).
    let margin = (inner.3 / 2).clamp(4, 24);
    let outer = clamp_rect(
        Region {
            x: inner.0 - margin,
            y: inner.1 - margin,
            width: (inner.2 + 2 * margin) as u32,
            height: (inner.3 + 2 * margin) as u32,
        },
        image.width,
        image.height,
    )?;
    let ring_hist = histogram_for(image, outer, Some(inner));
    let ring_area: u32 = ring_hist.values().map(|b| b.count).sum();
    // A block flush against the capture edge leaves no visible scene: with
    // no ring there is nothing to disambiguate with and nothing to dress
    // the pill in, so both default to the interior reading.
    let scene = if ring_area >= MIN_RING_PIXELS {
        Some(mode(&ring_hist)?.color())
    } else {
        None
    };
    let (mut text, pill) = match scene {
        Some(ring) => {
            let (text, pill) = if distinct.dist_sq(ring) <= CLUSTER_DIST_SQ
                && mode_color.dist_sq(ring) > CLUSTER_DIST_SQ
            {
                (mode_color, ring)
            } else {
                (distinct, ring)
            };
            (text, pill)
        }
        None => (distinct, mode_color),
    };

    // 3. Contrast floor: the pill can be far lighter/darker than the
    //    block's own backdrop (the pill is the scene, not the box).
    text = enforce_contrast(text, pill);
    Some(BlockStyle { text, pill })
}

/// WCAG 2.x relative luminance.
fn luminance(color: Rgb) -> f64 {
    fn channel(v: u8) -> f64 {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast ratio, 1.0 (indistinguishable) to 21.0 (black on white).
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (lo, hi) = match luminance(a).total_cmp(&luminance(b)) {
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => (luminance(a), luminance(b)),
        std::cmp::Ordering::Greater => (luminance(b), luminance(a)),
    };
    (hi + 0.05) / (lo + 0.05)
}

/// Nudges `text` toward black or white — whichever extreme buys more
/// contrast against `pill` — until the pair clears [`CONTRAST_FLOOR`].
/// The direction is chosen by endpoint contrast, then the smallest mix that
/// passes wins, keeping the sampled color as close to the scene as the
/// floor allows.
fn enforce_contrast(text: Rgb, pill: Rgb) -> Rgb {
    if contrast(text, pill) >= CONTRAST_FLOOR {
        return text;
    }
    let to_black = contrast(Rgb::BLACK, pill);
    let to_white = contrast(Rgb::WHITE, pill);
    let end = if to_black >= to_white {
        Rgb::BLACK
    } else {
        Rgb::WHITE
    };
    const STEPS: f64 = 20.0;
    for step in 1..=20 {
        let mixed = text.lerp(end, f64::from(step) / STEPS);
        if contrast(mixed, pill) >= CONTRAST_FLOOR {
            return mixed;
        }
    }
    end // unreachable for real pills, but never return a failing color
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 200;
    const H: u32 = 120;

    fn solid(color: Rgb) -> CapturedImage {
        let mut bgra = Vec::with_capacity((W * H * 4) as usize);
        for _ in 0..W * H {
            bgra.extend_from_slice(&[color.b, color.g, color.r, 255]);
        }
        CapturedImage {
            width: W,
            height: H,
            bgra,
        }
    }

    fn paint_rect(image: &mut CapturedImage, x: i32, y: i32, w: u32, h: u32, color: Rgb) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let index = (((y + dy) * W as i32 + (x + dx)) * 4) as usize;
                image.bgra[index] = color.b;
                image.bgra[index + 1] = color.g;
                image.bgra[index + 2] = color.r;
            }
        }
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Region {
        Region {
            x,
            y,
            width: w,
            height: h,
        }
    }

    /// White horizontal text bars on a dark-blue block, scene dark blue
    /// everywhere else — the canonical "white text stays white" fixture.
    fn white_on_navy() -> CapturedImage {
        let mut image = solid(Rgb {
            r: 10,
            g: 20,
            b: 60,
        });
        let (bx, by, bw, _bh) = (40, 40, 100, 30);
        for row in [0, 8, 16] {
            paint_rect(
                &mut image,
                bx + 4,
                by + row,
                bw - 8,
                4,
                Rgb::WHITE,
            );
        }
        image
    }

    fn close_enough(a: Rgb, b: Rgb) -> bool {
        a.dist_sq(b) < 15 * 15
    }

    #[test]
    fn white_text_stays_white_on_its_dark_backdrop() {
        let style = sample_block_style(&white_on_navy(), rect(40, 40, 100, 30)).unwrap();
        assert!(
            close_enough(style.text, Rgb::WHITE),
            "sampled text {:?} should be white",
            style.text
        );
        assert_eq!(
            style.pill,
            Rgb {
                r: 10,
                g: 20,
                b: 60
            },
            "pill takes the scene around the block"
        );
    }

    #[test]
    fn dark_text_on_a_light_scene_stays_dark() {
        let scene = Rgb {
            r: 230,
            g: 230,
            b: 160,
        };
        let mut image = solid(scene);
        // Dark text bars directly on the scene (no box in between).
        for row in [44, 52] {
            paint_rect(&mut image, 50, row, 80, 4, Rgb {
                r: 40,
                g: 40,
                b: 40,
            });
        }
        let style = sample_block_style(&image, rect(50, 40, 80, 24)).unwrap();
        assert!(
            close_enough(style.text, Rgb {
                r: 40,
                g: 40,
                b: 40
            }),
            "sampled text {:?} should be the dark bars",
            style.text
        );
        assert!(
            close_enough(style.pill, scene),
            "sampled pill {:?} should be the light scene",
            style.pill
        );
    }

    #[test]
    fn dense_text_majority_still_reads_as_text() {
        // The box is mostly text (white bars covering ~3/4) over a dark
        // backdrop that matches the scene — the ring must swap the guess.
        let scene = Rgb {
            r: 20,
            g: 20,
            b: 30,
        };
        let mut image = solid(scene);
        paint_rect(&mut image, 40, 40, 100, 30, scene); // box backdrop
        for row in [0, 6, 12, 18, 24] {
            paint_rect(&mut image, 44, 40 + row, 92, 5, Rgb::WHITE);
        }
        let style = sample_block_style(&image, rect(40, 40, 100, 30)).unwrap();
        assert!(
            close_enough(style.text, Rgb::WHITE),
            "sampled text {:?} should be the (majority) white text",
            style.text
        );
    }

    #[test]
    fn monochrome_block_fails_sampling() {
        let image = solid(Rgb {
            r: 40,
            g: 40,
            b: 40,
        });
        assert_eq!(sample_block_style(&image, rect(20, 20, 80, 24)), None);
    }

    #[test]
    fn rect_outside_the_image_fails_sampling() {
        let image = white_on_navy();
        assert_eq!(sample_block_style(&image, rect(500, 500, 100, 20)), None);
    }

    #[test]
    fn rect_overhanging_the_edge_is_clamped() {
        // A block poking past the right/bottom edges: the in-image part
        // still samples white-on-navy; the ring beyond the image is simply
        // not there.
        let mut image = solid(Rgb {
            r: 10,
            g: 20,
            b: 60,
        });
        paint_rect(&mut image, 155, 105, 40, 4, Rgb::WHITE);
        paint_rect(&mut image, 155, 112, 40, 4, Rgb::WHITE);
        let style = sample_block_style(&image, rect(150, 100, 100, 30)).unwrap();
        assert!(close_enough(style.text, Rgb::WHITE), "text {:?}", style.text);
    }

    #[test]
    fn block_filling_the_capture_pills_with_its_own_backdrop() {
        // No ring exists; the pill falls back to the block's background
        // color (the navy between the text bars).
        let navy = Rgb {
            r: 10,
            g: 20,
            b: 60,
        };
        let mut image = solid(navy);
        paint_rect(&mut image, 4, 10, 192, 4, Rgb::WHITE);
        let style = sample_block_style(&image, rect(0, 0, W, H)).unwrap();
        assert_eq!(style.pill, navy);
        assert!(close_enough(style.text, Rgb::WHITE));
    }

    #[test]
    fn low_contrast_text_is_darkened_to_the_floor() {
        // White text inside a dark box, sitting on a light-gray scene:
        // the pill comes out light gray, so the sampled white would fail
        // against it and must be pushed toward black until readable.
        let scene = Rgb {
            r: 150,
            g: 150,
            b: 150,
        };
        let mut image = solid(scene);
        paint_rect(&mut image, 40, 40, 100, 30, Rgb {
            r: 30,
            g: 30,
            b: 30,
        });
        paint_rect(&mut image, 44, 44, 92, 4, Rgb::WHITE);
        let style = sample_block_style(&image, rect(40, 40, 100, 30)).unwrap();
        assert!(
            contrast(style.text, style.pill) >= CONTRAST_FLOOR,
            "adjusted text {:?} vs pill {:?} must clear the floor",
            style.text,
            style.pill
        );
        assert!(
            luminance(style.text) < luminance(Rgb::WHITE),
            "the adjustment should have darkened the white text"
        );
    }

    #[test]
    fn dark_text_in_a_dark_scene_comes_out_readable() {
        // Dark text bars on a light box, surrounded by a dark scene: the
        // dark ring color means the sampled dark text can never clear the
        // floor — the output must be readable against the dark pill
        // regardless of which interior role each color lands in.
        let scene = Rgb {
            r: 30,
            g: 30,
            b: 30,
        };
        let mut image = solid(scene);
        paint_rect(&mut image, 40, 40, 100, 30, Rgb {
            r: 200,
            g: 200,
            b: 200,
        });
        paint_rect(&mut image, 44, 44, 92, 4, Rgb {
            r: 25,
            g: 25,
            b: 25,
        });
        let style = sample_block_style(&image, rect(40, 40, 100, 30)).unwrap();
        assert!(
            contrast(style.text, style.pill) >= CONTRAST_FLOOR,
            "adjusted text {:?} vs pill {:?} must clear the floor",
            style.text,
            style.pill
        );
        assert!(
            luminance(style.text) > luminance(Rgb {
                r: 25,
                g: 25,
                b: 25
            }),
            "readable output against a dark pill must be lighter than the dark text",
        );
    }

    #[test]
    fn readable_sample_is_not_touched() {
        // White bars on black clear the floor many times over — the text
        // must survive byte-exact.
        let mut image = solid(Rgb::BLACK);
        for row in [44, 52, 60] {
            paint_rect(&mut image, 44, row, 92, 4, Rgb::WHITE);
        }
        let style = sample_block_style(&image, rect(40, 40, 100, 30)).unwrap();
        assert_eq!(style.text, Rgb::WHITE);
        assert_eq!(style.pill, Rgb::BLACK);
    }

    #[test]
    fn hex_is_lowercase_rrggbb() {
        assert_eq!(hex(Rgb::WHITE), "#ffffff");
        assert_eq!(
            hex(Rgb {
                r: 10,
                g: 160,
                b: 7
            }),
            "#0aa007"
        );
    }

    #[test]
    fn tiny_block_with_two_colors_still_samples() {
        // A 16×6 px box: 24 white text px among 96 — above the absolute
        // floor (4 px), well over the 3% share. Sampling must not bail.
        let mut image = solid(Rgb {
            r: 10,
            g: 20,
            b: 60,
        });
        paint_rect(&mut image, 62, 60, 12, 2, Rgb::WHITE);
        let style = sample_block_style(&image, rect(60, 58, 16, 6)).unwrap();
        assert!(close_enough(style.text, Rgb::WHITE));
    }

    #[test]
    fn sampled_style_serializes_as_camel_case_hex() {
        let style = SampledStyle::from_block(&BlockStyle {
            text: Rgb::WHITE,
            pill: Rgb::BLACK,
        });
        assert_eq!(style.text_color, "#ffffff");
        assert_eq!(style.pill_color, "#000000");
    }
}
