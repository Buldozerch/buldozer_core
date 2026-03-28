//! High-level runner helpers for typical wallet workers.
//!
//! This module holds logic that is usually identical across projects:
//! - select wallets by range/exact ids
//! - shuffle
//! - build `WalletSeed` list with minimal/full log identity
//! - initialize `reqwest` clients with proxy rotation
//! - consume reserve proxies and update DB proxy when a reserve proxy is used

use crate::net_wallet::{Wallet, WalletClientOptions, WalletSeed};
use crate::wallet_db::{MainDataKind, WalletView};
use crate::worker_settings::WorkerSettings;
use futures::stream::{self, StreamExt};
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::files;
use crate::run_utils;
use crate::wallet_db::WalletDb;

pub struct SeedTask {
    pub id: i64,
    pub original_proxy: String,
    pub seed: WalletSeed,
}

/// A fully prepared wallet ready for runtime use.
///
/// This is a higher-level type than [`crate::net_wallet::Wallet`]: it keeps the
/// initialized proxy-bound HTTP client as well as the original `main_data` from DB.
///
/// Security: do not `Debug` this type (it may hold private keys).
#[derive(Clone)]
pub struct RuntimeWallet {
    pub id: i64,

    /// DB main data.
    /// - `Web3`: private key
    /// - `SimpleWeb3`: address
    /// - `Email`/`Steam`: corresponding account data
    pub main_data: String,

    /// Resolved identity address used by the HTTP wallet.
    pub address: String,

    /// Initialized proxy-bound HTTP client.
    pub http: Wallet,
}

/// Filters + optionally shuffles wallet rows based on settings.
pub fn select_rows(mut rows: Vec<WalletView>, settings: &WorkerSettings) -> Vec<WalletView> {
    rows.sort_by_key(|r| r.id);

    let [start, end] = settings.range_wallets_to_run;
    let range_is_all = start == 0 && end == 0;
    if !range_is_all {
        rows.retain(|r| {
            let id = r.id as usize;
            id >= start && id <= end
        });
    } else if !settings.exact_wallets_to_run.is_empty() {
        let exact: HashSet<usize> = settings.exact_wallets_to_run.iter().copied().collect();
        rows.retain(|r| exact.contains(&(r.id as usize)));
    }

    if settings.shuffle_wallets {
        rows.shuffle(&mut rand::rng());
    }

    rows
}

pub fn build_seed_tasks(
    rows: Vec<WalletView>,
    settings: &WorkerSettings,
    main_data_kind: MainDataKind,
) -> Vec<SeedTask> {
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let proxy = match &row.proxy {
            Some(p) => p.clone(),
            None => {
                log::warn!("wallet {} has no proxy", row.id);
                continue;
            }
        };

        let address = match (main_data_kind, &row.address) {
            (MainDataKind::Web3, None) => {
                log::warn!("wallet {} has no address yet; skip", row.id);
                continue;
            }
            (_, Some(a)) => a.clone(),
            (_, None) => row.main_data.clone(),
        };

        let log_name = if settings.show_wallet_full_logs {
            build_full_log_name_parts(row.id, main_data_kind, &row.main_data, &address)
        } else {
            format!("[{}]", row.id)
        };

        out.push(SeedTask {
            id: row.id,
            original_proxy: proxy.clone(),
            seed: WalletSeed {
                id: row.id,
                address,
                proxy,
                log_name,
            },
        });
    }

    out
}

fn build_seed_tasks_and_main_data(
    rows: Vec<WalletView>,
    settings: &WorkerSettings,
    main_data_kind: MainDataKind,
) -> (Vec<SeedTask>, HashMap<i64, String>) {
    let mut tasks = Vec::with_capacity(rows.len());
    let mut main_data_by_id: HashMap<i64, String> = HashMap::with_capacity(rows.len());

    for row in rows {
        let id = row.id;
        let main_data = row.main_data;

        let proxy = match row.proxy {
            Some(p) => p,
            None => {
                log::warn!("wallet {} has no proxy", id);
                continue;
            }
        };

        let address = match (main_data_kind, row.address) {
            (MainDataKind::Web3, None) => {
                log::warn!("wallet {} has no address yet; skip", id);
                continue;
            }
            (_, Some(a)) => a,
            (_, None) => main_data.clone(),
        };

        let log_name = if settings.show_wallet_full_logs {
            build_full_log_name_parts(id, main_data_kind, &main_data, &address)
        } else {
            format!("[{}]", id)
        };

        tasks.push(SeedTask {
            id,
            original_proxy: proxy.clone(),
            seed: WalletSeed {
                id,
                address,
                proxy,
                log_name,
            },
        });

        // Keep `main_data` only for wallets that we actually run.
        let prev = main_data_by_id.insert(id, main_data);
        debug_assert!(prev.is_none());
    }

    (tasks, main_data_by_id)
}

