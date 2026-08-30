//! Classification engine: the storage knowledge base (spec section 32). Each
//! rule recognises one real-world storage location by a path-matching
//! predicate and attaches a `StorageCategoryDef` explaining what it is, why
//! it exists, whether it regenerates, and whether removing it is reversible.
//!
//! Rules are intentionally simple substring/suffix matches on the
//! OS-normalised path, not regex or heuristics -- deterministic and
//! auditable, per spec section 8 (the safety engine downstream must be able
//! to trust exactly why a path was classified a given way).

use crate::model::{SafetyLevel, StorageCategoryDef};
use std::path::Path;

pub struct ClassificationRule {
    pub category: StorageCategoryDef,
    /// Path segments (case-insensitive) that, if all present as path
    /// components in order, match this rule. E.g. ["Library", "Developer",
    /// "Xcode", "DerivedData"] matches .../Library/Developer/Xcode/DerivedData.
    matcher: fn(&str) -> bool,
}

pub struct ClassificationEngine {
    rules: Vec<ClassificationRule>,
}

impl ClassificationEngine {
    pub fn new() -> Self {
        Self { rules: default_rules() }
    }

    /// First matching rule wins; rules are ordered most-specific-first so a
    /// nested match (e.g. node_modules/.cache inside node_modules) is not
    /// shadowed by its broader parent rule.
    pub fn classify(&self, path: &Path) -> Option<&StorageCategoryDef> {
        let normalized = path.to_string_lossy().replace('\\', "/").to_lowercase();
        self.rules
            .iter()
            .find(|rule| (rule.matcher)(&normalized))
            .map(|rule| &rule.category)
    }

    pub fn rules(&self) -> &[ClassificationRule] {
        &self.rules
    }
}

impl Default for ClassificationEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn cat(
    id: &str,
    display_name: &str,
    description: &str,
    what_happens: &str,
    regeneratable: bool,
    reversible: bool,
    safety: SafetyLevel,
) -> StorageCategoryDef {
    StorageCategoryDef {
        id: id.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        what_happens_if_removed: what_happens.to_string(),
        regeneratable,
        reversible,
        safety,
    }
}

macro_rules! rule {
    ($needle:expr, $category:expr) => {
        ClassificationRule {
            category: $category,
            matcher: |p: &str| p.contains($needle),
        }
    };
}

