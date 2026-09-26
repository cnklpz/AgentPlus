//! Local gateway: protocol conversion (convert), the HTTP server (server), the
//! per-forward error circuit breaker (breaker), the per-agent inbound keys (keys) and the
//! conversation ids some upstreams route by (session).

pub mod breaker;
pub mod convert;
pub mod keys;
pub mod server;
pub mod session;

pub(crate) use crate::util::{clip, lock};
