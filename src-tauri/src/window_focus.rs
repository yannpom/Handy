//! Capture and restore the frontmost application window.
//!
//! When a transcription shortcut is pressed, the module records which app
//! **and which specific window** was in the foreground.  Before pasting the
//! result it can re-activate that exact window so the text lands in the
//! correct place — even if the user switched apps or windows during
//! transcription.

use log::{debug, error, warn};
use std::sync::Mutex;

/// Opaque handle representing the window that was focused at capture time.
#[derive(Debug, Clone)]
pub struct FocusedApp {
    #[cfg(target_os = "macos")]
    pid: i32,
    /// Window title at capture time (used as primary match key).
    #[cfg(target_os = "macos")]
    window_title: Option<String>,
    /// Window position (x, y) at capture time (disambiguation).
    #[cfg(target_os = "macos")]
    window_pos: Option<(i32, i32)>,
    /// Window size (w, h) at capture time (disambiguation).
    #[cfg(target_os = "macos")]
    window_size: Option<(i32, i32)>,

    #[cfg(target_os = "windows")]
    hwnd: isize,

    #[cfg(target_os = "linux")]
    _unused: (),
}

/// Module-level storage for the last captured focused app.
static CAPTURED_APP: Mutex<Option<FocusedApp>> = Mutex::new(None);

// ── Public API ──────────────────────────────────────────────────────────

/// Snapshot the currently focused application and its active window.
pub fn capture_focused_app() {
    let app = platform_capture();
    match &app {
        Some(a) => debug!("Captured focused app: {:?}", a),
        None => warn!("Could not capture focused app"),
    }
    *CAPTURED_APP.lock().unwrap() = app;
}

/// Re-activate the previously captured window and clear the snapshot.
/// Returns `true` if focus was successfully restored.
pub fn restore_focused_app() -> bool {
    let app = CAPTURED_APP.lock().unwrap().take();
    match app {
        Some(a) => {
            debug!("Restoring focus to: {:?}", a);
            platform_restore(&a)
        }
        None => {
            debug!("No captured app to restore");
            false
        }
    }
}

/// Discard any stored snapshot without restoring focus (e.g. on cancel).
pub fn clear_captured_app() {
    *CAPTURED_APP.lock().unwrap() = None;
}

// ── macOS implementation ────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn platform_capture() -> Option<FocusedApp> {
    use std::process::Command;

    // Single osascript call that captures the PID of the frontmost process
    // *and* the title, position, and size of its focused window via the
    // Accessibility API (AXFocusedWindow attribute).
    //
    // Output format: PID|title|x,y|w,h
    // If the window attributes are unavailable we still get the PID.
    let script = r#"
tell application "System Events"
    set frontProc to first process whose frontmost is true
    set appPID to unix id of frontProc
    try
        set focusedWin to value of attribute "AXFocusedWindow" of frontProc
        set winTitle to value of attribute "AXTitle" of focusedWin
        set winPos to value of attribute "AXPosition" of focusedWin
        set winSize to value of attribute "AXSize" of focusedWin
        return (appPID as text) & "|" & winTitle & "|" & (item 1 of winPos as text) & "," & (item 2 of winPos as text) & "|" & (item 1 of winSize as text) & "," & (item 2 of winSize as text)
    on error
        return (appPID as text) & "|||"
    end try
