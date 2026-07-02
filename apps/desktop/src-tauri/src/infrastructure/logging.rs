use std::path::Path;
use tracing_subscriber::EnvFilter;

pub fn init_logging(app_data_dir: &Path) {
    let log_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::daily(&log_dir, "imagedb.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Keep the guard alive for the duration of the app by leaking it intentionally.
    // This is acceptable for a desktop app's logging setup.
    std::mem::forget(_guard);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,imagedb_desktop_lib=debug"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .init();

    tracing::info!(
        "ImageDB logging initialized, log dir: {}",
        log_dir.display()
    );
}
