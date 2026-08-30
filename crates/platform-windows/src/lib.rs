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
        // Real allocated-size/reparse-point enrichment (GetCompressedFileSizeW,
        // FSCTL_GET_REPARSE_POINT) is a follow-up; scanner.rs's approximation
        // (allocated_size == logical_size) already gives every entry a
        // usable value, so a no-op here is safe, never a silent panic.
        Ok(())
    }

    fn move_to_trash(&self, path: &Path) -> anyhow::Result<()> {
        trash::delete(path).map_err(|e| anyhow::anyhow!("failed to move {} to Recycle Bin: {e}", path.display()))
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        let s = path.to_string_lossy().to_ascii_lowercase();

        // Filename-based checks: these are protected no matter which drive
        // or directory they live in.
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let name = name.to_ascii_lowercase();
            if name == "pagefile.sys" || name == "hiberfil.sys" || name == "swapfile.sys" {
                return true;
            }
        }
        if s.contains(r"\system volume information") || s.contains(r"\$recycle.bin") {
            return true;
        }

        // The actual Windows install directory, wherever it is (not
        // necessarily C:\) -- SystemRoot is set by Windows itself.
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()).to_ascii_lowercase();
        if s.starts_with(&windir) {
            return true;
        }

        // Any drive's root-level system paths.
        const PROTECTED_SUFFIXES: &[&str] = &[r":\programdata\microsoft\windows"];
        PROTECTED_SUFFIXES.iter().any(|p| s.contains(p))
    }
}
