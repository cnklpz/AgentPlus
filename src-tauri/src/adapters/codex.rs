//! Codex (desktop + CLI share `~/.codex/config.toml`).
//! Model picker contents come from the catalog named by `model_catalog_json`;
//! each entry's `visibility` ("list" | "hide") decides whether it shows up.
//! Provider keys live in `~/.codex/.env` under the provider's `env_key`.

use super::msg;
use super::{Plan, Endpoint};
use crate::dotenv;
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;
use toml_edit::{value, Array, DocumentMut, Item, Table, TableLike};

pub const ID: &str = "codex";
pub const NAME: &str = "Codex";
const EFFORTS: [&str; 7] = ["low", "medium", "high", "xhigh", "persistent", "ultra", "max"];

fn inject_file() -> &'static str {
    l("AgentPlus · Codex UI injection", "AgentPlus · Codex 界面注入")
}

/// A UI injection AgentPlus applies when it restarts the desktop app: the setting key,
/// the store flag, and the (en, zh) diff lines for turning it on and off.
pub struct Injection {
    pub key: &'static str,
    flag: &'static str,
    on: (&'static str, &'static str),
    off: (&'static str, &'static str),
}

/// Fast display, full model names, send after the quota runs out, hidden usage banners.
pub const INJECTIONS: [Injection; 4] = [
    Injection {
        key: "fast_inject",
        flag: "fastInject",
        on: ("+ Enable Fast display injection (takes effect after restarting Codex via AgentPlus)", "+ 启用 Fast 显示注入（通过 AgentPlus 重启 Codex 后生效）"),
        off: ("- Disable Fast display injection", "- 停用 Fast 显示注入"),
    },
    Injection {
        key: "full_names",
        flag: "fullModelNames",
        on: ("+ Enable full model name injection (takes effect after restarting Codex via AgentPlus)", "+ 启用完整模型名注入（通过 AgentPlus 重启 Codex 后生效）"),
        off: ("- Disable full model name injection", "- 停用完整模型名注入"),
    },
    Injection {
        key: "quota_unlock",
        flag: "quotaUnlock",
        on: ("+ Enable send-after-quota injection (takes effect after restarting Codex via AgentPlus)", "+ 启用额度用完仍可发送注入（通过 AgentPlus 重启 Codex 后生效）"),
        off: ("- Disable send-after-quota injection", "- 停用额度用完仍可发送注入"),
    },
    Injection {
        key: "hide_usage_banner",
        flag: "hideUsageBanner",
        on: ("+ Enable hide-usage-banner injection (takes effect after restarting Codex via AgentPlus)", "+ 启用隐藏用量提示横幅注入（通过 AgentPlus 重启 Codex 后生效）"),
        off: ("- Disable hide-usage-banner injection", "- 停用隐藏用量提示横幅注入"),
    },
];

/// Which injections are on, in `INJECTIONS` order.
fn injections_on(store: &Value) -> [bool; 4] {
    INJECTIONS.map(|i| store::get_flag(store, ID, i.flag))
}
/// Codex writes the catalog's Fast tier id ("priority") when Fast is picked in its menu.
const FAST_TIER: &str = "priority";

pub const MARKER: &str = "config.toml";
pub const WSL_SCRIPT: &str = "codex --version 2>/dev/null; pgrep -x codex >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".codex";

/// `$CODEX_HOME` (Windows side only), else `~/.codex`.
pub fn default_dir() -> PathBuf {
    crate::env::agent_var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

/// The folder picked in AgentPlus, else the default.
pub fn codex_home() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

/// The desktop app (MSIX package).
pub fn detect() -> Install {
    crate::process::detect_codex()
}

pub(crate) fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn env_path() -> PathBuf {
    codex_home().join(".env")
}

pub(crate) fn load_doc() -> Result<(DocumentMut, TextMeta)> {
    let (text, meta) = read_text(&config_path())?;
    let doc = text.parse::<DocumentMut>().map_err(|e| anyhow!(tr!("Couldn't parse config.toml: {e}", "config.toml 解析失败：{e}")))?;
    Ok((doc, meta))
}

/// The catalog file named by `model_catalog_json` (a Linux path in WSL mode, `~/…`).
pub(crate) fn catalog_path(doc: &DocumentMut) -> Option<PathBuf> {
    doc.get("model_catalog_json").and_then(|i| i.as_str()).map(crate::env::resolve_path)
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

/// The protocol a provider speaks: "chat" for `wire_api = "chat"`, else Codex's default "responses".
fn wire_api(item: &Item) -> &'static str {
    if item.get("wire_api").and_then(|v| v.as_str()) == Some("chat") { "chat" } else { "responses" }
}

fn provider_mut<'a>(doc: &'a mut DocumentMut, id: &str) -> Result<&'a mut dyn TableLike> {
    doc.get_mut("model_providers")
        .and_then(|t| t.get_mut(id))
        .and_then(|t| t.as_table_like_mut())
        .ok_or_else(|| msg::no_provider(id))
}

fn not_a_table(name: &str) -> anyhow::Error {
    anyhow!(tr!("{name} in config.toml is not a table", "config.toml 里的 {name} 不是表"))
}

/// A top-level section (`[tui]`, `[desktop]`) to edit: an existing table or inline table as
/// it is, a new `[name]` table when missing. Anything else is an error (indexing would panic).
fn section_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut dyn TableLike> {
    if doc.get(name).is_none() {
        doc.insert(name, Item::Table(Table::new()));
    }
    doc.get_mut(name).and_then(|i| i.as_table_like_mut()).ok_or_else(|| not_a_table(name))
}

