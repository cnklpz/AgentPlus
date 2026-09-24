//! Kilo Code 7.x (CLI `kilo` and the VS Code extension; an OpenCode fork): the same
//! config format as OpenCode, in `~/.config/kilo/kilo.json(c)` (or `opencode.json` there,
//! or the file named by `KILO_CONFIG`), keys in `~/.local/share/kilo/auth.json`.
//! Disabling uses the native `disabled_providers` list; providers logged in through
//! `kilo auth` without a config entry show up read-only.

use super::{Plan, Endpoint};
use super::ocfmt::{Dirty, Fmt};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub const ID: &str = "kilo";
#[allow(dead_code)]
pub const NAME: &str = "Kilo Code";
/// `kilo.jsonc` / `opencode.json` in the same dir also count (see `config_path`).
#[allow(dead_code)]
pub const MARKER: &str = "kilo.json";
#[allow(dead_code)]
pub const WSL_SCRIPT: &str = "(kilo --version || kilocode --version) 2>/dev/null | head -n 1; pgrep -x kilo >/dev/null && echo @running; true";
#[allow(dead_code)]
pub const WSL_MARKER: &str = ".config/kilo";

#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests point the adapter at a temp home (config, auth, store and backups all inside it).
fn test_home() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_HOME.with(|t| t.borrow().clone())
    }
    #[cfg(not(test))]
    {
        None
    }
}

fn base_home() -> PathBuf {
    test_home().unwrap_or_else(home)
}

/// Kilo's default config dir, `~/.config/kilo`.
#[allow(dead_code)]
pub fn default_dir() -> PathBuf {
    base_home().join(".config").join("kilo")
}

fn dir() -> PathBuf {
    if test_home().is_some() {
        return default_dir();
    }
    super::dir_override(ID).unwrap_or_else(|| env_config().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(default_dir))
}

