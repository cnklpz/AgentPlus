//! Multi-device sync through a shared folder (cloud drive, network share, USB).
//! Exports providers and model lists for every agent — never API keys — and turns
//! an imported file into ordinary draft ops the user reviews before applying.

use crate::adapters;
use crate::model::{slug, AgentState};
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

const FILE: &str = "agentplus-sync.json";

pub fn folder() -> Option<String> {
    store::get_str(&store::load(), "sync", "folder")
}

pub fn set_folder(path: &str) -> Result<()> {
    let p = PathBuf::from(path.trim());
    if !p.is_dir() {
        return Err(anyhow!("文件夹不存在：{}", p.display()));
    }
    let mut s = store::load();
    store::set_str(&mut s, "sync", "folder", &p.to_string_lossy());
    store::save(&s)
}

fn sync_path() -> Result<PathBuf> {
    Ok(PathBuf::from(folder().ok_or_else(|| anyhow!("先设置同步文件夹"))?).join(FILE))
}

fn export_agent(st: &AgentState) -> Value {
    let providers: Vec<Value> = st
        .providers
        .iter()
        .filter(|p| p.editable && p.base_url.is_some())
        .map(|p| {
            let models: Vec<Value> = p.models.iter().map(|m| json!({ "id": m.id, "name": m.name, "context": m.context, "visible": m.visible })).collect();
            json!({ "name": p.name, "baseUrl": p.base_url, "api": p.api, "enabled": p.enabled, "models": models })
        })
        .collect();
    let catalog: Vec<Value> = st
        .catalog
        .as_ref()
        .map(|c| c.iter().filter(|m| m.deletable).map(|m| json!({ "id": m.id, "name": m.name, "context": m.context })).collect())
        .unwrap_or_default();
    json!({ "providers": providers, "customModels": catalog })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub folder: Option<String>,
    pub file_exists: bool,
    pub exported_at: Option<String>,
    pub machine: Option<String>,
}

pub fn status() -> SyncStatus {
    let f = folder();
    let file = f.as_ref().map(|d| PathBuf::from(d).join(FILE));
    let v: Option<Value> = file.as_ref().and_then(|p| fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok());
    SyncStatus {
        folder: f,
        file_exists: file.map(|p| p.exists()).unwrap_or(false),
        exported_at: v.as_ref().and_then(|v| v["exportedAt"].as_str().map(String::from)),
        machine: v.as_ref().and_then(|v| v["machine"].as_str().map(String::from)),
    }
}

pub fn export() -> Result<String> {
    let path = sync_path()?;
    let mut agents = serde_json::Map::new();
    let mut n = 0;
    for a in adapters::ALL {
        if let Ok(st) = adapters::state(a) {
            let v = export_agent(&st);
            n += v["providers"].as_array().map(|x| x.len()).unwrap_or(0);
            agents.insert(a.to_string(), v);
        }
    }
    let doc = json!({
        "version": 1,
        "exportedAt": chrono::Local::now().to_rfc3339(),
        "machine": std::env::var("COMPUTERNAME").unwrap_or_default(),
        "agents": agents,
    });
    fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(format!("已导出 {n} 个供应商到 {}（不含密钥）", display_path(&path)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub agent: String,
    pub title: String,
    pub detail: String,
    /// Draft ops in the frontend's JSON shape, keyed like the frontend draft.
    pub ops: Vec<(String, Value)>,
}

fn norm(u: &str) -> String {
    u.trim().trim_end_matches('/').to_lowercase()
}

/// Compares the sync file with this machine and proposes additions.
pub fn preview_import() -> Result<Vec<Suggestion>> {
    let path = sync_path()?;
    let doc: Value = serde_json::from_str(&fs::read_to_string(&path).map_err(|_| anyhow!("同步文件夹里还没有 {FILE}，先在另一台设备导出"))?)?;
    let mut out = vec![];
    for a in adapters::ALL {
        let Some(remote) = doc["agents"].get(a) else { continue };
        let Ok(local) = adapters::state(a) else { continue };
        for rp in remote["providers"].as_array().cloned().unwrap_or_default() {
            let base = rp["baseUrl"].as_str().unwrap_or_default();
            let name = rp["name"].as_str().unwrap_or_default();
            let api = rp["api"].as_str().unwrap_or("chat");
            let lp = local.providers.iter().find(|p| p.base_url.as_deref().map(norm) == Some(norm(base)));
            let rmodels: Vec<Value> = rp["models"].as_array().cloned().unwrap_or_default();
            match lp {
                None => {
                    let ids: Vec<String> = rmodels.iter().filter(|m| m["visible"].as_bool().unwrap_or(true)).filter_map(|m| m["id"].as_str().map(String::from)).collect();
                    let key = format!("pu:sync-{a}-{}", slug(name));
                    out.push(Suggestion {
                        agent: a.into(),
                        title: format!("添加供应商「{name}」"),
                        detail: format!("{base} · {} 个模型 · 密钥需要在本机填写", ids.len()),
                        ops: vec![(key, json!({ "op": "upsert_provider", "provider": { "id": null, "name": name, "baseUrl": base, "api": api, "apiKey": null, "models": ids } }))],
                    });
                }
                Some(lp) => {
                    if lp.builtin || local.catalog.is_some() {
                        continue; // Codex models live in the catalog, handled below
                    }
                    let missing: Vec<&Value> = rmodels.iter().filter(|m| m["id"].as_str().map(|id| !lp.models.iter().any(|x| x.id == id)).unwrap_or(false)).collect();
                    if !missing.is_empty() {
                        let ops = missing
                            .iter()
                            .map(|m| {
                                let id = m["id"].as_str().unwrap_or_default();
                                (format!("mu:{}|{id}", lp.id), json!({ "op": "upsert_model", "provider": lp.id, "model": { "id": id, "name": m["name"], "context": m["context"] } }))
                            })
                            .collect();
                        out.push(Suggestion {
                            agent: a.into(),
                            title: format!("「{}」补充 {} 个模型", lp.name, missing.len()),
                            detail: missing.iter().filter_map(|m| m["id"].as_str()).take(6).collect::<Vec<_>>().join("、"),
                            ops,
                        });
                    }
                }
            }
        }
        if let Some(cat) = &local.catalog {
            let missing: Vec<Value> = remote["customModels"].as_array().cloned().unwrap_or_default().into_iter().filter(|m| m["id"].as_str().map(|id| !cat.iter().any(|x| x.id == id)).unwrap_or(false)).collect();
            if !missing.is_empty() {
                let ops = missing
                    .iter()
                    .map(|m| {
                        let id = m["id"].as_str().unwrap_or_default();
                        (format!("mu:*|{id}"), json!({ "op": "upsert_model", "provider": "*", "model": { "id": id, "name": m["name"], "context": m["context"] } }))
                    })
                    .collect();
                out.push(Suggestion {
                    agent: a.into(),
                    title: format!("Codex 模型目录补充 {} 个自定义模型", missing.len()),
                    detail: missing.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect::<Vec<_>>().join("、"),
                    ops,
                });
            }
        }
    }
    Ok(out)
}
