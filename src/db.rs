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
pub async fn backup(pool: &SqlitePool) -> Result<Backup, BackupError> {
    // Its own task: a request dropped mid-copy (timeout, closed connection) does not stop it
    // before the copy is unlinked, and the next copy waits until this one is done.
    let pool = pool.clone();
    tokio::spawn(async move { copy(&pool).await })
        .await
        .map_err(|e| BackupError::Io(std::io::Error::other(e)))?
}

async fn copy(pool: &SqlitePool) -> Result<Backup, BackupError> {
    // One copy at a time, until it is read: each one takes the size of the database on disk.
    static RUNNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let permit = RUNNING.acquire().await.expect("never closed");

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
    Ok(Backup {
        file: file?,
        _permit: permit,
    })
}

/// A database copy; the next one waits until this one is dropped.
pub struct Backup {
    pub file: tokio::fs::File,
    _permit: tokio::sync::SemaphorePermit<'static>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn copies_in(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".backup-")
            })
            .count()
    }

    /// A request timeout drops the handler while the copy is being written: the copy must still
    /// be removed, and the next backup must wait for it.
    #[tokio::test]
    async fn an_abandoned_backup_leaves_no_copy_behind() {
        let dir = std::env::temp_dir().join(format!("uptime-backup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = init_pool(&format!("sqlite://{}/uptime.db", dir.display()))
            .await
            .unwrap();

        // Large enough for the copy to take a while.
        sqlx::query(
            "CREATE TABLE filler AS WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n \
             WHERE i < 20000) SELECT randomblob(4096) AS b FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Dropped once the copy is being written, as a request timeout would.
        let abandoned = backup(&pool);
        tokio::select! {
            _ = abandoned => panic!("the copy finished before it could be abandoned"),
            () = async {
                while copies_in(&dir) == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            } => {}
        }
        let next = tokio::time::timeout(Duration::from_secs(10), backup(&pool))
            .await
            .expect("the abandoned copy releases its turn")
            .unwrap();
        drop(next);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(copies_in(&dir), 0);
        pool.close().await;
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
