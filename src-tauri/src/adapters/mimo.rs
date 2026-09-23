//! MiMo Desktop: engine config `~/.config/mimocode/mimocode.jsonc` (OpenCode-style
//! `provider.<id>.models`), app preferences in `%APPDATA%\Xiaomi MiMo\preferences.json`.
//! Hiding a model / disabling a provider removes its entry from mimocode.jsonc and
//! stashes the definition in the AgentPlus store so it can be restored untouched.

use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub const ID: &str = "mimo";
const SKILLS: [&str; 4] = ["agents", "claude", "codex", "opencode"];
const PREFS: [(&str, &str, &str); 4] = [
    ("trayEnabled", "托盘图标", "trayEnabled"),
    ("voiceFeedback", "语音反馈", "voiceFeedback"),
    ("gitVersionPinEnabled", "Git 版本固定", "gitVersionPinEnabled"),
    ("uncommittedHintEnabled", "提示未提交的改动", "uncommittedHintEnabled"),
];

fn engine_path() -> PathBuf {
    home().join(".config").join("mimocode").join("mimocode.jsonc")
}
fn app_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(home).join("Xiaomi MiMo")
}
fn prefs_path() -> PathBuf {
    app_dir().join("preferences.json")
}
fn catalog_path() -> PathBuf {
    app_dir().join("model-catalog.json")
}

fn api_of(npm: &str) -> &'static str {
    if npm.contains("anthropic") {
        "Anthropic"
    } else if npm.ends_with("/openai") {
        "Responses"
    } else {
        "Chat"
    }
}

/// Returns (config, meta, has_comments).
fn load_engine() -> Result<(Value, TextMeta, bool)> {
    let (text, meta) = read_text(&engine_path())?;
    let (clean, had) = strip_jsonc(&text);
    let v = serde_json::from_str(&clean).map_err(|e| anyhow!("mimocode.jsonc 解析失败：{e}"))?;
    Ok((v, meta, had))
}

fn stash(root: &Value, key: &str) -> Map<String, Value> {
    root.get(ID).and_then(|a| a.get(key)).and_then(|x| x.as_object()).cloned().unwrap_or_default()
}

fn ctx_of(def: &Value) -> Option<String> {
    def.pointer("/limit/context").and_then(|x| x.as_u64()).map(fmt_ctx)
}

