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
//!
//! Without a ChatGPT login (relays), `from_codex` uses the list built into the installed
//! Codex instead: `codex debug models --bundled` run with an empty CODEX_HOME prints it,
//! with no sign-in and no network. When no copy of Codex can be run, the list is read
//! straight out of the program, where Codex compiles it in.

use crate::adapters::codex::{catalog_path, chatgpt_signed_in, codex_home, config_path, load_doc, CODEX_HIDDEN, ID};
use crate::store;
use crate::util::{backup_tagged, display_path, read_json, str_field, str_list, write_bytes_atomic, write_json, write_text_atomic, TextMeta};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
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
        return Err(anyhow!(crate::i18n::l("A fetch is already in progress. Finish or cancel it first", "已经在获取中，先完成或取消上一次")));
    }
    let (mut doc, meta) = load_doc()?;
    let catalog = catalog_of(&doc);
    let mut files = vec![config_path()];
    if catalog.exists() {
        files.push(catalog.clone());
    }
    let (en, zh) = crate::history::REASON_OFFICIAL;
    let dir = backup_tagged(ID, &files, crate::i18n::l(en, zh))?;

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
    let src = PathBuf::from(st.get("config").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(crate::i18n::l("Backed-up config.toml not found", "找不到备份的 config.toml")))?);
    let bytes = fs::read(&src).with_context(|| tr!("Failed to read backup {}", "读取备份 {} 失败", src.display()))?;
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
    let st = state(&root).ok_or_else(|| anyhow!(crate::i18n::l("No fetch in progress", "没有进行中的获取")))?;
    if cache_models(started_ms(&st)).is_none() {
        anyhow::bail!("{}", tr!("No new {CACHE} yet: make sure Codex has restarted, is signed in with a ChatGPT account, and open the model picker once", "还没有新的 {CACHE}：请确认 Codex 已重启并用 ChatGPT 账号登录，打开一次模型选择"));
    }
    let catalog = PathBuf::from(st.get("catalogFile").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(crate::i18n::l("Backup info is incomplete", "备份信息不完整")))?);

    // New catalog = the cache file as-is, plus AgentPlus's custom models from the old one.
    let (cache, _) = read_json(&cache_path())?;
    let models = write_catalog(&catalog, cache, &mut root)?;

    // Put the user's config back, making sure it reads the catalog we just wrote.
    restore_config(&st)?;
    point_at(&catalog)?;
    clear(&mut root)?;
    Ok(models)
}

/// Writes `new` (`{"models": [...]}`) to `catalog`, keeping AgentPlus's custom models from
/// the catalog there now, and notes in `root` (the caller saves it) which models Codex
/// itself hides (internal or retired ones), so they aren't ticked by default later.
/// Returns the models it lists.
fn write_catalog(catalog: &Path, mut new: Value, root: &mut Value) -> Result<Vec<FetchModel>> {
    let custom: Vec<String> = str_list(store::agent_get(root, ID, "customModels")).unwrap_or_default();
    let hidden: Vec<String> = new
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter(|m| m.get("visibility").and_then(|x| x.as_str()) == Some("hide"))
        .filter_map(|m| m.get("slug").and_then(|s| s.as_str()).map(String::from))
        .collect();
    store::set_value(root, ID, CODEX_HIDDEN, json!(hidden));
    let (old, meta) = match read_json(catalog) {
        Ok((v, m)) => (Some(v), m),
        Err(_) => (None, TextMeta::NEW),
    };
    if let (Some(old), Some(list)) = (old, new.get_mut("models").and_then(|m| m.as_array_mut())) {
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
    write_json(catalog, &new, meta)?;
    Ok(new
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(FetchModel {
                slug: m.get("slug")?.as_str()?.to_string(),
                name: str_field(m, "display_name"),
                visible: m.get("visibility").and_then(|x| x.as_str()) != Some("hide"),
            })
        })
        .collect())
}

/// Makes config.toml read `catalog` when it names no catalog yet.
fn point_at(catalog: &Path) -> Result<()> {
    let (mut doc, meta) = load_doc()?;
    if doc.get("model_catalog_json").is_none() {
        doc["model_catalog_json"] = value(display_path(catalog));
        write_text_atomic(&config_path(), &doc.to_string(), meta)?;
    }
    Ok(())
}

