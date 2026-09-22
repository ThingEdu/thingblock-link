//! Served pack root: HTTP for the sandboxed browser, local paths for arduino-cli (lib/firmware).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Single version: contents are pinned to the helper install, so no per-pack version is tracked.
#[derive(Debug)]
pub struct ResourceRoot {
    root: PathBuf,
}

impl ResourceRoot {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let root = path.canonicalize().map_err(|e| {
            Error::Resource(format!(
                "resource root {} is unreadable: {e}",
                path.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(Error::Resource(format!(
                "resource root {} is not a directory",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    /// Left canonical (verbatim on Windows): only `std::fs`-based consumers read it.
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn resolve_lib_dir(&self, pack: &str, lib: &str) -> Result<PathBuf> {
        let dir = self
            .root
            .join(pack)
            .join(lib)
            .canonicalize()
            .map_err(|e| Error::Resource(format!("lib {pack}/{lib} is unreadable: {e}")))?;
        if !dir.starts_with(&self.root) {
            return Err(Error::Resource(format!(
                "lib {pack}/{lib} escapes the resource root"
            )));
        }
        if !dir.is_dir() {
            return Err(Error::Resource(format!(
                "lib {pack}/{lib} is not a directory"
            )));
        }
        // arduino-cli can't read Windows `\\?\` paths; strip only after the canonical containment check.
        Ok(dunce::simplified(&dir).to_path_buf())
    }

    /// A file, not a dir: arduino-cli's `import_file` reads sibling images (bootloader etc.) by name.
    pub fn resolve_firmware_file(&self, pack: &str, file: &str) -> Result<PathBuf> {
        let path = self
            .root
            .join(pack)
            .join(file)
            .canonicalize()
            .map_err(|e| Error::Resource(format!("firmware {pack}/{file} is unreadable: {e}")))?;
        if !path.starts_with(&self.root) {
            return Err(Error::Resource(format!(
                "firmware {pack}/{file} escapes the resource root"
            )));
        }
        if !path.is_file() {
            return Err(Error::Resource(format!(
                "firmware {pack}/{file} is not a file"
            )));
        }
        // arduino-cli can't read Windows `\\?\` paths; strip only after the canonical containment check.
        Ok(dunce::simplified(&path).to_path_buf())
    }
}
