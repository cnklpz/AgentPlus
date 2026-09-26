//! Backups under `~/.agentplus/backups/<stamp>/<agent>/` and rollback.

use crate::i18n::l;
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
    /// Every file knows where it goes back to, and that file still exists.
    pub restorable: bool,
    /// Why it can't be rolled back automatically.
    pub blocked: Option<String>,
    /// Blocked because the original file is gone (the UI shows a short label for it).
    pub blocked_missing: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub name: String,
    pub path: Option<String>,
    /// A snapshot of one environment's profiles, never the entire shared store.
    #[serde(skip)]
    profile_scope: Option<String>,
}

/// Adds the profiles that belong to the config being backed up. The manifest records
/// the environment now, so restoring a Windows backup while viewing WSL stays in Windows.
pub fn backup_profiles(dir: &Path, scope: &str, root: &Value) -> Result<()> {
    let name = "agentplus-profiles.json";
    let value = root.get(scope).and_then(|a| a.get("profiles")).unwrap_or(&Value::Null);
    write_private_atomic(&dir.join(name), &serde_json::to_vec_pretty(value)?)?;
    let path = agentplus_dir().join("store.json");
    let mut manifest = read_json(&dir.join("manifest.json"))?.0;
    manifest["files"].as_array_mut().ok_or_else(|| anyhow!(l("无效的备份清单", "Invalid backup manifest")))?
        .push(serde_json::json!({ "name": name, "path": path, "profileScope": scope }));
    write_private_atomic(&dir.join("manifest.json"), &serde_json::to_vec_pretty(&manifest)?)
}

fn root() -> PathBuf {
    agentplus_dir().join("backups")
}

/// Fixed backup reasons, (zh, en). A reason is stored in the language of the moment and
/// shown in the current one (`reason_text`), so both sides use these same pairs.
pub const REASON_APPLY: (&str, &str) = ("应用配置", "Apply config");
pub const REASON_OFFICIAL: (&str, &str) = ("获取官方模型列表前", "Before fetching official model list");
const REASON_CLEANUP: (&str, &str) = ("Codex 清理", "Codex cleanup");
const REASON_REPAIR: (&str, &str) = ("会话修复", "Session repair");

