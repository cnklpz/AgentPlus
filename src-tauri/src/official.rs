//! "Fetch the official model list" for Codex.
//!
//! Codex downloads the model list only when it talks to OpenAI with a ChatGPT login
//! (Plus or higher), and stores it in `~/.codex/models_cache.json`. The flow:
//!
//! 1. `start`: back up config.toml (and the catalog), then point Codex at the official
//!    provider with no custom catalog, so it will fetch the list.
//! 2. The user restarts Codex and signs in once; `status` watches for a fresh cache.
//! 3. `finish`: copy the cache over the catalog (keeping AgentPlus's custom models),
//!    put the backed-up config.toml back, and make sure it points at the catalog.
//! 4. `cancel`: put config.toml back without touching the catalog.
//!
//! The in-progress state lives in store.json, so a restart of AgentPlus can resume or undo.

use crate::adapters::codex::{codex_home, ID};
use crate::store;
use crate::util::{backup_tagged, display_path, expand_tilde, read_json, read_text, write_json, write_text_atomic, TextMeta};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use toml_edit::{value, DocumentMut};

const CACHE: &str = "models_cache.json";
const DEFAULT_CATALOG: &str = "~/.codex/models.json";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FetchModel {
    pub slug: String,
    pub name: String,
    pub visible: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FetchStatus {
    /// A fetch is in progress (config.toml is temporarily switched).
    pub active: bool,
    pub started_at: Option<String>,
    pub backup_dir: Option<String>,
    /// A cache newer than the start has appeared.
    pub cache_ready: bool,
    pub cache_models: usize,
    pub cache_path: String,
    pub catalog_path: String,
    /// Account logged in with ChatGPT (auth.json has tokens).
    pub chatgpt_login: bool,
}

fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn cache_path() -> PathBuf {
    codex_home().join(CACHE)
}

fn state(root: &Value) -> Option<Value> {
    store::agent_get(root, ID, "officialFetch").cloned().filter(|v| v.is_object())
}

/// Catalog file named in a config text (or the default one).
fn catalog_of(doc: &DocumentMut) -> PathBuf {
    let p = doc.get("model_catalog_json").and_then(|v| v.as_str()).unwrap_or(DEFAULT_CATALOG);
    expand_tilde(p)
}

fn chatgpt_login() -> bool {
    let Ok(text) = fs::read_to_string(codex_home().join("auth.json")) else { return false };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    v.get("tokens").map(|t| !t.is_null()).unwrap_or(false)
}

fn cache_models(since_ms: i64) -> Option<Vec<Value>> {
    let p = cache_path();
    let modified = fs::metadata(&p).ok()?.modified().ok()?;
    let ms = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    if ms < since_ms {
        return None;
    }
    let v: Value = serde_json::from_str(&fs::read_to_string(&p).ok()?).ok()?;
    v.get("models").and_then(|m| m.as_array()).cloned().filter(|m| !m.is_empty())
}

/// True while config.toml is temporarily switched to the official provider.
pub fn active() -> bool {
    state(&store::load()).is_some()
}

pub fn status() -> FetchStatus {
    let root = store::load();
    let doc = read_text(&config_path()).ok().and_then(|(t, _)| t.parse::<DocumentMut>().ok());
    let mut s = FetchStatus {
        cache_path: display_path(&cache_path()),
        chatgpt_login: chatgpt_login(),
        ..Default::default()
    };
    if let Some(st) = state(&root) {
        let started = st.get("startedMs").and_then(|x| x.as_i64()).unwrap_or(0);
        s.active = true;
        s.started_at = st.get("startedAt").and_then(|x| x.as_str()).map(String::from);
        s.backup_dir = st.get("backupDir").and_then(|x| x.as_str()).map(String::from);
        s.catalog_path = st.get("catalog").and_then(|x| x.as_str()).unwrap_or(DEFAULT_CATALOG).to_string();
        if let Some(m) = cache_models(started) {
            s.cache_ready = true;
            s.cache_models = m.len();
        }
    } else {
        s.catalog_path = doc.as_ref().map(|d| display_path(&catalog_of(d))).unwrap_or_else(|| DEFAULT_CATALOG.into());
    }
    s
}

pub fn start() -> Result<FetchStatus> {
    let mut root = store::load();
    if state(&root).is_some() {
        return Err(anyhow!(crate::i18n::l("已经在获取中，先完成或取消上一次", "A fetch is already in progress. Finish or cancel it first")));
    }
    let (text, meta) = read_text(&config_path())?;
    let mut doc: DocumentMut = text.parse().map_err(|e| anyhow!(tr!("config.toml 解析失败：{e}", "Failed to parse config.toml: {e}")))?;
    let catalog = catalog_of(&doc);
    let mut files = vec![config_path()];
    if catalog.exists() {
        files.push(catalog.clone());
    }
    let dir = backup_tagged(ID, &files, crate::i18n::l("获取官方模型列表前", "Before fetching official model list"))?;

    // Official provider, no custom catalog: Codex will fetch and cache the list.
    doc["model_provider"] = value("openai");
    doc.remove("model_catalog_json");
    write_text_atomic(&config_path(), &doc.to_string(), meta)?;

    let now = chrono::Local::now();
    store::set_value(&mut root, ID, "officialFetch", json!({
        "startedMs": now.timestamp_millis(),
        "startedAt": now.format("%H:%M:%S").to_string(),
        "backupDir": dir.to_string_lossy(),
        "config": dir.join("config.toml").to_string_lossy(),
        "catalog": display_path(&catalog),
        "catalogFile": catalog.to_string_lossy(),
    }));
    store::save(&root)?;
    Ok(status())
}

fn restore_config(st: &Value) -> Result<()> {
    let src = PathBuf::from(st.get("config").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(crate::i18n::l("找不到备份的 config.toml", "Backed-up config.toml not found")))?);
    let bytes = fs::read(&src).with_context(|| tr!("读取备份 {} 失败", "Failed to read backup {}", src.display()))?;
    let tmp = config_path().with_extension("toml.agentplus-tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, config_path())?;
    Ok(())
}

fn clear(root: &mut Value) -> Result<()> {
    store::set_value(root, ID, "officialFetch", Value::Null);
    store::save(root)
}

/// Copies the fresh cache over the catalog and restores config.toml.
pub fn finish() -> Result<Vec<FetchModel>> {
    let mut root = store::load();
    let st = state(&root).ok_or_else(|| anyhow!(crate::i18n::l("没有进行中的获取", "No fetch in progress")))?;
    let started = st.get("startedMs").and_then(|x| x.as_i64()).unwrap_or(0);
    let models = cache_models(started).ok_or_else(|| anyhow!(tr!("还没有新的 {CACHE}：请确认 Codex 已重启并用 ChatGPT 账号登录，打开一次模型选择", "No new {CACHE} yet: make sure Codex has restarted, is signed in with a ChatGPT account, and open the model picker once")))?;
    let catalog = PathBuf::from(st.get("catalogFile").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(crate::i18n::l("备份信息不完整", "Backup info is incomplete")))?);

    // New catalog = the cache file as-is, plus AgentPlus's custom models from the old one.
    let (mut cache, _) = read_json(&cache_path())?;
    let custom: Vec<String> = store::agent_get(&root, ID, "customModels")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let (old, meta) = match read_json(&catalog) {
        Ok((v, m)) => (Some(v), m),
        Err(_) => (None, TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }),
    };
    if let (Some(old), Some(list)) = (old, cache.get_mut("models").and_then(|m| m.as_array_mut())) {
        for m in old.get("models").and_then(|m| m.as_array()).into_iter().flatten() {
            let slug = m.get("slug").and_then(|s| s.as_str()).unwrap_or_default();
            if custom.iter().any(|c| c == slug) && !list.iter().any(|x| x.get("slug").and_then(|s| s.as_str()) == Some(slug)) {
                list.push(m.clone());
            }
        }
    }
    if let Some(dir) = catalog.parent() {
        fs::create_dir_all(dir)?;
    }
    write_json(&catalog, &cache, meta)?;

    // Put the user's config back, making sure it reads the catalog we just wrote.
    restore_config(&st)?;
    let (text, cmeta) = read_text(&config_path())?;
    let mut doc: DocumentMut = text.parse().map_err(|e| anyhow!(tr!("config.toml 解析失败：{e}", "Failed to parse config.toml: {e}")))?;
    if doc.get("model_catalog_json").is_none() {
        doc["model_catalog_json"] = value(display_path(&catalog));
        write_text_atomic(&config_path(), &doc.to_string(), cmeta)?;
    }
    clear(&mut root)?;

    let _ = models;
    Ok(cache
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(FetchModel {
                slug: m.get("slug")?.as_str()?.to_string(),
                name: m.get("display_name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                visible: m.get("visibility").and_then(|x| x.as_str()) != Some("hide"),
            })
        })
        .collect())
}

