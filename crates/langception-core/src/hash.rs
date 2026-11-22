//! Change-detection hashes for the Auto-translate loop (ticket 05): a
//! lossless frame checksum that decides whether OCR runs at all, and the
//! per-Text-Block dHash that replaces text-hash dedup.
//!
//! The two have different jobs, hence different functions. The frame
//! checksum must never miss a change (a missed change is a silently stale
//! translation), so it reads every byte. The block dHash is dedup: it must
//! ignore a pixel or two of OCR box jitter but flip wholesale when the text
//! changes, which is exactly what a downscaled difference hash does.

use crate::ports::{CapturedImage, Region};

/// Lossless checksum of a captured frame: every changed pixel flips it, so
/// an unchanged checksum between two captures proves the whole region is
/// pixel-identical and OCR can be skipped entirely (the fast path that
/// makes an unchanged screen cost capture + hashing only).
///
/// Mixed 8 bytes at a time over the raw BGRA buffer. The alpha byte is
/// included: GDI output is deterministic per session, and a driver that
/// varied it between captures would only lose the fast path (OCR would run
/// every tick, as before ticket 05) — never correctness.
pub(crate) fn frame_checksum(image: &CapturedImage) -> u64 {
    // FNV-style mix, folded 8 bytes per step to keep a 4K frame (32 MB)
    // comfortably inside the per-tick budget.
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let (chunks, remainder) = image.bgra.as_chunks::<8>();
    for word in chunks {
        hash = (hash ^ u64::from_le_bytes(*word)).wrapping_mul(PRIME);
    }
    for &byte in remainder {
        hash = (hash ^ byte as u64).wrapping_mul(PRIME);
    }
    hash
}

