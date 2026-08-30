# Spacewise Architecture

## Product philosophy

Scan -> Understand -> Recommend -> Review -> Clean -> Undo.
"Know what's using your space -- and what's actually safe to remove."

## Repository layout

```
spacewise/
  Cargo.toml                  workspace root
  crates/
    core/                     spacewise-core -- platform-agnostic engine
    platform-macos/           MacOSAdapter (compiled only on macOS)
    platform-windows/         WindowsAdapter (compiled only on Windows)
    platform-linux/           LinuxAdapter stub (not shipped in V1)
  apps/
    desktop/                  Tauri + React/TS shell
      src/                    frontend (treemap, sunburst, dashboard, cleanup basket)
      src-tauri/               Rust backend (spacewise-desktop), wires core <-> UI via Tauri commands
  docs/
    ARCHITECTURE.md           this file
```

## Engine pipeline (spec section 28)

```
Scanner
  -> StorageModel (FileEntry / DirectoryAggregate, crates/core/src/model.rs)
  -> ClassificationEngine (crates/core/src/classification.rs)
  -> SafetyEngine            (deterministic, rule-based -- never AI-only, spec section 8)
  -> RecommendationEngine    (safety/age/regeneratability/confidence, never size-only)
  -> CleanupPlanner          (builds a reviewable plan; never executes)
  -> USER APPROVAL           (cleanup basket UI)
  -> CleanupExecutor         (Trash/Recycle Bin, or quarantine; validates + re-canonicalizes paths)
  -> Result + HistoryEngine  (undo where possible, historical snapshot recorded)
```

A scanner never decides what to delete. A recommendation never executes deletion. Every
stage before "USER APPROVAL" is read-only.

## PlatformAdapter seam

`spacewise-core::adapter::PlatformAdapter` is the only trait boundary between
platform-agnostic logic and OS-specific code:

- `enrich_metadata` -- OS-specific metadata std::fs can't give us (APFS clones/purgeable,
  NTFS reparse points/hardlink counts)
- `move_to_trash` -- Trash on macOS / Recycle Bin on Windows, never a hard delete unless the
  category requires the quarantine path (spec section 10)
- `is_protected_root` -- hardcoded deletion-allowlist rejection for system-critical paths

`spacewise-platform-macos` and `spacewise-platform-windows` each implement this trait and
are `#![cfg(target_os = "...")]`-gated so only the relevant one compiles per target.
`spacewise-platform-linux` exists as an unimplemented stub purely so the seam is proven
cross-platform from day one, per spec section 2/1.10 -- Linux is not a V1 target.

## Storage database

Embedded SQLite (`crates/core/src/db.rs`) holds Scan, Volume, FileEntry,
DirectoryAggregate, StorageCategory, CleanupCandidate, CleanupAction,
ApplicationAssociation, DuplicateGroup, HistoricalSnapshot, Recommendation
(spec section 27). Aggregated/compressed history is retained; not every scanned
filename is kept forever.

## Build phases (spec section 34)

This scaffold covers **Phase 1** (architecture + repo structure) only. Phases 2-12
(scanner, storage model, desktop shell polish, treemap, classification knowledge base,
safety engine, cleanup basket + trash, developer intelligence, app uninstaller,
history/recommendations, final security/perf pass) are not yet implemented -- every
public function in `crates/core/src/*.rs` is currently `unimplemented!()` or an empty
stub, intentionally, so each phase lands as its own reviewed change.

## Workflow tooling

This repo uses `garrytan/gstack` (installed solo, globally, for Claude Code) as
process scaffolding around each phase gate -- `/plan-ceo-review` and `/plan-eng-review`
before a phase starts, `/review` + `/qa` before a phase merges, `/cso` before the
destructive-operation phases (cleanup basket, uninstaller), `/ship` + `/land-and-deploy`
at release. gstack is not a runtime dependency of the shipped app.
