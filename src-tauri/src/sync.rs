//! Multi-device sync through a shared folder (cloud drive, network share, USB).
//! Exports the provider library and every agent's providers and model lists, and turns an
//! imported file into ordinary draft ops (or library entries) the user reviews first.
//!
//! With a sync password the whole content is encrypted (see `seal`); without one the file is
//! plain JSON. Either way it carries the API keys only when "Sync API keys" is on — in a plain
//! file that means keys in clear text, which the page warns about loudly. Providers refer to a
//! key by its fingerprint, and the key itself is only read from the file in the backend, when
//! a draft that uses it is previewed or applied.

use crate::adapters;
use crate::library::{self, LibEntry, LibInput};
use crate::model::{key_fingerprint, slug, AgentState};
use crate::seal;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const FILE: &str = "agentplus-sync.json";
/// Earlier exports, one file each, next to `FILE`.
const HISTORY_DIR: &str = "agentplus-sync-history";
const FORMAT: &str = "agentplus-sync";
const VERSION: u64 = 2;
/// Store section (`store.json` → `sync`).
const SECTION: &str = "sync";

pub fn folder() -> Option<String> {
    store::get_str(&store::load(), SECTION, "folder")
}

pub fn set_folder(path: &str) -> Result<()> {
    // Always a Windows folder (a cloud drive, a share): not resolved against the WSL target.
    let p = PathBuf::from(path.trim());
    require_dir(&p)?;
    store::update(|s| {
        store::set_str(s, SECTION, "folder", &p.to_string_lossy());
        Ok(())
    })
}

fn sync_path() -> Result<PathBuf> {
    Ok(PathBuf::from(folder().ok_or_else(|| anyhow!(crate::i18n::l("Set a sync folder first", "先设置同步文件夹")))?).join(FILE))
}

// ------------------------------------------------------------ password and options

/// The saved sync password; None when encryption is off.
fn password_in(root: &Value) -> Result<Option<String>> {
    match store::agent_get(root, SECTION, "password") {
        Some(v) if v.is_object() => seal::unprotect(v).map(Some),
        _ => Ok(None),
    }
}

fn password() -> Result<Option<String>> {
    password_in(&store::load())
}

fn has_password_in(root: &Value) -> bool {
    store::agent_get(root, SECTION, "password").is_some_and(|v| v.is_object())
}

/// Put API keys into exports. Unset, it follows the password: on for encrypted files, off
/// for plain ones (clear-text keys only when the user turns this on explicitly).
fn include_keys_in(root: &Value) -> bool {
    store::agent_get(root, SECTION, "includeKeys").and_then(|v| v.as_bool()).unwrap_or_else(|| has_password_in(root))
}

pub fn set_include_keys(on: bool) -> Result<()> {
    store::update(|s| {
        store::set_value(s, SECTION, "includeKeys", Value::Bool(on));
        Ok(())
    })
}

/// Saves (or with None, removes) the sync password. `verify`: only accept a password that
/// opens the current sync file (unlocking a file another device exported).
pub fn set_password(password: Option<&str>, verify: bool) -> Result<String> {
    let Some(pw) = password else {
        store::update(|s| {
            store::set_value(s, SECTION, "password", Value::Null);
            Ok(())
        })?;
        return Ok(crate::i18n::l("Sync password removed. Later exports are not encrypted", "已移除同步密码，之后导出的文件不加密").to_string());
    };
    let pw = seal::check_password(pw)?;
    let file = folder().map(|d| PathBuf::from(d).join(FILE)).filter(|p| p.exists());
    // An unreadable file is left for export to replace: it doesn't stop saving a password.
    let opens = match file.as_deref().and_then(|p| read_json(p).ok()).map(|(doc, _)| doc).filter(|d| d.get("encryption").is_some()) {
        None => None,
        Some(doc) => match open_doc(&doc, Some(&pw)) {
            Ok(_) => Some(true),
            Err(e) if verify => return Err(e),
            Err(_) => Some(false),
        },
    };
    let stored = seal::protect(&pw)?;
    store::update(|s| {
        store::set_value(s, SECTION, "password", stored);
        Ok(())
    })?;
    Ok(match opens {
        Some(true) => crate::i18n::l("Sync password saved; it opens the sync file", "同步密码已保存，能打开现有的同步文件").to_string(),
        Some(false) => crate::i18n::l(
            "Sync password saved. The current sync file was encrypted with a different password: export again to replace it, and enter the new password on your other devices",
            "同步密码已保存。现有的同步文件是用另一个密码加密的：重新导出即可替换，其他设备也要改填新密码",
        )
        .to_string(),
        None => crate::i18n::l("Sync password saved. Exports are encrypted from now on", "同步密码已保存，之后导出的文件都会加密").to_string(),
    })
}

pub fn generate_password() -> Result<String> {
    seal::generate_key()
}

/// Saves a sync key into a text file the user picks. None when cancelled.
pub fn save_key(owner: isize, key: &str) -> Result<Option<String>> {
    let key = seal::check_password(key)?;
    let Some(path) = crate::dialog::save_txt(owner, crate::i18n::l("Save sync key", "保存同步密钥"), "AgentPlus-sync-key.txt")? else {
        return Ok(None);
    };
    // Private like the store: the key opens everything in the sync folder.
    write_private_atomic(&path, key_file_text(&key, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()).as_bytes())?;
    Ok(Some(tr!("Sync key saved to {}", "同步密钥已保存到 {}", display_path(&path))))
}

fn key_file_text(key: &str, at: &str) -> String {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let lines = [
        tr!("AgentPlus sync key (created {at})", "AgentPlus 同步密钥（生成于 {at}）"),
        String::new(),
        key.to_string(),
        String::new(),
        crate::i18n::l("Enter this key as the sync password on every device that syncs.", "在每台参与同步的设备上，把这串密钥填为同步密码。").to_string(),
        crate::i18n::l("Keep this file safe: with it and the sync folder, anyone can read what is synced.", "请妥善保管此文件：拿到它和同步文件夹的人都能读取同步的内容。").to_string(),
    ];
    lines.join(nl) + nl
}

// ------------------------------------------------------------ the file

/// The authenticated part of an encrypted file's header, in a fixed shape.
fn aad(exported_at: &str, machine: &str, h: &seal::Header) -> Vec<u8> {
    serde_json::to_vec(&json!({ "format": FORMAT, "version": VERSION, "exportedAt": exported_at, "machine": machine, "encryption": h.to_json() })).unwrap_or_default()
}

