//! Windows screen capture via GDI `BitBlt` — a thin adapter outside automated
//! tests (per the spec's testing decisions); exercised by the manual smoke
//! checklist.

use langception_core::ports::{CaptureError, CaptureSource, CapturedImage, Region};

#[derive(Default)]
pub struct GdiCaptureSource;

impl GdiCaptureSource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl CaptureSource for GdiCaptureSource {
    /// GDI `BitBlt` of the configured region takes a few milliseconds, well
    /// under the slice-1 cadence, so it runs inline rather than on the
    /// blocking pool.
    async fn capture(&self, region: Region) -> Result<CapturedImage, CaptureError> {
        capture_region_blocking(region)
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
    unsafe {
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
    }
}
