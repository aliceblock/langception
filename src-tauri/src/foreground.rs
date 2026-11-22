//! The app behind the foreground window (ticket 07): a Profile activates
//! when its app becomes the foreground window, so the shell needs to know
//! which executable owns it and where its window sits on screen. Thin
//! Win32 adapter — exercised by the manual smoke checklist, not by
//! automated tests (same decision as capture.rs).
//!
//! `PROCESS_QUERY_LIMITED_INFORMATION` is used deliberately: it works for
//! elevated processes too, so a game running as admin is still matched.

use langception_core::ports::Region;

/// The app behind the current foreground window: its executable file name,
/// its window title, and the window's screen rect in PHYSICAL px.
#[derive(Debug, Clone, PartialEq)]
pub struct ForegroundApp {
    pub exe: String,
    pub title: String,
    pub rect: Region,
}

/// One candidate for the app picker (Settings → Profiles): an executable
/// with the title of one of its visible windows (the longest one found).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RunningApp {
    pub exe: String,
    pub title: String,
}

/// The app behind the current foreground window: its executable file name,
/// its window title, and the window's screen rect in PHYSICAL px. `None`
/// when there is nothing usable — no foreground window, a minimized or
/// invisible one, Langception itself (its windows are never translation
/// sources per ADR-0003, so they can never activate a Profile either), or
/// a process whose executable cannot be queried.
pub fn foreground_app() -> Option<ForegroundApp> {
    foreground_app_impl()
}

/// A visible top-level window of `exe`: the largest one (the app's main
/// window), for drawing Profile Capture Zones relative to it. `None` when
/// the app has no visible window right now (not running, or minimized to
/// the tray).
pub fn app_window_rect(exe: &str) -> Option<Region> {
    let wanted = exe.trim().to_lowercase();
    windows_of_process()
        .into_iter()
        .filter(|window| window.exe.eq_ignore_ascii_case(&wanted))
        .max_by_key(|window| window.rect.width as i64 * window.rect.height as i64)
        .map(|window| window.rect)
}

/// Every visible top-level app window, grouped per executable for the
/// app picker. Langception's own windows are excluded.
pub fn running_apps() -> Vec<RunningApp> {
    let mut apps: Vec<RunningApp> = Vec::new();
    for window in windows_of_process() {
        match apps.iter_mut().find(|app| app.exe == window.exe) {
            Some(existing) => {
                if window.title.len() > existing.title.len() {
                    existing.title = window.title;
                }
            }
            None => apps.push(RunningApp {
                exe: window.exe,
                title: window.title,
            }),
        }
    }
    apps.sort_by_key(|app| app.title.to_lowercase());
    apps
}

#[derive(Debug)]
struct ProcessWindow {
    exe: String,
    title: String,
    rect: Region,
}

#[cfg(windows)]
fn foreground_app_impl() -> Option<ForegroundApp> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None; // nothing there, or Langception itself
        }
        let exe = process_image_name(pid)?;
        let rect = window_rect(hwnd)?;
        let title = window_title(hwnd);
        Some(ForegroundApp { exe, title, rect })
    }
}

/// The fallback shape for off-Windows builds: no foreground app, so no
/// Profile ever matches and the global target always applies.
#[cfg(not(windows))]
fn foreground_app_impl() -> Option<ForegroundApp> {
    None
}

#[cfg(windows)]
fn windows_of_process() -> Vec<ProcessWindow> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let list = &mut *(lparam.0 as *mut Vec<HWND>);
        list.push(hwnd);
        true.into()
    }

    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: the callback only appends to the vector whose pointer we
    // pass; EnumWindows runs synchronously before returning.
    unsafe {
        if EnumWindows(Some(collect), LPARAM(&mut hwnds as *mut _ as isize)).is_err() {
            return Vec::new();
        }
    }
    let mut windows = Vec::new();
    for hwnd in hwnds {
        if let Some(window) = process_window(hwnd) {
            windows.push(window);
        }
    }
    windows
}

#[cfg(not(windows))]
fn windows_of_process() -> Vec<ProcessWindow> {
    Vec::new()
}

#[cfg(windows)]
fn process_window(hwnd: windows::Win32::Foundation::HWND) -> Option<ProcessWindow> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let exe = process_image_name(pid)?;
        let rect = window_rect(hwnd)?;
        Some(ProcessWindow {
            exe,
            title: window_title(hwnd),
            rect,
        })
    }
}

#[cfg(windows)]
/// The executable FILE name ("game.exe") of `pid`, or `None` when the
/// process cannot be opened/name-queried (a protected process has no
/// readable image name — such an app simply never matches a Profile).
fn process_image_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let result = (|| {
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
            .ok()?;
            let path = String::from_utf16_lossy(&buffer[..len as usize]);
            std::path::Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })();
        let _ = CloseHandle(handle);
        result
    }
}

#[cfg(windows)]
fn window_rect(hwnd: windows::Win32::Foundation::HWND) -> Option<Region> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    // SAFETY: a valid, local RECT pointer for the duration of the call.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Region {
        x: rect.left,
        y: rect.top,
        width: width as u32,
        height: height as u32,
    })
}

#[cfg(windows)]
fn window_title(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

    let mut buffer = [0u16; 512];
    // SAFETY: the buffer outlives the call and its length is passed in.
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    let len = len.max(0) as usize;
    String::from_utf16_lossy(&buffer[..len.min(buffer.len())])
}
