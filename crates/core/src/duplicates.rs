//! Multi-stage duplicate detection: size match -> partial hash -> full
//! cryptographic hash. Never treats filename equality as a duplicate signal
//! (spec section 16). Phase 9 (shares infra with developer-tools scan).

pub struct DuplicateEngine;
