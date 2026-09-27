// Hide the Windows console window for the GUI app.
// The in-app Logs panel captures all output instead.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod components;
mod log_buffer;
mod theme;
mod views;

use gpui::*;
use gpui_component::*;
use std::borrow::Cow;
use tracing_subscriber::layer::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Asset source that embeds the icons directory at compile time.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        // Strip leading slash if present
        let path = path.trim_start_matches('/');
        // Walk the statically embedded directory map
        match ICON_ASSETS.iter().find(|(p, _)| *p == path) {
            Some((_, data)) => Ok(Some(Cow::Borrowed(data))),
            None => Ok(None),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(ICON_ASSETS
            .iter()
            .filter_map(|(p, _)| {
                let p = *p;
                if p.starts_with(prefix) {
                    Some(SharedString::from(p))
                } else {
                    None
                }
            })
            .collect())
    }
}

/// Statically embedded icon assets.
static ICON_ASSETS: &[(&str, &[u8])] = &[
    (
        "icons/window-close.svg",
        include_bytes!("../assets/icons/window-close.svg"),
    ),
    (
        "icons/window-minimize.svg",
        include_bytes!("../assets/icons/window-minimize.svg"),
    ),
    (
        "icons/window-maximize.svg",
        include_bytes!("../assets/icons/window-maximize.svg"),
    ),
    (
        "icons/window-restore.svg",
        include_bytes!("../assets/icons/window-restore.svg"),
    ),
    (
        "icons/layout-dashboard.svg",
        include_bytes!("../assets/icons/layout-dashboard.svg"),
    ),
    (
        "icons/gallery-vertical-end.svg",
        include_bytes!("../assets/icons/gallery-vertical-end.svg"),
    ),
    (
        "icons/inspector.svg",
        include_bytes!("../assets/icons/inspector.svg"),
    ),
    (
        "icons/globe.svg",
        include_bytes!("../assets/icons/globe.svg"),
    ),
    (
        "icons/settings-2.svg",
        include_bytes!("../assets/icons/settings-2.svg"),
    ),
    ("icons/map.svg", include_bytes!("../assets/icons/map.svg")),
    (
        "icons/settings.svg",
        include_bytes!("../assets/icons/settings.svg"),
    ),
    (
        "icons/square-terminal.svg",
        include_bytes!("../assets/icons/square-terminal.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/check.svg",
        include_bytes!("../assets/icons/check.svg"),
    ),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/minus.svg",
        include_bytes!("../assets/icons/minus.svg"),
    ),
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    (
        "icons/loader.svg",
        include_bytes!("../assets/icons/loader.svg"),
    ),
    (
        "icons/circle.svg",
        include_bytes!("../assets/icons/circle.svg"),
    ),
    (
        "icons/circle-check.svg",
        include_bytes!("../assets/icons/circle-check.svg"),
    ),
    (
        "icons/circle-x.svg",
        include_bytes!("../assets/icons/circle-x.svg"),
    ),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    (
        "icons/alert-triangle.svg",
        include_bytes!("../assets/icons/alert-triangle.svg"),
    ),
    (
        "icons/trash-2.svg",
        include_bytes!("../assets/icons/trash-2.svg"),
    ),
    ("icons/edit.svg", include_bytes!("../assets/icons/edit.svg")),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    (
        "icons/clipboard.svg",
        include_bytes!("../assets/icons/clipboard.svg"),
    ),
    ("icons/eye.svg", include_bytes!("../assets/icons/eye.svg")),
    (
        "icons/eye-off.svg",
        include_bytes!("../assets/icons/eye-off.svg"),
    ),
    (
        "icons/nav-dashboard.svg",
        include_bytes!("../assets/icons/nav-dashboard.svg"),
    ),
    (
        "icons/nav-nodes.svg",
        include_bytes!("../assets/icons/nav-nodes.svg"),
    ),
    (
        "icons/nav-groups.svg",
        include_bytes!("../assets/icons/nav-groups.svg"),
    ),
    (
        "icons/nav-connections.svg",
        include_bytes!("../assets/icons/nav-connections.svg"),
    ),
    (
        "icons/nav-rules.svg",
        include_bytes!("../assets/icons/nav-rules.svg"),
    ),
    (
        "icons/nav-logs.svg",
        include_bytes!("../assets/icons/nav-logs.svg"),
    ),
    (
        "icons/nav-settings.svg",
        include_bytes!("../assets/icons/nav-settings.svg"),
    ),
    ("icons/bolt.svg", include_bytes!("../assets/icons/bolt.svg")),
    (
        "icons/sockrocket.svg",
        include_bytes!("../assets/icons/sockrocket.svg"),
    ),
    ("logo-icon.svg", include_bytes!("../assets/logo-icon.svg")),
];