/// `KILO_CONFIG` (a file path) only applies to the Windows side.
fn env_config() -> Option<PathBuf> {
    if test_home().is_some() || crate::env::is_wsl() {
        return None;
    }
    std::env::var_os("KILO_CONFIG").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

/// The config file Kilo reads: `KILO_CONFIG`, else the first existing of
/// kilo.jsonc / kilo.json / opencode.json, else a new kilo.json.
fn config_path() -> PathBuf {
    if test_home().is_none() && super::dir_override(ID).is_none() {
        if let Some(p) = env_config() {
            return p;
        }
    }
    let d = dir();
    ["kilo.jsonc", "kilo.json", "opencode.json"].iter().map(|n| d.join(n)).find(|p| p.exists()).unwrap_or_else(|| d.join("kilo.json"))
}

fn auth_path() -> PathBuf {
    base_home().join(".local").join("share").join("kilo").join("auth.json")
}

fn fmt() -> Fmt {
    Fmt { agent: ID, path: config_path(), auth: Some(auth_path()), native_disable: true }
}

fn load_root() -> Value {
    match test_home() {
        Some(h) => std::fs::read_to_string(h.join("agentplus-store.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({})),
        None => store::load(),
    }
}

fn save_root(v: &Value) -> Result<()> {
    match test_home() {
        Some(h) => Ok(std::fs::write(h.join("agentplus-store.json"), serde_json::to_string_pretty(v)?)?),
        None => store::save(v),
    }
}

fn do_backup(files: &[PathBuf]) -> Result<PathBuf> {
    match test_home() {
        Some(h) => {
            let d = h.join("agentplus-backup");
            std::fs::create_dir_all(&d)?;
            for f in files.iter().filter(|f| f.exists()) {
                std::fs::copy(f, d.join(f.file_name().unwrap()))?;
            }
            Ok(d)
        }
        None => backup(ID, files),
    }
}

// ---------- detection ----------

fn npm_version(pkg: &str) -> Option<String> {
    let p = dirs::data_dir()?.join("npm").join("node_modules").join(pkg).join("package.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

/// Newest `kilocode.kilo-code-<version>` folder in the VS Code extensions dir.
fn vscode_extension() -> Option<String> {
    let dir = dirs::home_dir()?.join(".vscode").join("extensions");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.strip_prefix("kilocode.kilo-code-").map(|v| v.split('-').next().unwrap_or(v).to_string())
        })
        .max_by_key(|v| v.split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>())
}

/// The `kilo` CLI (npm `@kilocode/cli` or a `kilo` binary on PATH) or the VS Code extension.
#[allow(dead_code)]
pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some(v) = npm_version("@kilocode/cli") {
        inst.installed = true;
        inst.version = Some(v);
    } else if let Some(exe) = crate::process::on_path(&["kilo.exe", "kilo.cmd", "kilocode.cmd"]) {
        inst.installed = true;
        if exe.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
            static V: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
            inst.version = V.get_or_init(|| crate::process::cli_version(&exe)).clone();
        }
    } else if let Some(v) = vscode_extension() {
        inst.installed = true;
        inst.version = Some(v);
    }
    inst.running = inst.installed && crate::process::any_process(|name, _| name.eq_ignore_ascii_case("kilo.exe"));
    inst
}

// ---------- state / plan ----------

#[allow(dead_code)]
pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = AgentState {
        id: ID.into(),
        name: NAME.into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "multi".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![f.file(), display_path(&auth_path())],
        current_provider: None,
        providers: vec![],
        catalog: None,
        catalog_file: None,
        settings: vec![],
        current: vec![],
        notes: vec![],
        readonly: false,
        fixed_pending: false,
        fixed_prompt: false,
        restartable: false,
        model_fields: vec![],
    };
    let root = load_root();
    let cfg = match f.load(true) {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push(tr!(
                    "{} 含注释，写回会丢失注释，已切换为只读。",
                    "{} contains comments, which would be lost on write, so it's read-only.",
                    config_path().file_name().unwrap().to_string_lossy()
                ));
            }
            cfg
        }
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    st.providers = f.providers(&cfg, &root);

    // Logged in with `kilo auth` (Kilo account or a built-in provider) but not configured here.
    if let Some((auth, _)) = f.load_auth() {
        if let Some(obj) = auth.as_object() {
            for (id, e) in obj {
                if st.providers.iter().any(|p| &p.id == id) {
                    continue;
                }
                let kind = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
                st.providers.push(Provider {
                    id: id.clone(),
                    name: id.clone(),
                    base_url: None,
                    host: if kind == "oauth" { l("账号登录（kilo auth）", "Account sign-in (kilo auth)").into() } else { l("内置供应商 · API Key", "Built-in provider · API key").into() },
                    apis: vec![l("内置", "Built-in").into()],
                    builtin: true,
                    enabled: true,
                    compatible: true,
                    reason: None,
                    models: vec![],
                    details: vec![
                        Kv::mono(l("凭据", "Credentials"), format!("auth.json · {id} · {}", if kind == "oauth" { l("OAuth 登录", "OAuth sign-in") } else { l("API Key", "API key") })),
                        Kv::text(
                            l("说明", "Note"),
                            l(
                                "Kilo Code 内置的供应商，模型列表来自 models.dev / Kilo 网关，在 Kilo 里用 /models 选择",
                                "A provider built into Kilo Code; its model list comes from models.dev / the Kilo gateway. Pick a model with /models in Kilo",
                            ),
                        ),
                    ],
                    editable: false,
                    api: "chat".into(),
                    has_key: true,
                    key_fp: None,
                    key_hint: None,
                    official_auth: false,
                });
            }
        }
    }

    let get_s = |k: &str| cfg.get(k).and_then(|x| x.as_str()).map(String::from);
    st.settings = vec![
        bool_setting(
            "autoupdate",
            NAME,
            l("自动更新", "Auto-update"),
            l("autoupdate：启动时自动下载新版本", "autoupdate: download new versions automatically on startup"),
            cfg.get("autoupdate").and_then(|x| x.as_bool()).unwrap_or(true),
        ),
        bool_setting(
            "share_disabled",
            NAME,
            l("禁用会话分享", "Disable session sharing"),
            l("share = \"disabled\"：不允许把会话分享成公开链接", "share = \"disabled\": don't allow sharing sessions as public links"),
            get_s("share").as_deref() == Some("disabled"),
        ),
    ];
    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled && !p.builtin).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::text(
            l("自定义供应商", "Custom providers"),
            if on.is_empty() { l("无", "None").into() } else { on.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(l("、", ", ")) },
        ),
        Kv::mono(l("默认模型", "Default model"), get_s("model").unwrap_or_else(|| "-".into())),
        Kv::mono(l("小模型", "Small model"), get_s("small_model").unwrap_or_else(|| "-".into())),
        Kv::text(l("可见模型", "Visible models"), tr!("{vis} 个", "{vis}")),
        Kv::mono(l("配置文件", "Config file"), f.file()),
    ];
    st.notes.push(
        l(
            "Kilo Code CLI 与 VS Code 扩展共用这份配置；改动对新会话生效。",
            "The Kilo Code CLI and VS Code extension share this config; changes apply to new sessions.",
        )
        .into(),
    );
    st
}

