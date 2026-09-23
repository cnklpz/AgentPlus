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

/// Returns `store[agent][key]`, creating nested objects as needed.
pub fn section<'a>(root: &'a mut Value, agent: &str, key: &str) -> &'a mut Map<String, Value> {
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
    let s = a.as_object_mut().unwrap().entry(key.to_string()).or_insert_with(|| json!({}));
    if !s.is_object() {
        *s = json!({});
    }
    s.as_object_mut().unwrap()
}

pub fn get_flag(root: &Value, agent: &str, key: &str) -> bool {
    root.get(agent).and_then(|a| a.get(key)).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_flag(root: &mut Value, agent: &str, key: &str, v: bool) {
    if !root.is_object() {
        *root = json!({});
    }
    let a = root
        .as_object_mut()
        .unwrap()
        .entry(agent.to_string())
        .or_insert_with(|| json!({}));
    if let Some(o) = a.as_object_mut() {
        o.insert(key.to_string(), Value::Bool(v));
    }
}
