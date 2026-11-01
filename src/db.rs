use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
use std::time::Duration;

pub async fn init_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
        // 512 KiB of page cache per connection instead of 2 MiB: about 5 MiB less RSS with
        // 100 monitors and a week of checks, same dashboard time (the OS caches the file).
        .pragma("cache_size", "-512");

    // An in-memory database lives only as long as its connection: keep exactly one, forever.
    let pool_options = if database_url.contains(":memory:") {
        SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
    } else {
        SqlitePoolOptions::new().max_connections(5)
    };

    let pool = pool_options.connect_with(options).await?;

    tracing::info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("Database migrations applied successfully.");

    Ok(pool)
}

/// A consistent copy of the database, taken while it runs, opened and already unlinked: the file
/// is gone once the handle is dropped. Written next to the database (a mounted volume has room
/// for it), in the temporary directory for an in-memory one.
pub async fn backup(pool: &SqlitePool) -> Result<tokio::fs::File, BackupError> {
    // One copy at a time: each one takes the size of the database on disk.
    static RUNNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = RUNNING.acquire().await.expect("never closed");

    let main_file: String =
        sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
            .fetch_one(pool)
            .await?;
    let dir = std::path::Path::new(&main_file)
        .parent()
        .filter(|_| !main_file.is_empty())
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    let path = dir.join(format!(".backup-{}.db", crate::auth::random_token()));

    let copied = sqlx::query("VACUUM INTO ?")
        .bind(path.to_string_lossy().as_ref())
        .execute(pool)
        .await;
    let file = match copied {
        Ok(_) => tokio::fs::File::open(&path)
            .await
            .map_err(BackupError::from),
        Err(e) => Err(e.into()),
    };
    // Unlinked whatever happened; an open handle still reads it.
    let _ = tokio::fs::remove_file(&path).await;
    file
}

#[derive(Debug)]
pub enum BackupError {
    Sql(sqlx::Error),
    Io(std::io::Error),
}

impl From<sqlx::Error> for BackupError {
    fn from(e: sqlx::Error) -> Self {
        Self::Sql(e)
    }
}

impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "database copy failed: {e}"),
            Self::Io(e) => write!(f, "reading the database copy failed: {e}"),
        }
    }
}

impl std::error::Error for BackupError {}