/// The file for `payload` (`agents`, `library`, `keys`): encrypted with a password, else plain.
/// `keys` is left out when empty.
fn seal_doc(payload: &Value, password: Option<&str>, exported_at: &str, machine: &str) -> Result<Value> {
    let mut doc = json!({ "format": FORMAT, "version": VERSION, "exportedAt": exported_at, "machine": machine });
    match password {
        Some(pw) => {
            let (h, sealed) = seal::seal(pw, &serde_json::to_vec(payload)?, |h| aad(exported_at, machine, h))?;
            doc["encryption"] = h.to_json();
            doc["data"] = json!(B64.encode(sealed));
        }
        None => {
            doc["agents"] = payload["agents"].clone();
            doc["library"] = payload["library"].clone();
            if payload["keys"].as_object().is_some_and(|k| !k.is_empty()) {
                doc["keys"] = payload["keys"].clone();
            }
        }
    }
    Ok(doc)
}

/// The content of a sync file (decrypted when it is encrypted).
fn open_doc(doc: &Value, password: Option<&str>) -> Result<Value> {
    if doc["version"].as_u64().is_some_and(|v| v > VERSION) {
        bail!("{}", crate::i18n::l("The sync file was made by a newer AgentPlus. Update AgentPlus on this device first", "同步文件来自更新版本的 AgentPlus，请先更新本机的 AgentPlus"));
    }
    let Some(enc) = doc.get("encryption") else {
        let keys = if doc["keys"].is_object() { doc["keys"].clone() } else { json!({}) };
        return Ok(json!({ "agents": doc["agents"], "library": doc["library"], "keys": keys }));
    };
    let pw = password.ok_or_else(|| anyhow!(crate::i18n::l("The sync file is encrypted. Enter its sync password first", "同步文件已加密，请先填写同步密码")))?;
    let h = seal::Header::from_json(enc)?;
    let sealed = doc["data"].as_str().and_then(|s| B64.decode(s).ok()).ok_or_else(|| anyhow!(crate::i18n::l("The sync file's encryption header is damaged", "同步文件的加密信息已损坏")))?;
    let plain = seal::open(pw, &h, &aad(doc["exportedAt"].as_str().unwrap_or_default(), doc["machine"].as_str().unwrap_or_default(), &h), &sealed)?;
    let v: Value = serde_json::from_slice(&plain)?;
    if !v.is_object() {
        bail!("{}", crate::i18n::l("The sync file's content is damaged", "同步文件的内容已损坏"));
    }
    Ok(v)
}

fn read_payload() -> Result<Value> {
    let path = sync_path()?;
    if !path.exists() {
        bail!("{}", tr!("No {FILE} in the sync folder yet. Export from another device first", "同步文件夹里还没有 {FILE}，先在另一台设备导出"));
    }
    let (doc, _) = read_json(&path)?;
    open_doc(&doc, password()?.as_deref())
}

/// The content of one sync record.
fn read_snapshot(id: &str) -> Result<Value> {
    if !is_snapshot_name(id) {
        bail!("{}", tr!("Not a sync record: {id}", "不是同步记录：{id}"));
    }
    let path = history_dir(Path::new(&folder().ok_or_else(|| anyhow!(crate::i18n::l("Set a sync folder first", "先设置同步文件夹")))?)).join(id);
    let (doc, _) = read_json(&path)?;
    open_doc(&doc, password()?.as_deref())
}

/// The API key stored under fingerprint `fp`: in the sync file, else in the newest record
/// that holds it (a restore from an older record).
pub fn key(fp: &str) -> Result<String> {
    let first = read_payload().and_then(|p| key_in(&p, fp));
    if first.is_ok() {
        return first;
    }
    let dir = folder().map(|f| history_dir(Path::new(&f)));
    for id in dir.as_deref().map(snapshots).unwrap_or_default() {
        if let Ok(k) = read_snapshot(&id).and_then(|p| key_in(&p, fp)) {
            return Ok(k);
        }
    }
    first
}

fn key_in(payload: &Value, fp: &str) -> Result<String> {
    payload["keys"][fp]
        .as_str()
        // The file may have been exported again since the import: never hand out another key.
        .filter(|k| key_fingerprint(k) == fp)
        .map(String::from)
        .ok_or_else(|| anyhow!(crate::i18n::l("The sync file no longer holds this API key. Compare again, or enter the key by hand", "同步文件里已经没有这个 API Key 了，请重新对比，或者手动填写")))
}

// ------------------------------------------------------------ export

/// Adds `key` to `keys` (when collecting) and returns its fingerprint.
fn keep_key(keys: &mut Option<&mut Map<String, Value>>, key: Option<String>) -> Option<String> {
    let (keys, k) = (keys.as_mut()?, key.filter(|k| !k.trim().is_empty())?);
    let fp = key_fingerprint(&k);
    keys.insert(fp.clone(), json!(k));
    Some(fp)
}

fn export_agent(agent: &str, st: &AgentState, mut keys: Option<&mut Map<String, Value>>) -> Value {
    let providers: Vec<Value> = st
        .providers
        .iter()
        .filter(|p| p.editable && p.base_url.is_some())
        .map(|p| {
            let models: Vec<Value> = p.models.iter().map(|m| json!({ "id": m.id, "name": m.name, "context": m.context, "visible": m.visible })).collect();
            let mut v = json!({ "name": p.name, "baseUrl": p.base_url, "api": p.api, "enabled": p.enabled, "models": models });
            if p.has_key && keys.is_some() {
                let key = adapters::provider_endpoint(agent, &p.id).ok().and_then(|(_, k, _)| k);
                if let Some(fp) = keep_key(&mut keys, key) {
                    v["keyFp"] = json!(fp);
                }
            }
            v
        })
        .collect();
    let catalog: Vec<Value> = st
        .catalog
        .as_ref()
        .map(|c| c.iter().filter(|m| m.deletable).map(|m| json!({ "id": m.id, "name": m.name, "context": m.context })).collect())
        .unwrap_or_default();
    json!({ "providers": providers, "customModels": catalog })
}

