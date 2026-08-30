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

## Proposal: scan retention model (not yet implemented)

Every scan currently persists a full FileEntry row per file forever. At
measured ~500-800 bytes/row (see session performance notes), a user who
scans their whole drive weekly for a year would accumulate 50+ full copies
of a multi-million-row table -- multiple GB of SQLite for a "lightweight
local cache" database. This needs a decision before it becomes a real
problem, not after.

Proposed model (three tiers, decreasing detail with age):

1. **Latest scan per root** -- keep the full `file_entries` table as today.
   This is what Storage/Treemap/Large Files/Duplicates need to stay fully
   interactive and searchable.
2. **Previous scans (recent history, e.g. last 5-10)** -- drop `file_entries`
   rows, keep `directory_aggregates` (already far smaller: thousands of
   rows, not millions) plus `historical_snapshots` (already exists,
   category totals only). This still answers "what changed in this
   directory since last time" at the folder level, just not "which
   individual file changed."
3. **Older scans** -- `historical_snapshots` only (already what
   `HistoryEngine` reads for the growth-diagnostic feature) -- category
   totals and overall size, nothing path-level. This is enough for "you
   gained 38 GB this month" without retaining any per-file data at all.

Mechanically: a background sweep (on app startup, or after each new scan
completes) that removes file_entries rows for scans past tier 1, and
eventually directory_aggregates rows for scans past tier 2. Needs a
decision on tier thresholds (scan count? age? both?) before implementation
-- that is a product call, not an engineering one, hence proposal-only here.

## Proposal: incremental (repeat) scan strategy (not yet implemented)

Full re-traversal on every scan is correct but wasteful when most of the
tree has not changed since last time. The current schema does not block
this -- FileEntry already carries modified_at, logical_size, and (on Unix)
a real filesystem_id (dev:inode), which are exactly the signals an
incremental scan needs.

Sketch, not committed: for a repeat scan of the same root, compare each
directory's own modified_at (the directory entry's own mtime, which most
filesystems update when its immediate contents change) against the
previous scan's recorded value for that path. Unchanged -> reuse the
previous scan's FileEntry rows for that subtree wholesale (re-pointed at
the new scan_id) instead of re-stating every file in it. Changed -> re-walk
that subtree only.

Deliberately not implemented yet because directory mtime semantics are not
reliable enough on their own (a file's content can change without its
parent directory's mtime changing, only additions/removals/renames
reliably bump it) to trust without a fallback correctness check, and the
two platform-native change-notification mechanisms that would make this
robust are real future work, not a quick addition:
  - Windows: USN Journal (per-volume, requires elevated access to read
    directly, but gives authoritative "what changed since X" without a walk)
  - macOS: FSEvents (per-volume, similar authoritative change history)

Both are noted as the eventual right foundation for a fast repeat-scan
mode; directory-mtime comparison would be a lower-confidence interim
heuristic. Given the instruction not to implement fragile incremental
scanning without high confidence, neither is implemented in this pass --
this section exists so the architectural decision (do not preclude it
later) is on record, not the feature itself.
