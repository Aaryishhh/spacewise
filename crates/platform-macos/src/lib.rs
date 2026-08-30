//! macOS PlatformAdapter: APFS clones/purgeable storage, Trash via
//! NSWorkspace/FileManager trashItem, Full Disk Access handling,
//! Application Support/Caches/Containers association (spec sections 4, 10, 13, 21).
#![cfg(target_os = "macos")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::FileEntry;
use std::path::Path;

pub struct MacOSAdapter;

impl PlatformAdapter for MacOSAdapter {
    fn enrich_metadata(&self, _entry: &mut FileEntry) -> anyhow::Result<()> {
        // Real APFS clone/purgeable-storage enrichment (via
        // getattrlist/F_LOG2PHYS) is a follow-up; scanner.rs's approximation
        // already gives every entry a usable allocated_size, so a no-op here
        // is safe, never a silent panic. NOTE: this crate is not compiled or
        // tested on this development machine (Windows) -- it only builds on
        // macOS CI/hardware, so treat it as unverified until then.
        Ok(())
    }

    fn move_to_trash(&self, path: &Path) -> anyhow::Result<()> {
        trash::delete(path).map_err(|e| anyhow::anyhow!("failed to move {} to Trash: {e}", path.display()))
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        const PROTECTED: &[&str] = &[
            "/System",
            "/private/var/db",
            "/Library/Apple",
            "/usr",
            "/bin",
            "/sbin",
            "/dev",
            "/Volumes/Preboot",
        ];
        PROTECTED.iter().any(|p| path.starts_with(p))
    }
}
