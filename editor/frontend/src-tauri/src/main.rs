#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rollout_editor::resolve_dev_server_port;

fn main() {
    let context = tauri::generate_context!();

    // Opt-in debug output to help diagnose config/dev-server issues on Windows.
    if std::env::var_os("ROLLOUT_TAURI_DEBUG_CONFIG").is_some() {
        eprintln!(
            "[rollout-editor] build.dev_path={:?} build.dist_dir={:?}",
            context.config().build.dev_path,
            context.config().build.dist_dir
        );
    }

    tauri::Builder::default()
        .setup(|app| {
            // We create the window in Rust rather than relying on the config's `tauri.windows`
            // because on some Windows setups we've observed the config-driven default window
            // attempting to load `index.html` from assets even in dev, causing `AssetNotFound`.
            //
            // In dev we explicitly load the Vite dev server; in release we load the bundled app.
            let window_url = if cfg!(debug_assertions) {
                let port = resolve_dev_server_port(|k| std::env::var(k).ok());

                let dev_url = format!("http://127.0.0.1:{port}/");
                let parsed = dev_url
                    .parse()
                    .unwrap_or_else(|_| panic!("invalid dev url: {dev_url}"));
                tauri::WindowUrl::External(parsed)
            } else {
                tauri::WindowUrl::App("index.html".into())
            };

            tauri::WindowBuilder::new(app, "main", window_url)
                .title("Rollout Editor")
                .inner_size(1100.0, 800.0)
                .resizable(true)
                .build()?;

            Ok(())
        })
        .run(context)
        .expect("error while running Tauri application");
}