fn provider_from(id: &str, def: &Value, enabled: bool, hidden: &Map<String, Value>) -> Provider {
    let base = def.pointer("/options/baseURL").and_then(|x| x.as_str()).map(String::from);
    let npm = def.get("npm").and_then(|x| x.as_str()).unwrap_or("");
    let mut models: Vec<Model> = def
        .get("models")
        .and_then(|m| m.as_object())
        .map(|m| m.iter().map(|(mid, d)| Model { id: mid.clone(), visible: true, readonly: false, tags: vec![], ctx: ctx_of(d) }).collect())
        .unwrap_or_default();
    let prefix = format!("{id}|");
    for (k, d) in hidden {
        if let Some(mid) = k.strip_prefix(&prefix) {
            models.push(Model { id: mid.to_string(), visible: false, readonly: false, tags: vec![], ctx: ctx_of(d) });
        }
    }
    Provider {
        id: id.into(),
        name: def.get("name").and_then(|x| x.as_str()).unwrap_or(id).to_string(),
        host: base.as_deref().map(host_of).unwrap_or_default(),
        base_url: base,
        apis: vec![api_of(npm).into()],
        builtin: false,
        enabled,
        compatible: true,
        reason: None,
        models,
        details: vec![
            Kv::mono("配置 ID", format!("provider.{id}")),
            Kv::mono("npm", if npm.is_empty() { "-".into() } else { npm.to_string() }),
            Kv::text("密钥", if def.pointer("/options/apiKey").and_then(|x| x.as_str()).map(|k| !k.is_empty()).unwrap_or(false) {
                "API Key · 明文保存在 mimocode.jsonc"
            } else {
                "未填写"
            }),
            Kv::text("状态", if enabled { "已启用" } else { "已停用 · 定义暂存在 AgentPlus" }),
        ],
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: "MiMo Desktop".into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "multi".into(),
        config_dir: engine_path().parent().unwrap().to_string_lossy().to_string(),
        files: vec![display_path(&engine_path()), display_path(&prefs_path())],
        current_provider: None,
        providers: vec![],
        catalog: None,
        catalog_file: None,
        settings: vec![],
        current: vec![],
        notes: vec![],
        readonly: false,
    };
    let root = store::load();
    let hidden = stash(&root, "hiddenModels");
    let parked = stash(&root, "disabledProviders");

    // Account models shipped by MiMo itself (read-only).
    if let Ok((cat, _)) = read_json(&catalog_path()) {
        let models: Vec<Model> = cat
            .get("models")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter(|m| m.get("modelType").and_then(|x| x.as_str()) == Some("TEXT"))
                    .filter_map(|m| m.get("id").and_then(|x| x.as_str()))
                    .map(|id| Model { id: id.into(), visible: true, readonly: true, tags: vec![], ctx: None })
                    .collect()
            })
            .unwrap_or_default();
        st.providers.push(Provider {
            id: "account".into(),
            name: "MiMo 账号内置".into(),
            base_url: None,
            host: "小米账号登录".into(),
            apis: vec!["账号".into()],
            builtin: true,
            enabled: true,
            compatible: true,
            reason: None,
            models,
            details: vec![
                Kv::text("认证方式", "小米账号登录"),
                Kv::mono("来源", "model-catalog.json"),
                Kv::text("说明", "MiMo Desktop 内置，模型列表由 MiMo 管理"),
            ],
        });
    }

    match load_engine() {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push("mimocode.jsonc 含注释，写回会丢失注释，已切换为只读。".into());
            }
            if let Some(p) = cfg.get("provider").and_then(|x| x.as_object()) {
                for (id, def) in p {
                    st.providers.push(provider_from(id, def, true, &hidden));
                }
            }
            for (id, def) in &parked {
                st.providers.push(provider_from(id, def, false, &hidden));
            }
        }
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
        }
    }

    let prefs = read_json(&prefs_path()).ok().map(|x| x.0).unwrap_or(json!({}));
    let get_b = |k: &str| prefs.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let skills: Vec<String> = SKILLS
        .iter()
        .filter(|s| prefs.pointer(&format!("/skillPathCompat/{s}")).and_then(|x| x.as_bool()).unwrap_or(false))
        .map(|s| format!("~/.{s}"))
        .collect();
    st.settings = vec![chips_setting(
        "skills",
        "技能",
        "读取其他工具的技能目录",
        "skillPathCompat：只读兼容，MiMo 不会写入这些目录",
        skills.clone(),
        &["~/.agents", "~/.claude", "~/.codex", "~/.opencode"],
    )];
    st.settings.extend(PREFS.iter().map(|(k, l, d)| bool_setting(k, "应用", l, d, get_b(k))));

    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::text("供应商", on.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join("、")),
        Kv::mono("默认模型", prefs.get("model").and_then(|x| x.as_str()).unwrap_or("-").to_string()),
        Kv::text("可见模型", format!("{vis} 个")),
        Kv::mono("技能兼容", if skills.is_empty() { "无".into() } else { skills.join(" ") }),
        Kv::text("托盘图标", if get_b("trayEnabled") { "开" } else { "关" }),
    ];
    st
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    let (mut cfg, cfg_meta, had_comments) = load_engine()?;
    let (mut prefs, prefs_meta) = read_json(&prefs_path())?;
    let mut root = store::load();
    let (ef, pf) = (display_path(&engine_path()), display_path(&prefs_path()));
    let mut diff = Diff::default();
    let (mut cfg_dirty, mut prefs_dirty, mut store_dirty) = (false, false, false);

    for op in ops {
        match op {
            Op::SetProviderEnabled { provider, enabled } => {
                let providers = cfg.as_object_mut().unwrap().entry("provider").or_insert_with(|| json!({}));
                let providers = providers.as_object_mut().ok_or_else(|| anyhow!("provider 不是对象"))?;
                let parked = store::section(&mut root, ID, "disabledProviders");
                if *enabled {
                    if let Some(def) = parked.remove(provider) {
                        providers.insert(provider.clone(), def);
                        diff.push(&ef, format!("+ provider.{provider}"), true);
                        cfg_dirty = true;
                        store_dirty = true;
                    }
                } else if let Some(def) = providers.remove(provider) {
                    parked.insert(provider.clone(), def);
                    diff.push(&ef, format!("- provider.{provider}（定义暂存在 AgentPlus，可恢复）"), false);
                    cfg_dirty = true;
                    store_dirty = true;
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let key = format!("{provider}|{model}");
                let models = cfg
                    .pointer_mut(&format!("/provider/{provider}"))
                    .and_then(|p| p.as_object_mut())
                    .ok_or_else(|| anyhow!("供应商 {provider} 未启用，先启用再调整模型"))?
                    .entry("models")
                    .or_insert_with(|| json!({}));
                let models = models.as_object_mut().ok_or_else(|| anyhow!("models 不是对象"))?;
                let hidden = store::section(&mut root, ID, "hiddenModels");
                if *visible {
                    if !models.contains_key(model) {
                        models.insert(model.clone(), hidden.remove(&key).unwrap_or_else(|| json!({})));
                        diff.push(&ef, format!("provider.{provider}.models + \"{model}\""), true);
                        cfg_dirty = true;
                        store_dirty = true;
                    }
                } else if let Some(def) = models.remove(model) {
                    hidden.insert(key, def);
                    diff.push(&ef, format!("provider.{provider}.models - \"{model}\""), false);
                    cfg_dirty = true;
                    store_dirty = true;
                }
            }
            Op::SetSetting { key, value } => {
                if key == "skills" {
                    let want: Vec<String> = value.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    for s in SKILLS {
                        let on = want.iter().any(|w| w == &format!("~/.{s}"));
                        let was = prefs.pointer(&format!("/skillPathCompat/{s}")).and_then(|x| x.as_bool()).unwrap_or(false);
                        if on != was {
                            if !prefs.get("skillPathCompat").map(|x| x.is_object()).unwrap_or(false) {
                                prefs["skillPathCompat"] = json!({});
                            }
                            prefs["skillPathCompat"][s] = json!(on);
                            diff.push(&pf, format!("skillPathCompat.{s} = {on}"), on);
                            prefs_dirty = true;
                        }
                    }
                } else if PREFS.iter().any(|(k, ..)| k == key) {
                    let on = value.as_bool().unwrap_or(false);
                    if prefs.get(key).and_then(|x| x.as_bool()).unwrap_or(false) != on {
                        prefs[key.as_str()] = json!(on);
                        diff.push(&pf, format!("{key} = {on}"), on);
                        prefs_dirty = true;
                    }
                } else {
                    return Err(anyhow!("未知设置 {key}"));
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!("MiMo Desktop 按启用/停用管理供应商")),
        }
    }

    if cfg_dirty && had_comments {
        return Err(anyhow!("mimocode.jsonc 含注释，为避免丢失注释不写入"));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (cfg_dirty || prefs_dirty) {
        let mut targets = vec![];
        if cfg_dirty { targets.push(engine_path()) }
        if prefs_dirty { targets.push(prefs_path()) }
        backup_dir = Some(backup(ID, &targets)?);
        if cfg_dirty {
            write_json(&engine_path(), &cfg, cfg_meta)?;
            written.push(engine_path());
        }
        if prefs_dirty {
            write_json(&prefs_path(), &prefs, prefs_meta)?;
            written.push(prefs_path());
        }
    }
    if !dry_run && store_dirty {
        store::save(&root)?;
    }
    Ok((diff, written, backup_dir))
}
