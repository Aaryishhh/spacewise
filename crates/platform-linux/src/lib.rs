//! LinuxAdapter stub: not shipped in V1, kept so spacewise-core's
//! PlatformAdapter seam is proven cross-platform from day one (spec section 1.10).
#![cfg(target_os = "linux")]

use spacewise_core::adapter::PlatformAdapter;
use spacewise_core::model::FileEntry;
use std::path::Path;

pub struct LinuxAdapter;

impl PlatformAdapter for LinuxAdapter {
    fn enrich_metadata(&self, _entry: &mut FileEntry) -> anyhow::Result<()> {
        unimplemented!("post-V1")
    }

    fn move_to_trash(&self, _path: &Path) -> anyhow::Result<()> {
        unimplemented!("post-V1: XDG trash spec")
    }

    fn is_protected_root(&self, path: &Path) -> bool {
        const PROTECTED: &[&str] = &["/proc", "/sys", "/boot"];
        PROTECTED.iter().any(|p| path.starts_with(p))
    }
}
