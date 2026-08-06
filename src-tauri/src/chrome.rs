//! The window as a piece of desktop furniture.
//!
//! There is one window and it is never resized. It used to grow and shrink
//! every time the paper opened and closed, and that is what made it blink: a
//! transparent, always-on-top window cannot be resized without sometimes
//! losing the race with the webview's relayout, and the frame it loses is a
//! frame with nothing drawn in it. Atomic `setFrame:`, suppressed layer
//! actions, pinned layer contents — none of them close that gap reliably.
//!
//! So the window is simply large enough for the widest thing it will ever
//! show, and the paper is revealed inside it rather than by it. What that
//! costs is a big invisible rectangle that would swallow clicks meant for
//! whatever is behind it. This module is the answer to that: the page says how
//! much of the window is real, and a poll turns click-through on the moment
//! the pointer is somewhere that only looks like a window.

use std::sync::{Mutex, OnceLock};

use tauri::Manager;

/// Which part of the window is solid, as a box anchored to its bottom-right
/// corner. Everything outside it lets the pointer through to the desktop.
static LIVE: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();
/// What we last told the window, so the poll only speaks when something
/// changed.
static IGNORING: OnceLock<Mutex<bool>> = OnceLock::new();

fn live() -> &'static Mutex<(f64, f64)> {
    LIVE.get_or_init(|| Mutex::new((112.0, 148.0)))
}
fn ignoring() -> &'static Mutex<bool> {
    IGNORING.get_or_init(|| Mutex::new(false))
}

/// Declare the part of the window that should catch the pointer.
///
/// The window used to be resized every time the paper opened and closed, and
/// that is what made it blink: a transparent, always-on-top window cannot be
/// resized without occasionally losing the race with the webview's relayout,
/// and the frame it loses is a frame with nothing drawn in it. So the window
/// is now one fixed size, always big enough for the widest layout, and never
/// resized at all.
///
/// The cost of a permanently large window is that it would swallow clicks
/// meant for whatever is behind it, since most of it is empty and invisible.
/// This is the answer to that: the window reports how much of itself is real,
/// and a poll turns click-through on whenever the pointer is outside it.
#[tauri::command]
pub fn set_interactive_rect(app: tauri::AppHandle, w: f64, h: f64) {
    if let Ok(mut cur) = live().lock() {
        *cur = (w, h);
    }
    start_pointer_watch(&app);
}

/// Poll the pointer and keep the window's click-through in step with it.
///
/// Polling rather than tracking the pointer from the page, because once the
/// window is ignoring events the page stops hearing about the pointer at all
/// — it cannot report its way out of the state it is in. AppKit will always
/// answer where the cursor is, whoever owns it.
fn start_pointer_watch(app: &tauri::AppHandle) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(32));
        let handle = app.clone();
        // AppKit is main-thread-only, and so is changing a window's flags
        if handle
            .clone()
            .run_on_main_thread(move || update_click_through(&handle))
            .is_err()
        {
            return; // the app is going away
        }
    });
}

fn update_click_through(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Some(inside) = pointer_over_live_area(&window) else {
        return;
    };
    let Ok(mut ignoring) = ignoring().lock() else {
        return;
    };
    if *ignoring == inside {
        *ignoring = !inside;
        let _ = window.set_ignore_cursor_events(*ignoring);
    }
}

/// Is the cursor over the solid part of the window?
///
/// Generously: the box is grown by a margin on every side, so click-through is
/// already off by the time the pointer arrives. Without it a quick click on
/// the way in could land in the gap between two polls and fall through to the
/// desktop — and a widget that occasionally ignores a click is worse than one
/// that occupies a little more room than it draws.
#[cfg(target_os = "macos")]
fn pointer_over_live_area(window: &tauri::WebviewWindow) -> Option<bool> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect};

    /// How far ahead of the pointer to wake up, in points.
    const MARGIN: f64 = 24.0;

    let ns_window = window.ns_window().ok()? as *mut AnyObject;
    let (lw, lh) = *live().lock().ok()?;

    unsafe {
        let frame: NSRect = msg_send![ns_window, frame];
        // Both of these are in Cocoa screen space: origin bottom-left.
        let cursor: NSPoint = msg_send![class!(NSEvent), mouseLocation];

        let right = frame.origin.x + frame.size.width;
        let bottom = frame.origin.y;
        Some(
            cursor.x >= right - lw - MARGIN
                && cursor.x <= right + MARGIN
                && cursor.y >= bottom - MARGIN
                && cursor.y <= bottom + lh + MARGIN,
        )
    }
}

#[cfg(not(target_os = "macos"))]
fn pointer_over_live_area(_window: &tauri::WebviewWindow) -> Option<bool> {
    Some(true)
}

