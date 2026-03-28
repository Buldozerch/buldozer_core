use log::{Level, LevelFilter, Log, Metadata, Record};
use time::format_description::OwnedFormatItem;
use time::OffsetDateTime;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Default timestamp format (time-rs format description, v2).
///
/// Syntax reference: https://time-rs.github.io/book/api/format-description.html
pub const DEFAULT_LOG_TS: &str =
    "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]";

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: String,
    pub level: Level,
    pub text: String,
}

struct TuiLogger {
    level: LevelFilter,
    prefixes: [&'static str; 2],
    tx: UnboundedSender<LogLine>,
    ts_format: OwnedFormatItem,
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

        let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
        let ts = now
            .format(&self.ts_format)
            .unwrap_or_else(|_| "0000-00-00 00:00:00.000".to_string());

        let line = LogLine {
            ts,
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
) -> Result<UnboundedReceiver<LogLine>, String> {
    init_tui_logger_with_ts(level, crate_prefix, DEFAULT_LOG_TS)
}

/// Initializes global logger with a custom timestamp format.
///
/// `ts_format` uses the time-rs format description syntax (recommended, v2).
/// As a convenience, if parsing as a time-rs description fails, we will try to parse it
/// as an `strftime` format.
pub fn init_tui_logger_with_ts(
    level: LevelFilter,
    crate_prefix: &'static str,
    ts_format: &str,
) -> Result<UnboundedReceiver<LogLine>, String> {
    let (tx, rx) = unbounded_channel();

    let ts_format =
        parse_ts_format(ts_format).map_err(|e| format!("invalid log timestamp format: {e}"))?;

    // Always include `buldozer_core` logs in addition to the consumer crate.
    let core_prefix = env!("CARGO_PKG_NAME");
    let logger = TuiLogger {
        level,
        prefixes: [crate_prefix, core_prefix],
        tx,
        ts_format,
    };
    log::set_boxed_logger(Box::new(logger)).map_err(|e| e.to_string())?;
    log::set_max_level(level);
    Ok(rx)
}

fn parse_ts_format(s: &str) -> Result<OwnedFormatItem, String> {
    // Prefer v2 format descriptions.
    if let Ok(v) = time::format_description::parse_owned::<2>(s) {
        return Ok(v);
    }
    time::format_description::parse_strftime_owned(s).map_err(|e| e.to_string())
}
