//! A default settings struct for typical wallet workers.
//!
//! This is the recommended settings type for most projects.

use crate::settings_core::CoreSettings;
use crate::settings_file::Validate;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Settings used by runner utilities and the preset TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerSettings {
    /// Concurrency for per-wallet jobs.
    pub threads: usize,
    /// Random delay range (seconds) before starting a wallet job.
    pub random_sleep_start_wallet_min: u64,
    /// Random delay range (seconds) before starting a wallet job.
    pub random_sleep_start_wallet_max: u64,
    /// How many reserve proxies to try per wallet.
    pub retry: usize,

    /// Common core settings (`log_level`, `db_encryption`, ...).
    #[serde(flatten)]
    pub core: CoreSettings,

    /// `[start, end]` wallet ids to run (1-based id from DB); `[0,0]` means all.
    pub range_wallets_to_run: [usize; 2],
    /// Shuffle wallets after filtering.
    pub shuffle_wallets: bool,
    /// Exact wallet ids to run (only when `range_wallets_to_run = [0,0]`).
    pub exact_wallets_to_run: Vec<usize>,

    /// If `true`, logs display full identity; otherwise logs display `[id]`.
    pub show_wallet_full_logs: bool,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            threads: 10,
            random_sleep_start_wallet_min: 0,
            random_sleep_start_wallet_max: 60,
            retry: 3,
            core: CoreSettings::default(),
            range_wallets_to_run: [0, 0],
            shuffle_wallets: true,
            exact_wallets_to_run: Vec::new(),
            show_wallet_full_logs: false,
        }
    }
}

impl Validate for WorkerSettings {
    fn validate(&self) -> Result<(), String> {
        if self.threads == 0 {
            return Err("threads must be > 0".into());
        }
        if self.random_sleep_start_wallet_min > self.random_sleep_start_wallet_max {
            return Err("random_sleep_start_wallet_min > random_sleep_start_wallet_max".into());
        }
        if self.retry == 0 {
            return Err("retry can't be 0".into());
        }

        self.core.validate()?;

        let [start, end] = self.range_wallets_to_run;
        let range_is_all = start == 0 && end == 0;
        if !range_is_all {
            if start == 0 || end == 0 {
                return Err("range_wallets_to_run must be [0,0] or both > 0".into());
            }
            if start > end {
                return Err("range_wallets_to_run start > end".into());
            }
        }

        if !range_is_all && !self.exact_wallets_to_run.is_empty() {
            return Err("exact_wallets_to_run works only when range_wallets_to_run = [0,0]".into());
        }

        if self.exact_wallets_to_run.contains(&0) {
            return Err("exact_wallets_to_run entries must be > 0".into());
        }

        let uniq: HashSet<usize> = self.exact_wallets_to_run.iter().copied().collect();
        if uniq.len() != self.exact_wallets_to_run.len() {
            return Err("exact_wallets_to_run contains duplicates".into());
        }

        Ok(())
    }
}
