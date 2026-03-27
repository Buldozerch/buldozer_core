//! File bootstrap utilities for workers.
//!
//! Responsible for:
//! - creating `files/` directory
//! - creating required empty txt files (only if missing)
//! - creating settings file (only if missing)
//! - merging a settings template into an existing settings file

use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::wallet_db::MainDataKind;

/// Describes the layout under a root directory (usually `files/`).
pub struct FilesLayout<'a> {
    pub root_dir: PathBuf,
    pub files: Vec<&'a str>,
    pub dirs: Vec<&'a str>,
    pub settings_file_name: &'a str,
    pub settings_template: &'a str,
}

/// Parameters for the default wallet-worker file layout.
pub struct WalletFilesLayoutParams {
    pub files_dir: &'static str,
    pub proxy_file_name: &'static str,
    pub reserve_proxy_file_name: &'static str,
    pub settings_file_name: &'static str,
    pub settings_template: &'static str,
    pub main_data_kind: MainDataKind,
    pub use_twitter: bool,
    pub use_discord: bool,
}

/// Builds a typical wallet-worker file layout.
///
/// The result can be passed to [`ensure_files_layout`].
pub fn wallet_files_layout(p: WalletFilesLayoutParams) -> FilesLayout<'static> {
    let mut files = vec![p.proxy_file_name, p.reserve_proxy_file_name];
    let mut dirs: Vec<&'static str> = Vec::new();

    match p.main_data_kind {
        MainDataKind::SimpleWeb3 => files.push("wallets.txt"),
        MainDataKind::Email => files.push("email.txt"),
        MainDataKind::Web3 => files.push("private.txt"),
        MainDataKind::Steam => {
            files.push("steam_data.txt");
            dirs.push("mafile");
        }
    }

    if p.use_twitter {
        files.push("twitter.txt");
    }
    if p.use_discord {
        files.push("discord.txt");
    }

    FilesLayout {
        root_dir: PathBuf::from(p.files_dir),
        files,
        dirs,
        settings_file_name: p.settings_file_name,
        settings_template: p.settings_template,
    }
}

/// Ensures that all files/dirs exist and that the settings template is merged.
pub fn ensure_files_layout(layout: &FilesLayout<'_>) -> io::Result<()> {
    fs::create_dir_all(&layout.root_dir)?;

    for d in &layout.dirs {
        fs::create_dir_all(layout.root_dir.join(d))?;
    }
    for f in &layout.files {
        ensure_one(&layout.root_dir.join(f))?;
    }

    let settings_path = layout.root_dir.join(layout.settings_file_name);
    ensure_settings_file(&settings_path, layout.settings_template)?;
    crate::settings_template::merge_settings_from_template(
        &settings_path,
        layout.settings_template,
    )?;

    Ok(())
}

/// Reads a txt file into trimmed, non-empty, non-comment (`#`) lines.
pub fn read_lines(path: impl AsRef<Path>) -> io::Result<Vec<String>> {
    let s = fs::read_to_string(path)?;
    Ok(s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect())
}

/// Writes lines to a file, appending a trailing newline when non-empty.
pub fn write_lines(path: impl AsRef<Path>, lines: &[String]) -> io::Result<()> {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(l);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    fs::write(path, out)
}

fn ensure_settings_file(path: &Path, contents: &str) -> io::Result<bool> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(contents.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

fn ensure_one(path: &Path) -> io::Result<bool> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}