/// Gives up: config.toml goes back to the backup, the catalog is untouched.
pub fn cancel() -> Result<()> {
    let mut root = store::load();
    let st = state(&root).ok_or_else(|| anyhow!(crate::i18n::l("没有进行中的获取", "No fetch in progress")))?;
    restore_config(&st)?;
    clear(&mut root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full start → cache → finish flow, then start → cancel, on a temp copy of ~/.codex.
    /// store.json and the tagged backups go to the temp dir too, so nothing is left in ~/.agentplus.
    /// `cargo test official_flow -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn official_flow_on_copy() {
        let real = dirs::home_dir().unwrap().join(".codex");
        let tmp = std::env::temp_dir().join(format!("agentplus-official-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        // Point the copy's catalog at the temp dir so the real models.json is never written.
        let catalog = tmp.join("models.json");
        fs::copy(real.join("models.json"), &catalog).unwrap();
        let text = fs::read_to_string(real.join("config.toml")).unwrap();
        let mut doc: DocumentMut = text.parse().unwrap();
        doc["model_catalog_json"] = value(catalog.to_string_lossy().replace('\\', "/"));
        let original = doc.to_string();
        fs::write(tmp.join("config.toml"), &original).unwrap();
        std::env::set_var("CODEX_HOME", &tmp);
        std::env::set_var("AGENTPLUS_HOME", tmp.join(".agentplus"));

        let st = start().unwrap();
        assert!(st.active && !st.cache_ready);
        let during: DocumentMut = fs::read_to_string(tmp.join("config.toml")).unwrap().parse().unwrap();
        assert_eq!(during.get("model_provider").and_then(|v| v.as_str()), Some("openai"));
        assert!(during.get("model_catalog_json").is_none());

        // What Codex would write after the login: same shape, one extra model.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (mut cache, _) = read_json(&catalog).unwrap();
        let mut extra = cache["models"][0].clone();
        extra["slug"] = json!("gpt-official-test");
        cache["models"].as_array_mut().unwrap().push(extra);
        fs::write(tmp.join(CACHE), serde_json::to_string_pretty(&cache).unwrap()).unwrap();
        assert!(status().cache_ready);

        let models = finish().unwrap();
        assert!(models.iter().any(|m| m.slug == "gpt-official-test"));
        assert_eq!(fs::read_to_string(tmp.join("config.toml")).unwrap(), original, "config.toml restored byte for byte");
        let (written, _) = read_json(&catalog).unwrap();
        assert!(written["models"].as_array().unwrap().iter().any(|m| m["slug"] == "gpt-official-test"));
        assert!(!status().active);

        // Cancel path: config back, catalog untouched.
        let before_catalog = fs::read(&catalog).unwrap();
        start().unwrap();
        cancel().unwrap();
        assert_eq!(fs::read_to_string(tmp.join("config.toml")).unwrap(), original);
        assert_eq!(fs::read(&catalog).unwrap(), before_catalog);
        assert!(!status().active);
        println!("ok: {} models, temp dir {}", models.len(), tmp.display());
        let _ = fs::remove_dir_all(&tmp);
    }
}
