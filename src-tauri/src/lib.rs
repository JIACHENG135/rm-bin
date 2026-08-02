use tauri::Manager;

/// Stub: later this will render the image onto a connected reMarkable.
#[tauri::command]
async fn send_to_remarkable(path: String) -> Result<String, String> {
    std::thread::sleep(std::time::Duration::from_millis(1400));
    Ok(format!("queued: {path}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![send_to_remarkable])
        .setup(|app| {
            let window = app.get_webview_window("main").expect("no main window");
            let _ = window.set_always_on_top(true);
            // clear any residual webview background so the window is truly transparent
            let _ = window.set_background_color(None);

            // macOS 26 "liquid glass": the system paints a translucent material
            // behind borderless windows / WKWebView's under-page background.
            // Clear both at the AppKit level.
            #[cfg(target_os = "macos")]
            {
                use objc2::runtime::AnyObject;
                use objc2::{class, msg_send};

                unsafe {
                    let ns_window = window.ns_window().unwrap() as *mut AnyObject;
                    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
                    let _: () = msg_send![ns_window, setOpaque: false];
                    let _: () = msg_send![ns_window, setBackgroundColor: clear];
                }

                if let Err(e) = window.with_webview(|webview| unsafe {
                    let wk = webview.inner() as *mut AnyObject;
                    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
                    let _: () = msg_send![wk, setUnderPageBackgroundColor: clear];
                }) {
                    eprintln!("with_webview failed: {e}");
                }

            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running rm-bin");
}
