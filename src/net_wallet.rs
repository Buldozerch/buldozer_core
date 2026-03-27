//! Proxy-aware HTTP wallet client.
//!
//! - uses `reqwest` + per-wallet proxy
//! - can try a few "reserve" proxies when primary proxy fails

use rand::seq::SliceRandom;
use reqwest::{Client, Proxy};
use std::time::Duration;

/// Seed used to create a [`Wallet`] instance.
#[derive(Debug, Clone)]
pub struct WalletSeed {
    pub id: i64,
    pub address: String,
    pub proxy: String,
    pub log_name: String,
}

/// A ready-to-use HTTP client bound to a chosen proxy.
#[derive(Clone)]
pub struct Wallet {
    pub id: i64,
    pub address: String,
    pub log_name: String,
    pub proxy: String,
    pub client: Client,
}

/// Options for building a [`Wallet`] client.
#[derive(Debug, Clone)]
pub struct WalletClientOptions {
    pub connect_timeout_s: u64,
    pub request_timeout_s: u64,
    pub proxy_check_url: String,
    pub proxy_check_timeout_s: u64,
}

impl Default for WalletClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout_s: 30,
            request_timeout_s: 30,
            proxy_check_url: "https://httpbin.org/ip".to_string(),
            proxy_check_timeout_s: 3,
        }
    }
}

impl Wallet {
    /// Builds a wallet client, trying primary proxy and then up to `retry` reserve proxies.
    pub async fn new(
        seed: WalletSeed,
        reserve: &[String],
        retry: usize,
        opts: &WalletClientOptions,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let candidates = build_candidates(&seed.proxy, reserve, retry);
        let (client, chosen_proxy) = build_client(candidates, opts).await?;

        if chosen_proxy != seed.proxy {
            log::info!("{} switched proxy to {}", seed.log_name, chosen_proxy);
        } else {
            log::debug!("{} using primary proxy {}", seed.log_name, chosen_proxy);
        }

        Ok(Self {
            id: seed.id,
            address: seed.address,
            log_name: seed.log_name,
            proxy: chosen_proxy,
            client,
        })
    }
}

/// Returns candidates in the order: primary + shuffled reserves (excluding primary).
pub fn build_candidates(primary: &str, reserve: &[String], retry: usize) -> Vec<String> {
    let mut rng = rand::rng();
    let mut out = Vec::with_capacity(1 + retry);
    out.push(primary.to_string());
    let mut shuffled = reserve.to_vec();
    shuffled.shuffle(&mut rng);
    out.extend(shuffled.into_iter().filter(|p| p != primary).take(retry));
    out
}

async fn build_client(
    candidates: Vec<String>,
    opts: &WalletClientOptions,
) -> Result<(Client, String), Box<dyn std::error::Error + Send + Sync>> {
    let total = candidates.len();
    let t0 = std::time::Instant::now();
    for (idx, proxy_str) in candidates.into_iter().enumerate() {
        let proxy = match Proxy::all(&proxy_str) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("bad proxy format {}: {}", proxy_str, e);
                continue;
            }
        };
        let client = Client::builder()
            .proxy(proxy)
            .connect_timeout(Duration::from_secs(opts.connect_timeout_s))
            .timeout(Duration::from_secs(opts.request_timeout_s))
            .build()?;
        let res = client
            .get(&opts.proxy_check_url)
            .timeout(Duration::from_secs(opts.proxy_check_timeout_s))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                log::debug!(
                    "proxy ok: {} (try {}/{}, {:?})",
                    proxy_str,
                    idx + 1,
                    total,
                    t0.elapsed()
                );
                return Ok((client, proxy_str));
            }
            _ => {}
        }
    }
    Err("no working proxy from candidates".into())
}
