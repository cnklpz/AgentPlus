//! Inbound keys for the local gateway. Every agent gets its own random key, written into
//! its config when a provider there points at the gateway. The gateway only answers
//! requests that carry one of these keys, and tells from the key which agent is calling.
//!
//! Kept in `~/.agentplus/store.json` under "gatewayKeys": { "<agent>": "agp-…" }.

use crate::store;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// What the UI puts in a provider that points at the gateway; `adapters::plan` swaps it
/// for the agent's own key. Configs written before per-agent keys still carry it, so the
/// gateway keeps accepting it (as `LEGACY`) until those entries are rewritten.
pub const PLACEHOLDER: &str = "agentplus-gateway";

/// Caller of requests that carry `PLACEHOLDER`.
pub const LEGACY: &str = "legacy";

/// OpenCode project configs ("opencode@<folder>") share their agent's key.
fn agent_base(agent: &str) -> &str {
    agent.split('@').next().unwrap_or(agent)
}

/// Tests bypass the store.
#[cfg(test)]
pub static TEST_KEYS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

fn parse(root: &Value) -> Vec<(String, String)> {
    root.get("gatewayKeys")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().filter_map(|(a, k)| k.as_str().map(|k| (a.clone(), k.to_string()))).collect())
        .unwrap_or_default()
}

/// (agent, key) pairs in a store snapshot.
fn stored_in(root: &Value) -> Vec<(String, String)> {
    #[cfg(test)]
    {
        let t = TEST_KEYS.lock().unwrap();
        if !t.is_empty() {
            return t.clone();
        }
    }
    parse(root)
}

fn random_key() -> Result<String> {
    let mut b = [0u8; 20];
    getrandom::getrandom(&mut b).map_err(|e| anyhow!(tr!("生成网关密钥失败：{e}", "Couldn't create a gateway key: {e}")))?;
    Ok(format!("agp-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()))
}

/// The agent's key, created on first use.
pub fn for_agent(agent: &str) -> Result<String> {
    let id = agent_base(agent).to_string();
    if let Some((_, k)) = stored_in(&store::load()).into_iter().find(|(a, _)| a == &id) {
        return Ok(k);
    }
    store::update(|s| {
        if let Some((_, k)) = stored_in(s).into_iter().find(|(a, _)| a == &id) {
            return Ok(k);
        }
        let k = random_key()?;
        if !s["gatewayKeys"].is_object() {
            s["gatewayKeys"] = json!({});
        }
        s["gatewayKeys"][&id] = json!(k);
        Ok(k)
    })
}

/// A key that only makes sense for the gateway: the placeholder or any agent's key.
pub fn is_gateway_key(key: &str) -> bool {
    let key = key.trim();
    key == PLACEHOLDER || stored_in(&store::load()).iter().any(|(_, k)| k == key)
}

/// Which agent a key belongs to (`LEGACY` for the placeholder), in a store snapshot.
pub fn caller_in(root: &Value, key: &str) -> Option<String> {
    let key = key.trim();
    if key == PLACEHOLDER {
        return Some(LEGACY.into());
    }
    stored_in(root).into_iter().find(|(_, k)| k == key).map(|(a, _)| a)
}

/// Agent → fingerprint of its key, so the UI can spot provider entries with an outdated
/// key without ever seeing the keys.
pub fn fingerprints_in(root: &Value) -> BTreeMap<String, String> {
    stored_in(root).into_iter().map(|(a, k)| (a, crate::model::key_fingerprint(&k))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_keys_differ() {
        let (a, b) = (random_key().unwrap(), random_key().unwrap());
        assert!(a.starts_with("agp-") && a.len() == 44, "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn stored_keys_resolve() {
        let root = json!({ "gatewayKeys": { "codex": "agp-1", "claude": "agp-2", "bad": 3 } });
        assert_eq!(parse(&root), vec![("codex".into(), "agp-1".into()), ("claude".into(), "agp-2".into())]);
        assert_eq!(agent_base("opencode@D:/x"), "opencode");
    }
}
