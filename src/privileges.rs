//! Started as root (the Docker image's default), the binary hands the database files to an
//! unprivileged user and becomes that user before doing anything else, as the entrypoints of
//! official images do with `chown` and `gosu`. A bind-mounted directory that Docker created as
//! root then works without a manual `chown`. Started as any other user, it changes nothing.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The database file of a `sqlite:` URL; `None` for an in-memory database.
pub fn database_file(database_url: &str) -> Option<PathBuf> {
    let rest = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))?;
    let path = rest.split('?').next().unwrap_or_default();
    (!path.is_empty() && path != ":memory:").then(|| PathBuf::from(path))
}

/// Gives the database's directory, the database and its journals to `uid:gid`, creating the
/// directory if needed. Only what SQLite writes: other files in the directory are left alone.
pub fn hand_over_database(database_url: &str, uid: u32, gid: u32) -> io::Result<()> {
    let Some(file) = database_file(database_url) else {
        return Ok(());
    };
    let dir = match file.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&dir)?;
    let mut paths = vec![dir];
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = file.clone().into_os_string();
        name.push(suffix);
        paths.push(PathBuf::from(name));
    }
    for path in paths {
        give(&path, uid, gid)?;
    }
    Ok(())
}

fn give(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if (meta.uid(), meta.gid()) == (uid, gid) {
        return Ok(());
    }
    // A symlink is not followed: a planted link must not hand over another file.
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// Becomes `uid:gid` for good: no supplementary groups, and no way back to root.
pub fn become_user(uid: u32, gid: u32) -> io::Result<()> {
    // SAFETY: plain system calls with valid arguments; glibc and musl apply them to every thread.
    // Called before the async runtime starts anyway.
    unsafe {
        if libc::setgroups(0, std::ptr::null()) != 0
            || libc::setgid(gid) != 0
            || libc::setuid(uid) != 0
        {
            return Err(io::Error::last_os_error());
        }
        if libc::setuid(0) == 0 {
            return Err(io::Error::other("root could be regained after dropping it"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_file_reads_sqlite_urls() {
        let file = |url| database_file(url).map(|p| p.display().to_string());
        assert_eq!(
            file("sqlite:///data/uptime.db").as_deref(),
            Some("/data/uptime.db")
        );
        assert_eq!(file("sqlite://uptime.db").as_deref(), Some("uptime.db"));
        assert_eq!(
            file("sqlite:data/u.db?mode=rwc").as_deref(),
            Some("data/u.db")
        );
        assert_eq!(file("sqlite::memory:"), None);
        assert_eq!(file("sqlite://:memory:"), None);
        assert_eq!(file("postgres://x"), None);
    }

    #[test]
    fn hand_over_creates_the_directory_and_skips_what_is_already_owned() {
        let dir = std::env::temp_dir().join(format!("uptime-privileges-{}", std::process::id()));
        let file = dir.join("nested/uptime.db");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        // Same owner: nothing to change, so no privilege is needed.
        let url = format!("sqlite://{}", file.display());
        hand_over_database(&url, meta.uid(), meta.gid()).unwrap();
        let fresh = dir.join("fresh/uptime.db");
        hand_over_database(
            &format!("sqlite://{}", fresh.display()),
            meta.uid(),
            meta.gid(),
        )
        .unwrap();
        assert!(fresh.parent().unwrap().is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
