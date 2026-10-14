//! Process start-up: logging, config loading and the eframe event loop.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, anyhow};
use everythux_core::config::Config;
use everythux_core::paths::AppPaths;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::app::{App, valid_rules};
use crate::backend::real::RealBackend;

/// Sets up logging to `<log_dir>/everythux.log` (non-blocking) and stderr. `RUST_LOG` overrides the
/// default level `info`. The returned guard must live until exit to flush the file.
fn init_logging(log_dir: &Path) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let (file_layer, guard) = match std::fs::create_dir_all(log_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::never(log_dir, "everythux.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false);
            (Some(layer), Some(guard))
        }
        Err(_) => (None, None),
    };
    // Ignore the error: a subscriber may already be installed (tests, embedding).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file_layer)
        .try_init();
    guard
}

/// Loads the config. A broken file is copied to `config.json.broken` and defaults are used; the
/// problem is returned as a warning for the banner.
fn load_config(paths: &AppPaths) -> (Config, Vec<String>) {
    match Config::load(&paths.config_file) {
        Ok(config) => (config, Vec::new()),
        Err(e) => {
            tracing::error!(error = %e, "cannot load config; using defaults");
            let backup = paths.config_file.with_extension("json.broken");
            let saved = std::fs::copy(&paths.config_file, &backup).is_ok();
            let mut message = format!("{e}. Using the default settings.");
            if saved {
                let _ = write!(
                    message,
                    " A copy of the file was saved as {}; the next settings change replaces {}.",
                    backup.display(),
                    paths.config_file.display()
                );
            }
            (Config::default(), vec![message])
        }
    }
}

/// Adapter from [`App`] to eframe.
struct EverythuxApp {
    app: App,
}

impl eframe::App for EverythuxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.show(ui);
    }
}

/// The orange magnifying glass shown in the taskbar and window decorations (see `packaging/everythux.svg`).
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

/// Decodes [`WINDOW_ICON_PNG`] into the RGBA data the windowing system expects.
fn window_icon() -> anyhow::Result<egui::IconData> {
    eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG).context("invalid PNG data")
}

/// Starts the application and blocks until the window is closed.
pub fn run() -> anyhow::Result<()> {
    let paths = AppPaths::discover().context("cannot determine the home directory")?;
    let _log_guard = init_logging(&paths.log_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), build = env!("EVERYTHUX_BUILD_ID"), config = %paths.config_file.display(), db = %paths.db_file.display(), "starting everythux");
    let (config, warnings) = load_config(&paths);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Everythux")
        .with_app_id("everythux")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([480.0, 240.0]);
    match window_icon() {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(error) => tracing::warn!(?error, "cannot decode the window icon, continuing without one"),
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "everythux",
        options,
        Box::new(move |cc| {
            let settings = everythux_index::IndexerSettings {
                volumes: config.volumes.clone(),
                exclusions: valid_rules(&config.exclusions),
                watch_ignore: Vec::new(),
                indexer: config.indexer.clone(),
            };
            let (backend, events) = RealBackend::start(&cc.egui_ctx, &paths, settings);
            let app = App::new(cc.egui_ctx.clone(), Box::new(backend), events, config, warnings);
            Ok(Box::new(EverythuxApp { app }))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_window_icon_decodes_to_a_square_rgba_image() {
        let icon = window_icon().expect("the embedded icon is a valid PNG");
        assert_eq!(icon.width, icon.height);
        assert!(icon.width >= 64, "icon too small: {}", icon.width);
        let pixels = usize::try_from(icon.width * icon.height).expect("fits");
        assert_eq!(icon.rgba.len(), pixels * 4);
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 0),
            "corners should be transparent"
        );
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255),
            "the glass should be opaque"
        );
    }

    #[test]
    fn broken_config_falls_back_to_defaults_and_keeps_a_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::in_dir(dir.path());
        std::fs::write(&paths.config_file, "{ not json").expect("write");
        let (config, warnings) = load_config(&paths);
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("config.json"), "{}", warnings[0]);
        assert_eq!(
            std::fs::read_to_string(&paths.config_file).expect("original untouched"),
            "{ not json"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.json.broken")).expect("backup"),
            "{ not json"
        );
    }

    #[test]
    fn missing_config_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (config, warnings) = load_config(&AppPaths::in_dir(dir.path()));
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
