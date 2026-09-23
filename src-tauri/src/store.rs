//! AgentPlus's own state in `~/.agentplus/store.json`:
//! per-agent switches and definitions stashed while a model/provider is hidden.

use crate::util::agentplus_dir;
use serde_json::{json, Map, Value};
use std::fs;

pub fn load() -> Value {
    let p = agentplus_dir().join("store.json");
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn save(v: &Value) -> anyhow::Result<()> {
    let dir = agentplus_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("store.json"), serde_json::to_string_pretty(v)?)?;
    Ok(())
}

/// Per-agent entries are kept apart per environment: "codex" on Windows, "codex@wsl:Ubuntu" in WSL.
fn scoped(agent: &str) -> String {
    if crate::env::is_wsl() && crate::adapters::ALL.contains(&agent) {
        format!("{agent}@{}", crate::env::id())
    } else {
        agent.to_string()
    }
}

fn agent_obj<'a>(root: &'a mut Value, agent: &str) -> &'a mut Map<String, Value> {
    let agent = &scoped(agent);
    if !root.is_object() {
        *root = json!({});
    }
    let a = root
        .as_object_mut()
        .unwrap()
        .entry(agent.to_string())
        .or_insert_with(|| json!({}));
    if !a.is_object() {
        *a = json!({});
    }
    a.as_object_mut().unwrap()
}

/// Returns `store[agent][key]` as an object, creating it as needed.
pub fn section<'a>(root: &'a mut Value, agent: &str, key: &str) -> &'a mut Map<String, Value> {
    let s = agent_obj(root, agent).entry(key.to_string()).or_insert_with(|| json!({}));
    if !s.is_object() {
        *s = json!({});
    }
    s.as_object_mut().unwrap()
}

/// `store[agent][key]` for the current environment.
pub fn agent_get<'a>(root: &'a Value, agent: &str, key: &str) -> Option<&'a Value> {
    root.get(scoped(agent)).and_then(|a| a.get(key))
}

pub fn get_flag(root: &Value, agent: &str, key: &str) -> bool {
    root.get(scoped(agent)).and_then(|a| a.get(key)).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_flag(root: &mut Value, agent: &str, key: &str, v: bool) {
    agent_obj(root, agent).insert(key.to_string(), Value::Bool(v));
}

pub fn get_str(root: &Value, agent: &str, key: &str) -> Option<String> {
    root.get(scoped(agent)).and_then(|a| a.get(key)).and_then(|v| v.as_str()).map(String::from)
}

pub fn set_str(root: &mut Value, agent: &str, key: &str, v: &str) {
    agent_obj(root, agent).insert(key.to_string(), Value::from(v));
}

pub fn set_value(root: &mut Value, agent: &str, key: &str, v: Value) {
    agent_obj(root, agent).insert(key.to_string(), v);
}