fn read_manifest(dir: &Path) -> Option<Value> {
    fs::read_to_string(dir.join("manifest.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// A file's modification time for display, in local time.
fn fmt_mtime(m: &fs::Metadata) -> Option<String> {
    m.modified().ok().map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
}

/// Original locations for backups made before manifests existed. Codex files go back to
/// the folder the adapter edits (the one picked in AgentPlus, else `$CODEX_HOME` / `~/.codex`).
fn legacy_path(agent: &str, name: &str) -> Option<PathBuf> {
    let h = home();
    let codex = crate::adapters::codex::codex_home();
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
    let manifest = read_manifest(agent_dir);
    let mut files = vec![];
    let mut bytes = 0;
    for e in fs::read_dir(agent_dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "manifest.json" || e.path().is_dir() {
            continue;
        }
        bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        let record = manifest
            .as_ref()
            .and_then(|m| m["files"].as_array())
            .and_then(|a| a.iter().find(|f| f["name"].as_str() == Some(&name)));
        let profile_scope = record.and_then(|f| f["profileScope"].as_str())
            .filter(|scope| *scope == agent || scope.strip_prefix(&format!("{agent}@wsl:")).is_some_and(|d| !d.is_empty()))
            .map(String::from);
        let path = if record.is_some_and(|f| f.get("profileScope").is_some()) {
            // Invalid snapshot metadata must never fall through to replacing store.json.
            profile_scope.as_ref().map(|_| agentplus_dir().join("store.json").to_string_lossy().to_string())
        } else {
            record.and_then(|f| f["path"].as_str().map(String::from))
                .or_else(|| legacy_path(&agent, &name).map(|p| p.to_string_lossy().to_string()))
        };
        files.push(BackupFile { name, path, profile_scope });
    }
    let reason = manifest
        .as_ref()
        .and_then(|m| m["reason"].as_str().map(reason_text))
        .unwrap_or_else(|| {
            let (zh, en) = match agent.as_str() {
                "codex-cleanup" => REASON_CLEANUP,
                "codex-repair" => REASON_REPAIR,
                _ => REASON_APPLY,
            };
            l(zh, en).into()
        });
    let missing: Vec<&str> = files.iter().filter(|f| f.profile_scope.is_none() && f.path.as_ref().is_some_and(|p| !Path::new(p).is_file())).map(|f| f.name.as_str()).collect();
    let mut blocked_missing = false;
    let blocked = if agent.starts_with("codex-") {
        Some(l("数据库类备份，请在「会话」页撤销或手动处理", "Database backup: undo it on the Sessions page or handle it manually").to_string())
    } else if agent == "claude" && !files.iter().any(|f| f.profile_scope.is_some()) {
        Some(l("备份缺少 Claude Code 配置档，请手动恢复并核对配置", "The backup has no Claude Code profiles. Restore it manually and check the config").to_string())
    } else if files.is_empty() || files.iter().any(|f| f.path.is_none()) {
        Some(l("不知道原文件放在哪", "Unknown original file location").to_string())
    } else if !missing.is_empty() {
        // Rolling back would recreate files nobody uses any more (e.g. a deleted temp dir).
        blocked_missing = true;
        Some(tr!("原文件已不存在：{}", "Original file no longer exists: {}", crate::i18n::join(&missing)))
    } else {
        None
    };
    Some(BackupEntry { id: format!("{stamp}/{agent}"), stamp: stamp.into(), agent, reason, files, bytes, restorable: blocked.is_none(), blocked, blocked_missing })
}

/// Backup reasons are stored in the language of the moment; show the known fixed ones
/// in the current language.
fn reason_text(r: &str) -> String {
    const KNOWN: &[(&str, &str)] = &[REASON_APPLY, REASON_OFFICIAL, REASON_CLEANUP, REASON_REPAIR];
    const PREFIX: &[(&str, &str, &str, &str)] = &[
        ("回滚到 ", " 之前", "Before rolling back to ", ""),
        ("项目配置 · ", "", "Project config · ", ""),
    ];
    if let Some((zh, en)) = KNOWN.iter().find(|(zh, en)| r == *zh || r == *en) {
        return l(zh, en).into();
    }
    for (zp, zs, ep, es) in PREFIX {
        let mid = r.strip_prefix(zp).and_then(|x| x.strip_suffix(zs)).or_else(|| r.strip_prefix(ep).and_then(|x| x.strip_suffix(es)));
        if let Some(mid) = mid {
            return if crate::i18n::is_en() { format!("{ep}{mid}{es}") } else { format!("{zp}{mid}{zs}") };
        }
    }
    r.into()
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

/// "<stamp>/<agent>" → (stamp, agent, folder), refusing anything that would point outside
/// the backups folder.
fn backup_dir(id: &str) -> Result<(&str, &str, PathBuf)> {
    match id.split_once('/') {
        Some((stamp, agent)) if plain_name(stamp) && plain_name(agent) => Ok((stamp, agent, root().join(stamp).join(agent))),
        _ => Err(anyhow!(l("无效的备份", "Invalid backup"))),
    }
}

/// Copies a backup's files back to their original places. The current files are
/// backed up first, so a rollback can itself be rolled back.
pub fn restore(id: &str) -> Result<String> {
    crate::store::transaction(|| restore_in(id))
}

fn restore_in(id: &str) -> Result<String> {
    let (stamp, agent, dir) = backup_dir(id)?;
    let entry = read_entry(stamp, &dir).ok_or_else(|| anyhow!(tr!("找不到备份 {id}", "Backup not found: {id}")))?;
    if !entry.restorable {
        return Err(anyhow!(tr!("这份备份不能自动回滚：{}", "This backup can't be rolled back automatically: {}", entry.blocked.unwrap_or(entry.reason))));
    }
    // Decode every profile snapshot before writing any files. Only profiles are restored;
    // library entries, gateway keys and other agents' settings keep their current values.
    let mut profiles = vec![];
    for f in &entry.files {
        if let Some(scope) = &f.profile_scope {
            let value = read_json(&dir.join(&f.name))?.0;
            if !value.is_object() && !value.is_null() {
                return Err(anyhow!(l("备份中的配置档无法读取，没有回滚", "Can't read the backed-up profiles; nothing was restored")));
            }
            profiles.push((scope.clone(), value));
        }
    }
    let targets: Vec<PathBuf> = entry.files.iter().filter(|f| f.profile_scope.is_none()).filter_map(|f| f.path.as_ref().map(PathBuf::from)).collect();
    let safety = backup_tagged(agent, &targets, &tr!("回滚到 {stamp} 之前", "Before rolling back to {stamp}"))?;
    for (scope, _) in &profiles {
        backup_profiles(&safety, scope, &crate::store::load())?;
    }
    for f in entry.files.iter().filter(|f| f.profile_scope.is_none()) {
        let to = PathBuf::from(f.path.as_ref().unwrap());
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        // Through a symlinked config (the backup recorded the link), retrying a locked file.
        // fs::copy keeps the backup's permissions.
        let put_back = || -> Result<()> {
            let target = resolve_link(&to)?;
            let tmp = tmp_sibling(&target);
            if let Err(e) = fs::copy(dir.join(&f.name), &tmp) {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
            replace_file(&tmp, &target, &to)
        };
        if let Err(e) = put_back() {
            return Err(anyhow!(tr!("恢复 {} 失败：{e}（回滚前的文件备份在 {}）", "Failed to restore {}: {e} (the pre-rollback files are backed up in {})", to.display(), display_path(&safety))));
        }
    }
    let restored_profiles = !profiles.is_empty();
    if restored_profiles {
        crate::store::update(|root| {
            for (scope, value) in profiles {
                if value.is_null() {
                    if let Some(a) = root.get_mut(&scope).and_then(Value::as_object_mut) {
                        a.remove("profiles");
                    }
                } else {
                    obj_at(root, &[&scope])?.insert("profiles".into(), value);
                }
            }
            Ok(())
        }).map_err(|e| anyhow!(tr!("恢复配置档失败：{e}（回滚前的备份在 {}）", "Failed to restore profiles: {e} (the pre-rollback backup is in {})", display_path(&safety))))?;
    }
    Ok(restored_message(targets.len(), restored_profiles, stamp, &display_path(&safety)))
}

/// The profile snapshot is not a file of the agent's: it is named apart from the count.
fn restored_message(files: usize, profiles: bool, stamp: &str, safety: &str) -> String {
    match (files, profiles) {
        (0, true) => tr!("已回滚 AgentPlus 配置档到 {stamp} 的状态（回滚前的备份在 {safety}）", "Rolled back the AgentPlus profiles to their state at {stamp} (the pre-rollback backup is in {safety})"),
        (n, true) => tr!("已回滚 {n} 个文件和 AgentPlus 配置档到 {stamp} 的状态（回滚前的备份在 {safety}）", "Rolled back {n} file(s) and the AgentPlus profiles to their state at {stamp} (the pre-rollback backup is in {safety})"),
        (n, false) => tr!("已回滚 {n} 个文件到 {stamp} 的状态（回滚前的文件备份在 {safety}）", "Rolled back {n} file(s) to their state at {stamp} (the pre-rollback files are backed up in {safety})"),
    }
}

// ---------------------------------------------------------------- detail

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDetail {
    pub id: String,
    /// Where the backup lives, `~`-folded.
    pub dir: String,
    /// RFC 3339, from the manifest (older backups have none).
    pub time: Option<String>,
    pub files: Vec<FileDetail>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetail {
    pub name: String,
    pub path: Option<String>,
    pub backup_bytes: u64,
    /// None when the original file no longer exists.
    pub current_bytes: Option<u64>,
    pub current_modified: Option<String>,
    /// The current file is byte-for-byte the backup: rolling back changes nothing.
    pub same: bool,
    /// Not text (e.g. a SQLite database), so no diff.
    pub binary: bool,
    /// Backup → current file, with unchanged stretches folded. Secrets are masked.
    pub diff: Vec<DiffRow>,
    pub added: u32,
    pub removed: u32,
    pub truncated: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffRow {
    /// " " unchanged, "-" only in the backup, "+" only in the current file, "…" folded.
    pub kind: &'static str,
    pub text: String,
    /// Line numbers in the backup / current file.
    pub old: Option<u32>,
    pub new: Option<u32>,
}

const CONTEXT: usize = 3;
const MAX_ROWS: usize = 1500;
/// Larger files (session databases, logs) are compared but never loaded for a diff.
const MAX_DIFF_BYTES: u64 = 2 << 20;

/// Byte-for-byte equal, read in chunks so big files never sit in memory.
fn same_content(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(fa), Ok(fb)) = (fs::File::open(a), fs::File::open(b)) else { return false };
    if fa.metadata().map(|m| m.len()).ok() != fb.metadata().map(|m| m.len()).ok() {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let n = match ra.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            return true;
        }
        if rb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
}

pub fn detail(id: &str) -> Result<BackupDetail> {
    let (stamp, _, dir) = backup_dir(id)?;
    let entry = read_entry(stamp, &dir).ok_or_else(|| anyhow!(tr!("找不到备份 {id}", "Backup not found: {id}")))?;
    let manifest = read_manifest(&dir);
    let files = entry.files.into_iter().map(|f| file_detail(&dir, f)).collect();
    Ok(BackupDetail {
        id: entry.id,
        dir: display_path(&dir),
        time: manifest.as_ref().and_then(|m| m["time"].as_str().map(String::from)),
        files,
    })
}

fn file_detail(dir: &Path, f: BackupFile) -> FileDetail {
    let old_path = dir.join(&f.name);
    let old_len = file_len(&old_path);
    let cur_path = f.path.as_ref().map(PathBuf::from);
    let meta = cur_path.as_ref().and_then(|p| fs::metadata(p).ok()).filter(|m| m.is_file());
    let profile = f.profile_scope.as_ref().map(|scope| {
        let root = crate::store::load();
        serde_json::to_vec_pretty(root.get(scope).and_then(|a| a.get("profiles")).unwrap_or(&Value::Null)).unwrap_or_default()
    });
    let mut d = FileDetail {
        name: f.name,
        path: f.path,
        backup_bytes: old_len,
        current_bytes: profile.as_ref().map(|p| p.len() as u64).or_else(|| meta.as_ref().map(|m| m.len())),
        current_modified: meta.as_ref().and_then(fmt_mtime),
        same: false,
        binary: false,
        diff: vec![],
        added: 0,
        removed: 0,
        truncated: false,
    };
    if old_len > MAX_DIFF_BYTES || d.current_bytes.is_some_and(|n| n > MAX_DIFF_BYTES) {
        d.same = if let Some(p) = &profile { fs::read(&old_path).is_ok_and(|old| old == *p) }
            else { cur_path.as_ref().is_some_and(|c| meta.is_some() && same_content(&old_path, c)) };
        d.binary = true;
        return d;
    }
    let old = fs::read(&old_path).unwrap_or_default();
    let cur = profile.or_else(|| meta.as_ref().and_then(|_| fs::read(cur_path.as_ref()?).ok()));
    let text = |b: &[u8]| std::str::from_utf8(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)).ok().filter(|s| !s.contains('\0')).map(String::from);
    let old_text = text(&old);
    let cur_text = match &cur {
        Some(c) => text(c),
        None => Some(String::new()),
    };
    d.backup_bytes = old.len() as u64;
    d.same = cur.as_deref() == Some(&old[..]);
    match (old_text, cur_text) {
        (Some(a), Some(b)) if !d.same => {
            let (rows, added, removed) = line_diff(&a, &b);
            d.truncated = rows.len() > MAX_ROWS;
            d.diff = rows.into_iter().take(MAX_ROWS).collect();
            d.added = added;
            d.removed = removed;
        }
        (Some(_), Some(_)) => {}
        _ => d.binary = true,
    }
    d
}

/// Line diff with folded context. Common head/tail are trimmed first; the middle uses
/// an LCS table, or plain replace when the files are too large for that.
fn line_diff(a: &str, b: &str) -> (Vec<DiffRow>, u32, u32) {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);

    // (kind, old index, new index)
    let mut ops: Vec<(&'static str, Option<usize>, Option<usize>)> = (0..head).map(|i| (" ", Some(i), Some(i))).collect();
    if ma.len() * mb.len() <= 4_000_000 {
        let (n, m) = (ma.len(), mb.len());
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if ma[i] == mb[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && ma[i] == mb[j] {
                ops.push((" ", Some(head + i), Some(head + j)));
                i += 1;
                j += 1;
            } else if i < n && (j == m || t[(i + 1) * w + j] >= t[i * w + j + 1]) {
                ops.push(("-", Some(head + i), None));
                i += 1;
            } else {
                ops.push(("+", None, Some(head + j)));
                j += 1;
            }
        }
    } else {
        ops.extend((0..ma.len()).map(|i| ("-", Some(head + i), None)));
        ops.extend((0..mb.len()).map(|j| ("+", None, Some(head + j))));
    }
    ops.extend((0..tail).map(|k| (" ", Some(a.len() - tail + k), Some(b.len() - tail + k))));

    let added = ops.iter().filter(|o| o.0 == "+").count() as u32;
    let removed = ops.iter().filter(|o| o.0 == "-").count() as u32;
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| o.0 != " ").map(|(k, _)| k).collect();
    let near = |k: usize| {
        let p = changed.partition_point(|&c| c + CONTEXT < k);
        changed.get(p).is_some_and(|&c| c <= k + CONTEXT)
    };
    let mut rows = vec![];
    let mut folded = 0;
    for (k, &(kind, o, n)) in ops.iter().enumerate() {
        if kind == " " && !near(k) {
            folded += 1;
            continue;
        }
        if folded > 0 {
            rows.push(DiffRow { kind: "…", text: tr!("{folded} 行未变", "{folded} unchanged line(s)"), old: None, new: None });
            folded = 0;
        }
        let text = if kind == "+" { b[n.unwrap()] } else { a[o.unwrap()] };
        rows.push(DiffRow { kind, text: mask_secrets(text), old: o.map(|x| x as u32 + 1), new: n.map(|x| x as u32 + 1) });
    }
    if folded > 0 {
        rows.push(DiffRow { kind: "…", text: tr!("{folded} 行未变", "{folded} unchanged line(s)"), old: None, new: None });
    }
    (rows, added, removed)
}

/// Keeps keys and tokens out of the UI: `sk-abcd…wxyz`.
fn mask_secrets(line: &str) -> String {
    use std::sync::OnceLock;
    static KV: OnceLock<regex::Regex> = OnceLock::new();
    static SK: OnceLock<regex::Regex> = OnceLock::new();
    let kv = KV.get_or_init(|| {
        regex::Regex::new(r#"(?i)((?:api[_-]?key|apikey|token|secret|password|authorization)[\w-]*["']?\s*[:=]\s*["']?(?:bearer\s+)?)([^"'\s,]{8,})"#).unwrap()
    });
    let sk = SK.get_or_init(|| regex::Regex::new(r"\b(?:sk|ak|pk)-[A-Za-z0-9_\-]{8,}").unwrap());
    let hide = |s: &str| {
        let c: Vec<char> = s.chars().collect();
        if c.len() <= 10 {
            "••••••".to_string()
        } else {
            format!("{}…{}", c[..4].iter().collect::<String>(), c[c.len() - 4..].iter().collect::<String>())
        }
    };
    let line = kv.replace_all(line, |m: &regex::Captures| format!("{}{}", &m[1], hide(&m[2])));
    sk.replace_all(&line, |m: &regex::Captures| hide(&m[0])).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_snapshots_are_scoped_reviewable_and_reversible() {
        use serde_json::json;
        let h = TestHome::new("profile-backup");
        let scope = "claude@wsl:Ubuntu";
        let initial = json!({scope:{"profiles":{"relay":{"roles":{"default":"old"},"apiKey":"abcdefgh12345678"}}}, "library":[{"id":"unrelated"}]});
        crate::store::save(&initial).unwrap();
        let cfg = h.0.join("settings.json");
        fs::write(&cfg, "old config").unwrap();
        let dir = backup("claude", std::slice::from_ref(&cfg)).unwrap();
        backup_profiles(&dir, scope, &initial).unwrap();
        let id_of = |dir: &Path| format!("{}/claude", dir.parent().unwrap().file_name().unwrap().to_string_lossy());
        let id = id_of(&dir);
        crate::store::update(|root| {
            root[scope]["profiles"]["relay"]["roles"]["default"] = json!("new");
            root["claude"] = json!({"profiles":{"local":{}}});
            Ok(())
        }).unwrap();
        fs::write(&cfg, "new config").unwrap();
        let d = detail(&id).unwrap();
        let p = d.files.iter().find(|f| f.name == "agentplus-profiles.json").unwrap();
        assert!(!p.same && p.added > 0 && p.removed > 0);
        assert!(p.diff.iter().all(|r| !r.text.contains("abcdefgh12345678") && !r.text.contains("unrelated")));
        assert!(restore(&id).unwrap().starts_with("已回滚 1 个文件和 AgentPlus 配置档到"));
        let root = crate::store::load();
        assert_eq!(root[scope], initial[scope]);
        assert_eq!(root["library"], initial["library"]);
        assert!(root["claude"]["profiles"]["local"].is_object());
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "old config");
        assert!(detail(&id).unwrap().files.iter().all(|f| f.same));
        let safety = list().unwrap().into_iter().find(|e| e.id != id).unwrap();
        restore(&safety.id).unwrap();
        assert_eq!(crate::store::load()[scope]["profiles"]["relay"]["roles"]["default"], "new");
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "new config");
        // A missing or invalid profile snapshot must not partly restore the files.
        fs::write(dir.join("agentplus-profiles.json"), "[]").unwrap();
        assert!(restore(&id).is_err());
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "new config");
        fs::remove_file(dir.join("agentplus-profiles.json")).unwrap();
        assert!(restore(&id).is_err());
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "new config");
    }

    #[test]
    fn profile_only_backup_restores_absence_without_clearing_the_store() {
        use serde_json::json;
        let _h = TestHome::new("profile-only");
        let initial = json!({"claude":{"autoRestart":true}});
        crate::store::save(&initial).unwrap();
        let dir = backup("claude", &[]).unwrap();
        backup_profiles(&dir, "claude", &initial).unwrap();
        crate::store::update(|r| { r["claude"]["profiles"] = json!({"relay":{}}); Ok(()) }).unwrap();
        let id = format!("{}/claude", dir.parent().unwrap().file_name().unwrap().to_string_lossy());
        assert!(list().unwrap().iter().any(|e| e.id == id && e.restorable));
        assert!(restore(&id).unwrap().starts_with("已回滚 AgentPlus 配置档到"), "no file was restored");
        assert_eq!(crate::store::load(), initial);
    }

    #[test]
    fn diff_folds_context_and_masks() {
        let a = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let b = a.replace("line 10", "api_key = \"sk-1234567890abcdef\"");
        let (rows, added, removed) = line_diff(&a, &b);
        assert_eq!((added, removed), (1, 1));
        assert_eq!(rows[0].text, "6 行未变");
        assert!(rows.iter().any(|r| r.kind == "+" && r.text == "api_key = \"sk-1…cdef\"" && r.new == Some(10)));
        assert!(rows.iter().any(|r| r.kind == "-" && r.text == "line 10" && r.old == Some(10)));
        assert_eq!(rows.last().unwrap().text, "7 行未变");
    }

    #[test]
    fn missing_original_blocks_rollback() {
        let dir = std::env::temp_dir().join(format!("agentplus-hist-{}", std::process::id())).join("codex");
        fs::create_dir_all(&dir).unwrap();
        let live = dir.parent().unwrap().join("live.toml");
        fs::write(&live, "a").unwrap();
        fs::write(dir.join("live.toml"), "a").unwrap();
        fs::write(dir.join("gone.toml"), "b").unwrap();
        let manifest = serde_json::json!({ "reason": "t", "files": [
            { "name": "live.toml", "path": live.to_string_lossy() },
            { "name": "gone.toml", "path": dir.parent().unwrap().join("nope").join("gone.toml").to_string_lossy() },
        ]});
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();

        let e = read_entry("x", &dir).unwrap();
        assert!(!e.restorable);
        assert_eq!(e.blocked.as_deref(), Some("原文件已不存在：gone.toml"));

        fs::remove_file(dir.join("gone.toml")).unwrap();
        let e = read_entry("x", &dir).unwrap();
        assert!(e.restorable && e.blocked.is_none());
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    /// A rollback writes through a symlinked config (the backup recorded the link path).
    #[cfg(unix)]
    #[test]
    fn restore_keeps_symlinks() {
        let h = TestHome::new("hist-link");
        let (real, link) = (h.0.join("dotfiles").join("config.toml"), h.0.join("config.toml"));
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "a = 1\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dir = backup("codex", std::slice::from_ref(&link)).unwrap();
        fs::write(&link, "a = 2\n").unwrap();
        let id = format!("{}/codex", dir.parent().unwrap().file_name().unwrap().to_string_lossy());
        restore(&id).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "a = 1\n");
        assert!(fs::read_dir(real.parent().unwrap()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("agentplus-tmp")));
        let e = list().unwrap().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.reason, "应用配置");
    }

    #[test]
    fn masks_bare_keys() {
        assert_eq!(mask_secrets("OPENAI_API_KEY=abcdefghijklmnop"), "OPENAI_API_KEY=abcd…mnop");
        assert_eq!(mask_secrets(r#""apiKey": "short123""#), r#""apiKey": "••••••""#);
        assert_eq!(mask_secrets("model = \"gpt-5\""), "model = \"gpt-5\"");
    }
}
