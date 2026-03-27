use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

pub async fn open_sqlite_pool(
    db_url: &str,
    db_key: Option<&str>,
    max_connections: u32,
) -> Result<SqlitePool, sqlx::Error> {
    let mut opts = SqliteConnectOptions::from_str(db_url)?;
    opts = opts.create_if_missing(true);

    if let Some(key) = db_key {
        // SQLCipher expects a quoted string.
        let escaped = key.replace("'", "''");
        opts = opts.pragma("key", format!("'{escaped}'"));
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await?;

    // Baseline pragmas for worker DB usage.
    sqlx::query("PRAGMA journal_mode = WAL;").execute(&pool).await?;
    sqlx::query("PRAGMA busy_timeout = 5000;").execute(&pool).await?;
    sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await?;

    Ok(pool)
}

pub async fn init_wallet_db(
    db_url: &str,
    db_key: Option<&str>,
    max_connections: u32,
) -> Result<SqlitePool, sqlx::Error> {
    let pool = open_sqlite_pool(db_url, db_key, max_connections).await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            proxy TEXT,
            proxy_status TEXT NOT NULL DEFAULT 'OK',
            completed INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

pub async fn exec(pool: &SqlitePool, sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(sql).execute(pool).await?;
    Ok(())
}

pub async fn exec_all(pool: &SqlitePool, ddls: &[&str]) -> Result<(), sqlx::Error> {
    for ddl in ddls {
        exec(pool, ddl).await?;
    }
    Ok(())
}

pub async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column_name: &str,
    column_decl: &str,
) -> Result<bool, sqlx::Error> {
    let sql = format!("PRAGMA table_info({table});");
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    let exists = rows
        .iter()
        .any(|r| r.try_get::<String, _>("name").is_ok_and(|n| n == column_name));
    if exists {
        return Ok(false);
    }

    let alter = format!("ALTER TABLE {table} ADD COLUMN {column_decl};");
    sqlx::query(&alter).execute(pool).await?;
    Ok(true)
}
