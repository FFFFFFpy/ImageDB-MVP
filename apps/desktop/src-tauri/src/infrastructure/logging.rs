use std::path::Path;
use tracing_appender::non_blocking::NonBlocking;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::EnvFilter;

pub fn init_logging(app_data_dir: &Path) {
    let log_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::daily(&log_dir, "imagedb.log");
    let (non_blocking_file, _file_guard) = tracing_appender::non_blocking(file_appender);

    // Keep the guard alive for the duration of the app by leaking it intentionally.
    // This is acceptable for a desktop app's logging setup.
    std::mem::forget(_file_guard);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,imagedb_desktop_lib=debug"));

    // Tee logs to BOTH the rolling file and stderr so the console window the
    // app opens (and `pnpm dev` / `cargo run` terminals) shows live output
    // for debugging. Without stderr, the console stays blank even though
    // logs are being written to disk.
    let tee = BoxMakeWriter::new(TeeMakeWriter {
        file: non_blocking_file,
    });

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(tee)
        .with_ansi(false)
        .with_target(true)
        .init();

    tracing::info!(
        "ImageDB logging initialized, log dir: {}",
        log_dir.display()
    );
}

/// `MakeWriter` that produces a writer teeing to both the non-blocking file
/// appender and stderr, so the visible console shows live log output while
/// the same lines are appended to the rolling log file.
#[derive(Clone)]
struct TeeMakeWriter {
    file: NonBlocking,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TeeMakeWriter {
    type Writer = TeeWriter;
    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter {
            file: self.file.make_writer(),
        }
    }
}

/// Write to both a non-blocking file writer and stderr.
struct TeeWriter {
    file: NonBlocking,
}

impl std::io::Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Write to the file first; the non-blocking channel drops on full.
        let _ = std::io::Write::write(&mut self.file, buf);
        // Mirror to stderr so the visible console shows live logs.
        std::io::stderr().write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::Write::flush(&mut self.file);
        std::io::stderr().flush()
    }
}
