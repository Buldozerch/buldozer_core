#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! This crate is meant to be consumed by small, project-specific workers.
//! The project keeps only its business logic (modules) and a small `run(db)`.

#[cfg(feature = "db")]
pub mod db;

#[cfg(feature = "bootstrap")]
pub mod bootstrap;

#[cfg(feature = "files")]
pub mod files;

#[cfg(feature = "run_utils")]
pub mod run_utils;

#[cfg(feature = "http_wallet")]
pub mod net_wallet;

#[cfg(feature = "settings_template")]
pub mod settings_template;

#[cfg(feature = "settings_core")]
pub mod settings_core;

#[cfg(feature = "settings_file")]
pub mod settings_file;

#[cfg(feature = "worker_settings")]
pub mod worker_settings;

#[cfg(feature = "worker_run")]
pub mod worker_run;

#[cfg(feature = "tui")]
pub mod tui_shell;

#[cfg(feature = "tui")]
pub mod worker_tui;

#[cfg(feature = "tui")]
pub use crossterm;

#[cfg(feature = "tui")]
pub use ratatui;

#[cfg(feature = "logger")]
pub mod tui_logger;

#[cfg(feature = "git_update")]
pub mod update;

#[cfg(feature = "wallet_db")]
pub mod wallet_db;
