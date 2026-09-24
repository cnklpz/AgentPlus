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

use crate::adapters::codex::{catalog_path, chatgpt_signed_in, codex_home, config_path, load_doc, ID};
use crate::store;
use crate::util::{backup_tagged, display_path, read_json, str_list, write_bytes_atomic, write_json, write_text_atomic, TextMeta};
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
    /// Account signed in with ChatGPT (auth.json, by Codex's own rules).
    pub chatgpt_login: bool,
}

fn cache_path() -> PathBuf {
    codex_home().join(CACHE)
}

fn state(root: &Value) -> Option<Value> {
    store::agent_get(root, ID, "officialFetch").cloned().filter(|v| v.is_object())
}

/// Catalog file named in config.toml (or the default one).
fn catalog_of(doc: &DocumentMut) -> PathBuf {
    catalog_path(doc).unwrap_or_else(|| crate::env::resolve_path(DEFAULT_CATALOG))
}

/// When the fetch started (ms since the epoch), from its store entry.
fn started_ms(st: &Value) -> i64 {
    st.get("startedMs").and_then(|x| x.as_i64()).unwrap_or(0)
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
    let doc = load_doc().ok().map(|(d, _)| d);
    let mut s = FetchStatus {
        cache_path: display_path(&cache_path()),
        chatgpt_login: chatgpt_signed_in(),
        ..Default::default()
    };
    if let Some(st) = state(&root) {
        let started = started_ms(&st);
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
    let (mut doc, meta) = load_doc()?;
    let catalog = catalog_of(&doc);
    let mut files = vec![config_path()];
    if catalog.exists() {
        files.push(catalog.clone());
    }
    let (zh, en) = crate::history::REASON_OFFICIAL;
    let dir = backup_tagged(ID, &files, crate::i18n::l(zh, en))?;

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
    // Byte for byte, and through a symlinked config.toml like `start`'s write.
    write_bytes_atomic(&config_path(), &bytes)
}

fn clear(root: &mut Value) -> Result<()> {
    store::set_value(root, ID, "officialFetch", Value::Null);
    store::save(root)
}

/// Copies the fresh cache over the catalog and restores config.toml.
pub fn finish() -> Result<Vec<FetchModel>> {
    let mut root = store::load();
    let st = state(&root).ok_or_else(|| anyhow!(crate::i18n::l("没有进行中的获取", "No fetch in progress")))?;
    if cache_models(started_ms(&st)).is_none() {
        anyhow::bail!("{}", tr!("还没有新的 {CACHE}：请确认 Codex 已重启并用 ChatGPT 账号登录，打开一次模型选择", "No new {CACHE} yet: make sure Codex has restarted, is signed in with a ChatGPT account, and open the model picker once"));
    }
    let catalog = PathBuf::from(st.get("catalogFile").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(crate::i18n::l("备份信息不完整", "Backup info is incomplete")))?);

    // New catalog = the cache file as-is, plus AgentPlus's custom models from the old one.
    let (mut cache, _) = read_json(&cache_path())?;
    let custom: Vec<String> = str_list(store::agent_get(&root, ID, "customModels")).unwrap_or_default();
    let (old, meta) = match read_json(&catalog) {
        Ok((v, m)) => (Some(v), m),
        Err(_) => (None, TextMeta::NEW),
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
    let (mut doc, cmeta) = load_doc()?;
    if doc.get("model_catalog_json").is_none() {
        doc["model_catalog_json"] = value(display_path(&catalog));
        write_text_atomic(&config_path(), &doc.to_string(), cmeta)?;
    }
    clear(&mut root)?;

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

    /// start → finish and start → cancel in a temp home: config.toml comes back byte for byte,
    /// and a symlinked config.toml (dotfile managers) stays a link.
    #[test]
    fn start_finish_cancel_restore_config() {
        let h = crate::util::TestHome::new("official");
        let codex = h.0.join(".codex");
        fs::create_dir_all(&codex).unwrap();
        let original = "model_provider = \"relay\"\r\nmodel_catalog_json = \"~/.codex/models.json\"\r\n\r\n[model_providers.relay]\r\nbase_url = \"https://r.example.com\"\r\n";
        let cfg = codex.join("config.toml");
        #[cfg(unix)]
        let real = {
            let real = h.0.join("dotfiles-config.toml");
            fs::write(&real, original).unwrap();
            std::os::unix::fs::symlink(&real, &cfg).unwrap();
            real
        };
        #[cfg(not(unix))]
        let real = {
            fs::write(&cfg, original).unwrap();
            cfg.clone()
        };
        fs::write(codex.join("models.json"), r#"{"models":[{"slug":"mine","display_name":"Mine"}]}"#).unwrap();
        assert!(!status().chatgpt_login);
        fs::write(codex.join("auth.json"), r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x","tokens":{"refresh_token":"r"}}"#).unwrap();
        assert!(!status().chatgpt_login, "an API-key sign-in with leftover tokens is not a ChatGPT login");
        fs::write(codex.join("auth.json"), r#"{"tokens":{"refresh_token":"r"}}"#).unwrap();
        assert!(status().chatgpt_login);

        let st = start().unwrap();
        assert!(st.active && !st.cache_ready);
        assert_eq!(st.catalog_path, "~/.codex/models.json");
        let during = fs::read_to_string(&real).unwrap();
        assert!(during.contains("model_provider = \"openai\"") && !during.contains("model_catalog_json"));
        assert!(finish().unwrap_err().to_string().starts_with("还没有新的 models_cache.json"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(codex.join(CACHE), r#"{"models":[{"slug":"gpt-new","display_name":"GPT New","visibility":"list"}]}"#).unwrap();
        let models = finish().unwrap();
        assert_eq!(models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(), ["gpt-new"]);
        assert_eq!(fs::read_to_string(&real).unwrap(), original);
        assert!(fs::symlink_metadata(&cfg).unwrap().file_type().is_symlink() == cfg!(unix));
        assert!(!status().active);

        start().unwrap();
        cancel().unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), original);
        assert!(fs::symlink_metadata(&cfg).unwrap().file_type().is_symlink() == cfg!(unix));
        assert!(fs::read_dir(&codex).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("agentplus-tmp")));
    }

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
