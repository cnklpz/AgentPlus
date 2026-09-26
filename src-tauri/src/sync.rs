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
use std::path::PathBuf;

const FILE: &str = "agentplus-sync.json";

pub fn folder() -> Option<String> {
    store::get_str(&store::load(), "sync", "folder")
}

pub fn set_folder(path: &str) -> Result<()> {
    // Always a Windows folder (a cloud drive, a share): not resolved against the WSL target.
    let p = PathBuf::from(path.trim());
    require_dir(&p)?;
    store::update(|s| {
        store::set_str(s, "sync", "folder", &p.to_string_lossy());
        Ok(())
    })
}

fn sync_path() -> Result<PathBuf> {
    Ok(PathBuf::from(folder().ok_or_else(|| anyhow!(crate::i18n::l("Set a sync folder first", "先设置同步文件夹")))?).join(FILE))
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
    let v: Option<Value> = file.as_deref().and_then(|p| read_json(p).ok()).map(|(v, _)| v);
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
    // A sync client may upload the file at any moment: never let it see half of it.
    write_text_atomic(&path, &serde_json::to_string_pretty(&doc)?, TextMeta::NEW)?;
    Ok(tr!("Exported {n} provider(s) to {} (API keys not included)", "已导出 {n} 个供应商到 {}（不含密钥）", display_path(&path)))
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

/// Compares the sync file with this machine and proposes additions.
pub fn preview_import() -> Result<Vec<Suggestion>> {
    let path = sync_path()?;
    if !path.exists() {
        anyhow::bail!("{}", tr!("No {FILE} in the sync folder yet. Export from another device first", "同步文件夹里还没有 {FILE}，先在另一台设备导出"));
    }
    let (doc, _) = read_json(&path)?;
    let mut out = vec![];
    for a in adapters::ALL {
        let Some(remote) = doc["agents"].get(a) else { continue };
        let Ok(local) = adapters::state(a) else { continue };
        for rp in remote["providers"].as_array().cloned().unwrap_or_default() {
            let base = rp["baseUrl"].as_str().unwrap_or_default();
            let name = rp["name"].as_str().unwrap_or_default();
            let api = rp["api"].as_str().unwrap_or("chat");
            let lp = local.providers.iter().find(|p| p.base_url.as_deref().map(norm_url) == Some(norm_url(base)));
            let rmodels: Vec<Value> = rp["models"].as_array().cloned().unwrap_or_default();
            match lp {
                None => {
                    let ids: Vec<String> = rmodels.iter().filter(|m| m["visible"].as_bool().unwrap_or(true)).filter_map(|m| m["id"].as_str().map(String::from)).collect();
                    // Per address too: two remote providers can share a name.
                    let key = format!("pu:sync-{a}-{}-{}", slug(name), slug(base));
                    out.push(Suggestion {
                        agent: a.into(),
                        title: tr!("Add provider \"{name}\"", "添加供应商「{name}」"),
                        detail: trn!(ids.len(), "{base} · {n} model · API key must be entered on this device", "{base} · {n} models · API key must be entered on this device", "{base} · {n} 个模型 · 密钥需要在本机填写"),
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
                            title: trn!(missing.len(), "Add {n} model to \"{}\"", "Add {n} models to \"{}\"", "「{}」补充 {n} 个模型", lp.name),
                            detail: crate::i18n::join(&missing.iter().filter_map(|m| m["id"].as_str()).take(6).collect::<Vec<_>>()),
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
                    title: trn!(missing.len(), "Add {n} custom model to the Codex model catalog", "Add {n} custom models to the Codex model catalog", "Codex 模型目录补充 {n} 个自定义模型"),
                    detail: crate::i18n::join(&missing.iter().filter_map(|m| m["id"].as_str()).collect::<Vec<_>>()),
                    ops,
                });
            }
        }
    }
    Ok(out)
}
