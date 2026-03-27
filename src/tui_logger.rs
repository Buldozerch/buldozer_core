use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub text: String,
}

struct TuiLogger {
    level: LevelFilter,
    prefixes: [&'static str; 2],
    tx: UnboundedSender<LogLine>,
}

impl Log for TuiLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if metadata.level() > self.level {
            return false;
        }
        let t = metadata.target();
        self.prefixes.iter().any(|p| t.starts_with(p))
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let line = LogLine {
            level: record.level(),
            text: format!("{}", record.args()),
        };
        let _ = self.tx.send(line);
    }

    fn flush(&self) {}
}

// Initializes global logger and returns a receiver for UI.
// Must be called only once.
pub fn init_tui_logger(
    level: LevelFilter,
    crate_prefix: &'static str,
) -> Result<UnboundedReceiver<LogLine>, SetLoggerError> {
    let (tx, rx) = unbounded_channel();

    // Always include `buldozer_core` logs in addition to the consumer crate.
    let core_prefix = env!("CARGO_PKG_NAME");
    let logger = TuiLogger {
        level,
        prefixes: [crate_prefix, core_prefix],
        tx,
    };
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(level);
    Ok(rx)
}
