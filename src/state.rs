use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Manages the lifecycle of temporary files created by the plugin.
///
/// This struct holds a thread-safe list of `PathBuf` for all temporary files
/// that have been created and need to be cleaned up, plus a count of shares
/// that are still in progress. It's intended to be managed as a Tauri state.
pub struct PluginTempFileManager {
    managed_files: Arc<Mutex<Vec<PathBuf>>>,
    active_shares: Arc<AtomicUsize>,
}

/// Marks a share as in progress until it is dropped. Mobile platforms track
/// this natively, so it is only used on desktop.
#[cfg(desktop)]
pub(crate) struct ActiveShareGuard(Arc<AtomicUsize>);

#[cfg(desktop)]
impl Drop for ActiveShareGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Default for PluginTempFileManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginTempFileManager {
    pub fn new() -> Self {
        Self {
            managed_files: Arc::new(Mutex::new(Vec::new())),
            active_shares: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Registers a temporary file so it is removed by the next cleanup.
    pub fn track(&self, path: PathBuf) {
        self.lock_files().push(path);
    }

    /// Returns true while at least one share started through the plugin is still running.
    pub fn is_sharing(&self) -> bool {
        self.active_shares.load(Ordering::SeqCst) > 0
    }

    #[cfg(desktop)]
    pub(crate) fn begin_share(&self) -> ActiveShareGuard {
        self.active_shares.fetch_add(1, Ordering::SeqCst);
        ActiveShareGuard(self.active_shares.clone())
    }

    /// Cleans up all files currently managed by this instance.
    ///
    /// This method deletes each tracked file and the per-file directory it was
    /// written to (when that directory is empty), then clears the list. It also
    /// handles a poisoned mutex gracefully by recovering the inner data.
    pub fn cleanup_all_managed_files(&self) {
        let mut files = self.lock_files();
        let mut errors = Vec::new();
        for path in files.drain(..) {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!("Failed to delete file {}: {}", path.display(), e));
                }
            }
            remove_empty_parent(&path);
        }
        if !errors.is_empty() {
            log::error!("Errors during cleanup: {:?}", errors);
        }
    }

    fn lock_files(&self) -> MutexGuard<'_, Vec<PathBuf>> {
        match self.managed_files.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::error!("Mutex was poisoned: {:?}", poisoned);
                poisoned.into_inner()
            }
        }
    }
}

/// Removes the per-file directory a temporary file was written to. `remove_dir`
/// only succeeds for empty directories, so shared directories are left alone.
fn remove_empty_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::PluginTempFileManager;

    #[cfg(desktop)]
    #[test]
    fn share_guard_tracks_in_progress_shares() {
        let manager = PluginTempFileManager::new();
        assert!(!manager.is_sharing());
        let first = manager.begin_share();
        let second = manager.begin_share();
        assert!(manager.is_sharing());
        drop(first);
        assert!(manager.is_sharing());
        drop(second);
        assert!(!manager.is_sharing());
    }

    #[test]
    fn cleanup_removes_files_and_their_empty_directory() {
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path().join("per-file");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hello").unwrap();

        let manager = PluginTempFileManager::new();
        manager.track(file.clone());
        manager.cleanup_all_managed_files();

        assert!(!file.exists());
        assert!(!dir.exists());
        assert!(root.path().exists());
    }
}
