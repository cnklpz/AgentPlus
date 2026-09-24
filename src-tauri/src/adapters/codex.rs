//! Codex (desktop + CLI share `~/.codex/config.toml`).
//! Model picker contents come from the catalog named by `model_catalog_json`;
//! each entry's `visibility` ("list" | "hide") decides whether it shows up.
//! Provider keys live in `~/.codex/.env` under the provider's `env_key`.

use super::{Plan, Endpoint};
use crate::i18n::l;
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

fn inject_file() -> &'static str {
    l("AgentPlus · Codex 界面注入", "AgentPlus · Codex UI injection")
}
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
    let doc = text.parse::<DocumentMut>().map_err(|e| anyhow!(tr!("config.toml 解析失败：{e}", "Couldn't parse config.toml: {e}")))?;
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

/// Sets `model_providers.<id>`, in the style the file already uses: a `[model_providers.x]`
/// table, or an entry of an inline `model_providers = { … }` (where a table item would be
/// silently dropped by toml_edit).
fn put_provider(doc: &mut DocumentMut, id: &str, item: Item) -> Result<()> {
    if doc.get("model_providers").is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        doc["model_providers"] = Item::Table(t);
    }
    match doc.get_mut("model_providers") {
        Some(Item::Table(t)) => {
            t.insert(id, match item {
                Item::Value(toml_edit::Value::InlineTable(it)) => Item::Table(it.into_table()),
                other => other,
            });
        }
        Some(Item::Value(toml_edit::Value::InlineTable(it))) => {
            let v = match item {
                Item::Table(t) => toml_edit::Value::InlineTable(t.into_inline_table()),
                Item::Value(v) => v,
                _ => return Err(anyhow!(tr!("供应商 {id} 的配置无效", "The config of provider {id} is invalid"))),
            };
            it.insert(id, v);
        }
        _ => return Err(anyhow!(l("config.toml 里的 model_providers 不是表", "model_providers in config.toml is not a table"))),
    }
    Ok(())
}

/// A provider field's value as it reads in config.toml, for diff lines.
fn field_text(item: &Item) -> String {
    let mut v = match item {
        Item::Value(v) => v.clone(),
        Item::Table(t) => {
            let mut it = t.clone().into_inline_table();
            it.fmt();
            toml_edit::Value::InlineTable(it)
        }
        other => return other.to_string().trim().to_string(),
    };
    v.decor_mut().clear();
    v.to_string()
}

fn provider_fields(item: Option<&Item>) -> Vec<(String, String)> {
    item.and_then(|i| i.as_table_like())
        .map(|t| t.iter().map(|(k, v)| (k.to_string(), field_text(v))).collect())
        .unwrap_or_default()
}

/// Diff lines for rewriting a provider table from `old` to `new` fields; true when anything differs.
fn push_field_diff(diff: &mut Diff, file: &str, id: &str, old: &[(String, String)], new: &[(String, String)]) -> bool {
    let mut changed = false;
    for (k, v) in new {
        match old.iter().find(|(ok, _)| ok == k) {
            Some((_, ov)) if ov == v => continue,
            Some((_, ov)) => diff.push(file, format!("[model_providers.{id}] {k} = {ov} → {v}"), true),
            None => diff.push(file, format!("[model_providers.{id}] + {k} = {v}"), true),
        }
        changed = true;
    }
    for (k, _) in old.iter().filter(|(k, _)| !new.iter().any(|(nk, _)| nk == k)) {
        diff.push(file, format!("[model_providers.{id}] - {k}"), false);
        changed = true;
    }
    changed
}

/// Copies `provider`'s table into `[model_providers.agentplus]` and points
/// `model_provider` at it. Returns whether config.toml changed.
fn mirror(doc: &mut DocumentMut, provider: &str, store: &mut Value, diff: &mut Diff, cfg_file: &str) -> Result<bool> {
    if provider == "openai" {
        return Err(anyhow!(l("OpenAI 官方账号不能使用固定 ID，请先切换到自定义供应商", "The OpenAI official account can't use the fixed ID; switch to a custom provider first")));
    }
    let mut table = provider_item(doc, provider).cloned().ok_or_else(|| anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")))?;
    if let Some(t) = table.as_table_like_mut() {
        t.insert("name", value(format!("AgentPlus（{provider}）")));
    }
    let old = provider_item(doc, FIXED_ID);
    let fresh = old.is_none();
    let old = provider_fields(old);
    let new = provider_fields(Some(&table));
    let mut changed = false;
    let raw = configured_provider(doc);
    if raw != FIXED_ID {
        doc["model_provider"] = value(FIXED_ID);
        diff.push(cfg_file, format!("model_provider = \"{raw}\" → \"{FIXED_ID}\""), true);
        changed = true;
    }
    if fresh {
        diff.push(cfg_file, tr!("+ [model_providers.{FIXED_ID}]（复制自「{provider}」）", "+ [model_providers.{FIXED_ID}] (copied from \"{provider}\")"), true);
    }
    if push_field_diff(diff, cfg_file, FIXED_ID, &old, &new) {
        put_provider(doc, FIXED_ID, table)?;
        changed = true;
    }
    store::set_str(store, ID, "fixedSource", provider);
    Ok(changed)
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

/// For writing: fails when the file exists but can't be read.
fn read_env_checked() -> Result<(Vec<String>, TextMeta)> {
    read_text_or_new(&env_path()).map(|(t, m)| (t.lines().map(String::from).collect(), m))
}

fn read_env() -> (Vec<String>, TextMeta) {
    read_env_checked().unwrap_or((vec![], TextMeta::NEW))
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
        l("已在 ~/.codex/.env 配置", "set in ~/.codex/.env")
    } else if std::env::var_os(name).is_some() {
        l("已在系统环境变量配置", "set in system environment variables")
    } else {
        l("未找到，请求会失败", "not found; requests will fail")
    }
}

// ---------------------------------------------------------------- ~/.codex/auth.json

#[derive(Debug, PartialEq)]
enum SignIn {
    ChatGpt,
    ApiKey,
    None,
    /// Credentials kept in the OS keyring; AgentPlus can't tell.
    Unknown,
}

/// How Codex is signed in, following its own rule: an explicit `auth_mode` wins,
/// otherwise a non-empty `OPENAI_API_KEY` means API-key mode.
fn sign_in(doc: &DocumentMut) -> SignIn {
    let Ok((v, _)) = read_json(&codex_home().join("auth.json")) else {
        let store = doc.get("cli_auth_credentials_store").and_then(|v| v.as_str()).unwrap_or("file");
        return if store == "file" { SignIn::None } else { SignIn::Unknown };
    };
    sign_in_from(&v)
}

fn sign_in_from(v: &Value) -> SignIn {
    let key = v.get("OPENAI_API_KEY").and_then(|x| x.as_str()).is_some_and(|x| !x.trim().is_empty());
    let tokens = v.get("tokens").and_then(|t| t.get("refresh_token").or_else(|| t.get("access_token"))).and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty());
    match v.get("auth_mode").and_then(|x| x.as_str()) {
        Some("chatgpt") | Some("chatgptAuthTokens") if tokens => SignIn::ChatGpt,
        Some("apikey") | Some("apiKey") if key => SignIn::ApiKey,
        Some(_) => SignIn::None,
        None if key => SignIn::ApiKey,
        None if tokens => SignIn::ChatGpt,
        None => SignIn::None,
    }
}

