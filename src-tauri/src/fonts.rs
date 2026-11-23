//! Installed system font families (ticket 15): the Text Style font selector
//! lists what the user actually has, via the DirectWrite system font
//! collection. Thin Win32 adapter — exercised by the manual smoke checklist,
//! not by automated tests (same decision as foreground.rs).
//!
//! The bundled "Noto Sans Thai" is always part of the returned list (it may
//! not be OS-installed; the app bundles it, see ui/fonts/), so the selector
//! can always offer the Thai-safe default.

/// Installed font family names, deduplicated and sorted
/// case-insensitively. Empty when DirectWrite is unavailable (off-Windows
/// builds, or a failing system) — the selector then just stays a typed
/// field, which keeps working.
pub fn font_families() -> Vec<String> {
    let mut families = system_font_families();
    if !families
        .iter()
        .any(|name| name.eq_ignore_ascii_case("Noto Sans Thai"))
    {
        families.push("Noto Sans Thai".to_string());
    }
    families.sort_by_key(|name| name.to_lowercase());
    families
}

#[cfg(windows)]
fn system_font_families() -> Vec<String> {
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED,
    };

    let factory: IDWriteFactory = match unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) } {
        Ok(factory) => factory,
        Err(_) => return Vec::new(),
    };
    let mut collection = None;
    if unsafe { factory.GetSystemFontCollection(&mut collection, false) }.is_err() {
        return Vec::new();
    }
    let Some(collection) = collection else {
        return Vec::new();
    };

    let count = unsafe { collection.GetFontFamilyCount() };
    let mut families = Vec::with_capacity(count as usize);
    for index in 0..count {
        let Ok(family) = (unsafe { collection.GetFontFamily(index) }) else {
            continue;
        };
        let Ok(names) = (unsafe { family.GetFamilyNames() }) else {
            continue;
        };
        if let Some(name) = unsafe { localized_name(&names) } {
            families.push(name);
        }
    }
    families.sort_by_key(|name| name.to_lowercase());
    families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    families
}

/// Picks the human name from a localized strings set: the English entry when
/// present ("Segoe UI" rather than "Segoe UI (German)" style localized
/// variants), otherwise the first entry.
#[cfg(windows)]
unsafe fn localized_name(
    names: &windows::Win32::Graphics::DirectWrite::IDWriteLocalizedStrings,
) -> Option<String> {
    use windows::core::PCWSTR;

    let count = unsafe { names.GetCount() };
    if count == 0 {
        return None;
    }
    let read = |index: u32| -> Option<String> {
        let len = unsafe { names.GetStringLength(index) }.ok()? as usize;
        let mut buffer = vec![0u16; len + 1];
        unsafe { names.GetString(index, &mut buffer) }.ok()?;
        let text = String::from_utf16_lossy(&buffer[..len]);
        let trimmed = text.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_string())
    };
    let locale = |index: u32| -> Option<String> {
        let len = unsafe { names.GetLocaleNameLength(index) }.ok()? as usize;
        let mut buffer = vec![0u16; len + 1];
        unsafe { names.GetLocaleName(index, &mut buffer) }.ok()?;
        Some(String::from_utf16_lossy(&buffer[..len]))
    };

    // Prefer an English name (any "en-*" locale); fall back to the first.
    for index in 0..count {
        if locale(index)
            .is_some_and(|tag| tag.to_ascii_lowercase().starts_with("en"))
        {
            return read(index).or_else(|| read(0));
        }
    }
    let _ = PCWSTR::null(); // keep the import honest when features shift
    read(0)
}

#[cfg(not(windows))]
fn system_font_families() -> Vec<String> {
    Vec::new()
}