/// Codex's built-in model list as AgentPlus ships it, for when no installed Codex yields one.
/// Refresh it with a new Codex: `codex debug models --bundled` with CODEX_HOME set to an
/// empty folder, keeping only `{"models": [...]}` (see `snapshot_is_a_catalog`).
const SNAPSHOT: &str = include_str!("codex_models.json");

/// Replaces the catalog with the model list built into the installed Codex (backing up
/// config.toml and the old catalog first) and points config.toml at it. Each copy of Codex
/// is asked in turn; when none can be run, the list is read out of the program file, and
/// failing that AgentPlus's own copy of it is used.
pub fn from_codex() -> Result<Vec<FetchModel>> {
    let exes = crate::process::codex_clis();
    for exe in &exes {
        match builtin_models(exe) {
            Ok(v) => {
                crate::applog::info("catalog", format!("model list from codex debug models ({})", exe.display()));
                return install(v);
            }
            Err(e) => crate::applog::warn("catalog", format!("codex debug models failed with {}: {e}", exe.display())),
        }
    }
    for exe in &exes {
        match fs::read(exe).ok().and_then(|b| embedded_catalog(&b)) {
            Some(v) => {
                crate::applog::info("catalog", format!("model list read from {}", exe.display()));
                return install(v);
            }
            None => crate::applog::warn("catalog", format!("no model list found in {}", exe.display())),
        }
    }
    crate::applog::info("catalog", format!("using AgentPlus's copy of the model list ({} Codex programs found)", exes.len()));
    install(parse_models(SNAPSHOT.as_bytes()).ok_or_else(|| anyhow!("codex_models.json"))?)
}

/// `codex debug models --bundled` with an empty CODEX_HOME: the list compiled into Codex,
/// untouched by the user's config, catalog or cache, and no network. Without `--bundled`
/// once more, for a Codex too old to know it.
fn builtin_models(exe: &Path) -> std::result::Result<Value, String> {
    let home = std::env::temp_dir().join(format!("agentplus-codex-models-{}", std::process::id()));
    fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    let mut last = String::new();
    for args in [&["debug", "models", "--bundled"][..], &["debug", "models"]] {
        match run(exe, args, &home) {
            Ok(out) => {
                let found = parse_models(&out).ok_or_else(|| crate::i18n::l("unexpected output", "输出格式不对").to_string());
                let _ = fs::remove_dir_all(&home);
                return found;
            }
            Err(e) => last = e,
        }
    }
    let _ = fs::remove_dir_all(&home);
    Err(last)
}

/// Runs Codex and returns its output; the error says why it failed (exit code and the last
/// line it printed, or why it couldn't start).
fn run(exe: &Path, args: &[&str], home: &Path) -> std::result::Result<Vec<u8>, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args).env("CODEX_HOME", home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::process::with_login_path(crate::process::no_window(&mut cmd));
    let mut child = cmd.spawn().map_err(|e| tr!("couldn't start ({e})", "无法启动（{e}）"))?;
    // Read on threads so a chatty child can't fill a pipe and stall.
    let read = |mut r: Box<dyn Read + Send>| std::thread::spawn(move || {
        let mut buf = vec![];
        let _ = r.read_to_end(&mut buf);
        buf
    });
    let out = read(Box::new(child.stdout.take().expect("piped")));
    let err = read(Box::new(child.stderr.take().expect("piped")));
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if t0.elapsed() < Duration::from_secs(60) => std::thread::sleep(Duration::from_millis(50)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match other {
                    Err(e) => e.to_string(),
                    _ => crate::i18n::l("no answer within 60 s", "60 秒内没有结果").to_string(),
                });
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        return Ok(out);
    }
    let err = String::from_utf8_lossy(&err);
    let said = err.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("WARNING: proceeding")).unwrap_or_default();
    let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    Err(if said.is_empty() { tr!("exit code {code}", "退出码 {code}") } else { tr!("exit code {code}: {}", "退出码 {code}：{}", crate::util::clip(said, 200)) })
}

/// The catalog in `codex debug models` output: a non-empty `models` list whose entries have slugs.
fn parse_models(stdout: &[u8]) -> Option<Value> {
    catalog_in(&serde_json::from_slice(stdout).ok()?)
}

