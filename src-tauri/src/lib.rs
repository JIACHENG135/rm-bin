mod rm;
mod settings;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager};

/// The strokes about to be drawn, in image coordinates and in drawing order —
/// sent once, as soon as tracing finishes, so the window can ink the same
/// lines onto its little screen that the tablet is inking onto the page.
const PLAN_EVENT: &str = "draw-plan";
/// How many strokes the tablet has inked so far, fractional. Paired with
/// `draw-plan` this is the whole synchronisation: the window isn't running an
/// animation that happens to take the right amount of time, it's drawing the
/// same strokes in the same order at the same moment.
const PROGRESS_EVENT: &str = "draw-progress";
/// Don't emit for every 480-byte chunk — the webview can't use the
/// resolution and the IPC traffic is pure overhead.
const PROGRESS_STEPS: f64 = 240.0;

/// Trace the image and draw it on the configured reMarkable, reporting how
/// far the ink has got as it goes.
#[tauri::command]
async fn send_to_remarkable(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let cfg = settings::load_settings(app.clone());
    // Tracing is seconds of CPU and the push is minutes of paced IO; neither
    // belongs on a runtime thread the UI shares.
    tauri::async_runtime::spawn_blocking(move || {
        let calib = rm::device::detect(&cfg.host, cfg.port)?;
        let plan = rm::draw::plan(&path, &calib)?;
        let (total, count) = (plan.bytes.len(), plan.stroke_count());

        let _ = app.emit(PLAN_EVENT, &plan.preview);
        let _ = app.emit(PROGRESS_EVENT, 0.0);

        // Fractional on purpose: a drawing of 39 strokes would otherwise
        // report only 39 times and the window would ink it in visible jerks.
        // `strokes_done` is continuous, so this can be finer than one stroke.
        let step = count as f64 / PROGRESS_STEPS;
        let mut last = 0.0f64;
        let result = rm::device::push(&cfg.host, cfg.port, &calib, &plan.bytes, |written| {
            let done = plan.strokes_done(written);
            if done - last >= step {
                last = done;
                let _ = app.emit(PROGRESS_EVENT, done);
            }
        });
        // Even on failure the window should settle rather than freeze
        // part-drawn; the error itself is what it reacts to.
        let _ = app.emit(PROGRESS_EVENT, count as f64);
        result?;

        Ok(format!("drew {count} strokes ({total} bytes) on {:?} at {}", calib.model, cfg.host))
    })
    .await
    .map_err(|e| format!("绘制任务中断：{e}"))?
}

/// macOS 26 "liquid glass": the system paints an opaque backdrop for the
/// NSWindow *and* WKWebView paints its own under-page background on top of
/// whatever the window put there. Both have to be cleared at the AppKit level,
/// otherwise a transparent window renders as a gray slab — and any
/// NSVisualEffectView underneath the webview stays invisible.
pub(crate) fn clear_native_background(window: &tauri::WebviewWindow) {
    let _ = window.set_background_color(None);

    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};

        unsafe {
            if let Ok(ns_window) = window.ns_window() {
                let ns_window = ns_window as *mut AnyObject;
                let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
                let _: () = msg_send![ns_window, setOpaque: false];
                let _: () = msg_send![ns_window, setBackgroundColor: clear];
            }
        }

        if let Err(e) = window.with_webview(|webview| unsafe {
            let wk = webview.inner() as *mut AnyObject;
            let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
            let _: () = msg_send![wk, setUnderPageBackgroundColor: clear];
        }) {
            eprintln!("with_webview failed: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            send_to_remarkable,
            settings::load_settings,
            settings::save_settings,
            settings::test_connection,
            settings::open_settings,
            settings::show_context_menu,
        ])
        .setup(|app| {
            // ————— macOS menu bar: ⌘, opens settings; ⌘V works in the field —————
            let settings_item = MenuItemBuilder::new("设置…")
                .id("open-settings")
                .accelerator("CmdOrCtrl+,")
                .build(app)?;
            let app_menu = SubmenuBuilder::new(app, "RM Bin")
                .about(None)
                .separator()
                .item(&settings_item)
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;
            let edit_menu = SubmenuBuilder::new(app, "编辑")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let window_menu = SubmenuBuilder::new(app, "窗口")
                .minimize()
                .close_window()
                .build()?;
            let menu = MenuBuilder::new(app)
                .items(&[&app_menu, &edit_menu, &window_menu])
                .build()?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, event| {
                if event.id() == "open-settings" {
                    let _ = settings::open_settings(app.clone());
                }
            });

            let window = app.get_webview_window("main").expect("no main window");
            let _ = window.set_always_on_top(true);
            clear_native_background(&window);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running rm-bin");
}
