//! LinuxAdapter stub: not shipped in V1, kept so spacewise-core's
//! PlatformAdapter seam is proven cross-platform from day one (spec section 1.10).
#![cfg(target_os = "linux")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::{FileEntry, InstalledApp};
use std::path::Path;

pub struct LinuxAdapter;

impl PlatformAdapter for LinuxAdapter {
    fn enrich_metadata(&self, _entry: &mut FileEntry) -> anyhow::Result<()> {
        Ok(())
    }

    fn move_to_trash(&self, path: &Path) -> anyhow::Result<()> {
        trash::delete(path).map_err(|e| anyhow::anyhow!("failed to move {} to trash: {e}", path.display()))
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        const PROTECTED: &[&str] = &["/proc", "/sys", "/boot"];
        PROTECTED.iter().any(|p| path.starts_with(p))
    }

    fn list_installed_apps(&self) -> anyhow::Result<Vec<InstalledApp>> {
        // Not a V1 target (spec section 1.10) -- no package-manager
        // integration (dpkg/rpm/pacman) yet.
        Ok(Vec::new())
    }
}
