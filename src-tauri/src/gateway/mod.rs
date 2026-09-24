//! Local gateway: protocol conversion (convert), the HTTP server (server), the
//! per-forward error circuit breaker (breaker) and the per-agent inbound keys (keys).

pub mod breaker;
pub mod convert;
pub mod keys;
pub mod server;

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
    fn clips_characters() {
        assert_eq!(clip("密钥密钥", 2), "密钥…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("", 0), "");
    }
}