/// Best-effort cleanup: clear system proxy and restore TUN routes so the OS
/// doesn't keep routing traffic to a dead local proxy after the app exits.
fn cleanup_on_exit() {
    // 1. Clear system proxy (retry once on failure)
    for attempt in 0..2 {
        match sockrocket_core::clear_system_proxy() {
            Ok(()) => break,
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("not supported") {
                    break;
                }
                if attempt == 0 {
                    // Brief pause before retry
                    std::thread::sleep(std::time::Duration::from_millis(100));
                } else {
                    eprintln!("cleanup: failed to clear system proxy: {}", e);
                }
            }
        }
    }

    // 2. Restore TUN routes if they were active
    sockrocket_core::emergency_restore_routes();

    // 3. Flush the buffered log file (try_lock: a panicking thread may still
    // hold the writer lock — never deadlock the exit path on it).
    if let Some(writer) = LOG_FILE_WRITER.get()
        && let Ok(mut buf) = writer.0.try_lock()
    {
        use std::io::Write as _;
        let _ = buf.flush();
    }
}

/// Buffered writer behind the tracing fmt layer's log file. `BufWriter`
/// isn't `Clone`, so the layer is handed an `Arc` to this wrapper instead —
/// tracing-subscriber's blanket `MakeWriter` impl for `Arc<W>` only needs
/// `&W: io::Write`, which the impl below provides via the mutex.
struct SharedLogWriter(std::sync::Mutex<std::io::BufWriter<std::fs::File>>);

impl std::io::Write for &SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

/// Handle kept around so cleanup_on_exit can flush the buffered log file.
static LOG_FILE_WRITER: std::sync::OnceLock<std::sync::Arc<SharedLogWriter>> =
    std::sync::OnceLock::new();

/// Write a timestamped line to %APPDATA%\sockrocket\startup.log (best-effort).
fn startup_log(msg: &str) {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let log_path = std::path::PathBuf::from(appdata)
            .join("sockrocket")
            .join("startup.log");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(f, "{}", msg);
        }
    }
}