/// `t[key] = item`: an existing key keeps its place and formatting (`insert` would reset it).
fn set_key(t: &mut dyn TableLike, key: &str, item: Item) {
    *t.entry(key).or_insert(Item::None) = item;
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
                _ => return Err(anyhow!(tr!("The config of provider {id} is invalid", "供应商 {id} 的配置无效"))),
            };
            it.insert(id, v);
        }
        _ => return Err(not_a_table("model_providers")),
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
        return Err(anyhow!(l("The OpenAI official account can't use the fixed ID; switch to a custom provider first", "OpenAI 官方账号不能使用固定 ID，请先切换到自定义供应商")));
    }
    let mut table = provider_item(doc, provider).cloned().ok_or_else(|| msg::no_provider(provider))?;
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
        diff.push(cfg_file, tr!("+ [model_providers.{FIXED_ID}] (copied from \"{provider}\")", "+ [model_providers.{FIXED_ID}]（复制自「{provider}」）"), true);
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

/// Codex skips ~/.codex/.env keys whose ASCII-uppercased name starts with `CODEX_`
/// (codex-rs/arg0 `load_dotenv`), so such an env_key only works from the process environment.
fn codex_ignores_in_file(name: &str) -> bool {
    name.to_ascii_uppercase().starts_with("CODEX_")
}

/// An env_key as Codex sees it, and whether it comes from ~/.codex/.env. Codex sets every
/// pair of that file into its environment in order, so the file beats the process
/// environment (an assignment there wins even when empty) and the last assignment of a key
/// wins — except for `CODEX_` names, which it never reads from the file.
fn env_lookup(name: &str) -> Option<(String, bool)> {
    let file = if codex_ignores_in_file(name) {
        None
    } else {
        dotenv::get_last(&read_env().0.join("\n"), name)
    };
    match file {
        Some(v) => Some((v, true)),
        None => crate::env::agent_var(name).map(|v| (v, false)),
    }
}

/// Value of an env_key (see `env_lookup`); None when unset or empty.
pub fn env_value(name: &str) -> Option<String> {
    env_lookup(name).map(|(v, _)| v).filter(|v| !v.is_empty())
}

/// Sets `name` to `val` so Codex (dotenvy) reads it back: the value is quoted when it needs
/// to be, and later duplicates of the key are dropped (they would win over the rewritten line).
fn set_env(lines: &mut Vec<String>, name: &str, val: &str) {
    *lines = dotenv::set_dotenvy(&lines.join("\n"), name, val).lines().map(String::from).collect();
}

