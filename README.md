# buldozer_core

`buldozer_core` is a reusable Rust core for building "wallet workers".

It provides:
- Files/bootstrap (`files/` folder, template settings, optional feature files)
- Settings primitives (core settings + worker settings)
- SQLite/SQLCipher DB (init + schema for wallets + import/sync from txt files)
- Proxy-aware HTTP wallet client (`reqwest` + proxy rotation)
- Runner helpers (select/shuffle wallets, initialize clients, reserve proxy consumption)
- Full-screen TUI shell (ratatui + crossterm) with logs, menus, cancel, git update modal, and secret prompt
- A ready-to-use worker TUI preset (`worker_tui`) so new projects can be very small

This crate is designed so your project only contains:
- project config (`files` layout + DB URL + mode toggles)
- your modules (what to do per wallet)
- a small `run(db)` that calls your module(s)

## Quickstart (new project)

Minimal `src/` layout:

```
src/
  config.rs
  main.rs
  run.rs
  modules.rs
  modules/undertape.rs
  settings_template.toml
```

### Cargo.toml

```toml
[dependencies]
buldozer_core = { git = "<your repo>", tag = "v0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
log = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### src/config.rs

This is the only place where you choose what files exist and how DB is interpreted.

```rust
use buldozer_core::files::{FilesLayout, WalletFilesLayoutParams};
use buldozer_core::wallet_db::{MainDataKind, WalletDbConfig};

pub const MAIN_DATA: MainDataKind = MainDataKind::SimpleWeb3;
pub const USE_TWITTER: bool = false;
pub const USE_DISCORD: bool = false;

pub const FILES_DIR: &str = "files";
pub const SETTINGS_FILE_NAME: &str = "settings.toml";
pub const PROXY_FILE_NAME: &str = "proxy.txt";
pub const RESERVE_PROXY_FILE_NAME: &str = "reserve_proxy.txt";
pub const SETTINGS_TEMPLATE_TOML: &str = include_str!("settings_template.toml");

pub const DB_URL: &str = "sqlite://./files/wallets.db?mode=rwc";

pub fn wallet_db_config() -> WalletDbConfig {
    WalletDbConfig {
        files_dir: FILES_DIR.to_string(),
        proxy_file_name: PROXY_FILE_NAME.to_string(),
        main_data_kind: MAIN_DATA,
        use_twitter: USE_TWITTER,
        use_discord: USE_DISCORD,
    }
}

pub fn files_layout() -> FilesLayout<'static> {
    buldozer_core::files::wallet_files_layout(WalletFilesLayoutParams {
        files_dir: FILES_DIR,
        proxy_file_name: PROXY_FILE_NAME,
        reserve_proxy_file_name: RESERVE_PROXY_FILE_NAME,
        settings_file_name: SETTINGS_FILE_NAME,
        settings_template: SETTINGS_TEMPLATE_TOML,
        main_data_kind: MAIN_DATA,
        use_twitter: USE_TWITTER,
        use_discord: USE_DISCORD,
    })
}

pub type Settings = buldozer_core::settings_file::SettingsFile;

pub fn load_settings() -> Result<Settings, Box<dyn std::error::Error + Send + Sync>> {
    let path = std::path::Path::new(FILES_DIR).join(SETTINGS_FILE_NAME);
    buldozer_core::settings_file::load_toml_file(path)
        .map_err(|e| std::io::Error::other(e).into())
}
```

### src/main.rs

```rust
mod config;
mod modules;
mod run;

use crate::config::{load_settings, wallet_db_config, DB_URL};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (settings, log_rx) = buldozer_core::bootstrap::init(
        &crate::config::files_layout(),
        env!("CARGO_PKG_NAME"),
        || load_settings(),
        |s| s.main.core.log_level_filter(),
    )?;

    let params = buldozer_core::worker_tui::WorkerTuiParams::new(
        env!("CARGO_PKG_NAME"),
        settings.main.core.check_git_updates,
        DB_URL.to_string(),
        settings.main.core.db_encryption,
        wallet_db_config(),
    )
    .with_actions(vec![
        "Start Mint".to_string(),
        "Withdraw ETH".to_string(),
    ]);

    // Optional: customize header text.
    // let params = params.with_header(buldozer_core::worker_tui::WorkerHeader {
    //     tagline: "My Worker".to_string(),
    //     extra_lines: vec!["Hello".to_string(), "World".to_string()],
    // });

    buldozer_core::worker_tui::start_worker_tui(params, log_rx, |action, db| async move {
        match action {
            0 => crate::run::run(&db).await.map_err(|e| e.to_string()),
            1 => crate::run::run(&db).await.map_err(|e| e.to_string()),
            _ => Err("unknown action".into()),
        }
    })
    .await?;

    Ok(())
}
```

### src/settings_template.toml

`WorkerSettings` live under the `[main]` table. Your project-specific sections can be added next to it.

```toml
[main]
threads = 10
random_sleep_start_wallet_min = 0
random_sleep_start_wallet_max = 60
retry = 3