fn export_library(root: &Value, mut keys: Option<&mut Map<String, Value>>) -> Vec<Value> {
    library::list_in(root)
        .into_iter()
        .map(|e| {
            let mut v = json!({ "name": e.name, "baseUrl": e.base_url, "api": e.api, "models": e.models });
            if e.has_key {
                let key = library::endpoint_in(root, &e.id).ok().and_then(|x| x.key);
                if let Some(fp) = keep_key(&mut keys, key) {
                    v["keyFp"] = json!(fp);
                }
            }
            v
        })
        .collect()
}

/// This device's name, as shown in the sync records.
fn machine() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|m| !m.is_empty()).or_else(sysinfo::System::host_name).unwrap_or_default()
}

/// What an export writes, before encryption; `hash` tells whether anything changed since the last one.
struct Built {
    payload: Value,
    hash: String,
    providers: usize,
    library: usize,
    keys: usize,
}

fn build(root: &Value) -> Built {
    let with_keys = include_keys_in(root);
    let mut keys = Map::new();
    let mut agents = Map::new();
    let mut providers = 0;
    for a in adapters::ALL {
        if let Ok(st) = adapters::state(a) {
            let v = export_agent(a, &st, with_keys.then_some(&mut keys));
            providers += v["providers"].as_array().map(|x| x.len()).unwrap_or(0);
            agents.insert(a.to_string(), v);
        }
    }
    let lib = export_library(root, with_keys.then_some(&mut keys));
    let (library, n_keys) = (lib.len(), keys.len());
    let payload = json!({ "agents": agents, "library": lib, "keys": keys });
    let hash = hex(ring::digest::digest(&ring::digest::SHA256, &serde_json::to_vec(&payload).unwrap_or_default()).as_ref());
    Built { payload, hash, providers, library, keys: n_keys }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Exports and automatic syncs run one at a time: both write the file and prune the records.
static RUN: Mutex<()> = Mutex::new(());

pub fn export() -> Result<String> {
    let _g = crate::util::lock(&RUN);
    let root = store::load();
    export_built(&root, build(&root))
}

fn export_built(root: &Value, b: Built) -> Result<String> {
    let path = sync_path()?;
    let pw = password_in(root)?;
    let with_keys = include_keys_in(root);
    let now = chrono::Local::now();
    let exported_at = now.to_rfc3339();
    let me = machine();
    let doc = seal_doc(&b.payload, pw.as_deref(), &exported_at, &me)?;
    let text = serde_json::to_string_pretty(&doc)?;
    // A sync client may upload the file at any moment: never let it see half of it.
    write_text_atomic(&path, &text, TextMeta::NEW)?;
    // The record is a copy of the same file; failing to keep one doesn't undo the export.
    let dir = history_dir(path.parent().unwrap_or(Path::new(".")));
    let record = std::fs::create_dir_all(&dir).map_err(anyhow::Error::from).and_then(|_| {
        let name = unique_name(&dir, now.with_timezone(&chrono::Utc), &me);
        write_text_atomic(&dir.join(name), &text, TextMeta::NEW)
    });
    if let Err(e) = record {
        crate::applog::warn("sync", format!("Couldn't keep a sync record: {e:#}"));
    }
    prune(&dir, options_in(root).keep);
    store::update(|s| {
        store::set_str(s, SECTION, "lastHash", &b.hash);
        store::set_str(s, SECTION, "lastAck", &exported_at);
        Ok(())
    })?;
    let (agents_n, lib_n) = (trn!(b.providers, "{n} agent provider", "{n} agent providers", "{n} 个 Agent 供应商"), trn!(b.library, "{n} library entry", "{n} library entries", "{n} 个供应商库条目"));
    let what = tr!("{agents_n} and {lib_n}", "{agents_n}、{lib_n}");
    let at = display_path(&path);
    Ok(match (pw.is_some(), with_keys) {
        (false, false) => tr!("Exported {what} to {at} (not encrypted, API keys not included)", "已导出{what}到 {at}（未加密，不含 API Key）"),
        (false, true) => trn!(
            b.keys,
            "Exported {what} to {at}. NOT encrypted: {n} API key in plain text",
            "Exported {what} to {at}. NOT encrypted: {n} API keys in plain text",
            "已导出{what}到 {at}。未加密：{n} 个 API Key 以明文写入"
        ),
        (true, true) => trn!(b.keys, "Exported {what} to {at}, encrypted, with {n} API key", "Exported {what} to {at}, encrypted, with {n} API keys", "已加密导出{what}到 {at}，含 {n} 个 API Key"),
        (true, false) => tr!("Exported {what} to {at}, encrypted (API keys not included)", "已加密导出{what}到 {at}（不含 API Key）"),
    })
}

// ------------------------------------------------------------ records

fn history_dir(folder: &Path) -> PathBuf {
    folder.join(HISTORY_DIR)
}

/// "20260927T022030.123Z_OFFICE-PC.json": UTC, so records from devices in other time zones sort
/// right; milliseconds, so exports in the same second do too.
fn snapshot_name(at: chrono::DateTime<chrono::Utc>, machine: &str) -> String {
    let m: String = machine.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(40).collect();
    format!("{}_{}.json", at.format("%Y%m%dT%H%M%S%.3fZ"), if m.is_empty() { "device".into() } else { m })
}

/// A record name not taken yet: a clash moves the time on by a millisecond, so names still sort by time.
fn unique_name(dir: &Path, at: chrono::DateTime<chrono::Utc>, machine: &str) -> String {
    (0..).map(|ms| snapshot_name(at + chrono::Duration::milliseconds(ms), machine)).find(|n| !dir.join(n).exists()).unwrap()
}

/// Only names this module writes: never a path, never another program's file.
fn is_snapshot_name(n: &str) -> bool {
    n.len() <= 80
        && n.ends_with(".json")
        && n.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && n.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && !n.contains("..")
}

/// Record ids, newest first.
fn snapshots(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).filter(|n| is_snapshot_name(n)).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.cmp(a));
    v
}

