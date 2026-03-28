//! One-call initialization for a typical worker binary.
//!
//! It:
//! - ensures `files/` layout + settings template merge
//! - loads settings (project decides the concrete settings type)
//! - initializes the TUI logger (crate-target filtered)

use crate::files::FilesLayout;
use crate::tui_logger::LogLine;
use tokio::sync::mpsc::UnboundedReceiver;

/// Bootstrap helper used by worker binaries.
///
/// `load_settings` is called after the files layout exists.
/// `get_level` extracts log level for this binary.
pub fn init<T, F, G>(
    layout: &FilesLayout<'_>,
    crate_prefix: &'static str,
    load_settings: F,
    get_level: G,
) -> Result<(T, UnboundedReceiver<LogLine>), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce() -> Result<T, Box<dyn std::error::Error + Send + Sync>>,
    G: FnOnce(&T) -> log::LevelFilter,
{
    init_with_ts(layout, crate_prefix, load_settings, get_level, |_| {
        crate::tui_logger::DEFAULT_LOG_TS
    })
}

/// Same as [`init`], but allows customizing the log timestamp format.
pub fn init_with_ts<T, F, G, H>(
    layout: &FilesLayout<'_>,
    crate_prefix: &'static str,
    load_settings: F,
    get_level: G,
    get_ts_format: H,
) -> Result<(T, UnboundedReceiver<LogLine>), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce() -> Result<T, Box<dyn std::error::Error + Send + Sync>>,
    G: FnOnce(&T) -> log::LevelFilter,
    H: FnOnce(&T) -> &'static str,
{
    crate::files::ensure_files_layout(layout)?;
    let settings = load_settings()?;
    let log_rx = crate::tui_logger::init_tui_logger_with_ts(
        get_level(&settings),
        crate_prefix,
        get_ts_format(&settings),
    )
    .map_err(|e| std::io::Error::other(e))?;
    Ok((settings, log_rx))
}
