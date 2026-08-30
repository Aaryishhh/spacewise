//! macOS PlatformAdapter: APFS clones/purgeable storage, Trash via
//! NSWorkspace/FileManager trashItem, Full Disk Access handling,
//! Application Support/Caches/Containers association (spec sections 4, 10, 13, 21).
#![cfg(target_os = "macos")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::{FileEntry, InstalledApp};
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

    /// Best-effort: lists top-level *.app bundles in /Applications. Does not
    /// parse Info.plist for a display name/version, to keep this simple and
    /// correct rather than guessing at plist structure with zero ability to
    /// verify it here (no macOS hardware on this development machine --
    /// UNVERIFIED, needs real macOS testing before shipping).
    fn list_installed_apps(&self) -> anyhow::Result<Vec<InstalledApp>> {
        let mut apps = Vec::new();
        let applications_dir = Path::new("/Applications");
        if !applications_dir.is_dir() {
            return Ok(apps);
        }
        for entry in std::fs::read_dir(applications_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                continue;
            }
            apps.push(InstalledApp {
                name,
                publisher: None,
                install_location: Some(path),
                estimated_size_bytes: None,
                uninstall_command: None,
            });
        }
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(apps)
    }
}
