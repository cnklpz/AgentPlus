//! Codex (desktop + CLI share `~/.codex/config.toml`).
//! Model picker contents come from the catalog named by `model_catalog_json`;
//! each entry's `visibility` ("list" | "hide") decides whether it shows up.
//! Provider keys live in `~/.codex/.env` under the provider's `env_key`.

use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;
use toml_edit::{value, Array, DocumentMut, Item, Table};

pub const ID: &str = "codex";
const EFFORTS: [&str; 7] = ["low", "medium", "high", "xhigh", "persistent", "ultra", "max"];
const INJECT_FILE: &str = "AgentPlus · Codex 界面注入";
/// Codex writes the catalog's Fast tier id ("priority") when Fast is picked in its menu.
const FAST_TIER: &str = "priority";

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| super::dir_override(ID))
        .unwrap_or_else(|| home().join(".codex"))
}

fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn env_path() -> PathBuf {
    codex_home().join(".env")
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

/// The raw `model_provider` value in config.toml.
pub fn configured_provider(doc: &DocumentMut) -> String {
    doc.get("model_provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string()
}

/// Fixed-id mode: `model_provider` stays `agentplus` and switching only rewrites
/// `[model_providers.agentplus]`, so sessions never change provider. The mode is
/// on exactly when config.toml points at it — no separate flag to drift.
pub const FIXED_ID: &str = "agentplus";

fn provider_item<'a>(doc: &'a DocumentMut, id: &str) -> Option<&'a Item> {
    doc.get("model_providers").and_then(|t| t.get(id))
}

fn provider_str(doc: &DocumentMut, id: &str, key: &str) -> Option<String> {
    provider_item(doc, id).and_then(|t| t.get(key)).and_then(|v| v.as_str()).map(String::from)
}

/// Copies `provider`'s table into `[model_providers.agentplus]` and points
/// `model_provider` at it.
fn mirror(doc: &mut DocumentMut, provider: &str, store: &mut Value, diff: &mut Diff, cfg_file: &str) -> Result<()> {
    if provider == "openai" {
        return Err(anyhow!("OpenAI 官方账号不能使用固定 ID，请先切换到自定义供应商"));
    }
    let mut table = provider_item(doc, provider).cloned().ok_or_else(|| anyhow!("找不到供应商 {provider}"))?;
    if let Some(t) = table.as_table_like_mut() {
        t.insert("name", value(format!("AgentPlus（{provider}）")));
    }
    doc["model_providers"][FIXED_ID] = table;
    let raw = configured_provider(doc);
    if raw != FIXED_ID {
        doc["model_provider"] = value(FIXED_ID);
        diff.push(cfg_file, format!("model_provider = \"{raw}\" → \"{FIXED_ID}\""), true);
    }
    diff.push(cfg_file, format!("[model_providers.{FIXED_ID}] ← 复制「{provider}」的整段配置（地址、密钥变量、接口、认证方式）"), true);
    store::set_str(store, ID, "fixedSource", provider);
    Ok(())
}

/// Which user-defined provider is effectively active (resolves the fixed-id mirror).
fn current_provider(doc: &DocumentMut, store: &Value) -> String {
    let raw = configured_provider(doc);
    if raw != FIXED_ID {
        return raw;
    }
    if let Some(src) = store::get_str(store, ID, "fixedSource") {
        return src;
    }
    // Fall back to matching the mirror's base_url against the real providers.
    let mirror = provider_str(doc, FIXED_ID, "base_url");
    doc.get("model_providers")
        .and_then(|t| t.as_table_like())
        .and_then(|t| {
            t.iter()
                .find(|(k, v)| *k != FIXED_ID && v.get("base_url").and_then(|b| b.as_str()) == mirror.as_deref())
                .map(|(k, _)| k.to_string())
        })
        .unwrap_or(raw)
}

// ---------------------------------------------------------------- ~/.codex/.env

fn read_env() -> (Vec<String>, TextMeta) {
    match read_text(&env_path()) {
        Ok((t, m)) => (t.lines().map(String::from).collect(), m),
        Err(_) => (vec![], TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }),
    }
}

fn env_line_key(l: &str) -> Option<&str> {
    let l = l.trim_start();
    let l = l.strip_prefix("export ").unwrap_or(l);
    l.split_once('=').map(|(k, _)| k.trim())
}

/// Value of an env_key: ~/.codex/.env first, then the process environment.
pub fn env_value(name: &str) -> Option<String> {
    let (lines, _) = read_env();
    lines
        .iter()
        .find(|l| env_line_key(l) == Some(name))
        .and_then(|l| l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_string()))
        .or_else(|| std::env::var(name).ok())
        .filter(|v| !v.is_empty())
}

