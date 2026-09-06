//! The served resource root — a single-version directory of installed packs
//! (block defs, generators, device manifests, vendored library sources) that the
//! helper exposes to the editor as static files.
//!
//! Flow 1 (resource serving): the static-file route serves this directory so the
//! browser can `import()` pack JS over HTTP. The browser is sandboxed and cannot
//! read the helper's filesystem, so an HTTP URL is the only handle it can use —
//! the machine path means nothing inside the page. This module owns just the
//! root's identity: validate and canonicalize it once at startup (fail fast if
//! absent) and hand its path to the route.
//!
//! Flow 2 (compile): [`ResourceRoot::resolve_lib_dir`] turns a browser-supplied
//! `{pack, lib}` reference into a local library directory the arduino-cli daemon
//! reads in place. That consumer *is* a local process, so it uses the path
//! directly — the asymmetry that makes Flow 1 an HTTP serve and Flow 2 a
//! filesystem read of the same root.
//!
//! Flow 3 (firmware): [`ResourceRoot::resolve_firmware_file`] turns a browser-supplied
//! `{pack, file}` reference into a pack-shipped firmware image for `flashFirmware`, the same
//! local-filesystem-read shape as Flow 2. Unlike Flow 2's directory, the resolved file is never
//! read in place — the bridge stages a copy of it elsewhere before flashing, since arduino-cli's
//! upload writes sibling files next to whatever it flashes and this directory must stay read-only.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The directory of installed packs the helper serves. Single version: its
/// contents are pinned to the helper install, so no per-pack version is tracked.
#[derive(Debug)]
pub struct ResourceRoot {
    /// Canonicalized at construction, so the static route gets a stable absolute
    /// path and later lib resolution can trust the root exists.
    root: PathBuf,
}

impl ResourceRoot {
    /// Validate and canonicalize the configured root, failing fast at startup with
    /// an actionable message if it is missing or is not a directory. Resolving the
    /// path here means the static route never serves through a dangling or
    /// relative root.
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

    /// The canonical root directory, for the static-file route to serve from.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolve a browser-supplied `{pack, lib}` reference to a local library
    /// directory under this root, for the arduino-cli daemon to read in place.
    ///
    /// The reference crosses the WS boundary, so it is untrusted: the path is
    /// canonicalized and asserted to stay inside the root (defeating `../`
    /// traversal that escapes it) and to be a directory. A missing or escaping
    /// reference is an actionable [`Error::Resource`] naming the offending pack
    /// and lib — never a silent miss.
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
        Ok(dir)
    }

    /// Resolve a prebuilt firmware image a device pack ships, for `flashFirmware`. The same
    /// containment rule as `resolve_lib_dir`: canonicalize, then refuse anything that leaves the
    /// root — the pack and file both come from the browser. Resolves to a file rather than a
    /// directory because arduino-cli's `import_file` names the app image, and reads its siblings
    /// (bootloader, partition table) from the same directory by name.
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
        Ok(path)
    }
}
