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

/// How long the window takes to ink a drawing that was sent as a file.
///
/// In `Mode::Pen` the window has something real to follow and this doesn't
/// apply. In `Mode::File` the page lands whole in about a second, so there is
/// no arrival to mirror and the window is honestly just replaying what was
/// sent. It still draws it stroke by stroke — watching it appear is the point
/// of the thing — but the clock is this constant rather than the tablet.
const FILE_REPLAY: std::time::Duration = std::time::Duration::from_millis(2200);

/// Trace the image and draw it on the configured reMarkable, reporting how
/// far the ink has got as it goes.
#[tauri::command]
async fn send_to_remarkable(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let cfg = settings::load_settings(app.clone());
    // Tracing is seconds of CPU and the push is minutes of paced IO; neither
    // belongs on a runtime thread the UI shares.
    tauri::async_runtime::spawn_blocking(move || {
        let r = match cfg.mode {
            settings::Mode::Pen => draw_with_pen(&app, &cfg, &path),
            settings::Mode::File => draw_as_file(&app, &cfg, &path),
            settings::Mode::Screen => show_on_screen(&app, &cfg, &path),
        };
        // The window's whole vocabulary for failure is a head-shake, which
        // says that something went wrong and nothing about what. Everything
        // here can fail for a reason that lives on the other end of an ssh
        // connection, so the reason goes to the terminal as well.
        match &r {
            Ok(msg) => eprintln!("[rm-bin] {msg}"),
            Err(e) => eprintln!("[rm-bin] {:?} failed: {e}", cfg.mode),
        }
        r
    })
    .await
    .map_err(|e| format!("绘制任务中断：{e}"))?
}

/// The device half, carried inside the app.
///
/// Embedded rather than shipped as a Tauri resource next to the binary: they
/// are 160 kB together, and a resource that can go missing turns a working
/// install into a runtime failure on a path that already has a device, an ssh
/// key and a stopped xochitl to go wrong. `include_bytes!` cannot be missing.
const RMFB_AGENT: &[u8] = include_bytes!("../resources/rmfb/rmfb-agent");
const RMFB_SHIM: &[u8] = include_bytes!("../resources/rmfb/librmfb.so");

/// Paint the image onto the panel itself.
///
/// There are no strokes here, so the window has nothing to ink — it holds the
/// photo and desaturates it as the bands land, which is what is happening on
/// the tablet: the same picture, arriving, in grey.
fn show_on_screen(
    app: &tauri::AppHandle,
    cfg: &settings::Settings,
    path: &str,
) -> Result<String, String> {
    rm::screen::deploy(&cfg.host, cfg.port, RMFB_AGENT, RMFB_SHIM)?;

    // An empty plan tells the frontend there is no line work coming, so it
    // keeps the photograph up instead of fading it out behind strokes.
    let _ = app.emit(PLAN_EVENT, Vec::<rm::draw::PreviewStroke>::new());
    let _ = app.emit(PROGRESS_EVENT, 0.0);

    let mut screen = rm::screen::Screen::open(&cfg.host, cfg.port)?;
    let grey = rm::screen::fit(path, &screen.panel)?;
    let (w, h) = (screen.panel.width, screen.panel.height);

    let result = rm::screen::show(&mut screen, &grey, |p| {
        let _ = app.emit(PROGRESS_EVENT, p);
    });
    let _ = app.emit(PROGRESS_EVENT, 1.0);
    if let Err(e) = result {
        // A half-painted panel with xochitl stopped is the worst state to
        // leave a tablet in; on success it stays, because the picture is the
        // whole point, but a failure gives the device straight back.
        drop(screen);
        let _ = rm::screen::restore(&cfg.host, cfg.port);
        return Err(e);
    }

    Ok(format!("showed {w}x{h} on the panel at {}", cfg.host))
}

/// Hand the tablet's own interface back, rather than waiting for the device's
/// own timer.
#[tauri::command]
async fn restore_device(app: tauri::AppHandle) -> Result<(), String> {
    let cfg = settings::load_settings(app);
    tauri::async_runtime::spawn_blocking(move || rm::screen::restore(&cfg.host, cfg.port))
        .await
        .map_err(|e| format!("恢复中断：{e}"))?
}

/// Replay the strokes through the pen digitizer, with the window inking each
/// one as the tablet inks it.
fn draw_with_pen(
    app: &tauri::AppHandle,
    cfg: &settings::Settings,
    path: &str,
) -> Result<String, String> {
    let calib = rm::device::detect(&cfg.host, cfg.port)?;
    let plan = rm::draw::plan(path, &calib)?;
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

    Ok(format!(
        "drew {count} strokes ({total} bytes) on {:?} at {}",
        calib.model, cfg.host
    ))
}

/// Write the strokes as a finished `.rm` notebook and install it.
///
/// The window is deliberately left showing the photo until the upload has
/// actually succeeded: unlike the pen path there is no partial state to be
/// faithful to, so the only honest moment to start inking is once the page is
/// really on the tablet. A failure therefore never draws anything.
fn draw_as_file(
    app: &tauri::AppHandle,
    cfg: &settings::Settings,
    path: &str,
) -> Result<String, String> {
    let calib = rm::device::detect(&cfg.host, cfg.port)?;
    let page = rm::draw::page(path, &calib)?;
    let count = page.preview.len();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let notebook = rm::upload::build(&rm::upload::name_from_path(path), &page.strokes, now_ms);
    let bytes = notebook.len();
    rm::upload::install(&cfg.host, cfg.port, &notebook)?;

    // On the tablet the drawing is already whole; this is the window catching
    // up with it.
    let _ = app.emit(PLAN_EVENT, &page.preview);
    let _ = app.emit(PROGRESS_EVENT, 0.0);
    let tick = FILE_REPLAY.div_f64(PROGRESS_STEPS);
    for i in 1..=PROGRESS_STEPS as u32 {
        std::thread::sleep(tick);
        let _ = app.emit(PROGRESS_EVENT, count as f64 * i as f64 / PROGRESS_STEPS);
    }

    Ok(format!(
        "wrote {count} strokes ({bytes} bytes) to notebook {} on {:?} at {}",
        notebook.doc, calib.model, cfg.host
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
            restore_device,
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
                } else if event.id() == "restore-device" {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = restore_device(app).await {
                            eprintln!("restore failed: {e}");
                        }
                    });
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