# core settings (flattened into `[main]`)
log_level = "info"
check_git_updates = true
db_encryption = false

range_wallets_to_run = [0, 0]
shuffle_wallets = true
exact_wallets_to_run = []
show_wallet_full_logs = false

# [rpc]
# url = "https://..."
```

### src/run.rs

This is where your per-wallet logic is applied.

```rust
use crate::config::{load_settings, FILES_DIR, RESERVE_PROXY_FILE_NAME};
use crate::modules::undertape::send_wl;
use buldozer_core::net_wallet::WalletClientOptions;
use buldozer_core::wallet_db::WalletDb;
use futures::stream::{self, StreamExt};
use std::path::Path;

pub async fn run(db: &WalletDb) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let settings = load_settings()?;
    let main = &settings.main;
    let reserve_file = Path::new(FILES_DIR)
        .join(RESERVE_PROXY_FILE_NAME)
        .to_str()
        .ok_or("reserve_path is not utf-8")?
        .to_string();

    let ready = buldozer_core::worker_run::prepare_wallets_from_reserve_file(
         db,
         &settings,
         &reserve_file,
         WalletClientOptions::default(),
     )
     .await
     .map_err(|e| format!("prepare wallets: {e}"))?;

    stream::iter(ready.iter())
        .for_each_concurrent(main.threads, |w| async {
            buldozer_core::run_utils::random_sleep_s(
                &w.http.log_name,
                main.random_sleep_start_wallet_min,
                main.random_sleep_start_wallet_max,
            )
            .await;

            // `w.main_data` contains the DB main data (e.g. Web3 private key).
            if let Err(e) = send_wl(&w.http).await {
                log::error!("{} failed: {}", w.http.log_name, e);
             }
        })
        .await;

    Ok(())
}
```

## Settings

Settings are loaded from `files/settings.toml`.

The template is merged automatically (missing keys are added, existing values preserved).

Key settings:
- `[main].threads`: concurrency for the per-wallet job stream
- `[main].retry`: how many reserve proxies to try (in addition to primary)
- `[main].random_sleep_start_wallet_*`: random delay before starting a wallet
- `[main].range_wallets_to_run`, `[main].exact_wallets_to_run`, `[main].shuffle_wallets`: wallet selection
- `[main].show_wallet_full_logs`: whether to show full identity in logs or `[id]`
- `[main].log_level`, `[main].check_git_updates`, `[main].db_encryption`: core behavior

## Files / import / sync

The workflow is:
1) Put data into txt files under `files/`
2) Use TUI -> `DB Actions` -> `Import` or `Sync`
3) DB consumes lines from files (queue semantics)

Files depend on `MainDataKind`:
- `SimpleWeb3`: `files/wallets.txt`
- `Web3`: `files/private.txt` (requires feature `evm` to derive address)
- `Email`: `files/email.txt`
- `Steam`: `files/steam_data.txt` + `files/mafile/`

Always present:
- `files/proxy.txt`
- `files/reserve_proxy.txt`

Optional:
- `files/twitter.txt`
- `files/discord.txt`

## DB encryption (SQLCipher)

If `db_encryption = true`, TUI prompts for a password.
The password is not stored in settings.

## Feature flags

`buldozer_core` is feature-gated. Default features enable everything.

Common setups:
- Full worker: default features (TUI + DB + runner)
- Headless runner: disable default features and enable `wallet_db`, `worker_run`, `worker_settings`, `settings_file`

## Rustdoc

Generate docs:

```bash
cargo doc -p buldozer_core --open
```
