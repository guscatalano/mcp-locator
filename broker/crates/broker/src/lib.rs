//! Broker internals, exposed as a library so integration tests can drive the catalog and the
//! dispatch layer directly rather than only through the pipe.

pub mod catalog;
pub mod dirs;
pub mod server;
