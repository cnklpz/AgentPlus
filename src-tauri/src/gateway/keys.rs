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
/// for the agent's own key. It is never an inbound credential; old configs must be
/// rewritten with the gateway page's key update action.
pub const PLACEHOLDER: &str = "agentplus-gateway";

/// Tests bypass the store.
#[cfg(test)]
pub static TEST_KEYS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

fn parse(root: &Value) -> Vec<(String, String)> {
    root.get("gatewayKeys")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().filter_map(|(a, k)| k.as_str().filter(|k| !k.trim().is_empty() && k.trim() != PLACEHOLDER).map(|k| (a.clone(), k.to_string()))).collect())
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
    getrandom::getrandom(&mut b).map_err(|e| anyhow!(tr!("Couldn't create a gateway key: {e}", "生成网关密钥失败：{e}")))?;
    Ok(format!("agp-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()))
}

/// The agent's key, created on first use. OpenCode project configs ("opencode@<folder>")
/// share their agent's key.
pub fn for_agent(agent: &str) -> Result<String> {
    let id = crate::adapters::base_agent(agent).to_string();
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

/// Which agent a real key belongs to, in a store snapshot.
pub fn caller_in(root: &Value, key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() || key == PLACEHOLDER {
        return None;
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
        assert_eq!(crate::adapters::base_agent("opencode@D:/x"), "opencode");
    }

    #[test]
    fn placeholder_is_never_an_inbound_credential() {
        for root in [json!({}), json!({ "gatewayKeys": { "codex": PLACEHOLDER } })] {
            assert_eq!(caller_in(&root, PLACEHOLDER), None);
            assert_eq!(caller_in(&root, &format!(" {PLACEHOLDER} ")), None);
        }
        assert!(parse(&json!({ "gatewayKeys": { "codex": PLACEHOLDER, "claude": "" } })).is_empty());
    }
}