/// Where the key comes from; agrees with `env_value` (an empty `KEY=` in .env is not a key).
fn key_status(name: &str) -> &'static str {
    match env_lookup(name) {
        Some((v, from_file)) if !v.is_empty() => {
            if from_file {
                l("set in ~/.codex/.env", "已在 ~/.codex/.env 配置")
            } else {
                l("set in system environment variables", "已在系统环境变量配置")
            }
        }
        _ => l("not found; requests will fail", "未找到，请求会失败"),
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

/// Signed in with a ChatGPT account (what Codex needs to download the official model list).
pub(crate) fn chatgpt_signed_in() -> bool {
    read_json(&codex_home().join("auth.json")).is_ok_and(|(v, _)| sign_in_from(&v) == SignIn::ChatGpt)
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
    let mut out = vec![Provider::builtin(
        "openai",
        l("OpenAI official", "OpenAI 官方"),
        l("ChatGPT account sign-in", "ChatGPT 账号登录"),
        "responses",
        api_label("responses"),
        vec![
            Kv::text(lbl::auth(), l("ChatGPT account sign-in (~/.codex/auth.json)", "ChatGPT 账号登录（~/.codex/auth.json）")),
            Kv::text("Fast", l("Shown natively by Codex when signed in with an account", "账号登录时 Codex 原生显示")),
            Kv::mono(lbl::config_id(), l("openai (built-in)", "openai（内置）")),
        ],
    )];
    if let Some(t) = doc.get("model_providers").and_then(|i| i.as_table_like()) {
        for (id, item) in t.iter() {
            if id == FIXED_ID {
                continue; // managed mirror, not shown as its own card
            }
            let get = |k: &str| item.get(k).and_then(|v| v.as_str()).map(String::from);
            let wire = get("wire_api").unwrap_or_else(|| "responses".into());
            let base = get("base_url");
            let api = wire_api(item);
            let chat = api == "chat";
            let env_key = get("env_key");
            let official_auth = item.get("requires_openai_auth").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut details = vec![
                Kv::mono(lbl::config_id(), format!("[model_providers.{id}]")),
                Kv::mono("wire_api", format!("\"{wire}\"")),
            ];
            if official_auth {
                details.push(Kv::text(l("Official sign-in mix", "官方登录混用"), l("On · Codex stays signed in with ChatGPT; requests go to this provider with its own API key", "已开启 · Codex 用 ChatGPT 账号登录，对话请求发往此供应商并使用它的密钥")));
            }
            match &env_key {
                Some(k) => details.push(Kv::text(lbl::api_key(), tr!("Environment variable {k} · {}", "环境变量 {k} · {}", key_status(k)))),
                None => details.push(Kv::text(lbl::api_key(), l("env_key not set", "未设置 env_key"))),
            }
            details.push(Kv::text("Fast", l("Hidden by Codex by default (can be shown via injection in \"Other settings\")", "Codex 默认隐藏（可在「其他设置」注入显示）")));
            out.push(Provider {
                details,
                id: id.to_string(),
                name: get("name").unwrap_or_else(|| id.to_string()),
                host: base.as_deref().map(host_of).unwrap_or_default(),
                base_url: base,
                apis: vec![api_label(api).into()],
                enabled: true,
                compatible: !chat,
                reason: if chat { Some(l("Codex no longer supports the Chat API", "Codex 已不支持 Chat 接口").into()) } else { None },
                editable: true,
                api: api.into(),
                has_key: env_key.as_deref().and_then(env_value).is_some(),
                official_auth,
                ..Default::default()
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
    let models = v.get_mut("models").and_then(|x| x.as_array_mut()).ok_or_else(|| anyhow!(l("The model catalog has an unexpected format", "模型目录格式不对")))?;
    let mut entry = models.iter().find(|m| m.is_object()).cloned().ok_or_else(|| anyhow!(l("The model catalog is empty; no entry to copy from", "模型目录是空的，没有可参照的条目")))?;
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
    let ctx = context.map(|c| tr!(", context {}", "，上下文 {}", fmt_ctx(c))).unwrap_or_default();
    Ok(tr!("+ {id} ({name}{ctx})", "+ {id}（{name}{ctx}）"))
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
            // The copied entry carries another model's context and input kinds: use what is known about this one.
            let g = crate::modelinfo::guess(ID, id);
            added.push(add_custom_model(v, store, id, None, g.as_ref().and_then(|g| g.context))?);
            if let (Some(g), Some(entry)) = (&g, catalog_entry(v, id)) {
                added.extend(crate::mfields::write(entry, crate::mfields::CODEX, &g.extra)?.into_iter().map(|l| format!("{id}  {l}")));
            }
        }
    }
    if shown + hidden + added.len() == 0 {
        return Ok(false);
    }
    diff.push(cat_file, trn!(list.len(), "Model list replaced with \"{who}\"'s {n} model ({shown} shown, {hidden} hidden)", "Model list replaced with \"{who}\"'s {n} models ({shown} shown, {hidden} hidden)", "模型列表换成「{who}」的 {n} 个模型（显示 {shown} 个，隐藏 {hidden} 个）"), true);
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
                tags.push(Tag::new("custom", l("Custom", "自定义")));
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
    let mut st = super::new_state(ID, NAME, inst, "single", &codex_home(), vec![display_path(&config_path())]);
    let (doc, _) = match load_doc() {
        Ok(d) => d,
        Err(e) => {
            st.fail(e);
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
        st.notes.push(l("config.toml has no model_catalog_json, so Codex fetches the model list online and it can't be edited here.", "config.toml 没有设置 model_catalog_json，模型列表由 Codex 在线获取，暂不能编辑。").into());
    }

    let [inject, full_names, quota, hide_banner] = injections_on(&store);
    let tier = service_tier(&doc);
    let sl = status_line(&doc);
    let effs = efforts(&doc);
    st.settings = vec![
        bool_setting("fixed_id", l("Provider switching", "供应商切换"), l("Fixed provider ID (recommended)", "固定供应商 ID（推荐开启）"),
            l("When on, model_provider stays agentplus and switching providers only changes its base URL and API key, so sessions don't disappear from Codex's recent list and archive after a switch. Turning it on copies the current provider over; turning it off points model_provider back at the current provider.",
              "开启后 model_provider 固定为 agentplus，切换供应商只改它的地址和密钥，会话不会因为切换而从 Codex 的最近列表和归档里消失。开启时会把当前供应商复制过去；关闭时 model_provider 改回当前供应商。"), fixed),
        bool_setting("fast_inject", "Fast", l("Show Fast in Codex", "在 Codex 中显示 Fast"),
            l("Codex only shows Fast when signed in with a ChatGPT account and hides it for custom providers. When on, restarting Codex through AgentPlus launches it with a debug port and lifts this restriction when the UI loads.",
              "Codex 只在 ChatGPT 账号登录时显示 Fast，用自定义供应商会被隐藏。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制。"), inject),
        bool_setting("fast_default", "Fast", l("Use Fast by default", "默认使用 Fast"),
            l("Writes service_tier = \"priority\" (the same value Codex writes when you pick Fast). The provider must support the priority tier.",
              "写入 service_tier = \"priority\"（与 Codex 自己切换 Fast 时写入的值相同）。供应商需要支持 priority 档位。"), is_fast_tier(&tier)),
        bool_setting("fast_cli", "Fast", l("Show fast-mode in the Codex CLI status line", "Codex CLI 状态栏显示 fast-mode"),
            l("The CLI shares its config with the desktop app; appends fast-mode to [tui].status_line.", "CLI 与桌面版共用配置，在 [tui].status_line 里追加 fast-mode。"), sl.as_ref().map(|x| x.iter().any(|x| x == "fast-mode")).unwrap_or(false)),
        chips_setting("efforts", l("Reasoning effort", "推理强度"), l("Reasoning efforts offered in the picker", "选择器里可选的推理强度"),
            l("Checked levels appear in the Codex desktop reasoning effort menu ([desktop] enabled-reasoning-efforts). Levels a model doesn't support are not shown.",
              "勾选的档位会出现在 Codex 桌面版的推理强度菜单里（[desktop] enabled-reasoning-efforts）。模型不支持的档位不会显示。"), effs.clone(), &EFFORTS)
            .with_hints(&[
                l("Fastest replies, light reasoning", "回复最快，推理较浅"),
                l("Balances speed and depth; good for everyday tasks", "速度和深度平衡，适合日常任务"),
                l("Deeper reasoning for complex problems", "推理更深，适合复杂问题"),
                l("Deeper reasoning than high", "比 high 更深的推理"),
                l("Persistent mode: after the request, keeps doing useful follow-up work until nothing is left", "持久模式：做完请求后继续主动做后续有用的工作，直到没有可做的"),
                l("Maximum reasoning, automatically splitting tasks across subagents (some models)", "最高推理，并自动把任务拆给子代理（部分模型支持）"),
                l("Maximum reasoning depth for the hardest problems", "最高推理深度，适合最难的问题"),
            ]),
        bool_setting("full_names", l("Interface", "界面"), l("Show full model names", "显示完整模型名"),
            l("Codex shows full model names only when signed in with a ChatGPT account; with custom providers it drops the \"GPT-\" prefix (GPT-6 Sol shows as 6 Sol). When on, restarting Codex through AgentPlus launches it with a debug port and turns this shortening off when the UI loads.",
              "Codex 只在 ChatGPT 账号登录时显示完整模型名，用自定义供应商会去掉「GPT-」前缀（GPT-6 Sol 显示成 6 Sol）。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时关掉这个缩写。"), full_names),
        bool_setting("quota_unlock", l("Official sign-in mix", "官方登录混用"), l("Keep sending after the ChatGPT quota runs out", "ChatGPT 额度用完后仍可发送"),
            l("Signed in with a ChatGPT account, Codex disables the send button once the account's quota runs out, even with the official sign-in mix on and requests actually going to the relay. When on, restarting Codex through AgentPlus launches it with a debug port and lifts this restriction when the UI loads (hide the usage banner with the next option).",
              "用 ChatGPT 账号登录时，账号额度用完后 Codex 会禁用发送按钮，即使开启了官方登录混用、请求其实发往中转站。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这条限制（额度提示横幅用下一项隐藏）。"), quota),
        bool_setting("hide_usage_banner", l("Official sign-in mix", "官方登录混用"), l("Hide usage banners", "隐藏用量提示横幅"),
            l("Signed in with a ChatGPT account, Codex shows that account's usage banners above the composer (\"You're out of Codex and Work usage\", running-low warnings, upgrade and reset-usage buttons), which have nothing to do with the relay's quota. When on, restarting Codex through AgentPlus launches it with a debug port and removes these banners when the UI loads.",
              "用 ChatGPT 账号登录时，输入框上方会显示这个账号的额度横幅（「Codex 和工作使用额度已用完」、即将用完的提醒、升级和重置使用量按钮），与中转站的额度无关。开启后，通过 AgentPlus 重启 Codex 时会带调试端口启动，并在界面加载时去掉这些横幅。"), hide_banner),
        bool_setting("ctx_usage", l("Interface", "界面"), l("Show context usage", "显示上下文用量"), "[desktop] show-context-window-usage", desktop_bool(&doc, "show-context-window-usage", true)),
        bool_setting("plain", l("Interface", "界面"), l("Plain text composer", "纯文本输入框"), "[desktop] composerPlainTextMode", desktop_bool(&doc, "composerPlainTextMode", false)),
    ];

    let prov = st.providers.iter().find(|p| p.id == cur);
    let custom = prov.map(|p| !p.builtin).unwrap_or(false);
    let mixed = prov.is_some_and(|p| p.official_auth);
    let signed = mixed.then(|| sign_in(&doc));
    match signed {
        Some(SignIn::None) => st.notes.push(l("The current provider uses the official sign-in mix, but Codex isn't signed in with a ChatGPT account yet. Sign in in Codex so requests can go to the relay.", "当前供应商开启了官方登录混用，但 Codex 还没有登录 ChatGPT 账号：在 Codex 里登录后，对话才会发往中转站。").into()),
        Some(SignIn::ApiKey) => st.notes.push(l("The current provider uses the official sign-in mix, but Codex is signed in with an API key (~/.codex/auth.json), so account features stay locked. Sign out in Codex and sign in with a ChatGPT account.", "当前供应商开启了官方登录混用，但 Codex 现在是 API Key 登录（~/.codex/auth.json），官方账号功能不会解锁：在 Codex 里退出后改用 ChatGPT 账号登录。").into()),
        _ => {}
    }
    st.current_model = doc.get("model").and_then(|v| v.as_str()).map(str::trim).filter(|m| !m.is_empty()).map(String::from);
    st.current = vec![
        Kv::mono("model_provider", format!("\"{raw}\"")),
        Kv::text(
            l("Fixed ID", "固定 ID"),
            if fixed {
                tr!("On · points at \"{cur}\"", "已开启 · 指向「{cur}」")
            } else if st.fixed_prompt {
                l("Pre-enabled · written on provider switch", "预开启 · 切换供应商时写入").to_string()
            } else {
                l("Off", "未开启").to_string()
            },
        ),
        Kv::mono("base_url", prov.and_then(|p| p.base_url.clone()).unwrap_or_else(|| l("ChatGPT account", "ChatGPT 账号").into())),
        Kv::text(
            l("Official sign-in mix", "官方登录混用"),
            match signed {
                None => l("Off", "未开启"),
                Some(SignIn::ChatGpt) => l("On · signed in with ChatGPT", "已开启 · ChatGPT 已登录"),
                Some(SignIn::ApiKey) => l("On · but signed in with an API key", "已开启 · 但当前是 API Key 登录"),
                Some(SignIn::None) => l("On · not signed in with ChatGPT", "已开启 · ChatGPT 未登录"),
                Some(SignIn::Unknown) => l("On · credentials are in the system keyring", "已开启 · 登录凭据在系统钥匙串"),
            },
        ),
        Kv::mono("model", doc.get("model").and_then(|v| v.as_str()).unwrap_or("-").to_string()),
        Kv::mono("service_tier", format!("\"{tier}\"")),
        Kv::text(l("Fast option", "Fast 选项"), if inject { l("Injected (takes effect when launched via AgentPlus)", "注入显示（经 AgentPlus 启动时生效）") } else if custom { l("Hidden by Codex", "被 Codex 隐藏") } else { l("Visible with the official account", "官方账号可见") }),
        Kv::mono(l("Model catalog", "模型目录"), match &st.catalog {
            Some(c) => tr!("{}/{} visible", "{}/{} 可见", c.iter().filter(|m| m.visible).count(), c.len()),
            None => l("Fetched online", "在线获取").into(),
        }),
        Kv::mono(l("Reasoning effort", "推理强度"), effs.join(" ")),
    ];
    st
}

/// Base URL + key of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (doc, _) = load_doc()?;
    let base = provider_str(&doc, id, "base_url").ok_or_else(|| anyhow!(tr!("Provider {id} has no base_url", "供应商 {id} 没有 base_url")))?;
    let key = provider_str(&doc, id, "env_key").and_then(|k| env_value(&k));
    let api = provider_item(&doc, id).map(wire_api).unwrap_or("responses");
    Ok((base, key, api.into()))
}

// ---------------------------------------------------------------- write

/// Official sign-in mix: `requires_openai_auth = true` keeps Codex on the ChatGPT sign-in
/// (account features stay unlocked) while the provider's own key (`env_key`, which Codex
/// prefers over the ChatGPT token) authenticates the requests to its `base_url`.
/// Returns true when the table changed.
fn set_official_auth(doc: &mut DocumentMut, id: &str, on: bool) -> Result<bool> {
    // Codex treats a provider named exactly "OpenAI" as its own backend (server-side compaction etc.), which relays don't support.
    if on && provider_str(doc, id, "name").as_deref() == Some("OpenAI") {
        return Err(anyhow!(l("With official sign-in mix on, the provider can't be named OpenAI (Codex treats that name as its own backend); pick another name", "开启官方登录混用时，供应商名称不能是 OpenAI（Codex 会把它当成官方后端），请换个名称")));
    }
    let t = provider_mut(doc, id)?;
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
        return Err(anyhow!(l("Fetching the official model list; config.toml is in a temporary state. Finish or cancel the fetch in \"Model list\" first", "正在获取官方模型列表，config.toml 是临时状态；先在「模型列表」里完成或取消获取")));
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
                    return Err(anyhow!(l("Codex only supports the Responses API", "Codex 只支持 Responses 接口")));
                }
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                let id = match &p.id {
                    Some(id) => {
                        provider_item(&doc, id).ok_or_else(|| msg::no_provider(id))?;
                        id.clone()
                    }
                    None => unique_id(&slug(&p.name), |c| c == "openai" || c == FIXED_ID || provider_item(&doc, c).is_some()),
                };
                let is_new = p.id.is_none();
                // A default name Codex would skip in .env (id `codex-…`) gets an AGENTPLUS_ prefix.
                let env_key = provider_str(&doc, &id, "env_key").unwrap_or_else(|| {
                    let k = format!("{}_API_KEY", id.to_uppercase().replace('-', "_"));
                    if codex_ignores_in_file(&k) {
                        format!("AGENTPLUS_{k}")
                    } else {
                        k
                    }
                });
                if is_new {
                    let mut t = Table::new();
                    t.insert("name", value(p.name.trim()));
                    t.insert("base_url", value(p.base_url.trim()));
                    t.insert("wire_api", value("responses"));
                    t.insert("env_key", value(env_key.as_str()));
                    put_provider(&mut doc, &id, Item::Table(t))?;
                    diff.push(&cfg_file, format!("+ [model_providers.{id}] name = \"{}\", base_url = \"{}\"", p.name.trim(), p.base_url.trim()), true);
                    cfg_dirty = true;
                } else {
                    for (k, v) in [("name", p.name.trim()), ("base_url", p.base_url.trim())] {
                        let old = provider_str(&doc, &id, k).unwrap_or_default();
                        if old != v {
                            set_key(provider_mut(&mut doc, &id)?, k, value(v));
                            diff.push(&cfg_file, format!("[model_providers.{id}] {k} = \"{v}\""), true);
                            cfg_dirty = true;
                        }
                    }
                    if provider_str(&doc, &id, "env_key").is_none() {
                        set_key(provider_mut(&mut doc, &id)?, "env_key", value(env_key.as_str()));
                        diff.push(&cfg_file, format!("[model_providers.{id}] env_key = \"{env_key}\""), true);
                        cfg_dirty = true;
                    }
                }
                if let Some(on) = p.official_auth {
                    if set_official_auth(&mut doc, &id, on)? {
                        cfg_dirty = true;
                        if on {
                            diff.push(&cfg_file, tr!("[model_providers.{id}] requires_openai_auth = true (official sign-in mix: keep the ChatGPT sign-in, send requests to this provider)", "[model_providers.{id}] requires_openai_auth = true（官方登录混用：保留 ChatGPT 登录，请求发往此供应商）"), true);
                        } else {
                            diff.push(&cfg_file, tr!("[model_providers.{id}] - requires_openai_auth (official sign-in mix off)", "[model_providers.{id}] - requires_openai_auth（关闭官方登录混用）"), false);
                        }
                    }
                }
                if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                    if codex_ignores_in_file(&env_key) {
                        return Err(anyhow!(tr!("Codex ignores ~/.codex/.env variables starting with CODEX_: rename env_key, or set {env_key} as a system environment variable", "Codex 不读取 ~/.codex/.env 里以 CODEX_ 开头的变量：请把 env_key 改成别的名字，或在系统环境变量里设置 {env_key}")));
                    }
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
                    return Err(msg::in_use(provider));
                }
                let removed = doc
                    .get_mut("model_providers")
                    .and_then(|t| t.as_table_like_mut())
                    .and_then(|t| t.remove(provider))
                    .is_some();
                if removed {
                    diff.push(&cfg_file, tr!("- [model_providers.{provider}] (API key in ~/.codex/.env is kept)", "- [model_providers.{provider}]（~/.codex/.env 里的密钥保留）"), false);
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
                    return Err(msg::no_provider(provider));
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
                let v = catalog_mut(&mut catalog)?;
                let entry = catalog_entry(v, model).ok_or_else(|| anyhow!(tr!("{model} is not in the model catalog", "模型目录里没有 {model}")))?;
                let want = if *visible { "list" } else { "hide" };
                let old = entry.get("visibility").and_then(|s| s.as_str()).unwrap_or("list").to_string();
                if old != want {
                    entry["visibility"] = Value::from(want);
                    diff.push(&cat_file, format!("{model}  visibility \"{old}\" → \"{want}\""), *visible);
                    cat_dirty = true;
                }
            }
            Op::UpsertModel { model: m, .. } => {
                let v = catalog_mut(&mut catalog)?;
                let id = m.id.trim().to_string();
                if id.is_empty() {
                    return Err(msg::model_id_required());
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
                    return Err(anyhow!(tr!("{model} is a built-in Codex model; it can be hidden but not deleted", "{model} 是 Codex 自带的模型，只能隐藏不能删除")));
                }
                let v = catalog_mut(&mut catalog)?;
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
                        diff.push(&cfg_file, tr!("model_provider = \"{FIXED_ID}\" → \"{cur}\" (fixed ID off)", "model_provider = \"{FIXED_ID}\" → \"{cur}\"（关闭固定 ID）"), false);
                        cfg_dirty = true;
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
                        set_key(section_mut(&mut doc, "tui")?, "status_line", value(list.iter().collect::<Array>()));
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
                        set_key(section_mut(&mut doc, "desktop")?, "enabled-reasoning-efforts", value(ordered.into_iter().collect::<Array>()));
                        cfg_dirty = true;
                    }
                }
                "ctx_usage" | "plain" => {
                    let k = if key == "ctx_usage" { "show-context-window-usage" } else { "composerPlainTextMode" };
                    let on = v.as_bool().unwrap_or(false);
                    if desktop_bool(&doc, k, key == "ctx_usage") != on {
                        set_key(section_mut(&mut doc, "desktop")?, k, value(on));
                        diff.push(&cfg_file, format!("[desktop] {k} = {on}"), on);
                        cfg_dirty = true;
                    }
                }
                other => {
                    let inj = INJECTIONS.iter().find(|i| i.key == other).ok_or_else(|| msg::unknown_setting(other))?;
                    let on = v.as_bool().unwrap_or(false);
                    if store::get_flag(&store, ID, inj.flag) != on {
                        store::set_flag(&mut store, ID, inj.flag, on);
                        let (en, zh) = if on { inj.on } else { inj.off };
                        diff.push(inject_file(), l(en, zh), on);
                        store_dirty = true;
                    }
                }
            },
            Op::SetProviderModels { provider, models } => {
                let list = clean_ids(models);
                let cur = switched.clone().unwrap_or_else(|| before.clone());
                if provider == &cur {
                    // The active provider's list *is* the catalog.
                    let v = catalog_mut(&mut catalog)?;
                    if apply_list(v, &mut store, &list, &mut diff, &cat_file, provider)? {
                        cat_dirty = true;
                        store_dirty = true;
                    }
                } else if set_stored_list(&mut store, provider, &list) {
                    diff.push(l("AgentPlus · per-provider model lists", "AgentPlus · 各供应商的模型列表"), tr!("\"{provider}\" model list: {} (takes effect when you switch to it)", "「{provider}」的模型列表：{} 个（切换到它时生效）", list.len()), true);
                    store_dirty = true;
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Codex can only use one provider at a time", "Codex 同时只能使用一个供应商"))),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
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

fn catalog_mut(c: &mut Option<(PathBuf, Value, TextMeta)>) -> Result<&mut Value> {
    c.as_mut().map(|(_, v, _)| v).ok_or_else(|| anyhow!(l("No editable model catalog", "没有可编辑的模型目录")))
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
    let [fast, full_names, quota, usage_banner] = injections_on(&store::load());
    crate::cdp::Patches { fast, full_names, quota, usage_banner }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn listed_custom_models_are_filled_in_from_the_catalog() {
        let _h = crate::util::TestHome::new("codex-seed");
        crate::modelinfo::test_cache_acme();
        let mut v = json!({ "models": [{ "slug": "gpt-x", "display_name": "GPT X", "context_window": 1050000, "input_modalities": ["text", "image"], "visibility": "list", "priority": 1 }] });
        let mut store = json!({});
        let mut diff = Diff::default();
        assert!(apply_list(&mut v, &mut store, &["acme-vision-9".into(), "mystery-x".into()], &mut diff, "models.json", "relay").unwrap());
        let e = catalog_entry(&mut v, "acme-vision-9").unwrap();
        assert_eq!((&e["context_window"], &e["input_modalities"]), (&json!(64000), &json!(["text", "image"])));
        assert_eq!(catalog_entry(&mut v, "mystery-x").unwrap()["context_window"], 1050000, "nothing known: the copied entry's values stay");
        assert_eq!(custom_models(&store), ["acme-vision-9", "mystery-x"]);
    }

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

    fn setting(key: &str, on: bool) -> Op {
        Op::SetSetting { key: key.into(), value: json!(on) }
    }

    fn diff_lines(d: &Diff) -> Vec<(String, String)> {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| (g.file.clone(), l.text.clone()))).collect()
    }

    /// A temp home with ~/.codex/config.toml (and .env when given).
    fn codex_home_with(tag: &str, config: &str, env: Option<&str>) -> crate::util::TestHome {
        let h = crate::util::TestHome::new(tag);
        let dir = h.0.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), config).unwrap();
        if let Some(e) = env {
            std::fs::write(dir.join(".env"), e).unwrap();
        }
        h
    }

    #[test]
    fn section_edits_keep_style_and_refuse_scalars() {
        let src = "model = \"m\"\n\n[desktop]\n# effort levels\nenabled-reasoning-efforts = [\"low\"]\ncomposerPlainTextMode = false\n";
        let mut doc = src.parse::<DocumentMut>().unwrap();
        set_key(section_mut(&mut doc, "desktop").unwrap(), "enabled-reasoning-efforts", value(["high"].into_iter().collect::<Array>()));
        assert_eq!(doc.to_string(), src.replace("[\"low\"]", "[\"high\"]"), "the key keeps its place and comment");
        // A missing section becomes a [tui] table, not an inline `tui = { … }`.
        set_key(section_mut(&mut doc, "tui").unwrap(), "status_line", value(["fast-mode"].into_iter().collect::<Array>()));
        assert!(doc.to_string().ends_with("\n[tui]\nstatus_line = [\"fast-mode\"]\n"), "{doc}");
        // An inline section stays inline.
        let mut doc = "desktop = { composerPlainTextMode = false }\n".parse::<DocumentMut>().unwrap();
        set_key(section_mut(&mut doc, "desktop").unwrap(), "composerPlainTextMode", value(true));
        assert_eq!(doc.to_string(), "desktop = { composerPlainTextMode = true }\n");
        // A scalar is an error, not a panic, and the document is left alone.
        let mut doc = "tui = 1\n".parse::<DocumentMut>().unwrap();
        assert_eq!(section_mut(&mut doc, "tui").err().unwrap().to_string(), "config.toml 里的 tui 不是表");
        assert_eq!(doc.to_string(), "tui = 1\n");
    }

    #[test]
    fn scalar_section_fails_the_plan_instead_of_panicking() {
        let _h = codex_home_with("codex-scalar", "tui = 1\ndesktop = \"x\"\n", None);
        assert!(plan(&[setting("fast_cli", true)], true).is_err());
        assert!(plan(&[setting("plain", true)], true).is_err());
    }

    #[test]
    fn chat_wire_providers_report_the_chat_api() {
        let cfg = "[model_providers.old]\nname = \"Old\"\nbase_url = \"https://o/v1\"\nwire_api = \"chat\"\n\n[model_providers.new]\nname = \"New\"\nbase_url = \"https://n/v1\"\n";
        let _h = codex_home_with("codex-wire", cfg, None);
        let doc = cfg.parse::<DocumentMut>().unwrap();
        let apis: Vec<_> = providers(&doc).into_iter().map(|p| (p.id, p.api, p.compatible)).collect();
        assert_eq!(apis[1..], [("old".into(), "chat".into(), false), ("new".into(), "responses".into(), true)]);
        assert_eq!(provider_endpoint("old").unwrap().2, "chat");
        assert_eq!(provider_endpoint("new").unwrap().2, "responses");
    }

    #[test]
    fn key_status_agrees_with_env_value() {
        let _h = codex_home_with("codex-keys", "", Some("# SET=commented\nEMPTY=\nSET=sk-1\nSET=sk-2 # note\nLATE=sk-x\nLATE=\n"));
        crate::env::set_test_vars(&[("EMPTY", "sk-env"), ("LATE", "sk-env"), ("SET", "sk-env"), ("ENVONLY", "sk-e")]);
        // Codex sets every .env pair in order: the last assignment wins, over the environment too.
        assert_eq!(env_value("SET").as_deref(), Some("sk-2"), "the last assignment counts, without its comment");
        assert_eq!(key_status("SET"), "已在 ~/.codex/.env 配置");
        // An empty assignment in .env shadows the environment, and Codex then has no key.
        assert_eq!(env_value("EMPTY"), None);
        assert_eq!(key_status("EMPTY"), "未找到，请求会失败");
        assert_eq!(env_value("LATE"), None, "a later empty assignment wins over an earlier key");
        assert_eq!(key_status("LATE"), "未找到，请求会失败");
        assert_eq!(env_value("ENVONLY").as_deref(), Some("sk-e"));
        assert_eq!(key_status("ENVONLY"), "已在系统环境变量配置");
        assert_eq!(key_status("NONE"), "未找到，请求会失败");
    }

    #[test]
    fn set_env_leaves_one_readable_line() {
        let text = |lines: &[String]| lines.join("\n");
        let mut lines: Vec<String> = ["# K=old", "K=sk-1", "A=1", "export K=sk-2", ""].map(String::from).to_vec();
        set_env(&mut lines, "K", "sk-new");
        assert_eq!(text(&lines), "# K=old\nK=sk-new\nA=1", "later duplicates are dropped");
        assert_eq!(dotenv::get_last(&text(&lines), "K").as_deref(), Some("sk-new"));
        let mut lines = vec!["A=1".to_string()];
        set_env(&mut lines, "K", "a #b");
        assert_eq!(text(&lines), "A=1\nK=\"a #b\"");
        assert_eq!(dotenv::get_last(&text(&lines), "K").as_deref(), Some("a #b"), "a value that needs quotes reads back");
        let mut lines = vec![];
        set_env(&mut lines, "K", "v");
        assert_eq!(lines, ["K=v"]);
    }

    #[test]
    fn key_edit_rewrites_a_duplicated_env_key() {
        let cfg = "model_provider = \"relay\"\n\n[model_providers.relay]\nname = \"relay\"\nbase_url = \"https://r/v1\"\nenv_key = \"RELAY_API_KEY\"\n";
        let _h = codex_home_with("codex-dupkey", cfg, Some("RELAY_API_KEY=sk-old\nA=1\nRELAY_API_KEY=sk-older\n"));
        assert_eq!(env_value("RELAY_API_KEY").as_deref(), Some("sk-older"));
        let input = ProviderInput {
            id: Some("relay".into()),
            name: "relay".into(),
            base_url: "https://r/v1".into(),
            api: "responses".into(),
            api_key: Some("sk-new".into()),
            models: vec![],
            key_from_library: None, key_from_sync: None,
            official_auth: Some(false),
        };
        plan(&[Op::UpsertProvider { provider: input }], false).unwrap();
        assert_eq!(std::fs::read_to_string(env_path()).unwrap(), "RELAY_API_KEY=sk-new\nA=1\n");
        assert_eq!(env_value("RELAY_API_KEY").as_deref(), Some("sk-new"));
    }

    #[test]
    fn state_reports_the_configured_model() {
        let _h = codex_home_with("codex-model", "model = \"gpt-x\"\n", None);
        assert_eq!(state(&Install::default()).current_model.as_deref(), Some("gpt-x"));
        for cfg in ["model = \"\"\n", ""] {
            std::fs::write(config_path(), cfg).unwrap();
            let st = state(&Install::default());
            assert_eq!(st.current_model, None, "{cfg:?}");
            if cfg.is_empty() {
                assert!(st.current.iter().any(|r| r.k == "model" && r.v == "-"), "the display row keeps its placeholder");
            }
        }
    }

    fn relay_input(id: Option<&str>, name: &str, key: &str) -> ProviderInput {
        ProviderInput {
            id: id.map(String::from),
            name: name.into(),
            base_url: "https://r/v1".into(),
            api: "responses".into(),
            api_key: Some(key.into()),
            models: vec![],
            key_from_library: None, key_from_sync: None,
            official_auth: None,
        }
    }

    #[test]
    fn codex_prefixed_keys_are_not_read_from_env_file() {
        let _h = codex_home_with("codex-prefix", "", Some("CODEX_X=sk\ncodex_y=sk\n"));
        crate::env::set_test_vars(&[]);
        for name in ["CODEX_X", "codex_y"] {
            assert_eq!(env_value(name), None, "{name}: Codex skips it in .env");
            assert_eq!(key_status(name), "未找到，请求会失败");
        }
        crate::env::set_test_vars(&[("CODEX_X", "sk-e"), ("codex_y", "sk-e")]);
        for name in ["CODEX_X", "codex_y"] {
            assert_eq!(env_value(name).as_deref(), Some("sk-e"));
            assert_eq!(key_status(name), "已在系统环境变量配置");
        }
        crate::env::set_test_vars(&[]);
    }

    #[test]
    fn new_codex_named_provider_gets_a_readable_env_key() {
        let _h = codex_home_with("codex-prefix-new", "", None);
        plan(&[Op::UpsertProvider { provider: relay_input(None, "Codex Relay", "sk-$new") }], false).unwrap();
        let doc = load_doc().unwrap().0;
        assert_eq!(provider_str(&doc, "codex-relay", "env_key").as_deref(), Some("AGENTPLUS_CODEX_RELAY_API_KEY"));
        assert_eq!(std::fs::read_to_string(env_path()).unwrap(), "AGENTPLUS_CODEX_RELAY_API_KEY=\"sk-\\$new\"\n", "a `$` is escaped for dotenvy");
        assert_eq!(env_value("AGENTPLUS_CODEX_RELAY_API_KEY").as_deref(), Some("sk-$new"));
    }

    #[test]
    fn key_edit_refuses_a_codex_prefixed_env_key() {
        let cfg = "[model_providers.relay]\nname = \"relay\"\nbase_url = \"https://r/v1\"\nenv_key = \"CODEX_X\"\n";
        let _h = codex_home_with("codex-prefix-edit", cfg, Some("A=1\n"));
        assert!(plan(&[Op::UpsertProvider { provider: relay_input(Some("relay"), "relay", "sk-new") }], true).is_err());
        assert!(plan(&[Op::UpsertProvider { provider: relay_input(Some("relay"), "relay", "sk-new") }], false).is_err());
        assert_eq!(std::fs::read_to_string(env_path()).unwrap(), "A=1\n");
    }

    #[test]
    fn key_only_edit_leaves_config_toml_alone() {
        let cfg = "model_provider = \"relay\"\n\n[model_providers.relay]\nname = \"relay\"\nbase_url = \"https://r/v1\"\nenv_key = \"RELAY_API_KEY\"\n";
        let h = codex_home_with("codex-keyonly", cfg, None);
        let input = |key: Option<&str>| ProviderInput {
            id: Some("relay".into()),
            name: "relay".into(),
            base_url: "https://r/v1".into(),
            api: "responses".into(),
            api_key: key.map(String::from),
            models: vec![],
            key_from_library: None, key_from_sync: None,
            official_auth: Some(false),
        };
        let (diff, written, _) = plan(&[Op::UpsertProvider { provider: input(Some("sk-new")) }], false).unwrap();
        assert_eq!(written, [h.0.join(".codex").join(".env")]);
        assert_eq!(diff_lines(&diff), [("~/.codex/.env".to_string(), format!("RELAY_API_KEY = {}", mask_key("sk-new")))]);
        assert_eq!(std::fs::read_to_string(config_path()).unwrap(), cfg);
        // Nothing to change at all: nothing is written.
        let (diff, written, backup) = plan(&[Op::UpsertProvider { provider: input(None) }], false).unwrap();
        assert!(diff.groups.is_empty() && written.is_empty() && backup.is_none());
        // A real field change still rewrites config.toml.
        let (_, written, _) = plan(&[Op::UpsertProvider { provider: ProviderInput { base_url: "https://r2/v1".into(), ..input(None) } }], false).unwrap();
        assert_eq!(written, [config_path()]);
        assert!(std::fs::read_to_string(config_path()).unwrap().contains("base_url = \"https://r2/v1\""));
    }

    #[test]
    fn injection_settings_toggle_their_store_flags() {
        let _h = codex_home_with("codex-inject", "", None);
        for (i, inj) in INJECTIONS.iter().enumerate() {
            let (diff, _, _) = plan(&[setting(inj.key, true)], false).unwrap();
            assert_eq!(diff_lines(&diff), [("AgentPlus · Codex 界面注入".to_string(), inj.on.1.to_string())]);
            let mut want = [false; 4];
            want[..=i].fill(true);
            assert_eq!(injections_on(&store::load()), want);
            // Already on: no change.
            assert!(plan(&[setting(inj.key, true)], false).unwrap().0.groups.is_empty());
        }
        let p = ui_patches();
        assert!(p.fast && p.full_names && p.quota && p.usage_banner);
        let (diff, _, _) = plan(&[setting("quota_unlock", false)], false).unwrap();
        assert_eq!(diff_lines(&diff)[0].1, "- 停用额度用完仍可发送注入");
        assert!(!ui_patches().quota);
        assert!(plan(&[setting("nope", true)], true).is_err());
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