/// dHash (difference hash) of a rectangular area of a captured frame:
/// grayscale, box-downscaled to 9×8, each of the 64 bits set when a cell
/// is brighter than its right neighbor.
///
/// Downscaling makes the hash stable under a pixel or two of OCR box
/// jitter; the horizontal gradient still flips wholesale when the text
/// changes. The rect is clamped to the image (an OCR box at the region
/// edge may poke past it); a rect fully outside hashes to 0. Cells smaller
/// than one source pixel (tiny blocks) average over an empty range to 0 —
/// still perfectly consistent between passes, which is all dedup needs.
///
/// Callers quantize the rect to the block grid first, so box jitter inside
/// one grid cell hashes the same pixels.
pub(crate) fn block_dhash(image: &CapturedImage, rect: Region) -> u64 {
    const COLS: usize = 9;
    const ROWS: usize = 8;

    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = (rect.x + rect.width as i32).min(image.width as i32);
    let bottom = (rect.y + rect.height as i32).min(image.height as i32);
    if right <= left || bottom <= top {
        return 0;
    }
    let rect_w = (right - left) as u64;
    let rect_h = (bottom - top) as u64;

    // Grayscale cell averages (BT.601 luma from BGRA, ignoring alpha).
    let mut gray = [[0u64; COLS]; ROWS];
    for (row, cells) in gray.iter_mut().enumerate() {
        let sy0 = top + ((row as u64 * rect_h / ROWS as u64) as i32);
        let sy1 = top + (((row as u64 + 1) * rect_h / ROWS as u64) as i32);
        for (col, cell) in cells.iter_mut().enumerate() {
            let sx0 = left + ((col as u64 * rect_w / COLS as u64) as i32);
            let sx1 = left + (((col as u64 + 1) * rect_w / COLS as u64) as i32);
            let mut sum = 0u64;
            let mut count = 0u64;
            for sy in sy0..sy1 {
                let row_base = (sy as u32 as usize) * image.width as usize;
                for sx in sx0..sx1 {
                    let i = (row_base + sx as usize) * 4;
                    let (b, g, r) = (
                        image.bgra[i] as u64,
                        image.bgra[i + 1] as u64,
                        image.bgra[i + 2] as u64,
                    );
                    sum += (r * 299 + g * 587 + b * 114) / 1000;
                    count += 1;
                }
            }
            *cell = sum.checked_div(count).unwrap_or(0);
        }
    }

    let mut hash = 0u64;
    for (row, cells) in gray.iter().enumerate() {
        for col in 0..COLS - 1 {
            if cells[col] > cells[col + 1] {
                hash |= 1 << (row * (COLS - 1) + col);
            }
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    fn image(width: u32, height: u32, paint: impl FnOnce(&mut [u8])) -> CapturedImage {
        let mut image = CapturedImage {
            width,
            height,
            bgra: vec![0; (width * height * 4) as usize],
        };
        paint(&mut image.bgra);
        image
    }

    /// Byte of pixel (x, y)'s blue channel.
    fn paint(bgra: &mut [u8], width: u32, x: u32, y: u32, value: u8) {
        bgra[((y * width + x) * 4) as usize] = value;
    }

    #[test]
    fn frame_checksum_detects_any_changed_byte() {
        let base = image(8, 8, |_| {});
        assert_eq!(frame_checksum(&base), frame_checksum(&image(8, 8, |_| {})));

        let changed = image(8, 8, |bgra| paint(bgra, 8, 7, 7, 1));
        assert_ne!(frame_checksum(&base), frame_checksum(&changed));
    }

    #[test]
    fn block_dhash_is_stable_for_identical_pixels() {
        let img = image(100, 100, |bgra| paint(bgra, 100, 30, 40, 255));
        assert_eq!(
            block_dhash(&img, rect(10, 20, 50, 30)),
            block_dhash(&img, rect(10, 20, 50, 30))
        );
    }

    #[test]
    fn block_dhash_flips_when_the_block_pixels_change() {
        let before = image(100, 100, |bgra| paint(bgra, 100, 30, 40, 255));
        let after = image(100, 100, |bgra| paint(bgra, 100, 30, 40, 0));
        assert_ne!(
            block_dhash(&before, rect(10, 20, 50, 30)),
            block_dhash(&after, rect(10, 20, 50, 30)),
            "a bright spot vanishing must flip gradient bits"
        );
    }

    #[test]
    fn block_dhash_ignores_changes_outside_the_rect() {
        let base = image(100, 100, |bgra| paint(bgra, 100, 30, 40, 255));
        // Same bright spot, plus a change far outside the block rect.
        let changed_elsewhere = image(100, 100, |bgra| {
            paint(bgra, 100, 30, 40, 255);
            paint(bgra, 100, 90, 90, 255);
        });
        assert_eq!(
            block_dhash(&base, rect(10, 20, 50, 30)),
            block_dhash(&changed_elsewhere, rect(10, 20, 50, 30)),
            "pixels outside the block rect must not affect its hash"
        );
    }

    #[test]
    fn block_dhash_is_stable_under_box_jitter_inside_the_rect() {
        // The same spot, read through rects shifted by a few pixels (the
        // quantization the orchestrator applies absorbs this; the hash
        // itself tolerates small shifts via downscaling).
        let img = image(100, 100, |bgra| paint(bgra, 100, 30, 40, 255));
        assert_eq!(
            block_dhash(&img, rect(10, 20, 50, 30)),
            block_dhash(&img, rect(12, 22, 50, 30))
        );
    }

    #[test]
    fn block_dhash_clamps_rects_poking_past_the_image() {
        let img = image(50, 50, |_| {});
        // Fully outside: deterministic 0, never a panic.
        assert_eq!(block_dhash(&img, rect(60, 60, 10, 10)), 0);
        // Partially outside: same result as the clamped-in rect.
        assert_eq!(
            block_dhash(&img, rect(40, 40, 20, 20)),
            block_dhash(&img, rect(40, 40, 10, 10))
        );
    }

    #[test]
    fn block_dhash_handles_tiny_rects() {
        let img = image(10, 10, |bgra| paint(bgra, 10, 3, 3, 255));
        // Smaller than the 9×8 cell grid: must not divide by zero or panic.
        let _ = block_dhash(&img, rect(0, 0, 3, 3));
        assert_eq!(block_dhash(&img, rect(0, 0, 1, 1)), 0);
    }

    #[test]
    fn block_dhash_distinguishes_left_and_right_bright_halves() {
        // Whole strips, not single pixels: box-averaging deliberately
        // dilutes a lone bright pixel into nothing (that is the jitter
        // tolerance), but a bright region must flip gradient bits.
        let left_lit = image(100, 100, |bgra| {
            for x in 10..20 {
                for y in 0..100 {
                    paint(bgra, 100, x, y, 255);
                }
            }
        });
        let right_lit = image(100, 100, |bgra| {
            for x in 80..90 {
                for y in 0..100 {
                    paint(bgra, 100, x, y, 255);
                }
            }
        });
        assert_ne!(
            block_dhash(&left_lit, rect(0, 0, 100, 100)),
            block_dhash(&right_lit, rect(0, 0, 100, 100))
        );
    }

    /// The ticket's per-tick budget: capture + hashing must fit ≤ 50 ms so
    /// the loop never falls behind its cadence. This measures the hashing
    /// half — the frame checksum every tick, plus one block hash over the
    /// whole frame (the most a single Text Block can ever ask for) — on a
    /// synthetic full-HD frame. The capture adapter's few milliseconds of
    /// GDI BitBlt are outside automated tests (testing decisions).
    #[test]
    fn hashing_a_full_hd_frame_stays_within_the_50ms_tick_budget() {
        let image = image(1920, 1080, |bgra| {
            // A non-uniform pattern so no gradient bit is trivially decided.
            for (i, byte) in bgra.iter_mut().enumerate() {
                *byte = (i % 251) as u8;
            }
        });
        let start = std::time::Instant::now();
        let _ = frame_checksum(&image);
        let _ = block_dhash(&image, rect(0, 0, 1920, 1080));
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "frame checksum + full-frame dHash took {elapsed:?}, budget is 50ms"
        );
    }
}
