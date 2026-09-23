//! Backups under `~/.agentplus/backups/<stamp>/<agent>/` and rollback.

use crate::util::*;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    /// "<stamp>/<agent>"
    pub id: String,
    pub stamp: String,
    pub agent: String,
    pub reason: String,
    pub files: Vec<BackupFile>,
    pub bytes: u64,
    /// Every file knows where it goes back to.
    pub restorable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub name: String,
    pub path: Option<String>,
}

fn root() -> PathBuf {
    agentplus_dir().join("backups")
}

/// Original locations for backups made before manifests existed.
fn legacy_path(agent: &str, name: &str) -> Option<PathBuf> {
    let h = home();
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".codex"));
    let app = dirs::config_dir().unwrap_or_else(|| h.clone()).join("Xiaomi MiMo");
    Some(match (agent, name) {
        ("codex", "config.toml" | "models.json" | ".env") => codex.join(name),
        ("zcode", "provider_config.json" | "setting.json") => h.join(".zcode").join("v2").join(name),
        ("mimo", "mimocode.jsonc") => h.join(".config").join("mimocode").join(name),
        ("mimo", "preferences.json") => app.join(name),
        _ => return None,
    })
}

fn read_entry(stamp: &str, agent_dir: &Path) -> Option<BackupEntry> {
    let agent = agent_dir.file_name()?.to_string_lossy().to_string();
    let manifest: Option<Value> = fs::read_to_string(agent_dir.join("manifest.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
    let mut files = vec![];
    let mut bytes = 0;
    for e in fs::read_dir(agent_dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "manifest.json" || e.path().is_dir() {
            continue;
        }
        bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        let path = manifest
            .as_ref()
            .and_then(|m| m["files"].as_array())
            .and_then(|a| a.iter().find(|f| f["name"].as_str() == Some(&name)))
            .and_then(|f| f["path"].as_str().map(String::from))
            .or_else(|| legacy_path(&agent, &name).map(|p| p.to_string_lossy().to_string()));
        files.push(BackupFile { name, path });
    }
    let reason = manifest
        .as_ref()
        .and_then(|m| m["reason"].as_str().map(String::from))
        .unwrap_or_else(|| match agent.as_str() {
            "codex-cleanup" => "Codex 清理".into(),
            "codex-repair" => "会话修复".into(),
            _ => "应用配置".into(),
        });
    let restorable = !files.is_empty() && files.iter().all(|f| f.path.is_some()) && !agent.starts_with("codex-");
    Some(BackupEntry { id: format!("{stamp}/{agent}"), stamp: stamp.into(), agent, reason, files, bytes, restorable })
}

pub fn list() -> Result<Vec<BackupEntry>> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(root()) else { return Ok(out) };
    for s in rd.flatten() {
        let stamp = s.file_name().to_string_lossy().to_string();
        if let Ok(agents) = fs::read_dir(s.path()) {
            for a in agents.flatten() {
                if a.path().is_dir() {
                    if let Some(e) = read_entry(&stamp, &a.path()) {
                        out.push(e);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(out)
}

/// Copies a backup's files back to their original places. The current files are
/// backed up first, so a rollback can itself be rolled back.
pub fn restore(id: &str) -> Result<String> {
    if id.contains("..") {
        return Err(anyhow!("无效的备份"));
    }
    let (stamp, agent) = id.split_once('/').ok_or_else(|| anyhow!("无效的备份"))?;
    let dir = root().join(stamp).join(agent);
    let entry = read_entry(stamp, &dir).ok_or_else(|| anyhow!("找不到备份 {id}"))?;
    if !entry.restorable {
        return Err(anyhow!("这份备份不能自动回滚（{}），请手动处理", entry.reason));
    }
    let targets: Vec<PathBuf> = entry.files.iter().filter_map(|f| f.path.as_ref().map(PathBuf::from)).collect();
    let safety = backup_tagged(agent, &targets, &format!("回滚到 {stamp} 之前"))?;
    for f in &entry.files {
        let to = PathBuf::from(f.path.as_ref().unwrap());
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.agentplus-tmp", to.display()));
        fs::copy(dir.join(&f.name), &tmp)?;
        fs::rename(&tmp, &to)?;
    }
    Ok(format!("已回滚 {} 个文件到 {stamp} 的状态（回滚前的文件备份在 {}）", entry.files.len(), display_path(&safety)))
}
