//! Settings that are common for all worker projects.

use log::LevelFilter;
use serde::{Deserialize, Serialize};

/// Core settings expected by `buldozer_core` infrastructure.
///
/// This is intended to be embedded into project settings using `#[serde(flatten)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreSettings {
    /// Log verbosity for this app only: trace|debug|info|warn|error|off.
    pub log_level: String,
    /// If enabled, TUI checks for updates (`git fetch`) and prompts when behind.
    pub check_git_updates: bool,
    /// If enabled, SQLite is encrypted with SQLCipher and TUI prompts for password.
    pub db_encryption: bool,
}

impl Default for CoreSettings {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            check_git_updates: true,
            db_encryption: false,
        }
    }
}

impl CoreSettings {
    /// Validates values and returns a human readable error.
    pub fn validate(&self) -> Result<(), String> {
        let _ = parse_level_filter(&self.log_level)
            .ok_or_else(|| format!("invalid log_level: {}", self.log_level))?;
        Ok(())
    }

    /// Returns a `log::LevelFilter` derived from `log_level`.
    pub fn log_level_filter(&self) -> LevelFilter {
        parse_level_filter(&self.log_level).unwrap_or(LevelFilter::Info)
    }
}

fn parse_level_filter(s: &str) -> Option<LevelFilter> {
    match s.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LevelFilter::Trace),
        "debug" => Some(LevelFilter::Debug),
        "info" => Some(LevelFilter::Info),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "error" => Some(LevelFilter::Error),
        "off" => Some(LevelFilter::Off),
        _ => None,
    }
}
