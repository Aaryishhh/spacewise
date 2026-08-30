//! PlatformAdapter trait: the one seam between spacewise-core and OS-specific
//! code (spec section 2). Implemented by spacewise-platform-{macos,windows,linux}.

use crate::model::FileEntry;
use std::path::Path;

pub trait PlatformAdapter: Send + Sync {
    /// OS-specific metadata spacewise-core cannot get from std::fs alone
    /// (APFS clone/purgeable flags, NTFS reparse points, hardlink counts, etc).
    fn enrich_metadata(&self, entry: &mut FileEntry) -> anyhow::Result<()>;

    /// Move a path to the OS trash/recycle bin rather than a hard delete.
    fn move_to_trash(&self, path: &Path) -> anyhow::Result<()>;

    /// Whether this path is inside a protected system root the deletion
    /// allowlist must always reject (spec section 10).
    fn is_protected_root(&self, path: &Path) -> bool;
}
