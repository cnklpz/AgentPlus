//! The shared provider library: name, address, key, protocol and known
//! models kept by AgentPlus itself, independent of any agent and of the Windows/WSL
//! target. Lives in `~/.agentplus/store.json` under "library". Keys never leave the backend.

use crate::adapters::msg;
use crate::model::{mask_key, slug, unique_id};
use crate::store;
use crate::util::{norm_url, str_field, str_list};
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
    let url = input.base_url.trim();
    if name.is_empty() {
        return Err(msg::name_required());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow!(crate::i18n::l("Base URL must start with http:// or https://", "地址需要以 http:// 或 https:// 开头")));
    }
    if !["responses", "chat", "anthropic"].contains(&input.api.as_str()) {
        return Err(anyhow!(tr!("Unknown API type {}", "未知接口类型 {}", input.api)));
    }
    // Read before taking the store lock: agent adapters may touch the store themselves.
    // Only a key that already goes to this host: the provider's applied address may differ
    // from the one saved here (an edit still pending, a changed address), and its key must
    // not start going to the new one.
    let host = crate::adapters::codex::host_of_url(url);
    let adopted = input
        .adopt_from
        .as_ref()
        .and_then(|(agent, provider)| crate::adapters::provider_endpoint(agent, provider).ok())
        .filter(|(base, _, _)| host.is_some() && crate::adapters::codex::host_of_url(base) == host)
        .and_then(|(_, k, _)| k);
    let new_key = input.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(str::to_string).or(adopted.clone());
    let endpoint = norm_url(url);
    let e = store::update(|root| {
        let mut list = entries(root);
        let pos = match &input.id {
            Some(id) => list.iter().position(|e| str_field(e, "id") == *id),
            // A new entry that another one already is (same address, protocol and key) updates that
            // one instead: the providers page shows one entry per such group and would hide the copy.
            None => list.iter().position(|e| {
                norm_url(&str_field(e, "baseUrl")) == endpoint
                    && str_field(e, "api") == input.api
                    && str_field(e, "apiKey") == new_key.as_deref().unwrap_or("")
            }),
        };
        if input.id.is_some() && pos.is_none() {
            return Err(anyhow!(crate::i18n::l("This entry is not in the provider library", "供应商库里没有这一项")));
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
    crate::sync::changed();
    Ok(to_entry(&e))
}

pub fn delete(id: &str) -> Result<()> {
    store::update(|root| {
        let list: Vec<Value> = entries(root).into_iter().filter(|e| str_field(e, "id") != id).collect();
        root["library"] = Value::Array(list);
        Ok(())
    })?;
    crate::sync::changed();
    Ok(())
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
    let e = entries(root).into_iter().find(|e| str_field(e, "id") == id).ok_or_else(|| anyhow!(tr!("Not in the provider library: {id}", "供应商库里没有 {id}")))?;
    let key = str_field(&e, "apiKey");
    Ok(LibEndpoint {
        name: str_field(&e, "name"),
        base_url: str_field(&e, "baseUrl"),
        key: (!key.is_empty()).then_some(key),
        api: str_field(&e, "api"),
        models: str_list(e.get("models")).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    fn input(id: Option<&str>, name: &str, url: &str, api: &str, key: Option<&str>) -> LibInput {
        LibInput { id: id.map(String::from), name: name.into(), base_url: url.into(), api: api.into(), api_key: key.map(String::from), models: Some(vec!["m".into()]), adopt_from: None }
    }

    /// A provider's key is adopted only for its own host: an address saved here that differs
    /// (an edit still pending) gets no key.
    #[test]
    fn a_key_is_adopted_only_for_its_own_host() {
        let _h = TestHome::new("library-adopt-host");
        let p = crate::model::ProviderInput { id: None, name: "Relay".into(), base_url: "https://a.example".into(), api: "anthropic".into(), api_key: Some("sk-a-secret".into()), models: vec![], key_from_library: None, key_from_sync: None, official_auth: None };
        crate::adapters::plan("claude", &[crate::model::Op::UpsertProvider { provider: p }], false).unwrap();
        let adopt = |url: &str| LibInput { id: None, name: "Relay".into(), base_url: url.into(), api: "anthropic".into(), api_key: None, models: None, adopt_from: Some(("claude".into(), "relay".into())) };
        assert!(!save(adopt("https://b.example")).unwrap().has_key);
        let same = save(adopt("https://a.example/v1")).unwrap();
        assert!(same.has_key);
        assert_eq!(endpoint(&same.id).unwrap().key.as_deref(), Some("sk-a-secret"));
    }

    #[test]
    fn adding_an_entry_that_exists_updates_it() {
        let _h = TestHome::new("library-dedupe");
        let a = save(input(None, "Relay", "https://relay.example.com/v1", "chat", Some("sk-a"))).unwrap();
        // Same address (another spelling), protocol and key: the same entry, renamed.
        let b = save(input(None, "Relay 2", "https://Relay.example.com:443/v1/", "chat", Some(" sk-a "))).unwrap();
        assert_eq!(b.id, a.id);
        assert_eq!(list().len(), 1);
        assert_eq!(list()[0].name, "Relay 2");
        // Another key or protocol is another entry.
        let c = save(input(None, "Relay", "https://relay.example.com/v1", "chat", Some("sk-b"))).unwrap();
        let d = save(input(None, "Relay", "https://relay.example.com/v1", "responses", Some("sk-a"))).unwrap();
        assert_ne!(c.id, a.id);
        assert_ne!(d.id, a.id);
        assert_eq!(list().len(), 3);
        // Key-less entries at one address are one entry too.
        let e = save(input(None, "Open", "https://open.example.com/v1", "chat", None)).unwrap();
        assert_eq!(save(input(None, "Open", "https://open.example.com/v1", "chat", Some(""))).unwrap().id, e.id);
        // Editing by id still edits that entry.
        assert_eq!(save(input(Some(&c.id), "Relay B", "https://relay.example.com/v1", "chat", None)).unwrap().id, c.id);
        assert_eq!(list().len(), 4);
    }

    #[test]
    fn endpoint_paths_and_query_values_remain_case_sensitive() {
        let _h = TestHome::new("library-url-case");
        let urls = [
            "https://relay.example/TeamA/v1",
            "https://relay.example/teama/v1",
            "https://relay.example/v1?team=A",
            "https://relay.example/v1?team=a",
            "https://relay.example/v1?Team=A",
            "https://relay.example/v1?team=A/",
            "https://User:Pass@relay.example/v1",
            "https://user:Pass@relay.example/v1",
            "https://User:pass@relay.example/v1",
            "https://relay.example/v1#A",
            "https://relay.example/v1#a",
            "https://relay.example/v1?",
            "https://relay.example/v1#",
            "https://relay.example/v1",
        ];
        let saved: Vec<_> = urls.iter().map(|url| save(input(None, "Relay", url, "chat", Some("sk-a"))).unwrap()).collect();
        assert_eq!(list().len(), urls.len());
        for (entry, url) in saved.iter().zip(urls) {
            assert_eq!(endpoint(&entry.id).unwrap().base_url, url);
        }
        let same = save(input(None, "Query", "https://RELAY.example:443/v1///?team=A/", "chat", Some("sk-a"))).unwrap();
        assert_eq!(same.id, saved[5].id);
        assert_eq!(list().len(), urls.len());
    }
}
