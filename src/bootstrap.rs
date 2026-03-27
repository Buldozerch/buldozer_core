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
    crate::files::ensure_files_layout(layout)?;
    let settings = load_settings()?;
    let log_rx = crate::tui_logger::init_tui_logger(get_level(&settings), crate_prefix)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok((settings, log_rx))
}
