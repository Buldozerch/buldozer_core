use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use time::format_description::OwnedFormatItem;
use time::OffsetDateTime;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use std::io::Seek;

/// Default timestamp format (time-rs format description, v2).
///
/// Syntax reference: https://time-rs.github.io/book/api/format-description.html
pub const DEFAULT_LOG_TS: &str =
    "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]";

/// Max size of the on-disk log file.
///
/// When the file exceeds this limit, it is truncated and logging continues from the start.
pub const MAX_LOG_FILE_BYTES: u64 = 10 * 1024 * 1024;

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

    file: Option<Mutex<FileLogger>>,
}

struct FileLogger {
    w: BufWriter<std::fs::File>,
    bytes_written: u64,
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

        if let Some(f) = &self.file {
            if let Ok(mut fl) = f.lock() {
                let msg = format!("{} [{}] {}\n", line.ts, line.level, line.text);
                let msg_len = msg.as_bytes().len() as u64;

                // If the next write would exceed the limit, truncate and start over.
                if fl.bytes_written.saturating_add(msg_len) > MAX_LOG_FILE_BYTES {
                    let _ = fl.w.flush();
                    let file = fl.w.get_mut();
                    let _ = file.set_len(0);
                    let _ = file.seek(std::io::SeekFrom::Start(0));
                    fl.bytes_written = 0;
                }

                if fl.w.write_all(msg.as_bytes()).is_ok() {
                    fl.bytes_written = fl.bytes_written.saturating_add(msg_len);
                    let _ = fl.w.flush();
                }
            }
        }
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
    init_tui_logger_with_ts_and_file(level, crate_prefix, ts_format, None)
}

/// Initializes global logger with a custom timestamp format and optional file output.
///
/// If `log_file_path` is provided, logs are appended to that file.
pub fn init_tui_logger_with_ts_and_file(
    level: LevelFilter,
    crate_prefix: &'static str,
    ts_format: &str,
    log_file_path: Option<PathBuf>,
) -> Result<UnboundedReceiver<LogLine>, String> {
    let (tx, rx) = unbounded_channel();

    let ts_format =
        parse_ts_format(ts_format).map_err(|e| format!("invalid log timestamp format: {e}"))?;

    let file = if let Some(p) = log_file_path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create logs dir: {e}"))?;
        }
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&p)
            .map_err(|e| format!("open log file {}: {e}", p.display()))?;
        Some(Mutex::new(FileLogger {
            w: BufWriter::new(f),
            bytes_written: 0,
        }))
    } else {
        None
    };

    // Always include `buldozer_core` logs in addition to the consumer crate.
    let core_prefix = env!("CARGO_PKG_NAME");
    let logger = TuiLogger {
        level,
        prefixes: [crate_prefix, core_prefix],
        tx,
        ts_format,
        file,
    };
    log::set_boxed_logger(Box::new(logger)).map_err(|e| e.to_string())?;
    log::set_max_level(level);
    Ok(rx)
}

/// Builds a default log file path under `<files_dir>/logs/`.
///
/// File name: `log_<MM-DD_HH>.log`
pub fn default_log_file_path(
    files_dir: impl AsRef<Path>,
    _crate_name: &str,
) -> Result<PathBuf, String> {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let ts_fmt = time::format_description::parse_owned::<2>("[month]-[day]_[hour]")
        .map_err(|e| e.to_string())?;
    let ts = now.format(&ts_fmt).map_err(|e| e.to_string())?;
    Ok(files_dir
        .as_ref()
        .join("logs")
        .join(format!("log_{ts}.log")))
}

fn parse_ts_format(s: &str) -> Result<OwnedFormatItem, String> {
    // Prefer v2 format descriptions.
    if let Ok(v) = time::format_description::parse_owned::<2>(s) {
        return Ok(v);
    }
    time::format_description::parse_strftime_owned(s).map_err(|e| e.to_string())
}
