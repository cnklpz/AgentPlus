//! The shared provider library ("总供应商"): name, address, key, protocol and known
//! models kept by AgentPlus itself, independent of any agent and of the Windows/WSL
//! target. Lives in `~/.agentplus/store.json` under "library". Keys never leave the backend.

use crate::model::{mask_key, slug};
use crate::store;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `from_agent` value of an ImportProvider that reads from the library.
pub const FROM: &str = "library";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibEntry {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub has_key: bool,
    /// "••••abcd"
    pub key_hint: Option<String>,
    pub models: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibInput {
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub api: String,
    /// None = keep the stored key.
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    /// (agent, provider): copy the key from there when none is stored yet.
    pub adopt_from: Option<(String, String)>,
}

fn entries(root: &Value) -> Vec<Value> {
    root.get("library").and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

fn to_entry(v: &Value) -> LibEntry {
    let key = str_of(v, "apiKey");
    LibEntry {
        id: str_of(v, "id"),
        name: str_of(v, "name"),
        base_url: str_of(v, "baseUrl"),
        api: str_of(v, "api"),
        has_key: !key.is_empty(),
        key_hint: (!key.is_empty()).then(|| mask_key(&key)),
        models: v
            .get("models")
            .and_then(|m| m.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default(),
    }
}

pub fn list() -> Vec<LibEntry> {
    entries(&store::load()).iter().map(to_entry).collect()
}

pub fn save(input: LibInput) -> Result<LibEntry> {
    let name = input.name.trim();
    let url = input.base_url.trim().trim_end_matches('/');
    if name.is_empty() {
        return Err(anyhow!("名称不能为空"));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow!("地址需要以 http:// 或 https:// 开头"));
    }
    if !["responses", "chat", "anthropic"].contains(&input.api.as_str()) {
        return Err(anyhow!("未知接口类型 {}", input.api));
    }
    let mut root = store::load();
    let mut list = entries(&root);
    let pos = input.id.as_ref().and_then(|id| list.iter().position(|e| str_of(e, "id") == *id));
    if input.id.is_some() && pos.is_none() {
        return Err(anyhow!("供应商库里没有这一项"));
    }
    let mut e = pos.map(|i| list[i].clone()).unwrap_or_else(|| {
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while list.iter().any(|e| str_of(e, "id") == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        json!({ "id": id })
    });
    e["name"] = json!(name);
    e["baseUrl"] = json!(url);
    e["api"] = json!(input.api);
    if let Some(m) = input.models {
        e["models"] = json!(m);
    }
    match input.api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        Some(k) => e["apiKey"] = json!(k),
        None if str_of(&e, "apiKey").is_empty() => {
            if let Some((agent, provider)) = &input.adopt_from {
                if let Ok((_, Some(k), _)) = crate::adapters::provider_endpoint(agent, provider) {
                    e["apiKey"] = json!(k);
                }
            }
        }
        None => {}
    }
    match pos {
        Some(i) => list[i] = e.clone(),
        None => list.push(e.clone()),
    }
    root["library"] = Value::Array(list);
    store::save(&root)?;
    Ok(to_entry(&e))
}

pub fn delete(id: &str) -> Result<()> {
    let mut root = store::load();
    let list: Vec<Value> = entries(&root).into_iter().filter(|e| str_of(e, "id") != id).collect();
    root["library"] = Value::Array(list);
    store::save(&root)
}

/// (name, base_url, key, api, models) of a library entry, for copying into an agent.
pub fn endpoint(id: &str) -> Result<(String, String, Option<String>, String, Vec<String>)> {
    let root = store::load();
    let e = entries(&root).into_iter().find(|e| str_of(e, "id") == id).ok_or_else(|| anyhow!("供应商库里没有 {id}"))?;
    let key = str_of(&e, "apiKey");
    let le = to_entry(&e);
    Ok((le.name, le.base_url, (!key.is_empty()).then_some(key), le.api, le.models))
}
