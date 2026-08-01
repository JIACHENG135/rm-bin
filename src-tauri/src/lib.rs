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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running rm-bin");
}
