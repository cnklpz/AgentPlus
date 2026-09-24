//! Local gateway: protocol conversion (convert), the HTTP server (server), the
//! per-forward error circuit breaker (breaker) and the per-agent inbound keys (keys).

pub mod breaker;
pub mod convert;
pub mod keys;
pub mod server;

use std::sync::{Mutex, MutexGuard};

/// Locks `m` even when a panic elsewhere poisoned it: the gateway's shared state stays
/// usable, so one failed request can't take every later one (or the status view) down.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `s` cut to at most `max` characters, with "…" when something was cut.
pub(crate) fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_characters_and_survives_poison() {
        assert_eq!(clip("密钥密钥", 2), "密钥…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("", 0), "");
        let m = Mutex::new(1);
        let _ = std::panic::catch_unwind(|| {
            let _g = m.lock().unwrap();
            panic!("poison");
        });
        assert!(m.is_poisoned());
        *lock(&m) += 1;
        assert_eq!(*lock(&m), 2);
    }
}