fn set_env(lines: &mut Vec<String>, name: &str, val: &str) {
    let line = format!("{name}={val}");
    match lines.iter_mut().find(|l| env_line_key(l) == Some(name)) {
        Some(l) => *l = line,
        None => lines.push(line),
    }
}

fn key_status(name: &str) -> &'static str {
    let (lines, _) = read_env();
    if lines.iter().any(|l| env_line_key(l) == Some(name)) {
        "已在 ~/.codex/.env 配置"
    } else if std::env::var_os(name).is_some() {
        "已在系统环境变量配置"
    } else {
        "未找到，请求会失败"
    }
}

// ---------------------------------------------------------------- read

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
        editable: false,
        api: "responses".into(),
        has_key: true,
    }];
    if let Some(t) = doc.get("model_providers").and_then(|i| i.as_table_like()) {
        for (id, item) in t.iter() {
            if id == FIXED_ID {
                continue; // managed mirror, not shown as its own card
            }
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
                editable: true,
                api: if chat { "chat".into() } else { "responses".into() },
                has_key: env_key.as_deref().and_then(env_value).is_some(),
            });
        }
    }
    out
}

fn load_catalog(doc: &DocumentMut) -> Option<(PathBuf, Value, TextMeta)> {
    let p = catalog_path(doc)?;
    let (v, meta) = read_json(&p).ok()?;
    Some((p, v, meta))
}

