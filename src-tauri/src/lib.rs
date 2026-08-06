/// `pub` so the debug binaries under `src/bin/` — which link against this
/// crate like any external consumer would — can reach `rm::pdf` and
/// `rm::device` without duplicating them.
pub mod rm;

/// What has been dropped in but not yet sent — the whole point of the app
/// since a drop stopped meaning a send.
pub mod pending;

/// The window itself: one fixed size, and which part of it catches the pointer.
mod chrome;
mod settings;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::Manager;

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
            pending::load_pending,
            pending::enqueue_images,
            pending::remove_pending,
            pending::restore_pending,
            pending::clear_pending,
            pending::flush_queue,
            pending::device_online,
            chrome::set_interactive_rect,
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
            // Clearing goes back to the window rather than straight to
            // `pending::clear_pending`, so the paper is seen to leave rather
            // than simply stop existing.
            app.on_menu_event(|app, event| match event.id().0.as_str() {
                "open-settings" => {
                    let _ = settings::open_settings(app.clone());
                }
                id @ "queue-clear" => {
                    use tauri::Emitter;
                    let _ = app.emit("menu-action", id);
                }
                _ => {}
            });

            let window = app.get_webview_window("main").expect("no main window");
            let _ = window.set_always_on_top(true);
            clear_native_background(&window);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running rm-bin");
}
