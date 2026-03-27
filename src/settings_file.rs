//! Helpers for loading settings from a TOML file.

use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;

/// Trait used by `buldozer_core` settings loaders.
pub trait Validate {
    fn validate(&self) -> Result<(), String>;
}

/// Loads and validates a TOML file.
pub fn load_toml_file<T: DeserializeOwned + Validate>(path: impl AsRef<Path>) -> Result<T, String> {
    let src = fs::read_to_string(path.as_ref()).map_err(|e| e.to_string())?;
    let v: T = toml::from_str(&src).map_err(|e| e.to_string())?;
    v.validate()?;
    Ok(v)
}
