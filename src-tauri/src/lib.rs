/// `pub` so the debug binaries under `src/bin/` — which link against this
/// crate like any external consumer would — can reach `rm::pdf` and
/// `rm::device` without duplicating them.
pub mod rm;
mod settings;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager};

/// No strokes to trace, so the window has nothing to ink — it holds the
/// dropped photo and fades it as the upload lands, which `PROGRESS_EVENT`
/// drives. `PLAN_EVENT` is emitted once, empty, just to tell the frontend
/// there's no line work coming.
const PLAN_EVENT: &str = "draw-plan";
const PROGRESS_EVENT: &str = "draw-progress";

/// Wrap the image in a one-page PDF and hand it to the tablet's importer.
#[tauri::command]
async fn send_to_remarkable(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let cfg = settings::load_settings(app.clone());
    // Building the PDF is seconds of CPU and the upload is network IO;
    // neither belongs on a runtime thread the UI shares.
    tauri::async_runtime::spawn_blocking(move || {
        let r = send_as_pdf(&app, &cfg, &path);
        match &r {
            Ok(msg) => eprintln!("[rm-bin] {msg}"),
            Err(e) => eprintln!("[rm-bin] pdf upload failed: {e}"),
        }
        r
    })
    .await
    .map_err(|e| format!("绘制任务中断：{e}"))?
}

/// No strokes and no panel takeover — the window holds the photograph and
/// desaturates it while the document crosses. That is the honest picture of
/// this path: what arrives is the image, unchanged apart from being in a
/// document.
fn send_as_pdf(
    app: &tauri::AppHandle,
    cfg: &settings::Settings,
    path: &str,
) -> Result<String, String> {
    let _ = app.emit(PLAN_EVENT, Vec::<(f64, f64)>::new());
    let _ = app.emit(PROGRESS_EVENT, 0.0);

    let pdf = rm::pdf::build(path)?;
    // Building is the slow half — encoding a large photograph — and the post
    // is one request with no progress to read, so this is the only honest
    // waypoint there is.
    let _ = app.emit(PROGRESS_EVENT, 0.6);

    let name = rm::upload::name_from_path(path);
    let size = pdf.len();
    let how = rm::pdf::deliver(&cfg.host, cfg.port, &name, &pdf, path, &cfg.gemini_api_key)?;
    let _ = app.emit(PROGRESS_EVENT, 1.0);

    // The ssh path may have renamed the document on Gemini's suggestion —
    // report the name it actually landed under, not the one it was offered.
    let (final_name, route) = match how {
        rm::pdf::Delivered::WebInterface => (name, "the web interface"),
        rm::pdf::Delivered::Ssh { name } => (name, "ssh (xochitl restarted)"),
    };
    Ok(format!(
        "uploaded \"{final_name}.pdf\" ({size} bytes) to {} via {route}",
        cfg.host
    ))
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
