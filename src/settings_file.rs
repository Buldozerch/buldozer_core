//! Helpers for loading settings from a TOML file.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::fs;
use std::path::Path;

use crate::worker_settings::WorkerSettings;

/// Trait used by `buldozer_core` settings loaders.
pub trait Validate {
    fn validate(&self) -> Result<(), String>;
}

/// Empty settings section used when a project has no extra TOML sections.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NoExtraSettings {}

impl Validate for NoExtraSettings {
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A settings file wrapper that stores base worker settings under `[main]`.
///
/// Projects can add extra sections next to `[main]` and parse them via `extra`.
///
/// Example:
///
/// ```toml
/// [main]
/// threads = 10
/// retry = 3
///
/// [rpc]
/// url = "https://..."
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsFile<T = NoExtraSettings>
where
    T: Default,
{
    pub main: WorkerSettings,

    #[serde(default)]
    #[serde(flatten)]
    pub extra: T,
}

impl<T> Validate for SettingsFile<T>
where
    T: Default + Validate,
{
    fn validate(&self) -> Result<(), String> {
        self.main.validate()?;
        self.extra.validate()?;
        Ok(())
    }
}

/// Loads and validates a TOML file.
pub fn load_toml_file<T: DeserializeOwned + Validate>(path: impl AsRef<Path>) -> Result<T, String> {
    let src = fs::read_to_string(path.as_ref()).map_err(|e| e.to_string())?;
    let v: T = toml::from_str(&src).map_err(|e| e.to_string())?;
    v.validate()?;
    Ok(v)
}
