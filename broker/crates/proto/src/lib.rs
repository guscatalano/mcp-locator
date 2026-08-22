//! Types and algorithms shared by the broker and its tools.
//!
//! Anything in here that has a TypeScript counterpart must stay behaviourally identical to it;
//! `conformance/` holds the fixtures and vectors that prove it.

pub mod canonical;
pub mod card;
pub mod hash;
pub mod rpc;

pub use canonical::canonicalize;
pub use card::{expand_card, expand_env, validate, Card, DiagnosticCode};
pub use hash::launch_hash;
