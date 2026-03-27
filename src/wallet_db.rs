//! Wallet database (SQLite / SQLCipher) and txt import/sync.
//!
//! Core responsibilities:
//! - initialize DB schema depending on [`MainDataKind`]
//! - import from txt files into DB (queue semantics)
//! - sync existing DB rows from txt files (by key or by index)
//! - provide a small query API used by runners

use crate::files::{read_lines, write_lines};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainDataKind {
    Web3,
    SimpleWeb3,
    Email,
    Steam,
}

#[derive(Debug, Clone)]
pub struct WalletDbConfig {
    pub files_dir: String,
    pub proxy_file_name: String,
    pub main_data_kind: MainDataKind,
    pub use_twitter: bool,
    pub use_discord: bool,
}

/// Wallet database wrapper.
///
/// Use [`WalletDb::init`] to open the DB and ensure schema exists.
#[derive(Clone)]
pub struct WalletDb {
    pool: SqlitePool,
    cfg: WalletDbConfig,
}

impl WalletDb {
    /// Opens the database (optionally encrypted) and ensures schema exists.
    pub async fn init(
        db_url: &str,
        db_key: Option<&str>,
        max_connections: u32,
        cfg: WalletDbConfig,
    ) -> Result<Self, sqlx::Error> {
        let pool = crate::db::init_wallet_db(db_url, db_key, max_connections).await?;
        init_schema(&pool, &cfg).await?;
        Ok(Self { pool, cfg })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns the DB configuration used to interpret schema and files.
    pub fn cfg(&self) -> &WalletDbConfig {
        &self.cfg
    }
}

async fn init_schema(pool: &SqlitePool, cfg: &WalletDbConfig) -> Result<(), sqlx::Error> {
    match cfg.main_data_kind {
        MainDataKind::Web3 => {
            crate::db::exec(
                pool,
                r#"
                CREATE TABLE IF NOT EXISTS wallet_web3 (
                    wallet_id INTEGER PRIMARY KEY,
                    private_key TEXT NOT NULL,
                    address TEXT,
                    FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
                );
                "#,
            )
            .await?;
            crate::db::exec(
                pool,
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_web3_private_key ON wallet_web3(private_key);"#,
            )
            .await?;
            crate::db::exec(
                pool,
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_web3_address ON wallet_web3(address);"#,
            )
            .await?;
        }
        MainDataKind::SimpleWeb3 => {
            crate::db::exec(
                pool,
                r#"
                CREATE TABLE IF NOT EXISTS wallet_simple_web3 (
                    wallet_id INTEGER PRIMARY KEY,
                    address TEXT NOT NULL,
                    FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
                );
                "#,
            )
            .await?;
            crate::db::exec(
                pool,
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_simple_web3_address ON wallet_simple_web3(address);"#,
            )
            .await?;
        }
        MainDataKind::Email => {
            crate::db::exec(
                pool,
                r#"
                CREATE TABLE IF NOT EXISTS wallet_email (
                    wallet_id INTEGER PRIMARY KEY,
                    email_data TEXT NOT NULL,
                    FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
                );
                "#,
            )
            .await?;
            crate::db::exec(
                pool,
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_email_email_data ON wallet_email(email_data);"#,
            )
            .await?;
        }
        MainDataKind::Steam => {
            crate::db::exec(
                pool,
                r#"
                CREATE TABLE IF NOT EXISTS wallet_steam (
                    wallet_id INTEGER PRIMARY KEY,
                    steam_data TEXT NOT NULL,
                    mafile_path TEXT NOT NULL,
                    FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
                );
                "#,
            )
            .await?;
            crate::db::exec(
                pool,
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_wallet_steam_steam_data ON wallet_steam(steam_data);"#,
            )
            .await?;
        }
    }

    if cfg.use_twitter {
        crate::db::exec(
            pool,
            r#"
            CREATE TABLE IF NOT EXISTS twitter_accounts (
                wallet_id INTEGER PRIMARY KEY,
                twitter_token TEXT,
                twitter_status TEXT,
                FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
            );
            "#,
        )
        .await?;
    }

    if cfg.use_discord {
        crate::db::exec(
            pool,
            r#"
            CREATE TABLE IF NOT EXISTS discord_accounts (
                wallet_id INTEGER PRIMARY KEY,
                discord_token TEXT,
                discord_status TEXT,
                FOREIGN KEY(wallet_id) REFERENCES wallets(id) ON DELETE CASCADE
            );
            "#,
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct WalletView {
    pub id: i64,
    pub main_data: String,
    pub address: Option<String>,
    pub proxy: Option<String>,
    pub proxy_status: String,
    pub twitter_token: Option<String>,
    pub twitter_status: Option<String>,
    pub discord_token: Option<String>,
    pub discord_status: Option<String>,
}

impl WalletDb {
    pub async fn get_all_wallets(&self) -> Result<Vec<WalletView>, sqlx::Error> {
        self.select_wallets(None).await
    }

    pub async fn get_wallets_by_proxy_status(&self, status: &str) -> Result<Vec<WalletView>, sqlx::Error> {
        self.select_wallets(Some(status)).await
    }

    pub async fn get_wallets_with_bad_proxy(&self) -> Result<Vec<WalletView>, sqlx::Error> {
        self.get_wallets_by_proxy_status("BAD").await
    }

    pub async fn set_wallet_proxy(&self, wallet_id: i64, proxy: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE wallets SET proxy = ?, proxy_status = 'OK' WHERE id = ?")
            .bind(proxy)
            .bind(wallet_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn select_wallets(&self, status_filter: Option<&str>) -> Result<Vec<WalletView>, sqlx::Error> {
        let (join_main, main_data_expr, address_expr) = match self.cfg.main_data_kind {
            MainDataKind::SimpleWeb3 => (
                "JOIN wallet_simple_web3 d ON d.wallet_id = w.id",
                "d.address",
                "d.address",
            ),
            MainDataKind::Web3 => (
                "JOIN wallet_web3 d ON d.wallet_id = w.id",
                "d.private_key",
                "d.address",
            ),
            MainDataKind::Email => (
                "JOIN wallet_email d ON d.wallet_id = w.id",
                "d.email_data",
                "NULL",
            ),
            MainDataKind::Steam => (
                "JOIN wallet_steam d ON d.wallet_id = w.id",
                "d.steam_data",
                "NULL",
            ),
        };

        let (twitter_select, twitter_join) = if self.cfg.use_twitter {
            (
                "t.twitter_token as twitter_token, t.twitter_status as twitter_status",
                "LEFT JOIN twitter_accounts t ON t.wallet_id = w.id",
            )
        } else {
            ("NULL as twitter_token, NULL as twitter_status", "")
        };

        let (discord_select, discord_join) = if self.cfg.use_discord {
            (
                "ds.discord_token as discord_token, ds.discord_status as discord_status",
                "LEFT JOIN discord_accounts ds ON ds.wallet_id = w.id",
            )
        } else {
            ("NULL as discord_token, NULL as discord_status", "")
        };

        let mut sql = String::new();
        sql.push_str("SELECT\n");
        sql.push_str("  w.id as id,\n");
        sql.push_str(&format!("  {main_data_expr} as main_data,\n"));
        sql.push_str(&format!("  {address_expr} as address,\n"));
        sql.push_str("  w.proxy as proxy,\n");
        sql.push_str("  w.proxy_status as proxy_status,\n");
        sql.push_str(&format!("  {twitter_select},\n"));
        sql.push_str(&format!("  {discord_select}\n"));
        sql.push_str("FROM wallets w\n");
        sql.push_str(join_main);
        sql.push('\n');
        if !twitter_join.is_empty() {
            sql.push_str(twitter_join);
            sql.push('\n');
        }
        if !discord_join.is_empty() {
            sql.push_str(discord_join);
            sql.push('\n');
        }
        if status_filter.is_some() {
            sql.push_str("WHERE w.proxy_status = ?\n");
        }
        sql.push_str("ORDER BY w.id\n");

        let q = sqlx::query_as::<_, WalletView>(&sql);
        let q = if let Some(status) = status_filter { q.bind(status) } else { q };
        q.fetch_all(&self.pool).await
    }
}

impl WalletDb {
    pub async fn import_from_files(&self) -> Result<(), sqlx::Error> {
        let main_data_file = main_data_file_name(self.cfg.main_data_kind);
        let main_lines = read_lines(format!("{}/{main_data_file}", self.cfg.files_dir))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if main_lines.is_empty() {
            return Ok(());
        }

        let proxies = read_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if proxies.len() != main_lines.len() {
            return Err(sqlx::Error::Protocol(format!(
                "proxy lines mismatch: proxies={} main_data={} ({})",
                proxies.len(),
                main_lines.len(),
                main_data_file
            )));
        }

        let twitter_tokens = self.read_feature_file("twitter.txt", main_lines.len(), self.cfg.use_twitter)?;
        let discord_tokens = self.read_feature_file("discord.txt", main_lines.len(), self.cfg.use_discord)?;

        let mut tx: Transaction<'_, Sqlite> = self.pool.begin().await?;

        for i in 0..main_lines.len() {
            let main_data_line = main_lines[i].as_str();
            let proxy = proxies[i].as_str();

            if let Some(wallet_id) = self.find_wallet_id_by_main_data(&mut tx, main_data_line).await? {
                self.update_wallet_proxy(&mut tx, wallet_id, proxy).await?;
                if self.cfg.main_data_kind == MainDataKind::Web3 {
                    self.ensure_web3_address(&mut tx, wallet_id, main_data_line).await?;
                }
                if let Some(tokens) = twitter_tokens.as_ref() {
                    self.upsert_twitter(&mut tx, wallet_id, &tokens[i]).await?;
                }
                if let Some(tokens) = discord_tokens.as_ref() {
                    self.upsert_discord(&mut tx, wallet_id, &tokens[i]).await?;
                }
                continue;
            }

            sqlx::query(
                r#"
                INSERT INTO wallets(proxy, proxy_status, completed)
                VALUES (?, 'OK', 0)
                "#,
            )
            .bind(proxy)
            .execute(&mut *tx)
            .await?;

            let wallet_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
                .fetch_one(&mut *tx)
                .await?;

            self.insert_main_data(&mut tx, wallet_id, main_data_line).await?;

            if let Some(tokens) = twitter_tokens.as_ref() {
                self.upsert_twitter(&mut tx, wallet_id, &tokens[i]).await?;
            }
            if let Some(tokens) = discord_tokens.as_ref() {
                self.upsert_discord(&mut tx, wallet_id, &tokens[i]).await?;
            }
        }

        tx.commit().await?;

        // Consume all.
        write_lines(format!("{}/{main_data_file}", self.cfg.files_dir), &[])
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        write_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name), &[])
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if twitter_tokens.is_some() {
            write_lines(format!("{}/twitter.txt", self.cfg.files_dir), &[])
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        }
        if discord_tokens.is_some() {
            write_lines(format!("{}/discord.txt", self.cfg.files_dir), &[])
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn sync_from_files(&self) -> Result<(), sqlx::Error> {
        let main_data_file = main_data_file_name(self.cfg.main_data_kind);
        let main_lines = read_lines(format!("{}/{main_data_file}", self.cfg.files_dir))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        // Index-based sync.
        if main_lines.is_empty() {
            let proxies = read_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name))
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

            let twitter_tokens = if self.cfg.use_twitter {
                Some(
                    read_lines(format!("{}/twitter.txt", self.cfg.files_dir))
                        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
                )
            } else {
                None
            };
            let discord_tokens = if self.cfg.use_discord {
                Some(
                    read_lines(format!("{}/discord.txt", self.cfg.files_dir))
                        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
                )
            } else {
                None
            };

            let mut tx: Transaction<'_, Sqlite> = self.pool.begin().await?;
            let consumed = self
                .sync_by_index(&mut tx, &proxies, twitter_tokens.as_ref(), discord_tokens.as_ref())
                .await?;
            tx.commit().await?;

            let remaining_proxies: Vec<String> = proxies.into_iter().skip(consumed).collect();
            write_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name), &remaining_proxies)
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

            if self.cfg.use_twitter {
                let remaining: Vec<String> = twitter_tokens.unwrap_or_default().into_iter().skip(consumed).collect();
                write_lines(format!("{}/twitter.txt", self.cfg.files_dir), &remaining)
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            }
            if self.cfg.use_discord {
                let remaining: Vec<String> = discord_tokens.unwrap_or_default().into_iter().skip(consumed).collect();
                write_lines(format!("{}/discord.txt", self.cfg.files_dir), &remaining)
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            }
            return Ok(());
        }

        // Keyed sync.
        let proxies = read_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if proxies.len() != main_lines.len() {
            return Err(sqlx::Error::Protocol(format!(
                "proxy lines mismatch: proxies={} main_data={} ({})",
                proxies.len(),
                main_lines.len(),
                main_data_file
            )));
        }

        let twitter_tokens = self.read_feature_file("twitter.txt", main_lines.len(), self.cfg.use_twitter)?;
        let discord_tokens = self.read_feature_file("discord.txt", main_lines.len(), self.cfg.use_discord)?;

        let mut tx: Transaction<'_, Sqlite> = self.pool.begin().await?;
        let mut processed = vec![false; main_lines.len()];

        for i in 0..main_lines.len() {
            let main_data_line = main_lines[i].as_str();
            let proxy = proxies[i].as_str();
            let Some(wallet_id) = self.find_wallet_id_by_main_data(&mut tx, main_data_line).await? else {
                continue;
            };
            self.update_wallet_proxy(&mut tx, wallet_id, proxy).await?;
            processed[i] = true;
            if self.cfg.main_data_kind == MainDataKind::Web3 {
                self.ensure_web3_address(&mut tx, wallet_id, main_data_line).await?;
            }
            if let Some(tokens) = twitter_tokens.as_ref() {
                self.upsert_twitter(&mut tx, wallet_id, &tokens[i]).await?;
            }
            if let Some(tokens) = discord_tokens.as_ref() {
                self.upsert_discord(&mut tx, wallet_id, &tokens[i]).await?;
            }
        }

        tx.commit().await?;

        let remaining_main: Vec<String> = main_lines
            .into_iter()
            .enumerate()
            .filter_map(|(i, v)| (!processed[i]).then_some(v))
            .collect();
        let remaining_proxies: Vec<String> = proxies
            .into_iter()
            .enumerate()
            .filter_map(|(i, v)| (!processed[i]).then_some(v))
            .collect();
        write_lines(format!("{}/{main_data_file}", self.cfg.files_dir), &remaining_main)
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        write_lines(format!("{}/{}", self.cfg.files_dir, self.cfg.proxy_file_name), &remaining_proxies)
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if self.cfg.use_twitter {
            let remaining: Vec<String> = twitter_tokens
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .filter_map(|(i, v)| (!processed[i]).then_some(v))
                .collect();
            write_lines(format!("{}/twitter.txt", self.cfg.files_dir), &remaining)
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        }
        if self.cfg.use_discord {
            let remaining: Vec<String> = discord_tokens
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .filter_map(|(i, v)| (!processed[i]).then_some(v))
                .collect();
            write_lines(format!("{}/discord.txt", self.cfg.files_dir), &remaining)
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        }
        Ok(())
    }

    fn read_feature_file(
        &self,
        name: &str,
        expected_lines: usize,
        enabled: bool,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        if !enabled {
            return Ok(None);
        }

        let tokens = read_lines(format!("{}/{name}", self.cfg.files_dir))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if tokens.len() != expected_lines {
            return Err(sqlx::Error::Protocol(format!(
                "{name} lines mismatch: {name}={} wallets={}",
                tokens.len(),
                expected_lines
            )));
        }

        Ok(Some(tokens))
    }

    async fn update_wallet_proxy(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        wallet_id: i64,
        proxy: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE wallets SET proxy = ? WHERE id = ?")
            .bind(proxy)
            .bind(wallet_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn upsert_twitter(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        wallet_id: i64,
        token: &str,
    ) -> Result<(), sqlx::Error> {
        if !self.cfg.use_twitter {
            return Ok(());
        }

        sqlx::query(
            r#"
            INSERT INTO twitter_accounts(wallet_id, twitter_token, twitter_status)
            VALUES (?, ?, 'OK')
            ON CONFLICT(wallet_id) DO UPDATE SET
              twitter_token = excluded.twitter_token
            "#,
        )
        .bind(wallet_id)
        .bind(token)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn upsert_discord(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        wallet_id: i64,
        token: &str,
    ) -> Result<(), sqlx::Error> {
        if !self.cfg.use_discord {
            return Ok(());
        }

        sqlx::query(
            r#"
            INSERT INTO discord_accounts(wallet_id, discord_token, discord_status)
            VALUES (?, ?, 'OK')
            ON CONFLICT(wallet_id) DO UPDATE SET
              discord_token = excluded.discord_token
            "#,
        )
        .bind(wallet_id)
        .bind(token)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn find_wallet_id_by_main_data(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        main_data_line: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        match self.cfg.main_data_kind {
            MainDataKind::SimpleWeb3 => {
                sqlx::query_scalar("SELECT wallet_id FROM wallet_simple_web3 WHERE address = ?")
                    .bind(main_data_line)
                    .fetch_optional(&mut **tx)
                    .await
            }
            MainDataKind::Email => {
                sqlx::query_scalar("SELECT wallet_id FROM wallet_email WHERE email_data = ?")
                    .bind(main_data_line)
                    .fetch_optional(&mut **tx)
                    .await
            }
            MainDataKind::Steam => {
                let steam_data = main_data_line.trim();
                sqlx::query_scalar("SELECT wallet_id FROM wallet_steam WHERE steam_data = ?")
                    .bind(steam_data)
                    .fetch_optional(&mut **tx)
                    .await
            }
            MainDataKind::Web3 => {
                let private_key = main_data_line.trim();
                sqlx::query_scalar("SELECT wallet_id FROM wallet_web3 WHERE private_key = ?")
                    .bind(private_key)
                    .fetch_optional(&mut **tx)
                    .await
            }
        }
    }

    async fn insert_main_data(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        wallet_id: i64,
        main_data_line: &str,
    ) -> Result<(), sqlx::Error> {
        match self.cfg.main_data_kind {
            MainDataKind::SimpleWeb3 => {
                sqlx::query(
                    r#"
                    INSERT INTO wallet_simple_web3(wallet_id, address)
                    VALUES (?, ?)
                    "#,
                )
                .bind(wallet_id)
                .bind(main_data_line)
                .execute(&mut **tx)
                .await?;
            }
            MainDataKind::Email => {
                sqlx::query(
                    r#"
                    INSERT INTO wallet_email(wallet_id, email_data)
                    VALUES (?, ?)
                    "#,
                )
                .bind(wallet_id)
                .bind(main_data_line)
                .execute(&mut **tx)
                .await?;
            }
            MainDataKind::Steam => {
                let steam_data = main_data_line.trim();
                let mafile_path = build_mafile_path(&self.cfg.files_dir, steam_data)
                    .map_err(sqlx::Error::Protocol)?;
                sqlx::query(
                    r#"
                    INSERT INTO wallet_steam(wallet_id, steam_data, mafile_path)
                    VALUES (?, ?, ?)
                    "#,
                )
                .bind(wallet_id)
                .bind(steam_data)
                .bind(mafile_path)
                .execute(&mut **tx)
                .await?;
            }
            MainDataKind::Web3 => {
                let private_key = main_data_line.trim();
                let address = private_key_to_address_checksum(private_key)
                    .map_err(sqlx::Error::Protocol)?;
                sqlx::query(
                    r#"
                    INSERT INTO wallet_web3(wallet_id, private_key, address)
                    VALUES (?, ?, ?)
                    "#,
                )
                .bind(wallet_id)
                .bind(private_key)
                .bind(address)
                .execute(&mut **tx)
                .await?;
            }
        }
        Ok(())
    }

    async fn ensure_web3_address(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        wallet_id: i64,
        private_key: &str,
    ) -> Result<(), sqlx::Error> {
        let addr = private_key_to_address_checksum(private_key).map_err(sqlx::Error::Protocol)?;
        sqlx::query("UPDATE wallet_web3 SET address = COALESCE(address, ?) WHERE wallet_id = ?")
            .bind(addr)
            .bind(wallet_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn sync_by_index(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        proxies: &[String],
        twitter_tokens: Option<&Vec<String>>,
        discord_tokens: Option<&Vec<String>>,
    ) -> Result<usize, sqlx::Error> {
        let wallet_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM wallets ORDER BY id")
            .fetch_all(&mut **tx)
            .await?;

        let n = wallet_ids.len().min(proxies.len());
        for i in 0..n {
            let wallet_id = wallet_ids[i];
            self.update_wallet_proxy(tx, wallet_id, &proxies[i]).await?;
            if let Some(tokens) = twitter_tokens
                && i < tokens.len()
            {
                self.upsert_twitter(tx, wallet_id, &tokens[i]).await?;
            }
            if let Some(tokens) = discord_tokens
                && i < tokens.len()
            {
                self.upsert_discord(tx, wallet_id, &tokens[i]).await?;
            }
        }
        Ok(n)
    }
}

fn main_data_file_name(kind: MainDataKind) -> &'static str {
    match kind {
        MainDataKind::SimpleWeb3 => "wallets.txt",
        MainDataKind::Email => "email.txt",
        MainDataKind::Web3 => "private.txt",
        MainDataKind::Steam => "steam_data.txt",
    }
}

fn build_mafile_path(files_dir: &str, steam_data: &str) -> Result<String, String> {
    let login = steam_data
        .split_once(':')
        .map(|(l, _)| l.trim())
        .unwrap_or(steam_data.trim());
    if login.is_empty() {
        return Err("steam_data login is empty".into());
    }
    Ok(format!("{files_dir}/mafile/{login}.mafile"))
}

fn private_key_to_address_checksum(private_key: &str) -> Result<String, String> {
    #[cfg(feature = "evm")]
    {
        use alloy_primitives::keccak256;
        use k256::ecdsa::SigningKey;

        let pk = private_key.trim();
        let pk = pk.strip_prefix("0x").unwrap_or(pk);
        if pk.len() != 64 {
            return Err("private key must be 32 bytes hex (64 chars)".into());
        }
        let bytes = alloy_primitives::hex::decode(pk).map_err(|e| format!("bad private key hex: {e}"))?;
        if bytes.len() != 32 {
            return Err("private key must decode to 32 bytes".into());
        }

        let signing_key = SigningKey::from_slice(&bytes)
            .map_err(|_| "invalid secp256k1 private key".to_string())?;
        let public_key = signing_key.verifying_key();
        let enc = public_key.to_encoded_point(false);
        let pub_bytes = enc.as_bytes();
        if pub_bytes.len() != 65 || pub_bytes[0] != 0x04 {
            return Err("unexpected public key encoding".into());
        }
        let hash = keccak256(&pub_bytes[1..]);
        let addr = alloy_primitives::Address::from_slice(&hash[12..]);
        Ok(addr.to_checksum(None))
    }

    #[cfg(not(feature = "evm"))]
    {
        let _ = private_key;
        Err("buldozer_core compiled without feature 'evm'".into())
    }
}
