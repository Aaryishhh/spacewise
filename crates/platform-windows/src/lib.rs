//! Windows PlatformAdapter: NTFS reparse points/junctions/hardlinks, Recycle
//! Bin via SHFileOperation/IFileOperation, WinSxS/pagefile/System Volume
//! Information exclusion, AppData association (spec sections 4, 10, 13, 20).
#![cfg(target_os = "windows")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::FileEntry;
use std::path::Path;

pub struct WindowsAdapter;

impl PlatformAdapter for WindowsAdapter {
    fn enrich_metadata(&self, _entry: &mut FileEntry) -> anyhow::Result<()> {
        unimplemented!("Phase 2: NTFS metadata (reparse points, hardlinks)")
    }

    fn move_to_trash(&self, _path: &Path) -> anyhow::Result<()> {
        unimplemented!("Phase 8: IFileOperation recycle bin")
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        let s = path.to_string_lossy().to_ascii_lowercase();
        const PROTECTED: &[&str] = &[
            r"c:\windows",
            r"c:\system volume information",
            r"c:\pagefile.sys",
            r"c:\hiberfil.sys",
        ];
        PROTECTED.iter().any(|p| s.starts_with(p))
    }
}
