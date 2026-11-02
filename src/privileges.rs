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
    if !is_dedicated(&dir, &file, uid, gid)? {
        return Err(io::Error::other(format!(
            "DATABASE_URL points into {}, a directory holding other files: put the database in a \
             directory of its own, such as sqlite:///data/uptime.db",
            dir.display()
        )));
    }
    give(&dir, uid, gid)?;
    for path in database_files(&file) {
        give(&path, uid, gid)?;
    }
    Ok(())
}

/// Whether the directory can be handed over: already owned by `uid:gid`, holding the database
/// already, or empty (a fresh volume, a bind mount Docker just created). Never `/`, `/etc` and the
/// like, which `DATABASE_URL=sqlite:///uptime.db` or a relative path would otherwise give away.
fn is_dedicated(dir: &Path, file: &Path, uid: u32, gid: u32) -> io::Result<bool> {
    if std::fs::canonicalize(dir)? == Path::new("/") {
        return Ok(false);
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if (meta.uid(), meta.gid()) == (uid, gid) || std::fs::symlink_metadata(file).is_ok() {
        return Ok(true);
    }
    for entry in std::fs::read_dir(dir)? {
        // A freshly formatted ext4 volume has one.
        if entry?.file_name() != "lost+found" {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The database and its journals.
fn database_files(file: &Path) -> Vec<PathBuf> {
    ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let mut name = file.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect()
}

/// Files created from now on are readable by their owner only: the database holds the password
/// hash and every channel's secrets.
pub fn restrict_new_files() {
    // SAFETY: umask has no preconditions and cannot fail.
    unsafe {
        libc::umask(0o077);
    }
}

/// Makes the existing database and journals readable by their owner only, for files created
/// before `restrict_new_files` existed. Symlinks are left alone.
pub fn restrict_database(database_url: &str) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Some(file) = database_file(database_url) else {
        return Ok(());
    };
    for path in database_files(&file) {
        // Opened without following a symlink, then changed through the handle: a link swapped in
        // after a check cannot redirect the chmod.
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path);
        let handle = match opened {
            Ok(handle) => handle,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            // ELOOP: a symlink, left alone.
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => continue,
            Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
        };
        let meta = handle.metadata()?;
        if meta.is_file() && meta.mode() & 0o077 != 0 {
            handle
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        }
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

    #[test]
    fn the_root_directory_is_never_handed_over() {
        let err = hand_over_database("sqlite:///uptime.db", 65532, 65532).unwrap_err();
        assert!(err.to_string().contains("DATABASE_URL"), "{err}");
    }

    #[test]
    fn a_shared_directory_is_never_handed_over() {
        let dir = std::env::temp_dir().join(format!("uptime-shared-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("passwd"), b"").unwrap();
        let url = format!("sqlite://{}/uptime.db", dir.display());
        let err = hand_over_database(&url, 65532, 65532).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(err.to_string().contains("DATABASE_URL"), "{err}");
    }

    #[test]
    fn new_files_are_private() {
        restrict_new_files();
        let file = std::env::temp_dir().join(format!("uptime-umask-{}", std::process::id()));
        std::fs::write(&file, b"").unwrap();
        let mode = std::fs::metadata(&file).unwrap().mode() & 0o777;
        std::fs::remove_file(&file).unwrap();
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn existing_database_files_become_private_without_following_links() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("uptime-restrict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("uptime.db");
        let wal = dir.join("uptime.db-wal");
        let target = dir.join("elsewhere");
        for path in [&file, &wal, &target] {
            std::fs::write(path, b"").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::os::unix::fs::symlink(&target, dir.join("uptime.db-shm")).unwrap();

        restrict_database(&format!("sqlite://{}", file.display())).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&wal), 0o600);
        assert_eq!(mode(&target), 0o644, "a symlink is not followed");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