pub async fn init_wallets(
    tasks: Vec<SeedTask>,
    reserve: Arc<Vec<String>>,
    retry: usize,
    concurrency: usize,
    opts: WalletClientOptions,
) -> Vec<(i64, String, Wallet)> {
    let opts = Arc::new(opts);
    stream::iter(tasks)
        .map(|t| {
            let reserve = Arc::clone(&reserve);
            let opts = Arc::clone(&opts);
            async move {
                let original_proxy = t.original_proxy;
                let wallet = Wallet::new(t.seed, &reserve, retry, &opts).await?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>((t.id, original_proxy, wallet))
            }
        })
        .buffer_unordered(concurrency)
        .filter_map(|res| async {
            match res {
                Ok(t) => Some(t),
                Err(e) => {
                    log::error!("init wallet failed: {e}");
                    None
                }
            }
        })
        .collect()
        .await
}

pub fn used_reserve_proxies(items: &[(i64, String, Wallet)]) -> HashSet<String> {
    let mut used = HashSet::new();
    for (_id, original_proxy, w) in items {
        if &w.proxy != original_proxy {
            used.insert(w.proxy.clone());
        }
    }
    used
}

/// Full preparation step used by many workers.
///
/// - loads reserve proxies from `reserve_proxy_file`
/// - selects wallets from DB based on settings
/// - initializes wallet clients (proxy rotation)
/// - updates DB proxy if a reserve proxy was selected
/// - removes used reserve proxies from the file (queue semantics)
///
/// Returns a ready-to-run list of initialized wallets.
pub async fn prepare_wallets_from_reserve_file(
    db: &WalletDb,
    settings: &WorkerSettings,
    reserve_proxy_file: &str,
    client_opts: WalletClientOptions,
) -> Result<Vec<RuntimeWallet>, String> {
    let reserve = files::read_lines(reserve_proxy_file).map_err(|e| e.to_string())?;
    let reserve = Arc::new(reserve);

    let rows = db.get_all_wallets().await.map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let rows = select_rows(rows, settings);
    let (tasks, mut main_data_by_id) =
        build_seed_tasks_and_main_data(rows, settings, db.cfg().main_data_kind);
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    let init_conc = run_utils::init_concurrency(settings.threads, tasks.len());
    log::info!(
        "init wallets: total={} init_concurrency={} check_concurrency={}",
        tasks.len(),
        init_conc,
        settings.threads
    );

    let wallets = init_wallets(
        tasks,
        Arc::clone(&reserve),
        settings.retry,
        init_conc,
        client_opts,
    )
    .await;

    let mut changed = 0usize;
    let used_reserve = used_reserve_proxies(&wallets);
    let mut ready = Vec::with_capacity(wallets.len());
    for (id, original_proxy, w) in wallets {
        if w.proxy != original_proxy {
            db.set_wallet_proxy(id, &w.proxy)
                .await
                .map_err(|e| e.to_string())?;
            changed += 1;
        }
        let main_data = match main_data_by_id.remove(&id) {
            Some(v) => v,
            None => {
                return Err(format!(
                    "internal error: missing main_data for initialized wallet id={id}"
                ))
            }
        };
        let address = w.address.clone();
        ready.push(RuntimeWallet {
            id,
            main_data,
            address,
            http: w,
        });
    }
    if changed > 0 {
        log::info!("updated proxy in db for {changed} wallets");
    }

    if !used_reserve.is_empty() {
        run_utils::remove_lines_trimmed(reserve_proxy_file, &used_reserve)?;
        log::info!("removed {} used reserve proxies", used_reserve.len());
    }

    Ok(ready)
}

fn build_identity_for_log_parts(kind: MainDataKind, main_data: &str, resolved_address: &str) -> String {
    match kind {
        MainDataKind::Steam | MainDataKind::Email => main_data
            .split_once(':')
            .map(|(l, _)| l.to_string())
            .unwrap_or_else(|| main_data.to_string()),
        MainDataKind::Web3 | MainDataKind::SimpleWeb3 => resolved_address.to_string(),
    }
}

fn build_full_log_name_parts(
    id: i64,
    kind: MainDataKind,
    main_data: &str,
    resolved_address: &str,
) -> String {
    let identity = build_identity_for_log_parts(kind, main_data, resolved_address);
    format!("[{id}] {identity}")
}
