use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::debug;

static SEQ: AtomicU64 = AtomicU64::new(0);

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best-effort: the OS temp sweep is the backstop.
        if let Err(e) = std::fs::remove_dir_all(&self.path) {
            debug!(path = %self.path.display(), error = %e, "failed to remove temp dir");
        }
    }
}