fn custom_models(store: &Value) -> Vec<String> {
    crate::store::agent_get(store, ID, "customModels")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn set_custom_models(store: &mut Value, list: &[String]) {
    store::set_value(store, ID, "customModels", Value::from(list.to_vec()));
}

fn catalog_models(v: &Value, custom: &[String]) -> Vec<Model> {
    let empty = vec![];
    let list = v.get("models").and_then(|m| m.as_array()).unwrap_or(&empty);
    list.iter()
        .filter_map(|m| {
            let id = m.get("slug")?.as_str()?.to_string();
            let fast = match m.get("service_tiers") {
                Some(Value::Array(a)) => a.iter().any(|t| t.get("id").and_then(|x| x.as_str()) == Some(FAST_TIER)),
                Some(Value::Object(o)) => o.get("id").and_then(|x| x.as_str()) == Some(FAST_TIER),
                _ => false,
            } || m.get("additional_speed_tiers").map(|x| x.to_string().contains("fast")).unwrap_or(false);
            let is_custom = custom.contains(&id);
            let mut tags = vec![];
            if fast {
                tags.push("Fast".to_string());
            }
            if is_custom {
                tags.push("自定义".to_string());
            }
            let context = m.get("context_window").and_then(|x| x.as_u64());
            Some(Model {
                visible: m.get("visibility").and_then(|x| x.as_str()) != Some("hide"),
                readonly: false,
                tags,
                ctx: context.map(fmt_ctx),
                name: m.get("display_name").and_then(|x| x.as_str()).map(String::from),
                context,
                deletable: is_custom,
                id,
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
        fixed_pending: false,
        fixed_prompt: false,
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
    let cur = current_provider(&doc, &store);
    let raw = configured_provider(&doc);
    let fixed = raw == FIXED_ID;
    st.providers = providers(&doc);
    st.current_provider = Some(cur.clone());
    // Not on the fixed id yet, but could be (a custom provider is active).
    st.fixed_pending = !fixed && raw != "openai";
    // Fixed id is the default: offer it as a pending change until the user declines once.
    st.fixed_prompt = st.fixed_pending && !store::get_flag(&store, ID, "fixedPromptDismissed");
    if let Some((p, v, _)) = load_catalog(&doc) {
        st.files.push(display_path(&p));
        st.catalog_file = Some(display_path(&p));
        st.catalog = Some(catalog_models(&v, &custom_models(&store)));
    } else {
        st.notes.push("config.toml 没有设置 model_catalog_json，模型列表由 Codex 在线获取，暂不能编辑。".into());
    }

    let inject = store::get_flag(&store, ID, "fastInject");
    let tier = service_tier(&doc);
    let sl = status_line(&doc);
    let effs = efforts(&doc);
    st.settings = vec![
        bool_setting("fixed_id", "供应商切换", "固定供应商 ID（推荐开启）",
            "开启后 model_provider 固定为 agentplus，切换供应商只改它的地址和密钥，会话不会因为切换而从 Codex 的最近列表和归档里消失。开启时会把当前供应商复制过去；关闭时 model_provider 改回当前供应商。", fixed),
        bool_setting("fast_inject", "Fast", "在 Codex 中显示 Fast",
            "Codex 只在 ChatGPT 账号登录时显示 Fast，用自定义供应商会被隐藏。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制。", inject),
        bool_setting("fast_default", "Fast", "默认使用 Fast",
            "写入 service_tier = \"priority\"（与 Codex 自己切换 Fast 时写入的值相同）。供应商需要支持 priority 档位。", is_fast_tier(&tier)),
        bool_setting("fast_cli", "Fast", "Codex CLI 状态栏显示 fast-mode",
            "CLI 与桌面版共用配置，在 [tui].status_line 里追加 fast-mode。", sl.as_ref().map(|l| l.iter().any(|x| x == "fast-mode")).unwrap_or(false)),
        chips_setting("efforts", "推理强度", "选择器里可选的推理强度",
            "勾选的档位会出现在 Codex 桌面版的推理强度菜单里（[desktop] enabled-reasoning-efforts）。模型不支持的档位不会显示。", effs.clone(), &EFFORTS)
            .with_hints(&[
                "回复最快，推理较浅",
                "速度和深度平衡，适合日常任务",
                "推理更深，适合复杂问题",
                "比 high 更深的推理",
                "持久模式：做完请求后继续主动做后续有用的工作，直到没有可做的",
                "最高推理，并自动把任务拆给子代理（部分模型支持）",
                "最高推理深度，适合最难的问题",
            ]),
        bool_setting("ctx_usage", "界面", "显示上下文用量", "[desktop] show-context-window-usage", desktop_bool(&doc, "show-context-window-usage", true)),
        bool_setting("plain", "界面", "纯文本输入框", "[desktop] composerPlainTextMode", desktop_bool(&doc, "composerPlainTextMode", false)),
    ];

    let prov = st.providers.iter().find(|p| p.id == cur);
    let custom = prov.map(|p| !p.builtin).unwrap_or(false);
    st.current = vec![
        Kv::mono("model_provider", format!("\"{raw}\"")),
        Kv::text(
            "固定 ID",
            if fixed {
                format!("已开启 · 指向「{cur}」")
            } else if st.fixed_prompt {
                "预开启 · 切换供应商时写入".to_string()
            } else {
                "未开启".to_string()
            },
        ),
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

/// Base URL + key of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<(String, Option<String>, String)> {
    let (doc, _) = load_doc()?;
    let base = provider_str(&doc, id, "base_url").ok_or_else(|| anyhow!("供应商 {id} 没有 base_url"))?;
    let key = provider_str(&doc, id, "env_key").and_then(|k| env_value(&k));
    Ok((base, key, "responses".into()))
}

// ---------------------------------------------------------------- write

fn unique_id(doc: &DocumentMut, name: &str) -> String {
    let base = slug(name);
    let taken = |id: &str| id == "openai" || id == FIXED_ID || provider_item(doc, id).is_some();
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken(c)).unwrap()
}

/// Applies `ops` in memory, records the diff, and writes files unless `dry_run`.
pub fn plan(ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    let (mut doc, meta) = load_doc()?;
    let cfg_file = display_path(&config_path());
    let env_file = display_path(&env_path());
    let mut catalog = load_catalog(&doc);
    let cat_file = catalog.as_ref().map(|(p, _, _)| display_path(p)).unwrap_or_default();
    let mut store = store::load();
    let (mut env_lines, env_meta) = read_env();
    let mut diff = Diff::default();
    let (mut cfg_dirty, mut cat_dirty, mut store_dirty, mut env_dirty) = (false, false, false, false);
    // A pending toggle of the fixed id applies to this batch; otherwise follow config.
    let fixed = ops
        .iter()
        .find_map(|o| match o {
            Op::SetSetting { key, value } if key == "fixed_id" => value.as_bool(),
            _ => None,
        })
        .unwrap_or_else(|| configured_provider(&doc) == FIXED_ID);
    // Provider edits first, then switches, so later ops see the result.
    let rank = |o: &Op| match o {
        Op::UpsertProvider { .. } => 0,
        Op::SetCurrentProvider { .. } => 1,
        Op::DeleteProvider { .. } => 2,
        _ => 3,
    };
    let mut ordered: Vec<&Op> = ops.iter().collect();
    ordered.sort_by_key(|o| rank(o));

    for op in ordered {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.api != "responses" {
                    return Err(anyhow!("Codex 只支持 Responses 接口"));
                }
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!("名称和地址不能为空"));
                }
                let id = match &p.id {
                    Some(id) => {
                        provider_item(&doc, id).ok_or_else(|| anyhow!("找不到供应商 {id}"))?;
                        id.clone()
                    }
                    None => unique_id(&doc, &p.name),
                };
                let is_new = p.id.is_none();
                let env_key = provider_str(&doc, &id, "env_key")
                    .unwrap_or_else(|| format!("{}_API_KEY", id.to_uppercase().replace('-', "_")));
                if is_new {
                    let mut t = Table::new();
                    t.insert("name", value(p.name.trim()));
                    t.insert("base_url", value(p.base_url.trim()));
                    t.insert("wire_api", value("responses"));
                    t.insert("env_key", value(env_key.as_str()));
                    doc["model_providers"][id.as_str()] = Item::Table(t);
                    diff.push(&cfg_file, format!("+ [model_providers.{id}] name = \"{}\", base_url = \"{}\"", p.name.trim(), p.base_url.trim()), true);
                } else {
                    for (k, v) in [("name", p.name.trim()), ("base_url", p.base_url.trim())] {
                        let old = provider_str(&doc, &id, k).unwrap_or_default();
                        if old != v {
                            doc["model_providers"][id.as_str()][k] = value(v);
                            diff.push(&cfg_file, format!("[model_providers.{id}] {k} = \"{v}\""), true);
                        }
                    }
                    if provider_str(&doc, &id, "env_key").is_none() {
                        doc["model_providers"][id.as_str()]["env_key"] = value(env_key.as_str());
                        diff.push(&cfg_file, format!("[model_providers.{id}] env_key = \"{env_key}\""), true);
                    }
                }
                cfg_dirty = true;
                if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                    set_env(&mut env_lines, &env_key, k.trim());
                    diff.push(&env_file, format!("{env_key} = {}", mask_key(k.trim())), true);
                    env_dirty = true;
                }
                // Keep the fixed-id mirror in sync with the provider it copies.
                if configured_provider(&doc) == FIXED_ID && store::get_str(&store, ID, "fixedSource").as_deref() == Some(id.as_str()) {
                    mirror(&mut doc, &id, &mut store, &mut diff, &cfg_file)?;
                    store_dirty = true;
                }
            }
            Op::DeleteProvider { provider } => {
                let raw = configured_provider(&doc);
                let src = store::get_str(&store, ID, "fixedSource");
                if &raw == provider || (raw == FIXED_ID && src.as_deref() == Some(provider.as_str())) {
                    return Err(anyhow!("「{provider}」正在使用，先切换到其他供应商再删除"));
                }
                let removed = doc
                    .get_mut("model_providers")
                    .and_then(|t| t.as_table_like_mut())
                    .and_then(|t| t.remove(provider))
                    .is_some();
                if removed {
                    diff.push(&cfg_file, format!("- [model_providers.{provider}]（~/.codex/.env 里的密钥保留）"), false);
                    cfg_dirty = true;
                }
            }
            Op::SetCurrentProvider { provider } => {
                let raw = configured_provider(&doc);
                if provider == "openai" || !fixed {
                    if &raw != provider {
                        doc["model_provider"] = value(provider.as_str());
                        diff.push(&cfg_file, format!("model_provider = \"{raw}\" → \"{provider}\""), true);
                        cfg_dirty = true;
                    }
                } else {
                    mirror(&mut doc, provider, &mut store, &mut diff, &cfg_file)?;
                    cfg_dirty = true;
                    store_dirty = true;
                }
            }
            Op::SetModelVisible { model, visible, .. } => {
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!("没有可编辑的模型目录"))?;
                let entry = catalog_entry(v, model).ok_or_else(|| anyhow!("模型目录里没有 {model}"))?;
                let want = if *visible { "list" } else { "hide" };
                let old = entry.get("visibility").and_then(|s| s.as_str()).unwrap_or("list").to_string();
                if old != want {
                    entry["visibility"] = Value::from(want);
                    diff.push(&cat_file, format!("{model}  visibility \"{old}\" → \"{want}\""), *visible);
                    cat_dirty = true;
                }
            }
            Op::UpsertModel { model: m, .. } => {
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!("没有可编辑的模型目录"))?;
                let id = m.id.trim().to_string();
                if id.is_empty() {
                    return Err(anyhow!("模型 ID 不能为空"));
                }
                if let Some(entry) = catalog_entry(v, &id) {
                    if let Some(n) = m.name.as_deref().filter(|n| !n.trim().is_empty()) {
                        if entry.get("display_name").and_then(|x| x.as_str()) != Some(n.trim()) {
                            entry["display_name"] = Value::from(n.trim());
                            diff.push(&cat_file, format!("{id}  display_name = \"{}\"", n.trim()), true);
                            cat_dirty = true;
                        }
                    }
                    if let Some(c) = m.context {
                        if entry.get("context_window").and_then(|x| x.as_u64()) != Some(c) {
                            entry["context_window"] = Value::from(c);
                            diff.push(&cat_file, format!("{id}  context_window = {c}"), true);
                            cat_dirty = true;
                        }
                    }
                } else {
                    let models = v.get_mut("models").and_then(|x| x.as_array_mut()).ok_or_else(|| anyhow!("模型目录格式不对"))?;
                    // Reuse an existing entry as the template so Codex gets every field it expects.
                    let mut entry = models.first().cloned().ok_or_else(|| anyhow!("模型目录是空的，没有可参照的条目"))?;
                    let prio = models.iter().filter_map(|x| x.get("priority").and_then(|p| p.as_i64())).max().unwrap_or(0) + 1;
                    let name = m.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| id.clone());
                    entry["slug"] = Value::from(id.as_str());
                    entry["display_name"] = Value::from(name.as_str());
                    entry["description"] = Value::from("自定义模型（AgentPlus 添加）");
                    entry["visibility"] = Value::from("list");
                    entry["priority"] = Value::from(prio);
                    for k in ["availability_nux", "upgrade"] {
                        if entry.get(k).is_some() {
                            entry[k] = Value::Null;
                        }
                    }
                    if let Some(c) = m.context {
                        entry["context_window"] = Value::from(c);
                    }
                    models.push(entry);
                    let mut custom = custom_models(&store);
                    custom.push(id.clone());
                    set_custom_models(&mut store, &custom);
                    diff.push(&cat_file, format!("+ {id}（{name}{}）", m.context.map(|c| format!("，上下文 {}", fmt_ctx(c))).unwrap_or_default()), true);
                    cat_dirty = true;
                    store_dirty = true;
                }
            }
            Op::DeleteModel { model, .. } => {
                let mut custom = custom_models(&store);
                if !custom.contains(model) {
                    return Err(anyhow!("{model} 是 Codex 自带的模型，只能隐藏不能删除"));
                }
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!("没有可编辑的模型目录"))?;
                if let Some(models) = v.get_mut("models").and_then(|x| x.as_array_mut()) {
                    models.retain(|m| m.get("slug").and_then(|s| s.as_str()) != Some(model));
                }
                custom.retain(|m| m != model);
                set_custom_models(&mut store, &custom);
                diff.push(&cat_file, format!("- {model}"), false);
                cat_dirty = true;
                store_dirty = true;
            }
            Op::SetSetting { key, value: v } => match key.as_str() {
                "fixed_id" => {
                    let on = v.as_bool().unwrap_or(false);
                    let raw = configured_provider(&doc);
                    let cur = current_provider(&doc, &store);
                    if on && raw != FIXED_ID {
                        mirror(&mut doc, &cur, &mut store, &mut diff, &cfg_file)?;
                        cfg_dirty = true;
                        store_dirty = true;
                    } else if !on && raw == FIXED_ID {
                        doc["model_provider"] = value(cur.as_str());
                        diff.push(&cfg_file, format!("model_provider = \"{FIXED_ID}\" → \"{cur}\"（关闭固定 ID）"), false);
                        cfg_dirty = true;
                    }
                }
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
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
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
        if env_dirty {
            targets.push(env_path());
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
        if env_dirty {
            write_text_atomic(&env_path(), &env_lines.join("\n"), env_meta)?;
            written.push(env_path());
        }
        if store_dirty {
            store::save(&store)?;
        }
    }
    Ok((diff, written, backup_dir))
}

fn catalog_entry<'a>(v: &'a mut Value, slug: &str) -> Option<&'a mut Value> {
    v.get_mut("models")
        .and_then(|m| m.as_array_mut())
        .and_then(|a| a.iter_mut().find(|m| m.get("slug").and_then(|s| s.as_str()) == Some(slug)))
}

/// The user declined the prefilled "turn on fixed id" change; stop offering it.
pub fn dismiss_fixed_prompt() -> Result<()> {
    let mut s = store::load();
    store::set_flag(&mut s, ID, "fixedPromptDismissed", true);
    store::save(&s)
}

pub fn fast_inject_enabled() -> bool {
    store::get_flag(&store::load(), ID, "fastInject")
}
