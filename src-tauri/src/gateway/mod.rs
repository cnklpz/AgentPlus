//! Local gateway: protocol conversion (convert), the HTTP server (server), the
//! per-forward error circuit breaker (breaker), the per-agent inbound keys (keys), the
//! conversation ids some upstreams route by (session) and the time zone override for
//! Codex's environment context (envctx).

pub mod breaker;
pub mod convert;
pub mod envctx;
pub mod keys;
pub mod server;
pub mod session;

pub(crate) use crate::util::{clip, lock};
