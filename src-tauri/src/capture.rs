//! Windows screen capture via GDI `BitBlt` — a thin adapter outside automated
//! tests (per the spec's testing decisions); exercised by the manual smoke
//! checklist. The Capture Visibility mask (ticket 46) is the one processing
//! step: the provider's rects are filled out of every frame AFTER the blit
//! and BEFORE any consumer — frame checksum, dHash, OCR, Auto Colors, and
//! the fused engines' cloud PNG all see the masked pixels (ADR-0004). The
//! overlap geometry lives in core (`Region::intersection`, unit-tested
//! there); this file only fills.

use langception_core::ports::{CaptureError, CaptureSource, CapturedImage, Region};

/// The Capture Visibility mask provider (ticket 46): returns the screen
/// rects (physical px) of Langception's own windows to black out of every
/// captured frame. When Capture Visibility is off the provider returns
/// empty — the windows are already excluded from capture (ADR-0003), and
/// masking nothing is free defense in depth.
pub type MaskRectsFn = Box<dyn Fn() -> Vec<Region> + Send + Sync>;

pub struct GdiCaptureSource {
    mask_rects: MaskRectsFn,
}

impl GdiCaptureSource {
    /// A source without masking — today's behavior and the loop-safety
    /// baseline (windows excluded from capture, nothing to fill).
    pub fn new() -> Self {
        Self {
            mask_rects: Box::new(Vec::new),
        }
    }

    /// Capture Visibility (ticket 46, ADR-0004): with the setting on, the
    /// shell provides the Settings/Results windows' current screen rects,
    /// read PER CAPTURE so window moves and the live toggle apply from the
    /// next frame.
    pub fn with_mask(mask_rects: MaskRectsFn) -> Self {
        Self { mask_rects }
    }
}

impl Default for GdiCaptureSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CaptureSource for GdiCaptureSource {
    /// GDI `BitBlt` of the configured region takes a few milliseconds, well
    /// under the slice-1 cadence, so it runs inline rather than on the
    /// blocking pool.
    async fn capture(&self, region: Region) -> Result<CapturedImage, CaptureError> {
        let mut image = capture_region_blocking(region)?;
        mask_image(&mut image, region, &(self.mask_rects)());
        Ok(image)
    }
}

/// Fills the part of each `rect` (screen physical px) that overlaps
/// `region` with opaque black, in place in the top-down BGRA buffer. A
/// rect beside the captured region fills nothing (touching edges are not
/// an intersection). This is the loop's guarantee that its own windows'
/// pixels never reach OCR, Auto Colors sampling, or a cloud engine's
/// upload — in either Capture Visibility mode (ticket 46).
fn mask_image(image: &mut CapturedImage, region: Region, rects: &[Region]) {
    let stride = image.width as usize * 4;
    for rect in rects {
        let Some(overlap) = rect.intersection(&region) else {
            continue;
        };
        let x0 = (overlap.x - region.x) as usize;
        let y0 = (overlap.y - region.y) as usize;
        for row in y0..y0 + overlap.height as usize {
            let start = row * stride + x0 * 4;
            let end = start + overlap.width as usize * 4;
            for px in image.bgra[start..end].as_chunks_mut::<4>().0 {
                px.copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
}

fn capture_region_blocking(region: Region) -> Result<CapturedImage, CaptureError> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
    };

    let width = region.width.max(1);
    let height = region.height.max(1);

    // SAFETY: standard GDI capture sequence; every resource created below is
    // released on every path (the inner closure only returns owned data).
    let image = unsafe {
        let hdc_screen: HDC = GetDC(None);
        if hdc_screen.is_invalid() {
            return Err(CaptureError::new("failed to get screen device context"));
        }

        let result = (|| {
            let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
            if hdc_mem.is_invalid() {
                return Err(CaptureError::new("failed to create memory device context"));
            }

            let inner = (|| {
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width as i32,
                        biHeight: -(height as i32), // negative = top-down rows
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                let hbitmap =
                    CreateDIBSection(Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                        .map_err(|e| {
                            CaptureError::new(format!("failed to create DIB section: {e}"))
                        })?;

                if bits.is_null() {
                    return Err(CaptureError::new("DIB section has no pixel buffer"));
                }

                let old = SelectObject(hdc_mem, HGDIOBJ(hbitmap.0));
                // Plain SRCCOPY, deliberately WITHOUT CAPTUREBLT (ADR-0003):
                // our own In-place Overlay is excluded from capture via
                // WDA_EXCLUDEFROMCAPTURE — but CAPTUREBLT + an excluded
                // window on screen poisons the whole blit (black capture),
                // so the layered-window flag must stay off. Plain SRCCOPY
                // captures normal windows fine and skips the overlay.
                let blt = BitBlt(
                    hdc_mem,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    Some(hdc_screen),
                    region.x,
                    region.y,
                    SRCCOPY,
                );

                let out = if blt.is_ok() {
                    let len = (width as usize) * (height as usize) * 4;
                    let slice = std::slice::from_raw_parts(bits as *const u8, len);
                    Ok(CapturedImage {
                        width,
                        height,
                        bgra: slice.to_vec(),
                    })
                } else {
                    Err(CaptureError::new(format!(
                        "BitBlt failed for region x={} y={} {}x{} (is it within the screen?)",
                        region.x, region.y, region.width, region.height
                    )))
                };

                SelectObject(hdc_mem, old);
                let _ = DeleteObject(HGDIOBJ(hbitmap.0));
                out
            })();

            let _ = DeleteDC(hdc_mem);
            inner
        })();

        ReleaseDC(None, hdc_screen);
        result
    }?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mask fill is pure buffer work (the geometry it relies on is
    /// `Region::intersection`, tested in core) — so it IS unit-testable
    /// here despite the adapter being otherwise outside automated tests.
    #[test]
    fn mask_fills_only_the_overlapping_rect_black() {
        let mut image = CapturedImage {
            width: 4,
            height: 4,
            bgra: vec![200; 4 * 4 * 4],
        };
        let region = Region {
            x: 100,
            y: 100,
            width: 4,
            height: 4,
        };
        // One rect overlapping the region's top-right 2×2, one fully
        // outside: the outside one must fill nothing.
        mask_image(
            &mut image,
            region,
            &[
                Region {
                    x: 102,
                    y: 100,
                    width: 10,
                    height: 2,
                },
                Region {
                    x: 0,
                    y: 0,
                    width: 5,
                    height: 5,
                },
            ],
        );
        let px = |x: usize, y: usize| -> [u8; 4] {
            image.bgra[y * 16 + x * 4..y * 16 + x * 4 + 4]
                .try_into()
                .unwrap()
        };
        for x in 0..4 {
            for y in 0..4 {
                let masked = x >= 2 && y < 2;
                assert_eq!(
                    px(x, y),
                    if masked { [0, 0, 0, 255] } else { [200; 4] },
                    "pixel ({x},{y}) wrong"
                );
            }
        }
    }
}
