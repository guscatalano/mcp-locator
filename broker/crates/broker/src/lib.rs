//! Broker internals, exposed as a library so integration tests can drive the catalog and the
//! dispatch layer directly rather than only through the pipe.

pub mod activation;
pub mod audit;
pub mod catalog;
pub mod consent;
pub mod dirs;
pub mod install;
pub mod prompt;
pub mod security;
pub mod server;