fn prune(dir: &Path, keep: u32) {
    for old in snapshots(dir).into_iter().skip(keep as usize) {
        if let Err(e) = std::fs::remove_file(dir.join(&old)) {
            crate::applog::warn("sync", format!("Couldn't remove old sync record {old}: {e}"));
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub exported_at: Option<String>,
    pub machine: Option<String>,
    pub encrypted: bool,
    /// Exported by this device.
    pub mine: bool,
    /// The same export as the current sync file.
    pub current: bool,
}

/// Deletes sync records (only the copies in the records folder; the sync file stays).
pub fn delete_records(ids: &[String]) -> Result<String> {
    let dir = history_dir(Path::new(&folder().ok_or_else(|| anyhow!(crate::i18n::l("Set a sync folder first", "先设置同步文件夹")))?));
    if let Some(bad) = ids.iter().find(|id| !is_snapshot_name(id)) {
        bail!("{}", tr!("Not a sync record: {bad}", "不是同步记录：{bad}"));
    }
    let _g = crate::util::lock(&RUN);
    for id in ids {
        match std::fs::remove_file(dir.join(id)) {
            Ok(()) => {}
            // Already gone (another device pruned it): nothing to do.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => bail!("{}", tr!("Couldn't delete the sync record {id}: {e}", "删除同步记录 {id} 失败：{e}")),
        }
    }
    Ok(trn!(ids.len(), "Deleted {n} sync record", "Deleted {n} sync records", "已删除 {n} 条同步记录"))
}

pub fn history() -> Vec<HistoryEntry> {
    let Some(f) = folder() else { return vec![] };
    let current = read_json(&Path::new(&f).join(FILE)).ok().and_then(|(v, _)| v["exportedAt"].as_str().map(String::from));
    let me = machine();
    let dir = history_dir(Path::new(&f));
    snapshots(&dir)
        .into_iter()
        .map(|id| {
            let v = read_json(&dir.join(&id)).map(|(v, _)| v).unwrap_or(Value::Null);
            let exported_at = v["exportedAt"].as_str().map(String::from);
            let machine = v["machine"].as_str().filter(|m| !m.is_empty()).map(String::from);
            HistoryEntry {
                current: exported_at.is_some() && exported_at == current,
                mine: machine.as_deref() == Some(me.as_str()),
                encrypted: v.get("encryption").is_some(),
                id,
                exported_at,
                machine,
            }
        })
        .collect()
}

// ------------------------------------------------------------ automatic sync

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncOptions {
    /// Sync when AgentPlus starts.
    pub on_start: bool,
    /// Sync after AgentPlus writes a config, the library or a rollback.
    pub on_change: bool,
    /// Sync records kept in the folder.
    pub keep: u32,
}

const KEEP_DEFAULT: u32 = 10;
const KEEP_MAX: u32 = 100;

fn options_in(root: &Value) -> SyncOptions {
    let flag = |k| store::agent_get(root, SECTION, k).and_then(|v| v.as_bool()).unwrap_or(false);
    let keep = store::agent_get(root, SECTION, "keep").and_then(|v| v.as_u64()).map(|n| n.clamp(1, KEEP_MAX as u64) as u32).unwrap_or(KEEP_DEFAULT);
    SyncOptions { on_start: flag("onStart"), on_change: flag("onChange"), keep }
}

pub fn set_options(o: SyncOptions) -> Result<()> {
    let keep = o.keep.clamp(1, KEEP_MAX);
    store::update(|s| {
        store::set_value(s, SECTION, "onStart", Value::Bool(o.on_start));
        store::set_value(s, SECTION, "onChange", Value::Bool(o.on_change));
        store::set_value(s, SECTION, "keep", json!(keep));
        Ok(())
    })?;
    if let Some(f) = folder() {
        let _g = crate::util::lock(&RUN);
        prune(&history_dir(Path::new(&f)), keep);
    }
    Ok(())
}

/// The sync file came from another device and holds an export this device hasn't seen yet
/// (compared or overwritten). An automatic sync then leaves it alone and asks for an import.
fn remote_pending(root: &Value, doc: &Value) -> bool {
    let from = doc["machine"].as_str().unwrap_or_default();
    if from.is_empty() || from == machine() {
        return false;
    }
    let at = |s: Option<&str>| s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    match (at(doc["exportedAt"].as_str()), at(store::get_str(root, SECTION, "lastAck").as_deref())) {
        (Some(file), Some(ack)) => file > ack,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Marks the current sync file as seen (after comparing it).
fn ack_current() {
    let Ok(path) = sync_path() else { return };
    if let Some(at) = read_json(&path).ok().and_then(|(v, _)| v["exportedAt"].as_str().map(String::from)) {
        let _ = store::update(|s| {
            store::set_str(s, SECTION, "lastAck", &at);
            Ok(())
        });
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Turned off for this trigger, or no folder.
    Off,
    Unchanged,
    Exported,
    /// Another device exported something new: import it first.
    RemotePending,
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoResult {
    pub outcome: Outcome,
    pub message: Option<String>,
}

/// An automatic sync: exports this device's config when it changed since the last export,
/// unless the file holds another device's export this device hasn't looked at yet.
/// `trigger`: "start" or "change".
pub fn auto(trigger: &str) -> AutoResult {
    let _g = crate::util::lock(&RUN);
    let root = store::load();
    let o = options_in(&root);
    let on = match trigger {
        "start" => o.on_start,
        "change" => o.on_change,
        _ => false,
    };
    let Some(f) = store::get_str(&root, SECTION, "folder").filter(|_| on) else {
        return AutoResult { outcome: Outcome::Off, message: None };
    };
    let fail = |e: anyhow::Error| AutoResult { outcome: Outcome::Failed, message: Some(tr!("Automatic sync failed: {e}", "自动同步失败：{e}")) };
    let path = Path::new(&f).join(FILE);
    let doc = match path.exists().then(|| read_json(&path).map(|(v, _)| v)).transpose() {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    if let Some(d) = doc.as_ref().filter(|d| remote_pending(&root, d)) {
        let from = d["machine"].as_str().unwrap_or_default();
        return AutoResult {
            outcome: Outcome::RemotePending,
            message: Some(tr!("{from} synced new changes. Compare and import them on the sync page before this device syncs", "{from} 同步了新的内容。请先在同步页对比导入，本机才会继续自动同步")),
        };
    }
    let b = build(&root);
    if doc.is_some() && store::get_str(&root, SECTION, "lastHash").as_deref() == Some(b.hash.as_str()) {
        return AutoResult { outcome: Outcome::Unchanged, message: None };
    }
    match export_built(&root, b) {
        Ok(m) => AutoResult { outcome: Outcome::Exported, message: Some(tr!("Synced automatically. {m}", "已自动同步。{m}")) },
        Err(e) => fail(e),
    }
}

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static CHANGE: AtomicU64 = AtomicU64::new(0);

/// Lets `changed` report to the window.
pub fn init(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// Called after AgentPlus writes something that goes into the sync file. Waits for a quiet
/// moment (applying several agents is several writes), then syncs if "on change" is on and
/// tells the window with a `sync-auto` event.
pub fn changed() {
    if !options_in(&store::load()).on_change {
        return;
    }
    let n = CHANGE.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        if CHANGE.load(Ordering::SeqCst) != n {
            return; // a later change restarts the wait
        }
        let r = auto("change");
        if let Some(app) = APP.get().filter(|_| r.outcome != Outcome::Off && r.outcome != Outcome::Unchanged) {
            use tauri::Emitter;
            let _ = app.emit("sync-auto", r);
        }
    });
}

// ------------------------------------------------------------ status

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub folder: Option<String>,
    pub file_exists: bool,
    pub exported_at: Option<String>,
    pub machine: Option<String>,
    /// The sync file is encrypted.
    pub file_encrypted: bool,
    /// A sync password is saved on this device.
    pub has_password: bool,
    /// Why the saved password can't be used (saved by another Windows account…).
    pub password_error: Option<String>,
    /// The saved password is protected by the OS (DPAPI), not only by file permissions.
    pub system_protected: bool,
    pub include_keys: bool,
    /// The sync file in the folder is not encrypted and holds API keys.
    pub file_plain_keys: bool,
    pub options: SyncOptions,
    /// The sync file holds another device's export this device hasn't compared yet.
    pub remote_pending: bool,
}

pub fn status() -> SyncStatus {
    let root = store::load();
    let f = store::get_str(&root, SECTION, "folder");
    let file = f.as_ref().map(|d| PathBuf::from(d).join(FILE));
    let v: Option<Value> = file.as_deref().and_then(|p| read_json(p).ok()).map(|(v, _)| v);
    let pw = password_in(&root);
    SyncStatus {
        folder: f,
        file_exists: file.map(|p| p.exists()).unwrap_or(false),
        exported_at: v.as_ref().and_then(|v| v["exportedAt"].as_str().map(String::from)),
        machine: v.as_ref().and_then(|v| v["machine"].as_str().map(String::from)).filter(|m| !m.is_empty()),
        file_encrypted: v.as_ref().is_some_and(|v| v.get("encryption").is_some()),
        file_plain_keys: v.as_ref().is_some_and(|v| v.get("encryption").is_none() && v["keys"].as_object().is_some_and(|k| !k.is_empty())),
        has_password: has_password_in(&root),
        password_error: pw.err().map(|e| e.to_string()),
        system_protected: seal::SYSTEM_PROTECTED,
        include_keys: include_keys_in(&root),
        options: options_in(&root),
        remote_pending: v.as_ref().is_some_and(|v| remote_pending(&root, v)),
    }
}

// ------------------------------------------------------------ import

/// A change to the provider library proposed by the sync file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibChange {
    /// Stable id of the change (not shown).
    pub key: String,
    /// The library entry to update; None adds a new one.
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub api: String,
    /// Models to add.
    pub models: Vec<String>,
    /// Store the key the sync file holds under this fingerprint.
    pub key_fp: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// The agent whose draft gets `ops`, or `library::FROM` for a `lib` change.
    pub agent: String,
    pub title: String,
    pub detail: String,
    /// Draft ops in the frontend's JSON shape, keyed like the frontend draft.
    pub ops: Vec<(String, Value)>,
    pub lib: Option<LibChange>,
}

/// The fingerprint of a remote entry's key when the file holds that key.
fn remote_key(entry: &Value, keys: &Value) -> Option<String> {
    entry["keyFp"].as_str().filter(|fp| keys[*fp].is_string()).map(String::from)
}

fn key_note(has: bool) -> &'static str {
    if has {
        crate::i18n::l("API key included", "含 API Key")
    } else {
        crate::i18n::l("API key must be entered on this device", "API Key 需要在本机填写")
    }
}

fn library_suggestions(remote: &[Value], local: &[LibEntry], keys: &Value) -> Vec<Suggestion> {
    let mut out = vec![];
    for r in remote {
        let (name, base, api) = (str_field(r, "name"), str_field(r, "baseUrl"), str_field(r, "api"));
        if name.is_empty() || base.is_empty() {
            continue;
        }
        let models = str_list(r.get("models")).unwrap_or_default();
        let fp = remote_key(r, keys);
        let key = format!("lib:{}|{api}", norm_url(&base));
        match local.iter().find(|e| norm_url(&e.base_url) == norm_url(&base) && e.api == api) {
            None => out.push(Suggestion {
                agent: library::FROM.into(),
                title: tr!("Add \"{name}\" to the provider library", "供应商库添加「{name}」"),
                detail: trn!(models.len(), "{base} · {n} model · {}", "{base} · {n} models · {}", "{base} · {n} 个模型 · {}", key_note(fp.is_some())),
                ops: vec![],
                lib: Some(LibChange { key, id: None, name, base_url: base, api, models, key_fp: fp }),
            }),
            Some(e) => {
                let missing: Vec<String> = models.into_iter().filter(|m| !e.models.contains(m)).collect();
                let fp = fp.filter(|_| !e.has_key);
                if missing.is_empty() && fp.is_none() {
                    continue;
                }
                let mut parts = vec![];
                if !missing.is_empty() {
                    parts.push(trn!(missing.len(), "add {n} model: {}", "add {n} models: {}", "补充 {n} 个模型：{}", crate::i18n::join(&missing.iter().take(6).collect::<Vec<_>>())));
                }
                if fp.is_some() {
                    parts.push(crate::i18n::l("fill in the API key", "补上 API Key").to_string());
                }
                out.push(Suggestion {
                    agent: library::FROM.into(),
                    title: tr!("Update \"{}\" in the provider library", "更新供应商库里的「{}」", e.name),
                    detail: parts.join(" · "),
                    ops: vec![],
                    lib: Some(LibChange { key, id: Some(e.id.clone()), name: e.name.clone(), base_url: e.base_url.clone(), api: e.api.clone(), models: missing, key_fp: fp }),
                });
            }
        }
    }
    out
}

fn agent_suggestions(a: &str, remote: &Value, local: &AgentState, keys: &Value) -> Vec<Suggestion> {
    let mut out = vec![];
    for rp in remote["providers"].as_array().cloned().unwrap_or_default() {
        let base = rp["baseUrl"].as_str().unwrap_or_default();
        let name = rp["name"].as_str().unwrap_or_default();
        let api = rp["api"].as_str().unwrap_or("chat");
        let lp = local.providers.iter().find(|p| p.base_url.as_deref().map(norm_url) == Some(norm_url(base)));
        let rmodels: Vec<Value> = rp["models"].as_array().cloned().unwrap_or_default();
        match lp {
            None => {
                let ids: Vec<String> = rmodels.iter().filter(|m| m["visible"].as_bool().unwrap_or(true)).filter_map(|m| m["id"].as_str().map(String::from)).collect();
                let fp = remote_key(&rp, keys);
                // Per address too: two remote providers can share a name.
                let key = format!("pu:sync-{a}-{}-{}", slug(name), slug(base));
                out.push(Suggestion {
                    agent: a.into(),
                    title: tr!("Add provider \"{name}\"", "添加供应商「{name}」"),
                    detail: trn!(ids.len(), "{base} · {n} model · {}", "{base} · {n} models · {}", "{base} · {n} 个模型 · {}", key_note(fp.is_some())),
                    ops: vec![(key, json!({ "op": "upsert_provider", "provider": { "id": null, "name": name, "baseUrl": base, "api": api, "apiKey": null, "models": ids, "keyFromSync": fp } }))],
                    lib: None,
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
                        lib: None,
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
                lib: None,
            });
        }
    }
    out
}

/// Compares the sync file (or a sync record) with this machine and proposes additions.
pub fn preview_import(snapshot: Option<&str>) -> Result<Vec<Suggestion>> {
    let payload = match snapshot {
        Some(id) => read_snapshot(id)?,
        None => read_payload()?,
    };
    if snapshot.is_none() {
        ack_current();
    }
    let keys = &payload["keys"];
    let mut out = library_suggestions(payload["library"].as_array().map(Vec::as_slice).unwrap_or_default(), &library::list(), keys);
    for a in adapters::ALL {
        let Some(remote) = payload["agents"].get(a) else { continue };
        let Ok(local) = adapters::state(a) else { continue };
        out.extend(agent_suggestions(a, remote, &local, keys));
    }
    Ok(out)
}

/// Writes the chosen library changes (they don't go through a draft: the library is AgentPlus's own).
pub fn adopt_library(changes: Vec<LibChange>) -> Result<String> {
    for c in &changes {
        let api_key = c.key_fp.as_deref().map(key).transpose()?;
        let input = match &c.id {
            None => LibInput { id: None, name: c.name.clone(), base_url: c.base_url.clone(), api: c.api.clone(), api_key, models: Some(c.models.clone()), adopt_from: None },
            Some(id) => {
                let e = library::endpoint(id)?;
                let new: Vec<String> = c.models.iter().filter(|m| !e.models.contains(m)).cloned().collect();
                let models = [e.models, new].concat();
                LibInput { id: Some(id.clone()), name: e.name, base_url: e.base_url, api: e.api, api_key, models: Some(models), adopt_from: None }
            }
        };
        library::save(input)?;
    }
    Ok(trn!(changes.len(), "Updated {n} provider library entry", "Updated {n} provider library entries", "已更新供应商库里的 {n} 项"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    fn payload() -> Value {
        json!({
            "agents": { "codex": { "providers": [], "customModels": [] } },
            "library": [{ "name": "Relay", "baseUrl": "https://relay.example.com/v1", "api": "chat", "models": ["m1"], "keyFp": key_fingerprint("sk-relay") }],
            "keys": { key_fingerprint("sk-relay"): "sk-relay" },
        })
    }

    #[test]
    fn plain_files_carry_keys_only_when_given() {
        let mut p = payload();
        p["keys"] = json!({});
        let doc = seal_doc(&p, None, "2026-09-27T10:00:00+08:00", "PC").unwrap();
        assert!(doc.get("keys").is_none() && doc.get("encryption").is_none());
        let back = open_doc(&doc, None).unwrap();
        assert_eq!(back["library"][0]["name"], "Relay");
        assert!(key_in(&back, &key_fingerprint("sk-relay")).is_err());
        // "Sync API keys" without a password: the keys are in clear text, and still import.
        let doc = seal_doc(&payload(), None, "t", "PC").unwrap();
        assert!(doc.to_string().contains("sk-relay"));
        assert_eq!(key_in(&open_doc(&doc, None).unwrap(), &key_fingerprint("sk-relay")).unwrap(), "sk-relay");
    }

    #[test]
    fn syncing_keys_follows_the_password_until_set() {
        let h = TestHome::new("sync-include-keys");
        assert!(!status().include_keys);
        set_password(Some("pass-1234"), false).unwrap();
        assert!(status().include_keys);
        set_include_keys(false).unwrap();
        assert!(!status().include_keys);
        set_password(None, false).unwrap();
        set_include_keys(true).unwrap();
        assert!(status().include_keys && !status().has_password);
        // The folder's file is flagged when it holds clear-text keys.
        let dir = h.0.join("share");
        std::fs::create_dir_all(&dir).unwrap();
        set_folder(&dir.to_string_lossy()).unwrap();
        std::fs::write(dir.join(FILE), seal_doc(&payload(), None, "t", "PC").unwrap().to_string()).unwrap();
        assert!(status().file_plain_keys);
        std::fs::write(dir.join(FILE), seal_doc(&payload(), Some("pass-1234"), "t", "PC").unwrap().to_string()).unwrap();
        assert!(!status().file_plain_keys);
    }

    #[test]
    fn encrypted_files_hide_everything_but_the_header() {
        let doc = seal_doc(&payload(), Some("pass-1234"), "2026-09-27T10:00:00+08:00", "PC").unwrap();
        let text = doc.to_string();
        for secret in ["sk-relay", "relay.example.com", "Relay"] {
            assert!(!text.contains(secret), "{secret}");
        }
        assert_eq!(doc["machine"], "PC");
        assert!(open_doc(&doc, None).is_err());
        assert!(open_doc(&doc, Some("pass-0000")).is_err());
        let back = open_doc(&doc, Some("pass-1234")).unwrap();
        assert_eq!(key_in(&back, &key_fingerprint("sk-relay")).unwrap(), "sk-relay");
    }

    #[test]
    fn changing_the_header_breaks_decryption() {
        let doc = seal_doc(&payload(), Some("pass-1234"), "2026-09-27T10:00:00+08:00", "PC").unwrap();
        for (k, v) in [("machine", json!("Other")), ("exportedAt", json!("2020-01-01T00:00:00Z"))] {
            let mut d = doc.clone();
            d[k] = v;
            assert!(open_doc(&d, Some("pass-1234")).is_err(), "{k}");
        }
        let mut d = doc.clone();
        d["version"] = json!(VERSION + 1);
        assert!(open_doc(&d, Some("pass-1234")).unwrap_err().to_string().contains("更新"));
    }

    /// A version 1 file (no format, no library) still imports.
    #[test]
    fn old_plain_files_still_open() {
        let back = open_doc(&json!({ "version": 1, "agents": { "codex": {} } }), Some("pass-1234")).unwrap();
        assert!(back["agents"]["codex"].is_object());
        assert!(back["library"].is_null());
    }

    #[test]
    fn keys_are_checked_against_their_fingerprint() {
        let p = json!({ "keys": { "abc": "sk-other" } });
        assert!(key_in(&p, "abc").is_err());
        let fp = key_fingerprint("sk-a");
        assert_eq!(key_in(&json!({ "keys": { fp.clone(): "sk-a" } }), &fp).unwrap(), "sk-a");
    }

    fn entry(id: &str, url: &str, api: &str, models: &[&str], has_key: bool) -> LibEntry {
        LibEntry { id: id.into(), name: id.into(), base_url: url.into(), api: api.into(), has_key, key_hint: None, key_fp: None, models: models.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn library_suggestions_add_update_or_skip() {
        let p = payload();
        let remote = p["library"].as_array().unwrap();
        let fp = key_fingerprint("sk-relay");
        // Missing locally: added with its key.
        let s = library_suggestions(remote, &[], &p["keys"]);
        assert_eq!(s.len(), 1);
        let c = s[0].lib.as_ref().unwrap();
        assert_eq!((c.id.as_deref(), c.models.clone(), c.key_fp.clone()), (None, vec!["m1".to_string()], Some(fp.clone())));
        assert_eq!(s[0].agent, library::FROM);
        // Same address (trailing slash, case) and protocol, without a key or the model: updated.
        let s = library_suggestions(remote, &[entry("relay", "https://Relay.example.com/v1/", "chat", &[], false)], &p["keys"]);
        let c = s[0].lib.as_ref().unwrap();
        assert_eq!((c.id.as_deref(), c.models.clone(), c.key_fp.clone()), (Some("relay"), vec!["m1".to_string()], Some(fp)));
        // Already has the model and a key of its own: nothing to do.
        assert!(library_suggestions(remote, &[entry("relay", "https://relay.example.com/v1", "chat", &["m1"], true)], &p["keys"]).is_empty());
        // Another protocol at the same address is another entry.
        assert_eq!(library_suggestions(remote, &[entry("relay", "https://relay.example.com/v1", "anthropic", &["m1"], true)], &p["keys"])[0].lib.as_ref().unwrap().id, None);
        // A key the file doesn't hold (a plain export) is not offered.
        assert_eq!(library_suggestions(remote, &[], &json!({}))[0].lib.as_ref().unwrap().key_fp, None);
    }

    #[test]
    fn new_agent_providers_take_the_key_by_reference() {
        let fp = key_fingerprint("sk-p");
        let remote = json!({ "providers": [{ "name": "P", "baseUrl": "https://p.example.com/v1", "api": "chat", "models": [{ "id": "m" }], "keyFp": fp }] });
        let local = AgentState::default();
        let s = agent_suggestions("zcode", &remote, &local, &json!({ fp.clone(): "sk-p" }));
        let op = &s[0].ops[0].1;
        assert_eq!(op["provider"]["keyFromSync"], json!(fp));
        assert_eq!(op["provider"]["apiKey"], Value::Null);
        assert!(!serde_json::to_string(&s[0].ops).unwrap().contains("sk-p"));
        let s = agent_suggestions("zcode", &remote, &local, &json!({}));
        assert_eq!(s[0].ops[0].1["provider"]["keyFromSync"], Value::Null);
    }

    #[test]
    fn password_is_saved_protected_and_removed() {
        let _h = TestHome::new("sync-password");
        assert!(set_password(Some("short"), false).is_err());
        set_password(Some("  pass-1234 "), false).unwrap();
        let root = store::load();
        assert_eq!(password_in(&root).unwrap().as_deref(), Some("pass-1234"));
        if seal::SYSTEM_PROTECTED {
            assert!(!root.to_string().contains("pass-1234"));
        }
        let st = status();
        assert!(st.has_password && st.password_error.is_none() && st.include_keys);
        set_password(None, false).unwrap();
        assert_eq!(password().unwrap(), None);
        assert!(!status().has_password);
    }

    #[test]
    fn unlocking_checks_the_password_against_the_file() {
        let h = TestHome::new("sync-unlock");
        let dir = h.0.join("share");
        std::fs::create_dir_all(&dir).unwrap();
        set_folder(&dir.to_string_lossy()).unwrap();
        let doc = seal_doc(&payload(), Some("pass-1234"), "t", "PC").unwrap();
        std::fs::write(dir.join(FILE), doc.to_string()).unwrap();
        assert!(status().file_encrypted);
        assert!(read_payload().is_err());
        assert!(set_password(Some("pass-0000"), true).is_err());
        assert_eq!(password().unwrap(), None);
        // Not verified: saved anyway (a new password for the next export).
        assert!(set_password(Some("pass-0000"), false).unwrap().contains("另一个密码"));
        set_password(Some("pass-1234"), true).unwrap();
        let fp = key_fingerprint("sk-relay");
        assert_eq!(key(&fp).unwrap(), "sk-relay");
        // A draft's reference turns into the key only when the op is resolved for writing.
        let op: crate::model::Op = serde_json::from_value(json!({ "op": "upsert_provider", "provider": { "id": null, "name": "R", "baseUrl": "https://relay.example.com/v1", "api": "chat", "apiKey": null, "keyFromSync": fp } })).unwrap();
        match &crate::adapters::resolve("zcode", &[op]).unwrap()[0] {
            crate::model::Op::UpsertProvider { provider } => assert_eq!((provider.api_key.as_deref(), provider.key_from_sync.as_deref()), (Some("sk-relay"), None)),
            other => panic!("{other:?}"),
        }
    }

    fn built(payload: Value, hash: &str) -> Built {
        Built { payload, hash: hash.into(), providers: 0, library: 1, keys: 1 }
    }

    /// A temp home with a sync folder set; returns the folder.
    fn share(h: &TestHome) -> PathBuf {
        let dir = h.0.join("share");
        std::fs::create_dir_all(&dir).unwrap();
        set_folder(&dir.to_string_lossy()).unwrap();
        dir
    }

    #[test]
    fn record_names_sort_by_time_and_stay_plain() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-09-27T10:20:30+08:00").unwrap().with_timezone(&chrono::Utc);
        let n = snapshot_name(at, "OFFICE PC/1");
        assert_eq!(n, "20260927T022030.000Z_OFFICE_PC_1.json");
        assert!(is_snapshot_name(&n));
        assert_eq!(snapshot_name(at, ""), "20260927T022030.000Z_device.json");
        for bad in ["../x.json", "a\\b.json", "notes.txt", ".20260927.json", "x.json", "2026/..json", "20260927T0..json"] {
            assert!(!is_snapshot_name(bad), "{bad}");
        }
        let h = TestHome::new("sync-unique");
        std::fs::write(h.0.join(&n), "").unwrap();
        assert_eq!(unique_name(&h.0, at, "OFFICE PC/1"), "20260927T022030.001Z_OFFICE_PC_1.json");
    }

    #[test]
    fn exports_keep_records_up_to_the_limit() {
        let h = TestHome::new("sync-records");
        let dir = share(&h);
        set_options(SyncOptions { on_start: false, on_change: false, keep: 3 }).unwrap();
        for i in 0..5 {
            export_built(&store::load(), built(payload(), &format!("h{i}"))).unwrap();
        }
        let list = history();
        assert_eq!(list.len(), 3, "{list:?}");
        assert!(list[0].current && list[0].mine && !list[0].encrypted);
        assert!(list.iter().skip(1).all(|e| !e.current));
        assert!(list.windows(2).all(|w| w[0].id > w[1].id), "newest first");
        // A record opens like the sync file, and the store remembers what was exported.
        assert_eq!(preview_payload(&list[2].id)["library"][0]["name"], "Relay");
        let root = store::load();
        assert_eq!(store::get_str(&root, SECTION, "lastHash").as_deref(), Some("h4"));
        assert_eq!(store::get_str(&root, SECTION, "lastAck"), status().exported_at);
        // Lowering the limit prunes right away; other files in the folder are never touched.
        std::fs::write(history_dir(&dir).join("notes.txt"), "mine").unwrap();
        set_options(SyncOptions { on_start: true, on_change: false, keep: 1 }).unwrap();
        assert_eq!(history().len(), 1);
        assert!(history_dir(&dir).join("notes.txt").exists());
        assert_eq!(status().options, SyncOptions { on_start: true, on_change: false, keep: 1 });
        assert!(read_snapshot("../agentplus-sync.json").is_err());
    }

    #[test]
    fn key_file_holds_the_key_on_its_own_line() {
        let k = "7K3M-QX9A-2PDV-H8WN-5TGE-R4CB-M1ZF-Y6JS";
        let text = key_file_text(k, "2026-09-27 10:20");
        assert!(text.lines().any(|l| l == k), "{text}");
        assert!(text.contains("2026-09-27 10:20"));
        assert_eq!(text.contains("\r\n"), cfg!(windows));
    }

    #[test]
    fn records_can_be_deleted_but_nothing_else() {
        let h = TestHome::new("sync-delete");
        let dir = share(&h);
        for i in 0..3 {
            export_built(&store::load(), built(payload(), &format!("d{i}"))).unwrap();
        }
        let list = history();
        let gone = vec![list[0].id.clone(), list[2].id.clone()];
        delete_records(&gone).unwrap();
        assert_eq!(history().iter().map(|e| e.id.clone()).collect::<Vec<_>>(), vec![list[1].id.clone()]);
        // The sync file stays; deleting again is fine; paths and other files are refused.
        assert!(dir.join(FILE).exists());
        delete_records(&gone).unwrap();
        for bad in ["../agentplus-sync.json", "notes.txt", "..\\x.json"] {
            assert!(delete_records(&[bad.to_string()]).is_err(), "{bad}");
        }
        assert!(dir.join(FILE).exists());
    }

    fn preview_payload(id: &str) -> Value {
        read_snapshot(id).unwrap()
    }

    #[test]
    fn another_devices_export_holds_off_automatic_sync() {
        let h = TestHome::new("sync-remote");
        let dir = share(&h);
        let other = |at: &str| std::fs::write(dir.join(FILE), seal_doc(&payload(), None, at, "OTHER-PC").unwrap().to_string()).unwrap();
        other("2026-09-27T10:00:00+08:00");
        assert!(status().remote_pending);
        // Off unless the trigger is turned on.
        assert_eq!(auto("start").outcome, Outcome::Off);
        set_options(SyncOptions { on_start: true, on_change: true, keep: 5 }).unwrap();
        let r = auto("start");
        assert_eq!(r.outcome, Outcome::RemotePending);
        assert!(r.message.unwrap().contains("OTHER-PC"));
        assert_eq!(auto("nonsense").outcome, Outcome::Off);
        // Comparing it counts as seen; a later export from there is pending again.
        ack_current();
        assert!(!status().remote_pending);
        other("2026-09-27T11:00:00+08:00");
        assert!(status().remote_pending);
        other("2026-09-27T09:00:00+08:00");
        assert!(!status().remote_pending, "older than what was seen");
    }

    #[test]
    fn keys_are_found_in_older_records() {
        let h = TestHome::new("sync-old-key");
        let dir = share(&h);
        set_password(Some("pass-1234"), false).unwrap();
        export_built(&store::load(), built(payload(), "a")).unwrap();
        // The newest export no longer holds the key.
        let mut p = payload();
        p["keys"] = json!({});
        p["library"] = json!([]);
        export_built(&store::load(), built(p, "b")).unwrap();
        assert!(dir.join(FILE).exists());
        assert_eq!(key(&key_fingerprint("sk-relay")).unwrap(), "sk-relay");
        assert!(key(&key_fingerprint("sk-none")).is_err());
    }
}