fn default_rules() -> Vec<ClassificationRule> {
    use SafetyLevel::*;
    vec![
        // -- developer tooling (spec section 12) ---------------------------
        rule!("/node_modules/.cache", cat("npm-cache-nested", "Package Manager Build Cache", "Cached build output inside node_modules.", "Regenerated automatically on next build.", true, true, Safe)),
        rule!("/node_modules", cat("node-modules", "node_modules", "Installed JavaScript/TypeScript dependencies for a project.", "Reinstalled automatically by running npm/pnpm/yarn install again.", true, true, Safe)),
        rule!("/target/debug", cat("cargo-target-debug", "Cargo Build Output (debug)", "Compiled Rust build artifacts for a project.", "Rebuilt automatically on the next `cargo build`.", true, true, Safe)),
        rule!("/target/release", cat("cargo-target-release", "Cargo Build Output (release)", "Compiled Rust build artifacts for a project.", "Rebuilt automatically on the next `cargo build --release`.", true, true, Safe)),
        rule!(".cargo/registry", cat("cargo-registry-cache", "Cargo Registry Cache", "Downloaded Rust crate sources and index, shared across all Rust projects.", "Re-downloaded automatically the next time it is needed.", true, true, Safe)),
        rule!(".npm/_cacache", cat("npm-cache", "npm Cache", "Downloaded package archives npm keeps to speed up future installs.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!("/.pnpm-store", cat("pnpm-store", "pnpm Store", "pnpm's shared content-addressable package store.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!("appdata/local/yarn/cache", cat("yarn-cache", "Yarn Cache", "Downloaded package archives Yarn keeps to speed up future installs.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!(".cache/pip", cat("pip-cache", "pip Cache", "Downloaded Python package archives.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!("/venv", cat("python-venv", "Python Virtual Environment", "An isolated Python environment with installed packages for one project.", "Recreated with `python -m venv` and reinstalling requirements.", true, true, Review)),
        rule!(".gradle/caches", cat("gradle-cache", "Gradle Cache", "Downloaded dependencies and build cache for Gradle projects.", "Re-downloaded automatically on the next Gradle build.", true, true, Safe)),
        rule!(".m2/repository", cat("maven-repo", "Maven Local Repository", "Downloaded Java dependencies for Maven projects.", "Re-downloaded automatically on the next Maven build.", true, true, Safe)),
        rule!("jetbrains", cat("jetbrains-cache", "JetBrains IDE Cache", "Index and cache files for IntelliJ/PyCharm/WebStorm/etc.", "Rebuilt automatically the next time the IDE opens the project.", true, true, Safe)),
        rule!("code/cachedextensionvsixs", cat("vscode-cache", "VS Code Extension Cache", "Cached VS Code extension install packages.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!("/.docker", cat("docker-data", "Docker Data", "Container images, volumes, and build cache.", "Images are re-pulled/rebuilt; volumes holding real data are lost -- review before removing.", false, false, Advanced)),
        rule!("/.git/", cat("git-internals", "Git Repository Data", "A project's version-control history and objects.", "Deleting this destroys the repository's local history -- only its checked-out files remain.", false, false, NeverAutoDelete)),
        rule!("/dist", cat("build-output-dist", "Build Output", "Compiled/bundled output from a frontend or library build.", "Regenerated by re-running the project's build command.", true, true, Safe)),
        rule!("android/build", cat("android-build", "Android Build Artifacts", "Compiled Android build outputs.", "Regenerated on the next Gradle/Android Studio build.", true, true, Safe)),

        // -- macOS developer tools (spec section 12/21) --------------------
        rule!("library/developer/xcode/deriveddata", cat("xcode-derived-data", "Xcode Derived Data", "Temporary build files generated by Xcode.", "Xcode recreates anything it needs the next time you build a project.", true, true, Safe)),
        rule!("library/developer/xcode/archives", cat("xcode-archives", "Xcode Archives", "Archived builds created for App Store submission or ad-hoc distribution.", "These are your only local copy of a specific archived build -- review before removing.", false, true, Review)),
        rule!("library/developer/coresimulator", cat("xcode-simulators", "Xcode Simulators", "iOS/watchOS/tvOS simulator runtimes and their device data.", "Simulator runtimes are redownloaded; a simulator's app data is lost.", true, true, Review)),
        rule!("library/developer/xcode/ios devicesupport", cat("xcode-device-support", "Xcode Device Support", "Debug symbols for physical devices you have connected for development.", "Redownloaded automatically the next time you connect that device.", true, true, Safe)),
        rule!("library/caches/org.swift.swiftpm", cat("spm-cache", "Swift Package Manager Cache", "Downloaded Swift package sources.", "Re-downloaded automatically on the next build.", true, true, Safe)),
        rule!("library/caches/homebrew", cat("homebrew-cache", "Homebrew Cache", "Downloaded installer packages Homebrew keeps after installing.", "Re-downloaded automatically if needed again.", true, true, Safe)),
        rule!("/pods/", cat("cocoapods", "CocoaPods", "Installed iOS/macOS dependency sources for a project.", "Reinstalled automatically by running `pod install` again.", true, true, Safe)),

        // -- browser caches (spec section 20/21) ---------------------------
        rule!("google/chrome/default/cache", cat("chrome-cache", "Chrome Cache", "Temporary web content Chrome stores to speed up page loads.", "Rebuilt automatically as you browse; you stay logged in and keep bookmarks/history.", true, true, Safe)),
        rule!("mozilla/firefox", cat("firefox-cache", "Firefox Cache", "Temporary web content Firefox stores to speed up page loads.", "Rebuilt automatically as you browse.", true, true, Safe)),
        rule!("microsoft/edge/user data/default/cache", cat("edge-cache", "Edge Cache", "Temporary web content Edge stores to speed up page loads.", "Rebuilt automatically as you browse.", true, true, Safe)),

        // -- Windows-specific (spec section 20) -----------------------------
        rule!("appdata/local/temp", cat("windows-temp", "Windows Temporary Files", "Temporary files created by Windows and installed applications.", "Files currently in use are skipped automatically; the rest are safe to remove.", true, true, Safe)),
        rule!(":/windows.old", cat("windows-old", "Previous Windows Installation", "A backup of your prior Windows installation kept after an upgrade.", "You lose the ability to roll back to the previous Windows version.", false, false, Review)),
        rule!("softwaredistribution/download", cat("windows-update-cache", "Windows Update Cache", "Downloaded Windows Update installer files.", "Re-downloaded automatically the next time updates are needed.", true, true, Safe)),
        rule!("deliveryoptimization", cat("delivery-optimization", "Delivery Optimization Files", "Cached update/app data Windows shares with other devices on your network.", "Re-downloaded automatically as needed.", true, true, Safe)),
        rule!("appdata/local/microsoft/windows/explorer", cat("thumbnail-cache", "Thumbnail Cache", "Cached thumbnail images for File Explorer.", "Regenerated automatically as folders are browsed.", true, true, Safe)),
        rule!("appdata/local/d3dscache", cat("directx-shader-cache", "DirectX Shader Cache", "Precompiled graphics shaders cached by DirectX.", "Regenerated automatically, causing a brief one-time recompile on next use.", true, true, Safe)),
        rule!("$recycle.bin", cat("recycle-bin", "Recycle Bin", "Files you have already deleted, kept for possible recovery.", "Permanently deletes files you may still want -- review before emptying.", false, false, Review)),
        rule!("system volume information", cat("system-volume-information", "System Volume Information", "Windows system data including restore points.", "System-managed; not meant for direct user deletion.", false, false, NeverAutoDelete)),
        rule!("pagefile.sys", cat("pagefile", "Virtual Memory Paging File", "Windows' virtual memory swap file.", "Required for normal system operation.", false, false, NeverAutoDelete)),
        rule!("hiberfil.sys", cat("hibernation-file", "Hibernation File", "Stores system state for hibernate/fast startup.", "Required for hibernate and Windows Fast Startup to work.", false, false, NeverAutoDelete)),

        // -- generic (fallback categories) ---------------------------------
        rule!("/downloads/", cat("downloads", "Downloads", "Files you have downloaded from the web or received elsewhere.", "Permanently removes files you downloaded -- review before removing.", false, true, Review)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_xcode_derived_data_as_safe() {
        let engine = ClassificationEngine::new();
        let cat = engine
            .classify(&PathBuf::from("/Users/x/Library/Developer/Xcode/DerivedData/App-abc"))
            .expect("should match");
        assert_eq!(cat.id, "xcode-derived-data");
        assert_eq!(cat.safety, SafetyLevel::Safe);
    }

    #[test]
    fn classifies_windows_pagefile_as_never_delete() {
        let engine = ClassificationEngine::new();
        let cat = engine.classify(&PathBuf::from(r"C:\pagefile.sys")).expect("should match");
        assert_eq!(cat.safety, SafetyLevel::NeverAutoDelete);
    }

    #[test]
    fn unrecognised_path_is_unclassified() {
        let engine = ClassificationEngine::new();
        assert!(engine.classify(&PathBuf::from("/Users/x/Documents/report.docx")).is_none());
    }
}
