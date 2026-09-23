//! Codex (desktop + CLI share `~/.codex/config.toml`).
//! Model picker contents come from the catalog named by `model_catalog_json`;
//! each entry's `visibility` ("list" | "hide") decides whether it shows up.

use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;
use toml_edit::{value, Array, DocumentMut, Item};

pub const ID: &str = "codex";
const EFFORTS: [&str; 7] = ["low", "medium", "high", "xhigh", "persistent", "ultra", "max"];
const INJECT_FILE: &str = "AgentPlus · Codex 界面注入";
/// Codex writes the catalog's Fast tier id ("priority") when Fast is picked in its menu.
const FAST_TIER: &str = "priority";

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn load_doc() -> Result<(DocumentMut, TextMeta)> {
    let (text, meta) = read_text(&config_path())?;
    let doc = text.parse::<DocumentMut>().map_err(|e| anyhow!("config.toml 解析失败：{e}"))?;
    Ok((doc, meta))
}

fn catalog_path(doc: &DocumentMut) -> Option<PathBuf> {
    doc.get("model_catalog_json").and_then(|i| i.as_str()).map(expand_tilde)
}

fn str_list(item: Option<&Item>) -> Option<Vec<String>> {
    item.and_then(|i| i.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

fn desktop_bool(doc: &DocumentMut, key: &str, default: bool) -> bool {
    doc.get("desktop").and_then(|t| t.get(key)).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn efforts(doc: &DocumentMut) -> Vec<String> {
    str_list(doc.get("desktop").and_then(|t| t.get("enabled-reasoning-efforts")))
        .unwrap_or_else(|| vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()])
}

fn status_line(doc: &DocumentMut) -> Option<Vec<String>> {
    str_list(doc.get("tui").and_then(|t| t.get("status_line")))
}

fn service_tier(doc: &DocumentMut) -> String {
    doc.get("service_tier").and_then(|v| v.as_str()).unwrap_or("default").to_string()
}

fn is_fast_tier(t: &str) -> bool {
    t == FAST_TIER || t == "fast"
}

fn current_provider(doc: &DocumentMut) -> String {
    doc.get("model_provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string()
}

fn providers(doc: &DocumentMut) -> Vec<Provider> {
    let mut out = vec![Provider {
        id: "openai".into(),
        name: "OpenAI 官方".into(),
        base_url: None,
        host: "ChatGPT 账号登录".into(),
        apis: vec!["Responses".into()],
        builtin: true,
        enabled: true,
        compatible: true,
        reason: None,
        models: vec![],
        details: vec![
            Kv::text("认证方式", "ChatGPT 账号登录（~/.codex/auth.json）"),
            Kv::text("Fast", "账号登录时 Codex 原生显示"),
            Kv::mono("配置 ID", "openai（内置）"),
        ],
    }];
    if let Some(t) = doc.get("model_providers").and_then(|i| i.as_table_like()) {
        for (id, item) in t.iter() {
            let get = |k: &str| item.get(k).and_then(|v| v.as_str()).map(String::from);
            let wire = get("wire_api").unwrap_or_else(|| "responses".into());
            let base = get("base_url");
            let chat = wire == "chat";
            let env_key = get("env_key");
            let mut details = vec![
                Kv::mono("配置 ID", format!("[model_providers.{id}]")),
                Kv::mono("wire_api", format!("\"{wire}\"")),
            ];
            match &env_key {
                Some(k) => details.push(Kv::text("密钥", format!("环境变量 {k} · {}", key_status(k)))),
                None => details.push(Kv::text("密钥", "未设置 env_key")),
            }
            details.push(Kv::text("Fast", "Codex 默认隐藏（可在「其他设置」注入显示）"));
            out.push(Provider {
                details,
                id: id.to_string(),
                name: get("name").unwrap_or_else(|| id.to_string()),
                host: base.as_deref().map(host_of).unwrap_or_default(),
                base_url: base,
                apis: vec![if chat { "Chat".into() } else { "Responses".into() }],
                builtin: false,
                enabled: true,
                compatible: !chat,
                reason: if chat { Some("Codex 已不支持 Chat 接口".into()) } else { None },
                models: vec![],
            });
        }
    }
    out
}

/// Where an env_key's value would come from — reports presence only, never the value.
fn key_status(name: &str) -> &'static str {
    let in_dotenv = std::fs::read_to_string(codex_home().join(".env"))
        .map(|s| s.lines().any(|l| l.trim_start().strip_prefix(name).map(|r| r.trim_start().starts_with('=')).unwrap_or(false)))
        .unwrap_or(false);
    if in_dotenv {
        "已在 ~/.codex/.env 配置"
    } else if std::env::var_os(name).is_some() {
        "已在系统环境变量配置"
    } else {
        "未找到，请求会失败"
    }
}

fn load_catalog(doc: &DocumentMut) -> Option<(PathBuf, Value, TextMeta)> {
    let p = catalog_path(doc)?;
    let (v, meta) = read_json(&p).ok()?;
    Some((p, v, meta))
}

fn catalog_models(v: &Value) -> Vec<Model> {
    let empty = vec![];
    let list = v.get("models").and_then(|m| m.as_array()).unwrap_or(&empty);
    list.iter()
        .filter_map(|m| {
            let id = m.get("slug")?.as_str()?.to_string();
            let tiers = m.get("service_tiers");
            let fast = match tiers {
                Some(Value::Array(a)) => a.iter().any(|t| t.get("id").and_then(|x| x.as_str()) == Some(FAST_TIER)),
                Some(Value::Object(o)) => o.get("id").and_then(|x| x.as_str()) == Some(FAST_TIER),
                _ => false,
            } || m.get("additional_speed_tiers").map(|x| x.to_string().contains("fast")).unwrap_or(false);
            let mut tags = vec![];
            if fast {
                tags.push("Fast".to_string());
            }
            Some(Model {
                id,
                visible: m.get("visibility").and_then(|x| x.as_str()) != Some("hide"),
                readonly: false,
                tags,
                ctx: m.get("context_window").and_then(|x| x.as_u64()).map(fmt_ctx),
            })
        })
        .collect()
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: "Codex".into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "single".into(),
        config_dir: codex_home().to_string_lossy().to_string(),
        files: vec![display_path(&config_path())],
        current_provider: None,
        providers: vec![],
        catalog: None,
        catalog_file: None,
        settings: vec![],
        current: vec![],
        notes: vec![],
        readonly: false,
    };
    let (doc, _) = match load_doc() {
        Ok(d) => d,
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    let store = store::load();
    let cur = current_provider(&doc);
    st.providers = providers(&doc);
    st.current_provider = Some(cur.clone());
    if let Some((p, v, _)) = load_catalog(&doc) {
        st.files.push(display_path(&p));
        st.catalog_file = Some(display_path(&p));
        st.catalog = Some(catalog_models(&v));
    } else {
        st.notes.push("config.toml 没有设置 model_catalog_json，模型列表由 Codex 在线获取，暂不能编辑。".into());
    }

    let inject = store::get_flag(&store, ID, "fastInject");
    let tier = service_tier(&doc);
    let sl = status_line(&doc);
    let effs = efforts(&doc);
    st.settings = vec![
        bool_setting("fast_inject", "Fast", "在 Codex 中显示 Fast",
            "Codex 只在 ChatGPT 账号登录时显示 Fast，用自定义供应商会被隐藏。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制。", inject),
        bool_setting("fast_default", "Fast", "默认使用 Fast",
            "写入 service_tier = \"priority\"（与 Codex 自己切换 Fast 时写入的值相同）。供应商需要支持 priority 档位。", is_fast_tier(&tier)),
        bool_setting("fast_cli", "Fast", "Codex CLI 状态栏显示 fast-mode",
            "CLI 与桌面版共用配置，在 [tui].status_line 里追加 fast-mode。", sl.as_ref().map(|l| l.iter().any(|x| x == "fast-mode")).unwrap_or(false)),
        chips_setting("efforts", "推理强度", "选择器里可选的推理强度", "[desktop] enabled-reasoning-efforts", effs.clone(), &EFFORTS),
        bool_setting("ctx_usage", "界面", "显示上下文用量", "[desktop] show-context-window-usage", desktop_bool(&doc, "show-context-window-usage", true)),
        bool_setting("plain", "界面", "纯文本输入框", "[desktop] composerPlainTextMode", desktop_bool(&doc, "composerPlainTextMode", false)),
    ];

    let prov = st.providers.iter().find(|p| p.id == cur);
    let custom = prov.map(|p| !p.builtin).unwrap_or(false);
    st.current = vec![
        Kv::mono("model_provider", format!("\"{cur}\"")),
        Kv::mono("base_url", prov.and_then(|p| p.base_url.clone()).unwrap_or_else(|| "ChatGPT 账号".into())),
        Kv::mono("model", doc.get("model").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        Kv::mono("service_tier", format!("\"{tier}\"")),
        Kv::text("Fast 选项", if inject { "注入显示（经 AgentPlus 启动时生效）" } else if custom { "被 Codex 隐藏" } else { "官方账号可见" }),
        Kv::mono("模型目录", match &st.catalog {
            Some(c) => format!("{}/{} 可见", c.iter().filter(|m| m.visible).count(), c.len()),
            None => "在线获取".into(),
        }),
        Kv::mono("推理强度", effs.join(" ")),
    ];
    st
}

/// Applies `ops` in memory, records the diff, and writes files unless `dry_run`.
pub fn plan(ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    let (mut doc, meta) = load_doc()?;
    let cfg_file = display_path(&config_path());
    let mut catalog = load_catalog(&doc);
    let cat_file = catalog.as_ref().map(|(p, _, _)| display_path(p)).unwrap_or_default();
    let mut store = store::load();
    let mut diff = Diff::default();
    let (mut cfg_dirty, mut cat_dirty, mut store_dirty) = (false, false, false);

    for op in ops {
        match op {
            Op::SetCurrentProvider { provider } => {
                let old = current_provider(&doc);
                if &old != provider {
                    doc["model_provider"] = value(provider.as_str());
                    diff.push(&cfg_file, format!("model_provider = \"{old}\" → \"{provider}\""), true);
                    cfg_dirty = true;
                }
            }
            Op::SetModelVisible { model, visible, .. } => {
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!("没有可编辑的模型目录"))?;
                let entry = v
                    .get_mut("models")
                    .and_then(|m| m.as_array_mut())
                    .and_then(|a| a.iter_mut().find(|m| m.get("slug").and_then(|s| s.as_str()) == Some(model)))
                    .ok_or_else(|| anyhow!("模型目录里没有 {model}"))?;
                let want = if *visible { "list" } else { "hide" };
                let old = entry.get("visibility").and_then(|s| s.as_str()).unwrap_or("list").to_string();
                if old != want {
                    entry["visibility"] = Value::from(want);
                    diff.push(&cat_file, format!("{model}  visibility \"{old}\" → \"{want}\""), *visible);
                    cat_dirty = true;
                }
            }
            Op::SetSetting { key, value: v } => match key.as_str() {
                "fast_inject" => {
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, "fastInject") != on {
                        store::set_flag(&mut store, ID, "fastInject", on);
                        diff.push(INJECT_FILE, if on { "+ 启用 Fast 显示注入（通过 AgentPlus 重启 Codex 后生效）" } else { "- 停用 Fast 显示注入" }, on);
                        store_dirty = true;
                    }
                }
                "fast_default" => {
                    let on = v.as_bool().unwrap_or(false);
                    let old = service_tier(&doc);
                    let new = if on { FAST_TIER } else { "default" };
                    if is_fast_tier(&old) != on {
                        doc["service_tier"] = value(new);
                        diff.push(&cfg_file, format!("service_tier = \"{old}\" → \"{new}\""), on);
                        cfg_dirty = true;
                    }
                }
                "fast_cli" => {
                    let on = v.as_bool().unwrap_or(false);
                    let mut list = status_line(&doc).unwrap_or_else(|| {
                        ["model-with-reasoning", "context-remaining", "current-dir"].iter().map(|s| s.to_string()).collect()
                    });
                    let has = list.iter().any(|x| x == "fast-mode");
                    if has != on {
                        if on { list.push("fast-mode".into()) } else { list.retain(|x| x != "fast-mode") }
                        doc["tui"]["status_line"] = value(list.iter().collect::<Array>());
                        diff.push(&cfg_file, format!("[tui] status_line {} \"fast-mode\"", if on { "+" } else { "-" }), on);
                        cfg_dirty = true;
                    }
                }
                "efforts" => {
                    let want: Vec<String> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    let old = efforts(&doc);
                    // keep the canonical order
                    let ordered: Vec<&str> = EFFORTS.iter().copied().filter(|e| want.iter().any(|w| w == e)).collect();
                    for e in EFFORTS {
                        let (w, o) = (want.iter().any(|x| x == e), old.iter().any(|x| x == e));
                        if w != o {
                            diff.push(&cfg_file, format!("[desktop] enabled-reasoning-efforts {} \"{e}\"", if w { "+" } else { "-" }), w);
                        }
                    }
                    if ordered.len() != old.len() || !ordered.iter().all(|e| old.iter().any(|o| o == e)) {
                        doc["desktop"]["enabled-reasoning-efforts"] = value(ordered.into_iter().collect::<Array>());
                        cfg_dirty = true;
                    }
                }
                "ctx_usage" | "plain" => {
                    let k = if key == "ctx_usage" { "show-context-window-usage" } else { "composerPlainTextMode" };
                    let on = v.as_bool().unwrap_or(false);
                    if desktop_bool(&doc, k, key == "ctx_usage") != on {
                        doc["desktop"][k] = value(on);
                        diff.push(&cfg_file, format!("[desktop] {k} = {on}"), on);
                        cfg_dirty = true;
                    }
                }
                other => return Err(anyhow!("未知设置 {other}")),
            },
            Op::SetProviderEnabled { .. } => return Err(anyhow!("Codex 同时只能使用一个供应商")),
        }
    }

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        let mut targets = vec![];
        if cfg_dirty {
            targets.push(config_path());
        }
        if cat_dirty {
            targets.push(catalog.as_ref().unwrap().0.clone());
        }
        if !targets.is_empty() {
            backup_dir = Some(backup(ID, &targets)?);
        }
        if cfg_dirty {
            write_text_atomic(&config_path(), &doc.to_string(), meta)?;
            written.push(config_path());
        }
        if cat_dirty {
            let (p, v, m) = catalog.as_ref().unwrap();
            write_json(p, v, *m)?;
            written.push(p.clone());
        }
        if store_dirty {
            store::save(&store)?;
        }
    }
    Ok((diff, written, backup_dir))
}

pub fn fast_inject_enabled() -> bool {
    store::get_flag(&store::load(), ID, "fastInject")
}
