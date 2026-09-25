//! The shared provider library ("总供应商"): name, address, key, protocol and known
//! models kept by AgentPlus itself, independent of any agent and of the Windows/WSL
//! target. Lives in `~/.agentplus/store.json` under "library". Keys never leave the backend.

use crate::adapters::msg;
use crate::model::{mask_key, slug, unique_id};
use crate::store;
use crate::util::{str_field, str_list};
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
    /// Same fingerprint as Provider.key_fp.
    pub key_fp: Option<String>,
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

fn to_entry(v: &Value) -> LibEntry {
    let key = str_field(v, "apiKey");
    LibEntry {
        id: str_field(v, "id"),
        name: str_field(v, "name"),
        base_url: str_field(v, "baseUrl"),
        api: str_field(v, "api"),
        has_key: !key.is_empty(),
        key_hint: (!key.is_empty()).then(|| mask_key(&key)),
        key_fp: (!key.is_empty()).then(|| crate::model::key_fingerprint(&key)),
        models: str_list(v.get("models")).unwrap_or_default(),
    }
}

pub fn list() -> Vec<LibEntry> {
    list_in(&store::load())
}

/// Library entries from an already loaded store.
pub fn list_in(root: &Value) -> Vec<LibEntry> {
    entries(root).iter().map(to_entry).collect()
}

pub fn save(input: LibInput) -> Result<LibEntry> {
    let name = input.name.trim();
    let url = input.base_url.trim().trim_end_matches('/');
    if name.is_empty() {
        return Err(msg::name_required());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow!(crate::i18n::l("地址需要以 http:// 或 https:// 开头", "Base URL must start with http:// or https://")));
    }
    if !["responses", "chat", "anthropic"].contains(&input.api.as_str()) {
        return Err(anyhow!(tr!("未知接口类型 {}", "Unknown API type {}", input.api)));
    }
    // Read before taking the store lock: agent adapters may touch the store themselves.
    let adopted = input.adopt_from.as_ref().and_then(|(agent, provider)| crate::adapters::provider_endpoint(agent, provider).ok()).and_then(|(_, k, _)| k);
    let e = store::update(|root| {
        let mut list = entries(root);
        let pos = input.id.as_ref().and_then(|id| list.iter().position(|e| str_field(e, "id") == *id));
        if input.id.is_some() && pos.is_none() {
            return Err(anyhow!(crate::i18n::l("供应商库里没有这一项", "This entry is not in the provider library")));
        }
        let mut e = pos.map(|i| list[i].clone()).unwrap_or_else(|| json!({ "id": unique_id(&slug(name), |c| list.iter().any(|e| str_field(e, "id") == c)) }));
        e["name"] = json!(name);
        e["baseUrl"] = json!(url);
        e["api"] = json!(input.api);
        if let Some(m) = input.models {
            e["models"] = json!(m);
        }
        match input.api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
            Some(k) => e["apiKey"] = json!(k),
            None if str_field(&e, "apiKey").is_empty() => {
                if let Some(k) = adopted {
                    e["apiKey"] = json!(k);
                }
            }
            None => {}
        }
        match pos {
            Some(i) => list[i] = e.clone(),
            None => list.push(e.clone()),
        }
        root["library"] = Value::Array(list);
        Ok(e)
    })?;
    Ok(to_entry(&e))
}

pub fn delete(id: &str) -> Result<()> {
    store::update(|root| {
        let list: Vec<Value> = entries(root).into_iter().filter(|e| str_field(e, "id") != id).collect();
        root["library"] = Value::Array(list);
        Ok(())
    })
}

/// A library entry with its key, for copying into an agent or calling its upstream.
pub struct LibEndpoint {
    pub name: String,
    pub base_url: String,
    pub key: Option<String>,
    pub api: String,
    pub models: Vec<String>,
}

pub fn endpoint(id: &str) -> Result<LibEndpoint> {
    endpoint_in(&store::load(), id)
}

/// `endpoint` from an already loaded store.
pub fn endpoint_in(root: &Value, id: &str) -> Result<LibEndpoint> {
    let e = entries(root).into_iter().find(|e| str_field(e, "id") == id).ok_or_else(|| anyhow!(tr!("供应商库里没有 {id}", "Not in the provider library: {id}")))?;
    let key = str_field(&e, "apiKey");
    Ok(LibEndpoint {
        name: str_field(&e, "name"),
        base_url: str_field(&e, "baseUrl"),
        key: (!key.is_empty()).then_some(key),
        api: str_field(&e, "api"),
        models: str_list(e.get("models")).unwrap_or_default(),
    })
}