#[allow(dead_code)]
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    fmt().endpoint(id)
}

#[allow(dead_code)]
pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let f = fmt();
    let existed = config_path().exists();
    let (mut cfg, cfg_meta, had_comments) = f.load(true)?;
    if !existed {
        // ocfmt seeds OpenCode's schema URL; a new Kilo file goes without one.
        if let Some(o) = cfg.as_object_mut() {
            o.remove("$schema");
        }
    }
    let mut auth = f.load_auth();
    let mut root = load_root();
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();
    let ef = f.file();

    for op in ops {
        if f.apply(op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty)? {
            continue;
        }
        match op {
            Op::SetSetting { key, value } => {
                let on = value.as_bool().unwrap_or(false);
                match key.as_str() {
                    "autoupdate" => {
                        if cfg.get("autoupdate").and_then(|x| x.as_bool()).unwrap_or(true) != on {
                            cfg["autoupdate"] = json!(on);
                            diff.push(&ef, format!("autoupdate = {on}"), on);
                            dirty.cfg = true;
                        }
                    }
                    "share_disabled" => {
                        let now = cfg.get("share").and_then(|x| x.as_str()) == Some("disabled");
                        if now != on {
                            if on {
                                cfg["share"] = json!("disabled");
                            } else if let Some(o) = cfg.as_object_mut() {
                                o.remove("share");
                            }
                            diff.push(&ef, if on { "share = \"disabled\"".to_string() } else { "- share".to_string() }, on);
                            dirty.cfg = true;
                        }
                    }
                    other => return Err(anyhow!(tr!("未知设置 {other}", "Unknown setting: {other}"))),
                }
            }
            Op::SetCurrentProvider { .. } => {
                return Err(anyhow!(l(
                    "Kilo Code 按启用/停用管理供应商，在 Kilo 里用 /models 选择模型",
                    "Kilo Code manages providers by enabling and disabling them; pick a model with /models in Kilo"
                )))
            }
            Op::SetProviderModels { .. } => return Err(anyhow!(l("每个供应商的模型已经各自独立，请直接编辑模型", "Each provider already has its own models; edit the models directly"))),
            Op::SetModelRoles { .. } => return Err(anyhow!(l("只有 Claude Code 需要分配模型角色", "Only Claude Code needs model roles"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            _ => unreachable!("handled by ocfmt"),
        }
    }

    if dirty.cfg && had_comments {
        return Err(anyhow!(l("配置文件含注释，为避免丢失注释不写入", "The config file contains comments; not writing it so they aren't lost")));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (dirty.cfg || dirty.auth) {
        let mut targets = vec![];
        if dirty.cfg {
            targets.push(config_path());
        }
        if dirty.auth {
            targets.push(auth_path());
        }
        backup_dir = Some(do_backup(&targets)?);
        if dirty.cfg {
            if let Some(d) = config_path().parent() {
                std::fs::create_dir_all(d)?;
            }
            write_json(&config_path(), &cfg, cfg_meta)?;
            written.push(config_path());
        }
        if dirty.auth {
            // Written in place (not tmp + rename) so the file keeps its owner-only permissions.
            let (a, _) = auth.as_ref().unwrap();
            if let Some(d) = auth_path().parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(auth_path(), serde_json::to_string_pretty(a)? + "\n")?;
            written.push(auth_path());
        }
    }
    if !dry_run && dirty.store {
        save_root(&root)?;
    }
    Ok((diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "$schema": "https://app.kilo.ai/config.json",
  "theme": "kilo",
  "model": "relay/glm-4.6",
  "provider": {
    "relay": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "My Relay",
      "options": { "baseURL": "https://relay.example.com/v1" },
      "models": {
        "glm-4.6": { "name": "GLM 4.6", "limit": { "context": 200000 } },
        "kimi-k2": {}
      }
    }
  },
  "mcp": { "x": { "type": "local", "command": ["x"] } }
}
"#;

    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            TEST_HOME.with(|t| *t.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str, cfg: Option<&str>, auth: Option<&str>) -> Home {
        let h = std::env::temp_dir().join(format!("agentplus-kilo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join(".config/kilo")).unwrap();
        if let Some(c) = cfg {
            std::fs::write(h.join(".config/kilo/kilo.json"), c).unwrap();
        }
        if let Some(a) = auth {
            std::fs::create_dir_all(h.join(".local/share/kilo")).unwrap();
            std::fs::write(h.join(".local/share/kilo/auth.json"), a).unwrap();
        }
        TEST_HOME.with(|t| *t.borrow_mut() = Some(h.clone()));
        Home(h)
    }

    fn cfg_of(h: &Home) -> Value {
        serde_json::from_str(&std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap()).unwrap()
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.clone())).collect::<Vec<_>>().join("\n")
    }

    fn upsert(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput {
                id: id.map(String::from),
                name: name.into(),
                base_url: base.into(),
                api: api.into(),
                api_key: key.map(String::from),
                models: models.iter().map(|s| s.to_string()).collect(),
                key_from_library: None,
                official_auth: None,
            },
        }
    }

    #[test]
    fn reads_state() {
        let _h = setup("state", Some(SAMPLE), Some(r#"{"relay":{"type":"api","key":"sk-relay-abcd"},"kilo":{"type":"oauth","access":"x"}}"#));
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert_eq!(st.providers.len(), 2);
        let p = &st.providers[0];
        assert_eq!((p.id.as_str(), p.name.as_str(), p.api.as_str(), p.has_key), ("relay", "My Relay", "chat", true));
        assert_eq!(p.models.len(), 2);
        assert_eq!(p.models[0].context, Some(200000));
        assert!(st.providers[1].builtin);
        let (base, key, api) = provider_endpoint("relay").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://relay.example.com/v1", Some("sk-relay-abcd"), "chat"));
    }

    #[test]
    fn missing_file_and_create() {
        let h = setup("create", None, None);
        let st = state(&Install::default());
        assert!(!st.readonly && st.providers.is_empty());
        let (d, w, _) = plan(&[upsert(None, "New One", "https://n.example.com/v1", "anthropic", Some("sk-new-9876"), &["m1"])], true).unwrap();
        assert!(w.is_empty());
        assert!(!h.0.join(".config/kilo/kilo.json").exists());
        let t = diff_text(&d);
        assert!(t.contains("••••9876") && !t.contains("sk-new-9876"), "{t}");
        let (_, w, _) = plan(&[upsert(None, "New One", "https://n.example.com/v1", "anthropic", Some("sk-new-9876"), &["m1"])], false).unwrap();
        assert_eq!(w.len(), 2); // config + auth.json (key goes to auth.json)
        let c = cfg_of(&h);
        assert!(c.get("$schema").is_none());
        assert_eq!(c.pointer("/provider/new-one/npm").unwrap(), "@ai-sdk/anthropic");
        let a: Value = serde_json::from_str(&std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap()).unwrap();
        assert_eq!(a.pointer("/new-one/key").unwrap(), "sk-new-9876");
    }

    #[test]
    fn edit_hide_disable_delete_roundtrip() {
        let h = setup("ops", Some(SAMPLE), None);
        let ops = vec![
            upsert(Some("relay"), "Relay 2", "https://relay2.example.com/v1", "responses", None, &[]),
            Op::SetModelVisible { provider: "relay".into(), model: "kimi-k2".into(), visible: false },
            Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: "qwen3".into(), name: Some("Qwen 3".into()), context: Some(128000), ..Default::default() } },
            Op::SetProviderEnabled { provider: "relay".into(), enabled: false },
        ];
        let (_, w, _) = plan(&ops, false).unwrap();
        assert_eq!(w.len(), 1);
        let c = cfg_of(&h);
        assert_eq!(c.pointer("/provider/relay/options/baseURL").unwrap(), "https://relay2.example.com/v1");
        assert_eq!(c.pointer("/provider/relay/npm").unwrap(), "@ai-sdk/openai");
        assert!(c.pointer("/provider/relay/models/kimi-k2").is_none());
        assert_eq!(c.pointer("/provider/relay/models/qwen3/limit/context").unwrap(), 128000);
        assert_eq!(c["disabled_providers"], json!(["relay"]));
        // unrelated content kept, in order
        assert_eq!(c["theme"], "kilo");
        assert_eq!(c["mcp"]["x"]["type"], "local");
        assert_eq!(c.as_object().unwrap().keys().next().unwrap(), "$schema");
        let st = state(&Install::default());
        let p = &st.providers[0];
        assert!(!p.enabled);
        assert!(p.models.iter().any(|m| m.id == "kimi-k2" && !m.visible));
        // show again + enable
        plan(&[Op::SetProviderEnabled { provider: "relay".into(), enabled: true }, Op::SetModelVisible { provider: "relay".into(), model: "kimi-k2".into(), visible: true }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(c["disabled_providers"], json!([]));
        assert!(c.pointer("/provider/relay/models/kimi-k2").is_some());
        plan(&[Op::DeleteModel { provider: "relay".into(), model: "qwen3".into() }, Op::DeleteProvider { provider: "relay".into() }], false).unwrap();
        assert!(cfg_of(&h).pointer("/provider/relay").is_none());
        assert!(plan(&[Op::SetModelRoles { provider: "relay".into(), roles: Default::default() }], true).is_err());
    }

    #[test]
    fn model_ids_with_slashes() {
        let h = setup("slash", Some(SAMPLE), None);
        let id = "anthropic/claude-sonnet-4~beta";
        let add = |ctx| Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: id.into(), context: Some(ctx), ..Default::default() } };
        plan(&[add(200000)], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["models"][id]["limit"]["context"], 200000);
        // Editing the existing entry keeps its other fields.
        plan(&[Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: id.into(), name: Some("Sonnet".into()), ..Default::default() } }], false).unwrap();
        let m = &cfg_of(&h)["provider"]["relay"]["models"][id];
        assert_eq!((m["name"].as_str(), m["limit"]["context"].as_u64()), (Some("Sonnet"), Some(200000)));
        plan(&[Op::SetModelVisible { provider: "relay".into(), model: id.into(), visible: false }], false).unwrap();
        assert!(cfg_of(&h)["provider"]["relay"]["models"].get(id).is_none());
        plan(&[Op::SetModelVisible { provider: "relay".into(), model: id.into(), visible: true }], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["models"][id]["name"], "Sonnet");
        plan(&[Op::DeleteModel { provider: "relay".into(), model: id.into() }], false).unwrap();
        assert!(cfg_of(&h)["provider"]["relay"]["models"].get(id).is_none());
    }

    #[test]
    fn unreadable_auth_is_never_rewritten() {
        let broken = r#"{"kilo":{"type":"oauth","access":"x"},"#;
        let h = setup("badauth", Some(SAMPLE), Some(broken));
        let cfg0 = std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap();
        // Setting a key fails instead of rewriting auth.json or putting the key in the config.
        assert!(plan(&[upsert(Some("relay"), "My Relay", "https://relay.example.com/v1", "chat", Some("sk-new-1234"), &[])], false).is_err());
        assert_eq!(std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap(), broken);
        assert_eq!(std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap(), cfg0);
        // Everything that doesn't touch keys still works.
        plan(&[upsert(Some("relay"), "Relay 2", "https://relay.example.com/v1", "chat", None, &[])], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["name"], "Relay 2");
        assert!(cfg_of(&h)["provider"]["relay"]["options"].get("apiKey").is_none());
        // A non-object auth.json is left alone too.
        std::fs::write(h.0.join(".local/share/kilo/auth.json"), "[]").unwrap();
        assert!(plan(&[upsert(Some("relay"), "My Relay", "https://relay.example.com/v1", "chat", Some("sk-new-5678"), &[])], false).is_err());
        assert_eq!(std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap(), "[]");
    }

    #[test]
    fn comments_make_readonly() {
        let _h = setup("jsonc", Some("{\n  // hi\n  \"provider\": {}\n}\n"), None);
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[upsert(None, "x", "https://x/v1", "chat", None, &["m"])], true).is_err());
    }

    /// Read-only look at the real machine: state and a dry-run plan (nothing is written).
    #[test]
    #[ignore]
    fn dump_kilo() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={}", inst.installed, inst.version, inst.running);
        let st = state(&inst);
        println!("dir={} files={:?} readonly={} notes={:?}", st.config_dir, st.files, st.readonly, st.notes);
        for p in &st.providers {
            println!("  provider {} ({}) api={} enabled={} has_key={} models={:?}", p.id, p.name, p.api, p.enabled, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        for kv in &st.current {
            println!("  {} = {}", kv.k, kv.v);
        }
        let (d, w, b) = plan(&[upsert(None, "Dry Run", "https://dry.example.com/v1", "chat", Some("sk-dryrun-0000"), &["m1"])], true).unwrap();
        println!("dry-run diff:\n{}", diff_text(&d));
        assert!(w.is_empty() && b.is_none());
    }
}
