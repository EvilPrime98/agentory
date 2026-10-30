//! File logging: everything `tracing` emits goes to
//! `<data dir>/Agentory/logs/agentory.log.<date>` (7 days kept) as well as the console.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer as _, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

pub fn log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Agentory")
        .join("logs")
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())
}

/// Keep the returned guard alive for the whole run: dropping it flushes and stops the writer.
pub fn init() -> Option<WorkerGuard> {
    let dir = log_dir();
    let appender = std::fs::create_dir_all(&dir).ok().and_then(|_| {
        tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("agentory.log")
            .max_log_files(7)
            .build(&dir)
            .ok()
    });

    let (writer, guard) = match appender {
        Some(a) => {
            let (w, g) = tracing_appender::non_blocking(a);
            (Some(w), Some(g))
        }
        None => (None, None),
    };
    let file = writer.map(|w| {
        fmt::layer()
            .with_ansi(false)
            .with_writer(w)
            .with_filter(filter())
    });

    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(filter()))
        .with(file)
        .init();

    // A panic on any thread lands in the log, not only on a console nobody sees.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
    tracing::info!(
        "Agentory {} starting (logs: {})",
        env!("CARGO_PKG_VERSION"),
        dir.display()
    );
    guard
}
