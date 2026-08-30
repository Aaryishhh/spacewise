//! Windows PlatformAdapter: NTFS reparse points/junctions/hardlinks, Recycle
//! Bin via SHFileOperation/IFileOperation, WinSxS/pagefile/System Volume
//! Information exclusion, AppData association (spec sections 4, 10, 13, 20).
#![cfg(target_os = "windows")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::{FileEntry, InstalledApp};
use std::path::{Path, PathBuf};
use winreg::enums::*;
use winreg::{RegKey, HKEY};

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

    fn list_installed_apps(&self) -> anyhow::Result<Vec<InstalledApp>> {
        // Standard Windows uninstall registry locations: HKLM covers
        // machine-wide installs (both native and 32-bit-on-64-bit via
        // WOW6432Node), HKCU covers per-user installs.
        const ROOTS: &[(HKEY, &str)] = &[
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        ];

        let mut apps = Vec::new();
        for (hive, subkey_path) in ROOTS {
            let root = RegKey::predef(*hive);
            let Ok(uninstall_key) = root.open_subkey(subkey_path) else { continue };
            for name in uninstall_key.enum_keys().filter_map(|k| k.ok()) {
                let Ok(entry) = uninstall_key.open_subkey(&name) else { continue };

                // SystemComponent=1 marks a shared runtime/driver, not a
                // user-facing app -- exclude it from the uninstaller list.
                let is_system_component: u32 = entry.get_value("SystemComponent").unwrap_or(0);
                if is_system_component == 1 {
                    continue;
                }
                let Ok(display_name) = entry.get_value::<String, _>("DisplayName") else { continue };
                if display_name.trim().is_empty() {
                    continue;
                }

                let publisher = entry.get_value::<String, _>("Publisher").ok();
                let install_location = entry
                    .get_value::<String, _>("InstallLocation")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .map(PathBuf::from);
                // EstimatedSize is stored in KB.
                let estimated_size_bytes = entry.get_value::<u32, _>("EstimatedSize").ok().map(|kb| kb as u64 * 1024);
                let uninstall_command = entry.get_value::<String, _>("UninstallString").ok();

                apps.push(InstalledApp {
                    name: display_name,
                    publisher,
                    install_location,
                    estimated_size_bytes,
                    uninstall_command,
                });
            }
        }

        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        apps.dedup_by(|a, b| a.name == b.name);
        Ok(apps)
    }
}
