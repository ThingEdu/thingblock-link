//! The served root of installed packs: HTTP static files for the sandboxed browser, and local
//! paths (lib dirs, firmware files) handed straight to arduino-cli.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The directory of installed packs the helper serves. Single version: contents are pinned to
/// the helper install, so no per-pack version is tracked.
#[derive(Debug)]
pub struct ResourceRoot {
    /// Canonicalized at construction, so the route gets a stable absolute path and later
    /// resolution can trust the root exists.
    root: PathBuf,
}

impl ResourceRoot {
    /// Validates and canonicalizes the configured root, failing fast at startup if it is
    /// missing or not a directory, so the route never serves a dangling or relative root.
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

    /// The canonical root for the static-file route. Left verbatim on Windows: only
    /// `std::fs`-based consumers (`ServeDir`) read it.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolves an untrusted browser `{pack, lib}` reference to a library dir under the root,
    /// which the arduino-cli daemon reads in place; rejects `../` escapes and non-directories.
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
        // arduino-cli can't read Windows `\\?\` paths; strip only after the canonical
        // containment check, so the check isn't weakened.
        Ok(dunce::simplified(&dir).to_path_buf())
    }

    /// Resolves an untrusted `{pack, file}` firmware image for `flashFirmware`, with the same
    /// containment rule. A file, since `import_file` reads siblings (bootloader etc.) by name.
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
        // arduino-cli can't read Windows `\\?\` paths; strip only after the canonical
        // containment check, so the check isn't weakened.
        Ok(dunce::simplified(&path).to_path_buf())
    }
}
