//! A directory of a test's own, removed when the test ends.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh, empty directory under the system's temporary directory, unique
/// to this process and call, deleted with everything in it on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates the directory; `label` goes into its name to tell tests apart.
    ///
    /// # Panics
    ///
    /// If the directory cannot be created: a test cannot run without it.
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("oss-{label}-{}-{n}", std::process::id()));
        // A leftover from an earlier process with the same id.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create a temporary directory");
        Self { path }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_directory_is_fresh_and_removed_on_drop() {
        let a = TempDir::new("t");
        let b = TempDir::new("t");
        assert_ne!(a.path(), b.path());
        std::fs::write(a.path().join("f"), b"x").unwrap();
        let kept = a.path().to_path_buf();
        drop(a);
        assert!(!kept.exists());
        assert!(b.path().is_dir());
    }
}