end tell
"#;

    let output = Command::new("osascript")
        .args(["-e", script])
        .output()
        .ok()?;

    if !output.status.success() {
        error!(
            "osascript capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let parts: Vec<&str> = raw.splitn(4, '|').collect();
    if parts.is_empty() {
        return None;
    }

    let pid: i32 = parts[0].parse().ok()?;

    let window_title = parts.get(1).and_then(|t| {
        let t = t.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    let window_pos = parts.get(2).and_then(|p| {
        let coords: Vec<&str> = p.split(',').collect();
        if coords.len() == 2 {
            Some((coords[0].trim().parse().ok()?, coords[1].trim().parse().ok()?))
        } else {
            None
        }
    });

    let window_size = parts.get(3).and_then(|s| {
        let dims: Vec<&str> = s.split(',').collect();
        if dims.len() == 2 {
            Some((dims[0].trim().parse().ok()?, dims[1].trim().parse().ok()?))
        } else {
            None
        }
    });

    Some(FocusedApp {
        pid,
        window_title,
        window_pos,
        window_size,
    })
}

#[cfg(target_os = "macos")]
fn platform_restore(app: &FocusedApp) -> bool {
    use std::process::Command;

    // Build a restore script that:
    // 1. Activates the correct application (by PID)
    // 2. Tries to AXRaise the exact window by matching title + position + size
    // 3. Falls back to title-only match (window may have moved/resized)
    // 4. Falls back to app-level activation (current behaviour) if no match

    let title_escaped = app
        .window_title
        .as_deref()
        .unwrap_or("")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    let (x, y) = app.window_pos.unwrap_or((-99999, -99999));
    let (w, h) = app.window_size.unwrap_or((-1, -1));
    let has_window_info = app.window_title.is_some();

    let script = if has_window_info {
        format!(
            r#"
tell application "System Events"
    set targetProc to first process whose unix id is {pid}
    set frontmost of targetProc to true
    set matched to false

    -- Try to match by title + position + size (most precise)
    repeat with w in windows of targetProc
        try
            set wTitle to name of w
            set wPos to position of w
            set wSize to size of w
            if wTitle = "{title}" and (item 1 of wPos) = {x} and (item 2 of wPos) = {y} and (item 1 of wSize) = {w} and (item 2 of wSize) = {h} then
                perform action "AXRaise" of w
                set matched to true
                exit repeat
            end if
        end try
    end repeat

    -- Fallback: match by title only (window may have moved)
    if not matched then
        repeat with w in windows of targetProc
            try
                if name of w = "{title}" then
                    perform action "AXRaise" of w
                    set matched to true
                    exit repeat
                end if
            end try
        end repeat
    end if
end tell
"#,
            pid = app.pid,
            title = title_escaped,
            x = x,
            y = y,
            w = w,
            h = h,
        )
    } else {
        // No window info captured — fall back to app-level activation only
        format!(
            r#"tell application "System Events" to set frontmost of first process whose unix id is {} to true"#,
            app.pid
        )
    };

    match Command::new("osascript").args(["-e", &script]).output() {
        Ok(output) if output.status.success() => {
            debug!(
                "Successfully restored focus to pid {} (window: {:?})",
                app.pid, app.window_title
            );
            // Give the window manager a moment to complete the switch.
            std::thread::sleep(std::time::Duration::from_millis(50));
            true
        }
        Ok(output) => {
            warn!(
                "osascript restore failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            false
        }
        Err(e) => {
            error!("Failed to run osascript for restore: {}", e);
            false
        }
    }
}

// ── Windows implementation ──────────────────────────────────────────────
// On Windows, GetForegroundWindow already returns the specific window
// handle (HWND), so per-window restore works out of the box.

#[cfg(target_os = "windows")]
fn platform_capture() -> Option<FocusedApp> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == std::ptr::null_mut() {
        return None;
    }
    Some(FocusedApp {
        hwnd: hwnd.0 as isize,
    })
}

#[cfg(target_os = "windows")]
fn platform_restore(app: &FocusedApp) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    let hwnd = windows::Win32::Foundation::HWND(app.hwnd as *mut _);
    let result = unsafe { SetForegroundWindow(hwnd) };
    if result.as_bool() {
        debug!("Successfully restored focus to hwnd {:?}", app.hwnd);
        std::thread::sleep(std::time::Duration::from_millis(50));
        true
    } else {
        warn!("SetForegroundWindow failed for hwnd {:?}", app.hwnd);
        false
    }
}

// ── Linux implementation (best-effort) ──────────────────────────────────

#[cfg(target_os = "linux")]
fn platform_capture() -> Option<FocusedApp> {
    // On Linux (especially Wayland), reliably capturing/restoring the
    // focused window is not feasible without compositor-specific protocols.
    // We return None so the feature gracefully degrades to the current
    // behavior (paste into whatever is focused).
    debug!("Window focus capture not supported on Linux");
    None
}

#[cfg(target_os = "linux")]
fn platform_restore(_app: &FocusedApp) -> bool {
    false
}
