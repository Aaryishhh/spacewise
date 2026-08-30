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
        unimplemented!("Phase 2: APFS metadata (clones, purgeable, hardlinks)")
    }

    fn move_to_trash(&self, _path: &Path) -> anyhow::Result<()> {
        unimplemented!("Phase 8: NSWorkspace/FileManager trash")
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        const PROTECTED: &[&str] = &["/System", "/private/var/db", "/Library/Apple"];
        PROTECTED.iter().any(|p| path.starts_with(p))
    }
}