fn catalog_in(v: &Value) -> Option<Value> {
    let models = v.get("models")?.as_array().filter(|a| !a.is_empty() && a.iter().all(|m| m.get("slug").and_then(|s| s.as_str()).is_some()))?;
    Some(json!({ "models": models }))
}

/// The model list Codex compiles into its program (`include_str!` of its models.json), found
/// in the file's bytes: the object around a `"models"` key whose entries all have a slug and
/// a display name.
fn embedded_catalog(bytes: &[u8]) -> Option<Value> {
    const KEY: &[u8] = b"\"models\"";
    let mut from = 0;
    for _ in 0..200 {
        let hit = from + bytes[from..].windows(KEY.len()).position(|w| w == KEY)?;
        from = hit + 1;
        // The object starts right before the key.
        let lo = hit.saturating_sub(200);
        let Some(open) = bytes[lo..hit].iter().rposition(|&b| b == b'{') else { continue };
        let json = &bytes[lo + open..bytes.len().min(lo + open + 16_000_000)];
        let Some(Ok(v)) = serde_json::Deserializer::from_slice(json).into_iter::<Value>().next() else { continue };
        let named = v.get("models").and_then(|m| m.as_array()).is_some_and(|a| a.len() >= 3 && a.iter().all(|m| m.get("display_name").is_some()));
        if let Some(c) = catalog_in(&v).filter(|_| named) {
            return Some(c);
        }
    }
    None
}

/// Writes `new` as the catalog (after backing up config.toml and the old catalog) and points
/// config.toml at it.
fn install(new: Value) -> Result<Vec<FetchModel>> {
    let mut root = store::load();
    if state(&root).is_some() {
        bail!("{}", crate::i18n::l("A fetch of the official list is in progress. Finish or cancel it first", "正在获取官方模型列表，先完成或取消它"));
    }
    let (doc, _) = load_doc()?;
    let catalog = catalog_of(&doc);
    let mut files = vec![config_path()];
    if catalog.exists() {
        files.push(catalog.clone());
    }
    let (en, zh) = crate::history::REASON_BUILTIN;
    backup_tagged(ID, &files, crate::i18n::l(en, zh))?;
    let models = write_catalog(&catalog, new, &mut root)?;
    store::save(&root)?;
    point_at(&catalog)?;
    Ok(models)
}

