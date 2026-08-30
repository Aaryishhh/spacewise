//! CleanupPlanner (builds a reviewable plan from approved candidates) and
//! CleanupExecutor (performs the actual deletion after user approval).
//!
//! Required flow (spec section 28): Scanner -> StorageModel -> Classification
//! -> Safety -> Recommendation -> CleanupPlan -> USER APPROVAL -> Executor -> Undo.
//! A recommendation must never call the executor directly. Phase 8.

pub struct CleanupPlanner;
pub struct CleanupExecutor;