fn main() {
    startup_log("[1] main() started");

    // Set up tracing with both a log file and in-memory capture.
    //
    // Do NOT write to the console: Windows console output blocks while the
    // user has text selected in the console window, and every thread that
    // emits a log event would freeze with it — including the gpui main
    // thread. A log file never blocks like that.
    let log_buf = log_buffer::new_log_buffer();
    let capture_layer = log_buffer::LogCaptureLayer::new(log_buf.clone());

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quinn=warn,h3=warn"));

    let file_writer = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .map(|d| d.join("sockrocket"))
        .and_then(|d| {
            std::fs::create_dir_all(&d).ok()?;
            std::fs::File::create(d.join("sockrocket.log")).ok()
        });

    // The capture layer feeds the in-app Logs page; filter it at INFO so
    // per-connection DEBUG events never pay formatting + mutex cost.
    let capture_layer = capture_layer.with_filter(tracing_subscriber::filter::LevelFilter::INFO);
    let registry = tracing_subscriber::registry().with(capture_layer);
    match file_writer {
        Some(file) => {
            // Buffer the log file (64 KB) — an unbuffered File costs one
            // write syscall per log event.
            let writer = std::sync::Arc::new(SharedLogWriter(std::sync::Mutex::new(
                std::io::BufWriter::with_capacity(64 * 1024, file),
            )));
            let _ = LOG_FILE_WRITER.set(writer.clone());
            let fmt = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_filter(filter);
            registry.with(fmt).init();
        }
        None => registry.with(filter).init(),
    }

    startup_log("[2] tracing initialized");

    // Register panic hook to clear system proxy even on panic
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        cleanup_on_exit();
        // Write crash log to %APPDATA%\sockrocket\crash.log for diagnosis
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let log_path = std::path::PathBuf::from(appdata)
                .join("sockrocket")
                .join("crash.log");
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let msg = format!("{}\n", info);
            let _ = std::fs::write(&log_path, &msg);
        }
        default_panic(info);
    }));

    // Create tokio runtime for async proxy/network operations.
    //
    // All data-plane work (relay polling, TLS record processing, AEAD crypto,
    // ipstack, wintun reads) runs on these threads — with only 2, a single
    // high-bandwidth AES-GCM stream can saturate one core and every other
    // connection queues behind it. Use all available cores; this is the
    // single largest throughput win on multi-core machines.
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build()
        .expect("Failed to create tokio runtime");
    let tokio_handle = rt.handle().clone();

    startup_log("[3] tokio runtime created");

    // Register Ctrl+C handler to clean up system proxy
    let ctrlc_handle = tokio_handle.clone();
    std::thread::spawn(move || {
        ctrlc_handle.block_on(async {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                cleanup_on_exit();
                std::process::exit(0);
            }
        });
    });

    // On Windows, register SetConsoleCtrlHandler for CTRL_CLOSE_EVENT,
    // CTRL_LOGOFF_EVENT, and CTRL_SHUTDOWN_EVENT. These are NOT caught
    // by Ctrl+C handlers and would otherwise skip cleanup.
    #[cfg(target_os = "windows")]
    {
        unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
            // CTRL_CLOSE_EVENT=2, CTRL_LOGOFF_EVENT=5, CTRL_SHUTDOWN_EVENT=6
            if ctrl_type == 2 || ctrl_type == 5 || ctrl_type == 6 {
                // Run cleanup inline — we have ~5s before Windows kills us
                cleanup_on_exit();
                return 1; // handled
            }
            0 // not handled
        }

        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: unsafe extern "system" fn(u32) -> i32,
                add: i32,
            ) -> i32;
        }

        unsafe { SetConsoleCtrlHandler(console_ctrl_handler, 1) };
    }

    startup_log("[4] creating GPUI application");
    let app = Application::new().with_assets(Assets);
    startup_log("[5] calling app.run");

    app.run(move |cx| {
        startup_log("[6] inside app.run callback");

        // Embed the v2 design fonts (Inter + JetBrains Mono, both OFL) so
        // text renders like ui-prototype-v2 on any machine.
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Regular.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Medium.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/Inter-SemiBold.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Bold.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf") as &[u8]),
                Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf") as &[u8]),
            ])
            .expect("failed to load embedded fonts");

        gpui_component::init(cx);
        // Force dark mode to match our fixed dark color scheme
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        // Application menu (macOS menu bar shows the first menu as the app name).
        // Without Quit here, the traffic-light close is the only exit path and
        // Cmd+Q / the app menu do nothing.
        cx.on_action(|_: &app::Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", app::Quit, None),
            KeyBinding::new("ctrl-q", app::Quit, None),
            KeyBinding::new("alt-f4", app::Quit, None),
            KeyBinding::new("ctrl-shift-c", app::ToggleProxy, None),
            KeyBinding::new("ctrl-k", app::OpenCommandPalette, None),
            KeyBinding::new("escape", app::CloseCommandPalette, Some("Palette")),
            KeyBinding::new("up", app::PaletteUp, Some("Palette")),
            KeyBinding::new("down", app::PaletteDown, Some("Palette")),
            KeyBinding::new("ctrl-1", app::SwitchToHome, None),
            KeyBinding::new("ctrl-2", app::SwitchToNodes, None),
            KeyBinding::new("ctrl-3", app::SwitchToConnections, None),
            KeyBinding::new("ctrl-4", app::SwitchToRules, None),
            KeyBinding::new("ctrl-5", app::SwitchToLogs, None),
            KeyBinding::new("ctrl-6", app::SwitchToSettings, None),
            KeyBinding::new("ctrl-7", app::SwitchToGroups, None),
            KeyBinding::new("ctrl-t", app::TestAllLatency, None),
            KeyBinding::new("ctrl-m", app::CycleProxyMode, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "Sockrocket".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Sockrocket", app::Quit),
            ],
        }]);
        cx.activate(true);
        // Closing the only window (red traffic light) should exit the app.
        // Keep the subscription for the process lifetime.
        std::mem::forget(cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        }));

        let handle = tokio_handle.clone();
        let buf = log_buf.clone();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(100.0), px(100.0)),
                size(px(960.0), px(640.0)),
            ))),
            window_min_size: Some(size(px(680.0), px(480.0))),
            titlebar: Some(app::AppState::titlebar_options()),
            ..Default::default()
        };

        startup_log("[7] calling cx.open_window");
        match cx.open_window(options, |window, cx| {
            startup_log("[8] inside open_window callback — constructing AppState");
            let view = cx.new(|cx| app::AppState::new(window, cx, handle, buf));
            startup_log("[9] AppState constructed — wrapping in Root");
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(_) => startup_log("[10] open_window succeeded"),
            Err(e) => {
                let msg = format!("[ERR] open_window failed: {e}");
                startup_log(&msg);
                panic!("Failed to open window: {e}");
            }
        }
    });

    startup_log("[11] app.run returned — process exiting");

    // App exited — always clean up system proxy
    cleanup_on_exit();
}