/// Gives up: config.toml goes back to the backup, the catalog is untouched.
pub fn cancel() -> Result<()> {
    let mut root = store::load();
    let st = state(&root).ok_or_else(|| anyhow!(crate::i18n::l("No fetch in progress", "没有进行中的获取")))?;
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

    #[test]
    fn debug_models_output_must_be_a_catalog() {
        let v = parse_models(br#"{"models":[{"slug":"gpt-a","display_name":"A"},{"slug":"gpt-b"}],"extra":1}"#).unwrap();
        assert_eq!(v, json!({ "models": [{ "slug": "gpt-a", "display_name": "A" }, { "slug": "gpt-b" }] }), "only the models are kept");
        for bad in [&b"not json"[..], br#"{"models":[]}"#, br#"{"models":[{"display_name":"no slug"}]}"#, br#"{"other":[]}"#, b""] {
            assert!(parse_models(bad).is_none(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// The fallback when Codex won't run: the list compiled into the program, found among
    /// other bytes and other "models" keys.
    #[test]
    fn embedded_catalog_is_found_in_program_bytes() {
        let list = r#"{"models":[{"slug":"a","display_name":"A","priority":1},{"slug":"b","display_name":"B"},{"slug":"c","display_name":"C"}]}"#;
        let mut bytes = b"\x00\xffMZ junk {\"models\": 3} more {\"models\":[{\"slug\":\"x\"}]}\x00\x01".to_vec();
        bytes.extend_from_slice(list.as_bytes());
        bytes.extend_from_slice(b"\x00trailing {\"not\":\"json");
        let v = embedded_catalog(&bytes).unwrap();
        assert_eq!(v["models"].as_array().unwrap().iter().map(|m| m["slug"].as_str().unwrap()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert!(embedded_catalog(b"no catalog here \"models\" {\"models\":[]}").is_none());
    }

    /// A Codex that can't be run says why, instead of a bare "failed".
    #[test]
    fn failed_runs_say_why() {
        let home = std::env::temp_dir();
        let err = run(Path::new("Z:/nowhere/codex.exe"), &["debug", "models"], &home).unwrap_err();
        assert!(err.starts_with("无法启动"), "{err}");
        #[cfg(windows)]
        {
            let err = run(Path::new("cmd"), &["/c", "echo WARNING: proceeding, even so 1>&2 & echo boom 1>&2 & exit 3"], &home).unwrap_err();
            assert_eq!(err, "退出码 3：boom");
        }
    }

    /// AgentPlus's own copy of Codex's list, the last fallback, must stay a usable catalog.
    #[test]
    fn snapshot_is_a_catalog() {
        let v = parse_models(SNAPSHOT.as_bytes()).expect("codex_models.json");
        let models = v["models"].as_array().unwrap();
        assert!(models.len() >= 3);
        assert!(models.iter().all(|m| m.get("display_name").is_some() && m.get("context_window").is_some()));
    }

    /// No catalog yet (an older setup): the built-in list becomes ~/.codex/models.json and
    /// config.toml reads it. Over an existing catalog, AgentPlus's custom models stay.
    #[test]
    fn builtin_list_installs_a_catalog() {
        let h = crate::util::TestHome::new("official-builtin");
        let codex = h.0.join(".codex");
        fs::create_dir_all(&codex).unwrap();
        let original = "model_provider = \"relay\"\n\n[model_providers.relay]\nbase_url = \"https://r.example.com/v1\"\n";
        fs::write(codex.join("config.toml"), original).unwrap();
        let built_in = json!({ "models": [{ "slug": "gpt-a", "display_name": "A", "visibility": "list" }, { "slug": "gpt-old", "visibility": "hide" }] });

        let models = install(built_in.clone()).unwrap();
        assert_eq!(models.iter().map(|m| (m.slug.as_str(), m.visible)).collect::<Vec<_>>(), [("gpt-a", true), ("gpt-old", false)]);
        let cfg = fs::read_to_string(codex.join("config.toml")).unwrap();
        assert_eq!(cfg, original.replace("\n\n", "\nmodel_catalog_json = \"~/.codex/models.json\"\n\n"), "a top-level key, the rest as it was");
        assert_eq!(read_json(&codex.join("models.json")).unwrap().0, built_in);
        let hidden = || str_list(store::agent_get(&store::load(), ID, CODEX_HIDDEN)).unwrap_or_default();
        assert_eq!(hidden(), ["gpt-old"], "what Codex hides is noted");
        assert!(crate::history::list().unwrap().iter().any(|b| b.agent == ID), "config.toml was backed up first");

        // Again, with a custom model AgentPlus added in the meantime: it survives the refresh.
        let mut cat = built_in.clone();
        cat["models"].as_array_mut().unwrap().push(json!({ "slug": "mine", "display_name": "Mine" }));
        fs::write(codex.join("models.json"), cat.to_string()).unwrap();
        let mut root = store::load();
        store::set_value(&mut root, ID, "customModels", json!(["mine"]));
        store::save(&root).unwrap();
        let models = install(json!({ "models": [{ "slug": "gpt-b" }] })).unwrap();
        assert_eq!(models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(), ["gpt-b", "mine"]);
        assert!(hidden().is_empty(), "a new list replaces the note");
        assert_eq!(str_list(store::agent_get(&store::load(), ID, "customModels")).unwrap(), ["mine"], "saving the note keeps the rest of the store");
        assert_eq!(fs::read_to_string(codex.join("config.toml")).unwrap(), cfg, "already pointing at the catalog: untouched");
    }

    /// The installed Codex really lists its models: `cargo test real_codex_lists_models -- --ignored`.
    #[test]
    #[ignore]
    fn real_codex_lists_models() {
        let slugs = |v: &Value| v["models"].as_array().unwrap().iter().map(|m| m["slug"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let exes = crate::process::codex_clis();
        assert!(!exes.is_empty(), "Codex not installed");
        for exe in &exes {
            let ran = builtin_models(exe).map(|v| slugs(&v));
            let read = embedded_catalog(&fs::read(exe).unwrap()).map(|v| slugs(&v));
            println!("{}\n  run: {ran:?}\n  read: {read:?}", exe.display());
            assert_eq!(ran.ok(), read, "running and reading give the same list");
        }
    }
}