// ---------------------------------------------------------------- read

fn providers(doc: &DocumentMut) -> Vec<Provider> {
    let mut out = vec![Provider {
        id: "openai".into(),
        name: l("OpenAI 官方", "OpenAI official").into(),
        base_url: None,
        host: l("ChatGPT 账号登录", "ChatGPT account sign-in").into(),
        apis: vec!["Responses".into()],
        builtin: true,
        enabled: true,
        compatible: true,
        reason: None,
        models: vec![],
        details: vec![
            Kv::text(l("认证方式", "Authentication"), l("ChatGPT 账号登录（~/.codex/auth.json）", "ChatGPT account sign-in (~/.codex/auth.json)")),
            Kv::text("Fast", l("账号登录时 Codex 原生显示", "Shown natively by Codex when signed in with an account")),
            Kv::mono(l("配置 ID", "Config ID"), l("openai（内置）", "openai (built-in)")),
        ],
        editable: false,
        api: "responses".into(),
        has_key: true,
        key_fp: None,
        key_hint: None,
        official_auth: false,
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
            let api = if chat { "chat" } else { "responses" };
            let env_key = get("env_key");
            let official_auth = item.get("requires_openai_auth").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut details = vec![
                Kv::mono(l("配置 ID", "Config ID"), format!("[model_providers.{id}]")),
                Kv::mono("wire_api", format!("\"{wire}\"")),
            ];
            if official_auth {
                details.push(Kv::text(l("官方登录混用", "Official sign-in mix"), l("已开启 · Codex 用 ChatGPT 账号登录，对话请求发往此供应商并使用它的密钥", "On · Codex stays signed in with ChatGPT; requests go to this provider with its own API key")));
            }
            match &env_key {
                Some(k) => details.push(Kv::text(l("密钥", "API key"), tr!("环境变量 {k} · {}", "Environment variable {k} · {}", key_status(k)))),
                None => details.push(Kv::text(l("密钥", "API key"), l("未设置 env_key", "env_key not set"))),
            }
            details.push(Kv::text("Fast", l("Codex 默认隐藏（可在「其他设置」注入显示）", "Hidden by Codex by default (can be shown via injection in \"Other settings\")")));
            out.push(Provider {
                details,
                id: id.to_string(),
                name: get("name").unwrap_or_else(|| id.to_string()),
                host: base.as_deref().map(host_of).unwrap_or_default(),
                base_url: base,
                apis: vec![api_label(api).into()],
                builtin: false,
                enabled: true,
                compatible: !chat,
                reason: if chat { Some(l("Codex 已不支持 Chat 接口", "Codex no longer supports the Chat API").into()) } else { None },
                models: vec![],
                editable: true,
                api: api.into(),
                has_key: env_key.as_deref().and_then(env_value).is_some(),
                key_fp: None,
                key_hint: None,
                official_auth,
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
    crate::util::str_list(crate::store::agent_get(store, ID, "customModels")).unwrap_or_default()
}

fn set_custom_models(store: &mut Value, list: &[String]) {
    store::set_value(store, ID, "customModels", Value::from(list.to_vec()));
}

/// Per-provider model lists (visible slugs), kept by AgentPlus. Codex has a single
/// catalog; switching provider swaps the stored list into it.
fn provider_models(store: &Value) -> serde_json::Map<String, Value> {
    crate::store::get_obj(store, ID, "providerModels")
}

fn stored_list(store: &Value, provider: &str) -> Option<Vec<String>> {
    crate::util::str_list(provider_models(store).get(provider))
}

/// Returns true when the stored list changed.
fn set_stored_list(store: &mut Value, provider: &str, list: &[String]) -> bool {
    if stored_list(store, provider).as_deref() == Some(list) {
        return false;
    }
    let mut all = provider_models(store);
    all.insert(provider.to_string(), Value::from(list.to_vec()));
    store::set_value(store, ID, "providerModels", Value::Object(all));
    true
}

fn visible_slugs(v: &Value) -> Vec<String> {
    v.get("models")
        .and_then(|m| m.as_array())
        .map(|a| {
            a.iter()
                .filter(|m| m.get("visibility").and_then(|x| x.as_str()) != Some("hide"))
                .filter_map(|m| m.get("slug").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Adds a user model to the catalog, cloning an existing entry so Codex gets every field.
fn add_custom_model(v: &mut Value, store: &mut Value, id: &str, name: Option<&str>, context: Option<u64>) -> Result<String> {
    let models = v.get_mut("models").and_then(|x| x.as_array_mut()).ok_or_else(|| anyhow!(l("模型目录格式不对", "The model catalog has an unexpected format")))?;
    let mut entry = models.first().cloned().ok_or_else(|| anyhow!(l("模型目录是空的，没有可参照的条目", "The model catalog is empty; no entry to copy from")))?;
    let prio = models.iter().filter_map(|x| x.get("priority").and_then(|p| p.as_i64())).max().unwrap_or(0) + 1;
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(id).to_string();
    entry["slug"] = Value::from(id);
    entry["display_name"] = Value::from(name.as_str());
    entry["description"] = Value::from("自定义模型（AgentPlus 添加）");
    entry["visibility"] = Value::from("list");
    entry["priority"] = Value::from(prio);
    for k in ["availability_nux", "upgrade"] {
        if entry.get(k).is_some() {
            entry[k] = Value::Null;
        }
    }
    if let Some(c) = context {
        entry["context_window"] = Value::from(c);
    }
    models.push(entry);
    let mut custom = custom_models(store);
    custom.push(id.to_string());
    set_custom_models(store, &custom);
    let ctx = context.map(|c| tr!("，上下文 {}", ", context {}", fmt_ctx(c))).unwrap_or_default();
    Ok(tr!("+ {id}（{name}{ctx}）", "+ {id} ({name}{ctx})"))
}

/// Makes exactly `list` visible in the catalog (adding unknown slugs as custom models).
fn apply_list(v: &mut Value, store: &mut Value, list: &[String], diff: &mut Diff, cat_file: &str, who: &str) -> Result<bool> {
    let (mut shown, mut hidden) = (0, 0);
    if let Some(models) = v.get_mut("models").and_then(|x| x.as_array_mut()) {
        for m in models.iter_mut() {
            let Some(slug) = m.get("slug").and_then(|s| s.as_str()).map(String::from) else { continue };
            let want = if list.contains(&slug) { "list" } else { "hide" };
            if m.get("visibility").and_then(|x| x.as_str()).unwrap_or("list") != want {
                m["visibility"] = Value::from(want);
                if want == "list" { shown += 1 } else { hidden += 1 }
            }
        }
    }
    let mut added = vec![];
    for id in list {
        if catalog_entry(v, id).is_none() {
            added.push(add_custom_model(v, store, id, None, None)?);
        }
    }
    if shown + hidden + added.len() == 0 {
        return Ok(false);
    }
    diff.push(cat_file, tr!("模型列表换成「{who}」的 {} 个模型（显示 {shown} 个，隐藏 {hidden} 个）", "Model list replaced with \"{who}\"'s {} model(s) ({shown} shown, {hidden} hidden)", list.len()), true);
    for a in added {
        diff.push(cat_file, a, true);
    }
    Ok(true)
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
                tags.push(Tag::new("fast", "Fast"));
            }
            if is_custom {
                tags.push(Tag::new("custom", l("自定义", "Custom")));
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
                extra: crate::mfields::read(m, crate::mfields::CODEX),
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
        restartable: false,
        model_fields: vec![],
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
        // Each provider's own list: the live catalog for the current one, the stored list for others.
        let live = visible_slugs(&v);
        for p in st.providers.iter_mut() {
            let list = if p.id == cur { Some(live.clone()) } else { stored_list(&store, &p.id) };
            p.models = list
                .unwrap_or_default()
                .into_iter()
                .map(|id| Model { id, visible: true, readonly: true, ..Default::default() })
                .collect();
        }
    } else {
        st.notes.push(l("config.toml 没有设置 model_catalog_json，模型列表由 Codex 在线获取，暂不能编辑。", "config.toml has no model_catalog_json, so Codex fetches the model list online and it can't be edited here.").into());
    }

    let inject = store::get_flag(&store, ID, "fastInject");
    let full_names = store::get_flag(&store, ID, "fullModelNames");
    let quota = store::get_flag(&store, ID, "quotaUnlock");
    let hide_banner = store::get_flag(&store, ID, "hideUsageBanner");
    let tier = service_tier(&doc);
    let sl = status_line(&doc);
    let effs = efforts(&doc);
    st.settings = vec![
        bool_setting("fixed_id", l("供应商切换", "Provider switching"), l("固定供应商 ID（推荐开启）", "Fixed provider ID (recommended)"),
            l("开启后 model_provider 固定为 agentplus，切换供应商只改它的地址和密钥，会话不会因为切换而从 Codex 的最近列表和归档里消失。开启时会把当前供应商复制过去；关闭时 model_provider 改回当前供应商。",
              "When on, model_provider stays agentplus and switching providers only changes its base URL and API key, so sessions don't disappear from Codex's recent list and archive after a switch. Turning it on copies the current provider over; turning it off points model_provider back at the current provider."), fixed),
        bool_setting("fast_inject", "Fast", l("在 Codex 中显示 Fast", "Show Fast in Codex"),
            l("Codex 只在 ChatGPT 账号登录时显示 Fast，用自定义供应商会被隐藏。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制。",
              "Codex only shows Fast when signed in with a ChatGPT account and hides it for custom providers. When on, restarting Codex through AgentPlus launches it with a debug port and lifts this restriction when the UI loads."), inject),
        bool_setting("fast_default", "Fast", l("默认使用 Fast", "Use Fast by default"),
            l("写入 service_tier = \"priority\"（与 Codex 自己切换 Fast 时写入的值相同）。供应商需要支持 priority 档位。",
              "Writes service_tier = \"priority\" (the same value Codex writes when you pick Fast). The provider must support the priority tier."), is_fast_tier(&tier)),
        bool_setting("fast_cli", "Fast", l("Codex CLI 状态栏显示 fast-mode", "Show fast-mode in the Codex CLI status line"),
            l("CLI 与桌面版共用配置，在 [tui].status_line 里追加 fast-mode。", "The CLI shares its config with the desktop app; appends fast-mode to [tui].status_line."), sl.as_ref().map(|x| x.iter().any(|x| x == "fast-mode")).unwrap_or(false)),
        chips_setting("efforts", l("推理强度", "Reasoning effort"), l("选择器里可选的推理强度", "Reasoning efforts offered in the picker"),
            l("勾选的档位会出现在 Codex 桌面版的推理强度菜单里（[desktop] enabled-reasoning-efforts）。模型不支持的档位不会显示。",
              "Checked levels appear in the Codex desktop reasoning effort menu ([desktop] enabled-reasoning-efforts). Levels a model doesn't support are not shown."), effs.clone(), &EFFORTS)
            .with_hints(&[
                l("回复最快，推理较浅", "Fastest replies, light reasoning"),
                l("速度和深度平衡，适合日常任务", "Balances speed and depth; good for everyday tasks"),
                l("推理更深，适合复杂问题", "Deeper reasoning for complex problems"),
                l("比 high 更深的推理", "Deeper reasoning than high"),
                l("持久模式：做完请求后继续主动做后续有用的工作，直到没有可做的", "Persistent mode: after the request, keeps doing useful follow-up work until nothing is left"),
                l("最高推理，并自动把任务拆给子代理（部分模型支持）", "Maximum reasoning, automatically splitting tasks across subagents (some models)"),
                l("最高推理深度，适合最难的问题", "Maximum reasoning depth for the hardest problems"),
            ]),
        bool_setting("full_names", l("界面", "Interface"), l("显示完整模型名", "Show full model names"),
            l("Codex 只在 ChatGPT 账号登录时显示完整模型名，用自定义供应商会去掉「GPT-」前缀（GPT-6 Sol 显示成 6 Sol）。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时关掉这个缩写。",
              "Codex shows full model names only when signed in with a ChatGPT account; with custom providers it drops the \"GPT-\" prefix (GPT-6 Sol shows as 6 Sol). When on, restarting Codex through AgentPlus launches it with a debug port and turns this shortening off when the UI loads."), full_names),
        bool_setting("quota_unlock", l("官方登录混用", "Official sign-in mix"), l("ChatGPT 额度用完后仍可发送", "Keep sending after the ChatGPT quota runs out"),
            l("用 ChatGPT 账号登录时，账号额度用完后 Codex 会禁用发送按钮，即使开启了官方登录混用、请求其实发往中转站。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制（额度提示横幅用下一项隐藏）。",
              "Signed in with a ChatGPT account, Codex disables the send button once the account's quota runs out, even with the official sign-in mix on and requests actually going to the relay. When on, restarting Codex through AgentPlus launches it with a debug port and lifts this restriction when the UI loads (hide the usage banner with the next option)."), quota),
        bool_setting("hide_usage_banner", l("官方登录混用", "Official sign-in mix"), l("隐藏用量提示横幅", "Hide usage banners"),
            l("用 ChatGPT 账号登录时，输入框上方会显示这个账号的额度横幅（「Codex 和工作使用额度已用完」、即将用完的提醒、升级和重置使用量按钮），与中转站的额度无关。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这些横幅。",
              "Signed in with a ChatGPT account, Codex shows that account's usage banners above the composer (\"You're out of Codex and Work usage\", running-low warnings, upgrade and reset-usage buttons), which have nothing to do with the relay's quota. When on, restarting Codex through AgentPlus launches it with a debug port and removes these banners when the UI loads."), hide_banner),
        bool_setting("ctx_usage", l("界面", "Interface"), l("显示上下文用量", "Show context usage"), "[desktop] show-context-window-usage", desktop_bool(&doc, "show-context-window-usage", true)),
        bool_setting("plain", l("界面", "Interface"), l("纯文本输入框", "Plain text composer"), "[desktop] composerPlainTextMode", desktop_bool(&doc, "composerPlainTextMode", false)),
    ];

    let prov = st.providers.iter().find(|p| p.id == cur);
    let custom = prov.map(|p| !p.builtin).unwrap_or(false);
    let mixed = prov.is_some_and(|p| p.official_auth);
    let signed = mixed.then(|| sign_in(&doc));
    match signed {
        Some(SignIn::None) => st.notes.push(l("当前供应商开启了官方登录混用，但 Codex 还没有登录 ChatGPT 账号：在 Codex 里登录后，对话才会发往中转站。", "The current provider uses the official sign-in mix, but Codex isn't signed in with a ChatGPT account yet. Sign in in Codex so requests can go to the relay.").into()),
        Some(SignIn::ApiKey) => st.notes.push(l("当前供应商开启了官方登录混用，但 Codex 现在是 API Key 登录（~/.codex/auth.json），官方账号功能不会解锁：在 Codex 里退出后改用 ChatGPT 账号登录。", "The current provider uses the official sign-in mix, but Codex is signed in with an API key (~/.codex/auth.json), so account features stay locked. Sign out in Codex and sign in with a ChatGPT account.").into()),
        _ => {}
    }
    st.current = vec![
        Kv::mono("model_provider", format!("\"{raw}\"")),
        Kv::text(
            l("固定 ID", "Fixed ID"),
            if fixed {
                tr!("已开启 · 指向「{cur}」", "On · points at \"{cur}\"")
            } else if st.fixed_prompt {
                l("预开启 · 切换供应商时写入", "Pre-enabled · written on provider switch").to_string()
            } else {
                l("未开启", "Off").to_string()
            },
        ),
        Kv::mono("base_url", prov.and_then(|p| p.base_url.clone()).unwrap_or_else(|| l("ChatGPT 账号", "ChatGPT account").into())),
        Kv::text(
            l("官方登录混用", "Official sign-in mix"),
            match signed {
                None => l("未开启", "Off"),
                Some(SignIn::ChatGpt) => l("已开启 · ChatGPT 已登录", "On · signed in with ChatGPT"),
                Some(SignIn::ApiKey) => l("已开启 · 但当前是 API Key 登录", "On · but signed in with an API key"),
                Some(SignIn::None) => l("已开启 · ChatGPT 未登录", "On · not signed in with ChatGPT"),
                Some(SignIn::Unknown) => l("已开启 · 登录凭据在系统钥匙串", "On · credentials are in the system keyring"),
            },
        ),
        Kv::mono("model", doc.get("model").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        Kv::mono("service_tier", format!("\"{tier}\"")),
        Kv::text(l("Fast 选项", "Fast option"), if inject { l("注入显示（经 AgentPlus 启动时生效）", "Injected (takes effect when launched via AgentPlus)") } else if custom { l("被 Codex 隐藏", "Hidden by Codex") } else { l("官方账号可见", "Visible with the official account") }),
        Kv::mono(l("模型目录", "Model catalog"), match &st.catalog {
            Some(c) => tr!("{}/{} 可见", "{}/{} visible", c.iter().filter(|m| m.visible).count(), c.len()),
            None => l("在线获取", "Fetched online").into(),
        }),
        Kv::mono(l("推理强度", "Reasoning effort"), effs.join(" ")),
    ];
    st
}

/// Base URL + key of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (doc, _) = load_doc()?;
    let base = provider_str(&doc, id, "base_url").ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 base_url", "Provider {id} has no base_url")))?;
    let key = provider_str(&doc, id, "env_key").and_then(|k| env_value(&k));
    Ok((base, key, "responses".into()))
}

// ---------------------------------------------------------------- write

/// Official sign-in mix: `requires_openai_auth = true` keeps Codex on the ChatGPT sign-in
/// (account features stay unlocked) while the provider's own key (`env_key`, which Codex
/// prefers over the ChatGPT token) authenticates the requests to its `base_url`.
/// Returns true when the table changed.
fn set_official_auth(doc: &mut DocumentMut, id: &str, on: bool) -> Result<bool> {
    // Codex treats a provider named exactly "OpenAI" as its own backend (server-side compaction etc.), which relays don't support.
    if on && provider_str(doc, id, "name").as_deref() == Some("OpenAI") {
        return Err(anyhow!(l("开启官方登录混用时，供应商名称不能是 OpenAI（Codex 会把它当成官方后端），请换个名称", "With official sign-in mix on, the provider can't be named OpenAI (Codex treats that name as its own backend); pick another name")));
    }
    let t = doc
        .get_mut("model_providers")
        .and_then(|t| t.get_mut(id))
        .and_then(|t| t.as_table_like_mut())
        .ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
    let old = t.get("requires_openai_auth").and_then(|v| v.as_bool()).unwrap_or(false);
    if old == on {
        return Ok(false);
    }
    if on {
        t.insert("requires_openai_auth", value(true));
    } else {
        t.remove("requires_openai_auth");
    }
    Ok(true)
}

/// Applies `ops` in memory, records the diff, and writes files unless `dry_run`.
pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    if crate::official::active() {
        return Err(anyhow!(l("正在获取官方模型列表，config.toml 是临时状态；先在「模型列表」里完成或取消获取", "Fetching the official model list; config.toml is in a temporary state. Finish or cancel the fetch in \"Model list\" first")));
    }
    let (mut doc, meta) = load_doc()?;
    let cfg_file = display_path(&config_path());
    let env_file = display_path(&env_path());
    let mut catalog = load_catalog(&doc);
    let cat_file = catalog.as_ref().map(|(p, _, _)| display_path(p)).unwrap_or_default();
    let mut store = store::load();
    // Only a key change writes .env: an unreadable one (UTF-16, GBK…) blocks just that.
    let env_read = read_env_checked();
    let (mut env_lines, env_meta) = env_read.as_ref().map(|(l, m)| (l.clone(), *m)).unwrap_or((vec![], TextMeta::NEW));
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
    let before = current_provider(&doc, &store);
    let mut switched: Option<String> = None;

    for op in ordered {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.api != "responses" {
                    return Err(anyhow!(l("Codex 只支持 Responses 接口", "Codex only supports the Responses API")));
                }
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                let id = match &p.id {
                    Some(id) => {
                        provider_item(&doc, id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
                        id.clone()
                    }
                    None => unique_id(&slug(&p.name), |c| c == "openai" || c == FIXED_ID || provider_item(&doc, c).is_some()),
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
                    put_provider(&mut doc, &id, Item::Table(t))?;
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
                if let Some(on) = p.official_auth {
                    if set_official_auth(&mut doc, &id, on)? {
                        if on {
                            diff.push(&cfg_file, tr!("[model_providers.{id}] requires_openai_auth = true（官方登录混用：保留 ChatGPT 登录，请求发往此供应商）", "[model_providers.{id}] requires_openai_auth = true (official sign-in mix: keep the ChatGPT sign-in, send requests to this provider)"), true);
                        } else {
                            diff.push(&cfg_file, tr!("[model_providers.{id}] - requires_openai_auth（关闭官方登录混用）", "[model_providers.{id}] - requires_openai_auth (official sign-in mix off)"), false);
                        }
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
                    cfg_dirty |= mirror(&mut doc, &id, &mut store, &mut diff, &cfg_file)?;
                    store_dirty = true;
                }
            }
            Op::DeleteProvider { provider } => {
                let raw = configured_provider(&doc);
                let src = store::get_str(&store, ID, "fixedSource");
                if &raw == provider || (raw == FIXED_ID && src.as_deref() == Some(provider.as_str())) {
                    return Err(anyhow!(tr!("「{provider}」正在使用，先切换到其他供应商再删除", "\"{provider}\" is in use; switch to another provider before deleting it")));
                }
                let removed = doc
                    .get_mut("model_providers")
                    .and_then(|t| t.as_table_like_mut())
                    .and_then(|t| t.remove(provider))
                    .is_some();
                if removed {
                    diff.push(&cfg_file, tr!("- [model_providers.{provider}]（~/.codex/.env 里的密钥保留）", "- [model_providers.{provider}] (API key in ~/.codex/.env is kept)"), false);
                    cfg_dirty = true;
                }
            }
            Op::SetCurrentProvider { provider } => {
                // Keep the old provider's list, then bring in the new one's.
                if let Some((_, v, _)) = catalog.as_mut() {
                    if provider != &before {
                        if set_stored_list(&mut store, &before, &visible_slugs(v)) {
                            store_dirty = true;
                        }
                        if let Some(list) = stored_list(&store, provider) {
                            if apply_list(v, &mut store, &list, &mut diff, &cat_file, provider)? {
                                cat_dirty = true;
                                store_dirty = true;
                            }
                        }
                    }
                }
                if provider != "openai" && provider_item(&doc, provider).is_none() {
                    return Err(anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")));
                }
                switched = Some(provider.clone());
                let raw = configured_provider(&doc);
                if provider == "openai" || !fixed {
                    if &raw != provider {
                        doc["model_provider"] = value(provider.as_str());
                        diff.push(&cfg_file, format!("model_provider = \"{raw}\" → \"{provider}\""), true);
                        cfg_dirty = true;
                    }
                } else {
                    cfg_dirty |= mirror(&mut doc, provider, &mut store, &mut diff, &cfg_file)?;
                    store_dirty = true;
                }
            }
            Op::SetModelVisible { model, visible, .. } => {
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!(l("没有可编辑的模型目录", "No editable model catalog")))?;
                let entry = catalog_entry(v, model).ok_or_else(|| anyhow!(tr!("模型目录里没有 {model}", "{model} is not in the model catalog")))?;
                let want = if *visible { "list" } else { "hide" };
                let old = entry.get("visibility").and_then(|s| s.as_str()).unwrap_or("list").to_string();
                if old != want {
                    entry["visibility"] = Value::from(want);
                    diff.push(&cat_file, format!("{model}  visibility \"{old}\" → \"{want}\""), *visible);
                    cat_dirty = true;
                }
            }
            Op::UpsertModel { model: m, .. } => {
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!(l("没有可编辑的模型目录", "No editable model catalog")))?;
                let id = m.id.trim().to_string();
                if id.is_empty() {
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
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
                    for (k, x) in &m.extra {
                        crate::mfields::check(crate::mfields::CODEX, k, x)?;
                    }
                    let line = add_custom_model(v, &mut store, &id, m.name.as_deref(), m.context)?;
                    diff.push(&cat_file, line, true);
                    cat_dirty = true;
                    store_dirty = true;
                }
                if let Some(entry) = catalog_entry(v, &id) {
                    for l in crate::mfields::write(entry, crate::mfields::CODEX, &m.extra)? {
                        diff.push(&cat_file, format!("{id}  {l}"), true);
                        cat_dirty = true;
                    }
                }
            }
            Op::DeleteModel { model, .. } => {
                let mut custom = custom_models(&store);
                if !custom.contains(model) {
                    return Err(anyhow!(tr!("{model} 是 Codex 自带的模型，只能隐藏不能删除", "{model} is a built-in Codex model; it can be hidden but not deleted")));
                }
                let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!(l("没有可编辑的模型目录", "No editable model catalog")))?;
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
                        cfg_dirty |= mirror(&mut doc, &cur, &mut store, &mut diff, &cfg_file)?;
                        store_dirty = true;
                    } else if !on && raw == FIXED_ID {
                        doc["model_provider"] = value(cur.as_str());
                        diff.push(&cfg_file, tr!("model_provider = \"{FIXED_ID}\" → \"{cur}\"（关闭固定 ID）", "model_provider = \"{FIXED_ID}\" → \"{cur}\" (fixed ID off)"), false);
                        cfg_dirty = true;
                    }
                }
                "fast_inject" => {
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, "fastInject") != on {
                        store::set_flag(&mut store, ID, "fastInject", on);
                        diff.push(inject_file(), if on { l("+ 启用 Fast 显示注入（通过 AgentPlus 重启 Codex 后生效）", "+ Enable Fast display injection (takes effect after restarting Codex via AgentPlus)") } else { l("- 停用 Fast 显示注入", "- Disable Fast display injection") }, on);
                        store_dirty = true;
                    }
                }
                "full_names" => {
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, "fullModelNames") != on {
                        store::set_flag(&mut store, ID, "fullModelNames", on);
                        diff.push(inject_file(), if on { l("+ 启用完整模型名注入（通过 AgentPlus 重启 Codex 后生效）", "+ Enable full model name injection (takes effect after restarting Codex via AgentPlus)") } else { l("- 停用完整模型名注入", "- Disable full model name injection") }, on);
                        store_dirty = true;
                    }
                }
                "quota_unlock" => {
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, "quotaUnlock") != on {
                        store::set_flag(&mut store, ID, "quotaUnlock", on);
                        diff.push(inject_file(), if on { l("+ 启用额度用完仍可发送注入（通过 AgentPlus 重启 Codex 后生效）", "+ Enable send-after-quota injection (takes effect after restarting Codex via AgentPlus)") } else { l("- 停用额度用完仍可发送注入", "- Disable send-after-quota injection") }, on);
                        store_dirty = true;
                    }
                }
                "hide_usage_banner" => {
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, "hideUsageBanner") != on {
                        store::set_flag(&mut store, ID, "hideUsageBanner", on);
                        diff.push(inject_file(), if on { l("+ 启用隐藏用量提示横幅注入（通过 AgentPlus 重启 Codex 后生效）", "+ Enable hide-usage-banner injection (takes effect after restarting Codex via AgentPlus)") } else { l("- 停用隐藏用量提示横幅注入", "- Disable hide-usage-banner injection") }, on);
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
                    let want: Vec<String> = crate::util::str_list(Some(v)).unwrap_or_default();
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
                other => return Err(anyhow!(tr!("未知设置 {other}", "Unknown setting: {other}"))),
            },
            Op::SetProviderModels { provider, models } => {
                let list = clean_ids(models);
                let cur = switched.clone().unwrap_or_else(|| before.clone());
                if provider == &cur {
                    // The active provider's list *is* the catalog.
                    let (_, v, _) = catalog.as_mut().ok_or_else(|| anyhow!(l("没有可编辑的模型目录", "No editable model catalog")))?;
                    if apply_list(v, &mut store, &list, &mut diff, &cat_file, provider)? {
                        cat_dirty = true;
                        store_dirty = true;
                    }
                } else if set_stored_list(&mut store, provider, &list) {
                    diff.push(l("AgentPlus · 各供应商的模型列表", "AgentPlus · per-provider model lists"), tr!("「{provider}」的模型列表：{} 个（切换到它时生效）", "\"{provider}\" model list: {} (takes effect when you switch to it)", list.len()), true);
                    store_dirty = true;
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Codex 同时只能使用一个供应商", "Codex can only use one provider at a time"))),
            Op::SetModelRoles { .. } => return Err(anyhow!(l("只有 Claude Code 需要分配模型角色", "Only Claude Code uses model roles"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    // The active provider's stored list always mirrors the catalog.
    if let Some((_, v, _)) = catalog.as_ref() {
        let cur = switched.unwrap_or(before);
        if set_stored_list(&mut store, &cur, &visible_slugs(v)) {
            store_dirty = true;
        }
    }

    if env_dirty {
        env_read?;
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

/// UI patches to apply when AgentPlus restarts Codex.
pub fn ui_patches() -> crate::cdp::Patches {
    let s = store::load();
    crate::cdp::Patches {
        fast: store::get_flag(&s, ID, "fastInject"),
        full_names: store::get_flag(&s, ID, "fullModelNames"),
        quota: store::get_flag(&s, ID, "quotaUnlock"),
        usage_banner: store::get_flag(&s, ID, "hideUsageBanner"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn official_auth_toggles_requires_openai_auth() {
        let mut doc = "[model_providers.relay]
name = \"relay\"
base_url = \"https://r.example.com/v1\"
wire_api = \"responses\"
env_key = \"RELAY_API_KEY\"
".parse::<DocumentMut>().unwrap();
        assert!(set_official_auth(&mut doc, "relay", true).unwrap());
        assert!(!set_official_auth(&mut doc, "relay", true).unwrap());
        let text = doc.to_string();
        assert!(text.contains("requires_openai_auth = true"));
        assert!(text.contains("env_key = \"RELAY_API_KEY\""), "the relay key must stay on env_key");
        assert!(providers(&doc).iter().any(|p| p.id == "relay" && p.official_auth));
        assert!(set_official_auth(&mut doc, "relay", false).unwrap());
        assert!(!doc.to_string().contains("requires_openai_auth"));
        assert!(set_official_auth(&mut doc, "missing", true).is_err());
    }

    #[test]
    fn mirror_diff_lists_only_the_fields_that_change() {
        let src = "model_provider = \"agentplus\"
[model_providers.klpz]
name = \"klpz\"
base_url = \"http://a:8080\"
wire_api = \"responses\"
env_key = \"KEY\"
requires_openai_auth = true

[model_providers.agentplus]
name = \"AgentPlus（klpz）\"
base_url = \"http://a:8080\"
wire_api = \"responses\"
env_key = \"KEY\"
requires_openai_auth = true
stale = 1

[model_providers.work]
name = \"work\"
base_url = \"http://b:8080\"
wire_api = \"responses\"
env_key = \"KEY\"
requires_openai_auth = true
http_headers = { X = \"1\" }
";
        let mut doc = src.parse::<DocumentMut>().unwrap();
        let mut store = json!({});
        let mut diff = Diff::default();
        assert!(mirror(&mut doc, "work", &mut store, &mut diff, "c").unwrap());
        let lines: Vec<_> = diff.groups[0].lines.iter().map(|l| (l.text.as_str(), l.add)).collect();
        assert_eq!(lines, [
            ("[model_providers.agentplus] name = \"AgentPlus（klpz）\" → \"AgentPlus（work）\"", true),
            ("[model_providers.agentplus] base_url = \"http://a:8080\" → \"http://b:8080\"", true),
            ("[model_providers.agentplus] + http_headers = { X = \"1\" }", true),
            ("[model_providers.agentplus] - stale", false),
        ]);
        assert_eq!(provider_str(&doc, FIXED_ID, "base_url").as_deref(), Some("http://b:8080"));
        assert_eq!(store::get_str(&store, ID, "fixedSource").as_deref(), Some("work"));
        // Mirroring the same provider again changes nothing and says nothing.
        let text = doc.to_string();
        let mut diff = Diff::default();
        assert!(!mirror(&mut doc, "work", &mut store, &mut diff, "c").unwrap());
        assert!(diff.groups.is_empty());
        assert_eq!(doc.to_string(), text);
        // First mirror: points model_provider at it and lists every copied field.
        let mut doc = "model_provider = \"work\"\n[model_providers.work]\nname = \"work\"\nbase_url = \"http://b\"\n".parse::<DocumentMut>().unwrap();
        let mut diff = Diff::default();
        assert!(mirror(&mut doc, "work", &mut store, &mut diff, "c").unwrap());
        let lines: Vec<_> = diff.groups[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, [
            "model_provider = \"work\" → \"agentplus\"",
            "+ [model_providers.agentplus]（复制自「work」）",
            "[model_providers.agentplus] + name = \"AgentPlus（work）\"",
            "[model_providers.agentplus] + base_url = \"http://b\"",
        ]);
    }

    #[test]
    fn put_provider_keeps_the_file_style() {
        let table = || {
            let mut t = Table::new();
            t.insert("name", value("N"));
            t.insert("base_url", value("https://n/v1"));
            Item::Table(t)
        };
        // Inline `model_providers = { … }`: toml_edit would drop a table item here.
        let mut doc = "model_provider = \"a\"\nmodel_providers = { a = { name = \"A\", base_url = \"https://a/v1\" } }\n".parse::<DocumentMut>().unwrap();
        put_provider(&mut doc, "new.one", table()).unwrap();
        let text = doc.to_string();
        assert!(text.contains("\"new.one\" = { name = \"N\", base_url = \"https://n/v1\" }"), "{text}");
        assert_eq!(provider_str(&doc.to_string().parse().unwrap(), "new.one", "base_url").as_deref(), Some("https://n/v1"));
        // Regular tables, and a missing section, get a [model_providers.x] table.
        for src in ["[model_providers.a]\nname = \"A\"\n", "model = \"m\"\n"] {
            let mut doc = src.parse::<DocumentMut>().unwrap();
            put_provider(&mut doc, "n", table()).unwrap();
            let text = doc.to_string();
            assert!(text.contains("[model_providers.n]\nname = \"N\""), "{text}");
            assert!(!text.contains("[model_providers]\n"), "no empty parent header: {text}");
        }
        // Copying an inline entry into a table section turns it into a table.
        let mut doc = "[model_providers.a]\nname = \"A\"\n".parse::<DocumentMut>().unwrap();
        let inline = "x = { name = \"I\" }".parse::<DocumentMut>().unwrap()["x"].clone();
        put_provider(&mut doc, "b", inline).unwrap();
        assert!(doc.to_string().contains("[model_providers.b]\nname = \"I\""));
        // Anything else is refused.
        let mut doc = "model_providers = 3\n".parse::<DocumentMut>().unwrap();
        assert!(put_provider(&mut doc, "n", table()).is_err());
        assert_eq!(doc.to_string(), "model_providers = 3\n");
    }

    #[test]
    fn official_auth_rejects_the_openai_name() {
        let mut doc = "[model_providers.x]
name = \"OpenAI\"
base_url = \"https://r.example.com/v1\"
".parse::<DocumentMut>().unwrap();
        assert!(set_official_auth(&mut doc, "x", true).is_err());
        assert!(!set_official_auth(&mut doc, "x", false).unwrap());
    }

    #[test]
    fn reads_sign_in_mode() {
        let tokens = json!({ "id_token": "i", "access_token": "a", "refresh_token": "r" });
        assert_eq!(sign_in_from(&json!({ "auth_mode": "chatgpt", "OPENAI_API_KEY": null, "tokens": tokens })), SignIn::ChatGpt);
        assert_eq!(sign_in_from(&json!({ "OPENAI_API_KEY": null, "tokens": tokens })), SignIn::ChatGpt);
        assert_eq!(sign_in_from(&json!({ "OPENAI_API_KEY": "sk-x", "tokens": tokens })), SignIn::ApiKey);
        assert_eq!(sign_in_from(&json!({ "auth_mode": "apikey", "OPENAI_API_KEY": "sk-x" })), SignIn::ApiKey);
        assert_eq!(sign_in_from(&json!({ "auth_mode": "chatgpt", "OPENAI_API_KEY": null })), SignIn::None);
        assert_eq!(sign_in_from(&json!({})), SignIn::None);
    }
}
