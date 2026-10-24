//! Cross-process locks. The desktop app can sit in the menu bar for days while the CLI or
//! the browser UI runs too, and the per-process `run_active` flag can't see across them:
//! - the measure lock (one per user) keeps a run or a monitoring cycle from overlapping
//!   another measurement from this machine, in this process or any other;
//! - the monitor lock (one per database) lets a single process record monitoring cycles.
//!
//! Both are advisory OS file locks, which the kernel releases if a process dies.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Paths this process holds, so [`busy_at`] can tell its own holds from another process's.
static HERE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// A held lock, released on drop. Holds no file when the lock file couldn't be opened:
/// measuring then goes ahead unguarded rather than being blocked forever.
pub struct Guard {
    _file: Option<File>,
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(file) = self._file.take() {
            let mut here = HERE.lock().unwrap();
            here.retain(|p| *p != self.path);
            drop(file);
        }
    }
}

/// The lock file if taken, `Some(None)` if it can't be used (unguarded), `None` if someone else holds it.
fn take(path: &Path) -> Option<Option<File>> {
    if let Some(dir) = path.parent() {
        crate::store::make_dir(dir);
    }
    let Ok(file) = OpenOptions::new().create(true).truncate(false).write(true).open(path) else {
        return Some(None);
    };
    match file.try_lock() {
        Ok(()) => Some(Some(file)),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(_)) => Some(None),
    }
}

/// Take the lock unless someone else holds it.
pub fn try_lock(path: &Path) -> Option<Guard> {
    let mut here = HERE.lock().unwrap();
    let file = take(path)?;
    if file.is_some() {
        here.push(path.to_path_buf());
    }
    Some(Guard { _file: file, path: path.to_path_buf() })
}

/// Whether another process holds the lock at `path`. Not this one: its holds are known in-process.
pub(crate) fn busy_at(path: &Path) -> bool {
    let here = HERE.lock().unwrap();
    !here.iter().any(|p| p == path) && take(path).is_none()
}

/// Whether another process is measuring (see [`measure_path`]). Checked where background fetches start.
pub fn busy() -> bool {
    busy_at(&measure_path())
}

/// Take the lock, waiting up to `patience` for its holder to let go.
pub async fn wait(path: &Path, patience: Duration) -> Option<Guard> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if let Some(g) = try_lock(path) {
            return Some(g);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Held while measuring, by runs and by each monitoring cycle.
pub fn measure_path() -> PathBuf {
    crate::store::default_path().with_file_name("measure.lock")
}

/// Held by whichever process records monitoring into the database at `db`.
pub fn monitor_path(db: &Path) -> PathBuf {
    db.with_extension("monitor.lock")
}

/// Held by whichever process records NTP monitoring into the database at `db`.
pub fn ntp_monitor_path(db: &Path) -> PathBuf {
    db.with_extension("ntp.lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_until_dropped() {
        let path = std::env::temp_dir().join(format!("recursa-lock-{:x}.lock", fastrand::u64(..)));
        let held = try_lock(&path).expect("free lock");
        // A second open file description conflicts even within one process.
        assert!(try_lock(&path).is_none());
        drop(held);
        assert!(try_lock(&path).is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn busy_means_held_by_someone_else() {
        let path = std::env::temp_dir().join(format!("recursa-lock-{:x}.lock", fastrand::u64(..)));
        assert!(!busy_at(&path));
        // Our own hold isn't "elsewhere", and checking leaves it held.
        let ours = try_lock(&path).expect("free lock");
        assert!(!busy_at(&path) && try_lock(&path).is_none());
        drop(ours);
        // A hold this process didn't register stands in for another process.
        let other = take(&path);
        assert!(busy_at(&path));
        drop(other);
        assert!(!busy_at(&path) && try_lock(&path).is_some());
        // A lock file that can't be opened never reads as busy.
        assert!(!busy_at(&path.join("no/such/dir")));
        let _ = std::fs::remove_file(&path);
    }
}
