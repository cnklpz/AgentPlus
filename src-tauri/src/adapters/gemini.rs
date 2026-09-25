//! Gemini CLI: providers are AgentPlus profiles (address, key, model list, default model).
//! Gemini CLI takes a relay only through environment variables, so switching writes
//! `GOOGLE_GEMINI_BASE_URL` / `GEMINI_API_KEY` into `~/.gemini/.env` (only those two lines;
//! every other line stays) and `security.auth.selectedType = "gemini-api-key"` (plus
//! `model.name` = the profile's default model) into `~/.gemini/settings.json`.
//! "Google 账号登录" (oauth-personal) removes the two variables again.
//!
//! Model roles: Gemini CLI has one model setting, `model.name`; `SetModelRoles` accepts the
//! role "default" for it (the first model of the list is the default when none is chosen).

use super::msg;
use super::profiles::{self, model_list, set_model_list};
use super::{Plan, Endpoint};
use crate::dotenv;
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub const ID: &str = "gemini";
pub const NAME: &str = "Gemini CLI";
/// Relative to the config dir.
pub const MARKER: &str = "settings.json";
pub const WSL_SCRIPT: &str = "gemini --version 2>/dev/null | head -n 1; pgrep -f '[b]in/gemini' >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".gemini/settings.json";

const GOOGLE: &str = "google";
/// `.env` points at a relay / key that is not an AgentPlus profile (yet).
const UNMANAGED: &str = "env";
/// Id prefix for other auth types (vertex-ai, cloud-shell, …), read-only.
const AUTH: &str = "auth:";
const BASE: &str = "GOOGLE_GEMINI_BASE_URL";
const KEY: &str = "GEMINI_API_KEY";
const OAUTH: &str = "oauth-personal";
const API_KEY_AUTH: &str = "gemini-api-key";
/// selectedType AgentPlus infers when settings.json names none but .env sets a relay address.
const GATEWAY_AUTH: &str = "gateway";
/// Where Gemini CLI goes without GOOGLE_GEMINI_BASE_URL.
const OFFICIAL_BASE: &str = "https://generativelanguage.googleapis.com";

/// An on/off setting stored as a boolean in settings.json (missing = on).
struct BoolSetting {
    key: &'static str,
    path: [&'static str; 2],
    /// (zh, en)
    label: (&'static str, &'static str),
    /// (zh, en)
    desc: (&'static str, &'static str),
}

const BOOL_SETTINGS: [BoolSetting; 2] = [
    BoolSetting {
        key: "autoupdate",
        path: ["general", "enableAutoUpdate"],
        label: ("自动更新", "Auto-update"),
        desc: ("general.enableAutoUpdate：启动时自动更新 Gemini CLI", "general.enableAutoUpdate: update Gemini CLI automatically on start"),
    },
    BoolSetting {
        key: "usage_stats",
        path: ["privacy", "usageStatisticsEnabled"],
        label: ("发送使用统计", "Send usage statistics"),
        desc: ("privacy.usageStatisticsEnabled：向 Google 发送使用统计（用中转时建议关闭）", "privacy.usageStatisticsEnabled: send usage statistics to Google (turning it off is recommended with a relay)"),
    },
];

// ---------------------------------------------------------------- paths

pub fn default_dir() -> PathBuf {
    home().join(".gemini")
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn settings_path() -> PathBuf {
    dir().join(MARKER)
}

fn env_path() -> PathBuf {
    dir().join(".env")
}

// ---------------------------------------------------------------- detection

/// npm global install (`%APPDATA%\npm\node_modules\@google\gemini-cli`). A CLI: `exe` stays None.
pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some(pkg) = crate::process::npm_global_package("@google/gemini-cli").filter(|p| p.is_file()) {
        inst.installed = true;
        inst.version = crate::process::package_version(&pkg);
    } else if let Some(cmd) = dirs::data_dir().map(|d| d.join("npm").join("gemini.cmd")).filter(|p| p.exists()) {
        inst.installed = true;
        inst.dir = cmd.parent().map(|p| p.to_path_buf());
    }
    inst
}

// ---------------------------------------------------------------- files

/// (settings, meta, had_comments). A missing file is `{}`.
fn load() -> Result<(Value, TextMeta, bool)> {
    let (text, meta) = read_text_or_new(&settings_path())?;
    if text.trim().is_empty() {
        return Ok((json!({}), meta, false));
    }
    let (v, had) = parse_jsonc_object(&text, "settings.json")?;
    Ok((v, meta, had))
}

// ---------------------------------------------------------------- profiles

fn clean_base(u: &str) -> String {
    u.trim().trim_end_matches('/').to_string()
}

fn drop_default_model(p: &mut Value) {
    if let Some(o) = p.as_object_mut() {
        o.remove("defaultModel");
    }
}

/// The model written to `model.name`: the chosen default, else the first visible model.
fn default_model(p: &Value) -> Option<String> {
    Some(str_field(p, "defaultModel")).filter(|m| !m.is_empty()).or_else(|| model_list(p).into_iter().find(|(_, v)| *v).map(|(id, _)| id))
}

fn auth_type(cfg: &Value) -> Option<String> {
    cfg.pointer("/security/auth/selectedType").and_then(|x| x.as_str()).map(String::from).filter(|x| !x.is_empty())
}

fn model_name(cfg: &Value) -> Option<String> {
    match cfg.get("model") {
        Some(Value::String(m)) => Some(m.clone()),
        Some(m) => m.get("name").and_then(|x| x.as_str()).map(String::from),
        None => None,
    }
    .filter(|m| !m.is_empty())
}

/// Whether .env sets the relay address or the key.
fn has_env_vars(env: &str) -> bool {
    dotenv::get(env, BASE).is_some() || dotenv::get(env, KEY).is_some()
}

/// The auth type Gemini CLI ends up with: settings first, else what the env implies.
fn effective_auth(cfg: &Value, env: &str) -> Option<String> {
    auth_type(cfg).or_else(|| {
        if dotenv::get(env, BASE).is_some() {
            Some(GATEWAY_AUTH.into())
        } else if dotenv::get(env, KEY).is_some() {
            Some(API_KEY_AUTH.into())
        } else {
            None
        }
    })
}

/// A profile matching the .env values (address first, key to tell same-address profiles apart).
fn matching_profile(env: &str, profs: &Map<String, Value>) -> Option<String> {
    let base = dotenv::get(env, BASE).map(|b| norm_url(&b)).unwrap_or_default();
    let key = dotenv::get(env, KEY);
    if base.is_empty() && key.is_none() {
        return None;
    }
    let same_base: Vec<(&String, &Value)> = profs.iter().filter(|(_, p)| norm_url(&str_field(p, "baseUrl")) == base).collect();
    same_base
        .iter()
        .find(|(_, p)| key.as_deref().map(|k| str_field(p, "apiKey") == k).unwrap_or(true))
        .or_else(|| same_base.iter().find(|_| !base.is_empty()))
        .map(|(id, _)| (*id).clone())
}

fn current(cfg: &Value, env: &str, profs: &Map<String, Value>) -> String {
    match effective_auth(cfg, env).as_deref() {
        None | Some(OAUTH) => GOOGLE.into(),
        // Without .env vars the key comes from the system environment: nothing to adopt.
        Some(t @ (API_KEY_AUTH | GATEWAY_AUTH)) => matching_profile(env, profs).unwrap_or_else(|| if has_env_vars(env) { UNMANAGED.into() } else { format!("{AUTH}{t}") }),
        Some(other) => format!("{AUTH}{other}"),
    }
}

fn auth_label(t: &str) -> &str {
    match t {
        "vertex-ai" => "Vertex AI",
        "cloud-shell" => "Cloud Shell",
        "compute-default-credentials" => l("Google Cloud 默认凭据", "Google Cloud default credentials"),
        API_KEY_AUTH => l("Gemini API 密钥（来自环境变量）", "Gemini API key (from environment variables)"),
        GATEWAY_AUTH => l("网关（gateway）", "Gateway (gateway)"),
        other => other,
    }
}

/// `managed`: an AgentPlus profile (not the unmanaged .env entry).
fn provider_of(id: &str, p: &Value, managed: bool) -> Provider {
    let base = str_field(p, "baseUrl");
    let key = str_field(p, "apiKey");
    let dflt = default_model(p);
    let models = model_list(p)
        .into_iter()
        .map(|(mid, visible)| Model { tags: if dflt.as_deref() == Some(mid.as_str()) { vec![Tag::default_model()] } else { vec![] }, id: mid, visible, deletable: true, ..Default::default() })
        .collect();
    let mut details = vec![
        Kv::mono(lbl::base_url(), if base.is_empty() { l("（Gemini 官方 API）", "(Gemini official API)").into() } else { base.clone() }),
        Kv::text(lbl::api_key(), if key.is_empty() { l("未填写", "Not set").into() } else { format!("{KEY} · {}", mask_key(&key)) }),
    ];
    if let Some(d) = &dflt {
        details.push(Kv::mono(lbl::default_model(), d.clone()));
    }
    if managed {
        details.push(Kv::text(l("保存位置", "Stored in"), l("AgentPlus 配置档（切换时写入 ~/.gemini/.env 和 settings.json）", "AgentPlus profile (written to ~/.gemini/.env and settings.json on switch)")));
    }
    let base = if base.is_empty() { OFFICIAL_BASE.to_string() } else { base };
    Provider {
        id: id.into(),
        name: str_field(p, "name"),
        host: host_of(&base),
        base_url: Some(base),
        apis: vec![api_label("gemini").into()],
        enabled: true,
        compatible: true,
        models,
        details,
        editable: true,
        api: "gemini".into(),
        has_key: !key.is_empty(),
        ..Default::default()
    }
}

fn unmanaged_profile(env: &str) -> Value {
    json!({
        "name": l(".env 里的配置", "Config in .env"),
        "baseUrl": dotenv::get(env, BASE).map(|b| clean_base(&b)).unwrap_or_default(),
        "apiKey": dotenv::get(env, KEY).unwrap_or_default(),
        "models": [],
    })
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "single", &dir(), vec![display_path(&settings_path()), display_path(&env_path())]);
    st.notes.push(l("改动对新启动的 gemini 生效。", "Changes apply to newly started gemini sessions.").into());
    let (cfg, _, had_comments) = match load() {
        Ok(x) => x,
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    if had_comments {
        st.readonly = true;
        st.notes.push(msg::comments_readonly("settings.json"));
    }
    let root = store::load();
    let (env, _) = dotenv::load(&env_path());
    let profs = profiles::load(&root, ID);
    let cur = current(&cfg, &env, &profs);
    st.current_provider = Some(cur.clone());

    st.providers.push(Provider::builtin(
        GOOGLE,
        l("Google 账号登录", "Google account sign-in"),
        l("Google 账号（oauth-personal）", "Google account (oauth-personal)"),
        "gemini",
        api_label("gemini"),
        vec![
            Kv::text(lbl::auth(), l("security.auth.selectedType = oauth-personal（凭据在 ~/.gemini/oauth_creds.json）", "security.auth.selectedType = oauth-personal (credentials in ~/.gemini/oauth_creds.json)")),
            Kv::text(lbl::note(), l("不设置 GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY，用 Google 账号和官方模型", "Leaves GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY unset; uses the Google account and official models")),
        ],
    ));
    if let Some(t) = cur.strip_prefix(AUTH) {
        st.providers.push(Provider::builtin(&cur, auth_label(t), l("其他认证方式", "Other auth method"), "gemini", api_label("gemini"), vec![Kv::mono("security.auth.selectedType", t.to_string()), Kv::text(lbl::note(), l("在 Gemini CLI 里用 /auth 管理", "Manage it with /auth in Gemini CLI"))]));
    }
    for (id, p) in &profs {
        st.providers.push(provider_of(id, p, true));
    }
    // A relay / key in .env that no profile covers: show it so it can be adopted.
    if has_env_vars(&env) && matching_profile(&env, &profs).is_none() {
        let mut prov = provider_of(UNMANAGED, &unmanaged_profile(&env), false);
        prov.details.push(Kv::text(lbl::note(), l("~/.gemini/.env 里的设置，不是 AgentPlus 保存的配置；编辑并保存一次后就会由 AgentPlus 管理", "Set in ~/.gemini/.env, not saved by AgentPlus; edit and save it once and AgentPlus will manage it")));
        st.providers.push(prov);
        if cur != UNMANAGED {
            st.notes.push(l("~/.gemini/.env 里设置了 GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY，但当前用的是 Google 账号登录（settings.json 的 selectedType 优先）。", "~/.gemini/.env sets GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY, but Google account sign-in is in use (selectedType in settings.json takes precedence).").into());
        }
    }
    for var in [BASE, KEY] {
        if crate::env::agent_var(var).is_some_and(|v| !v.trim().is_empty()) {
            st.notes.push(tr!("系统环境变量 {var} 已设置，它会覆盖 ~/.gemini/.env 里的同名设置。", "System environment variable {var} is set and overrides the same setting in ~/.gemini/.env."));
        }
    }
    st.notes.push(l("项目目录（或上级目录）里有 .env 时，Gemini CLI 会先读它，而不是 ~/.gemini/.env。", "When the project directory (or a parent) has a .env, Gemini CLI reads that instead of ~/.gemini/.env.").into());

    st.settings = BOOL_SETTINGS.iter().map(|s| bool_setting(s.key, NAME, l(s.label.0, s.label.1), l(s.desc.0, s.desc.1), setting_on(&cfg, &s.path))).collect();
    let cur_name = st.providers.iter().find(|p| p.id == cur).map(|p| p.name.clone()).unwrap_or_default();
    st.current_model = model_name(&cfg);
    st.current = vec![
        Kv::text(lbl::provider(), cur_name),
        Kv::mono("selectedType", auth_type(&cfg).unwrap_or_else(|| l("-（未设置）", "- (not set)").into())),
        Kv::mono(BASE, dotenv::get(&env, BASE).unwrap_or_else(|| "-".into())),
        Kv::mono("model.name", model_name(&cfg).unwrap_or_else(|| l("-（默认）", "- (default)").into())),
    ];
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let p = if id == UNMANAGED {
        unmanaged_profile(&dotenv::load(&env_path()).0)
    } else if id == GOOGLE || id.starts_with(AUTH) {
        return Err(anyhow!(l("Google 账号登录没有可用的地址", "Google account sign-in has no usable base URL")));
    } else {
        profiles::load(&store::load(), ID).get(id).cloned().ok_or_else(|| msg::no_provider(id))?
    };
    let base = str_field(&p, "baseUrl");
    let base = if base.is_empty() { OFFICIAL_BASE.to_string() } else { base };
    let key = str_field(&p, "apiKey");
    Ok((base, (!key.is_empty()).then_some(key), "gemini".into()))
}

// ---------------------------------------------------------------- plan

/// A boolean setting (missing = on).
fn setting_on(cfg: &Value, path: &[&str]) -> bool {
    cfg.pointer(&jptr(path)).and_then(|x| x.as_bool()).unwrap_or(true)
}

/// Sets `path` in settings.json; a parent that is not an object is an error, not replaced.
fn set_at(cfg: &mut Value, path: &[&str], v: Value) -> Result<()> {
    let (last, parents) = path.split_last().expect("non-empty path");
    obj_at(cfg, parents)?.insert(last.to_string(), v);
    Ok(())
}

/// The profile whose models an op edits; the built-in entries have none.
fn profile_mut<'a>(profs: &'a mut Map<String, Value>, id: &str) -> Result<&'a mut Value> {
    if id == GOOGLE || id.starts_with(AUTH) {
        return Err(anyhow!(l("Google 账号登录没有模型列表可编辑", "Google account sign-in has no model list to edit")));
    }
    if id == UNMANAGED {
        return Err(anyhow!(l("先编辑并保存一次「.env 里的配置」，让 AgentPlus 接管后再改模型", "Edit and save \"Config in .env\" once so AgentPlus takes it over, then change its models")));
    }
    profiles::get_mut(profs, id)
}

/// What the active provider needs in .env and settings.json.
struct Target {
    base: Option<String>,
    key: Option<String>,
    auth: &'static str,
    model: Option<String>,
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut cfg, meta, had_comments) = load()?;
    let cfg0 = cfg.clone();
    let mut root = store::load();
    let mut profs = profiles::load(&root, ID);
    // Snapshot: a change to the active profile is re-applied to the live config.
    let profs0 = profs.clone();
    // Unlike the read-only views, a write refuses an .env it can't read (UTF-16, GBK…).
    let (env0, env_meta) = read_text_or_new(&env_path())?;
    let mut env = env0.clone();
    let before = current(&cfg, &env0, &profs);
    let mut cur = before.clone();
    let file = display_path(&settings_path());
    let envfile = display_path(&env_path());
    let store_label = l("AgentPlus · Gemini CLI 配置档", "AgentPlus · Gemini CLI profiles");
    let mut diff = Diff::default();
    let mut store_dirty = false;

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.api != "gemini" {
                    return Err(anyhow!(l("Gemini CLI 只支持 Gemini 协议；其他协议的中转请经本地网关接入", "Gemini CLI only supports the Gemini protocol; connect relays using other protocols through the local gateway")));
                }
                if p.name.trim().is_empty() {
                    return Err(msg::name_required());
                }
                let base = clean_base(&p.base_url);
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                match p.id.as_deref() {
                    None | Some(UNMANAGED) => {
                        let id = profiles::new_id(&profs, &p.name, &[GOOGLE, UNMANAGED]);
                        let adopting = p.id.as_deref() == Some(UNMANAGED);
                        let key = if adopting { key.or_else(|| dotenv::get(&env0, KEY)) } else { key };
                        let mut prof = json!({ "name": p.name.trim(), "baseUrl": base, "apiKey": key.clone().unwrap_or_default(), "models": profiles::models_value(&p.models) });
                        if adopting {
                            if let Some(m) = model_name(&cfg) {
                                prof["defaultModel"] = json!(m);
                                if !model_list(&prof).iter().any(|(x, _)| x == &m) {
                                    let mut list = model_list(&prof);
                                    list.insert(0, (m, true));
                                    set_model_list(&mut prof, &list);
                                }
                            }
                        }
                        profs.insert(id.clone(), prof);
                        let adopt = if adopting { l("（接管 .env 里的配置）", " (takes over the config in .env)") } else { "" };
                        let where_: &str = if base.is_empty() { l("官方 API", "official API") } else { &base };
                        profiles::push_added(&mut diff, store_label, p.name.trim(), adopt, where_, key.as_deref());
                        if adopting && cur == UNMANAGED {
                            cur = id;
                        }
                        store_dirty = true;
                    }
                    Some(id) if id == GOOGLE || id.starts_with(AUTH) => return Err(anyhow!(l("Google 账号登录不能编辑", "Google account sign-in can't be edited"))),
                    Some(id) => {
                        let e = profiles::get_mut(&mut profs, id)?;
                        store_dirty |= profiles::edit(e, id, p.name.trim(), &base, key.as_deref(), &mut diff, store_label);
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                if provider == GOOGLE || provider == UNMANAGED || provider.starts_with(AUTH) {
                    return Err(anyhow!(l("这一项不能删除", "This entry can't be deleted")));
                }
                if provider == &cur {
                    return Err(msg::in_use(provider));
                }
                profiles::delete(&mut profs, provider, &mut diff, store_label)?;
                store_dirty = true;
            }
            Op::SetCurrentProvider { provider } => {
                if provider.starts_with(AUTH) && provider != &before {
                    return Err(anyhow!(l("这种认证方式请在 Gemini CLI 里用 /auth 选择", "Choose this auth method with /auth in Gemini CLI")));
                }
                if provider == UNMANAGED && !has_env_vars(&env0) {
                    return Err(anyhow!(l(".env 里没有 GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY", ".env has no GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY")));
                }
                if provider != GOOGLE && provider != UNMANAGED && !provider.starts_with(AUTH) && !profs.contains_key(provider) {
                    return Err(msg::no_provider(provider));
                }
                cur = provider.clone();
            }
            Op::SetModelVisible { provider, model, visible } => {
                let p = profile_mut(&mut profs, provider)?;
                // The default model can't be hidden (hiding it again is a no-op, not an error).
                let hidden = model_list(p).iter().any(|(x, v)| x == model && !v);
                if !*visible && !hidden && str_field(p, "defaultModel") == *model {
                    return Err(anyhow!(tr!("{model} 是默认模型，不能隐藏；先换一个默认模型", "{model} is the default model and can't be hidden; choose another default model first")));
                }
                store_dirty |= profiles::set_visible(p, provider, model, *visible, &mut diff, store_label);
            }
            Op::UpsertModel { provider, model: m } => {
                let p = profile_mut(&mut profs, provider)?;
                store_dirty |= profiles::add_model(p, provider, &m.id, &mut diff, store_label)?;
            }
            Op::DeleteModel { provider, model } => {
                let p = profile_mut(&mut profs, provider)?;
                if profiles::delete_model(p, provider, model, &mut diff, store_label) {
                    if str_field(p, "defaultModel") == *model {
                        drop_default_model(p);
                    }
                    store_dirty = true;
                }
            }
            Op::SetProviderModels { provider, models } => {
                let p = profile_mut(&mut profs, provider)?;
                profiles::set_models(p, provider, models, &mut diff, store_label);
                let dflt = str_field(p, "defaultModel");
                if !model_list(p).iter().any(|(m, _)| *m == dflt) {
                    drop_default_model(p);
                }
                store_dirty = true;
            }
            Op::SetModelRoles { provider, roles } => {
                if roles.keys().any(|k| k != "default") {
                    return Err(anyhow!(l("Gemini CLI 只有「默认模型」（model.name）一个角色", "Gemini CLI has only one role: \"Default model\" (model.name)")));
                }
                let p = profile_mut(&mut profs, provider)?;
                let want = roles.get("default").map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
                if want.clone().unwrap_or_default() != str_field(p, "defaultModel") {
                    match &want {
                        Some(m) => {
                            p["defaultModel"] = json!(m);
                            let mut list = model_list(p);
                            if !list.iter().any(|(x, _)| x == m) {
                                list.push((m.clone(), true));
                                set_model_list(p, &list);
                            }
                        }
                        None => drop_default_model(p),
                    }
                    diff.push(store_label, tr!("「{provider}」默认模型 = {}", "\"{provider}\" default model = {}", want.as_deref().unwrap_or(l("（列表第一个）", "(first in list)"))), want.is_some());
                    store_dirty = true;
                }
            }
            Op::SetSetting { key, value } => {
                let on = value.as_bool().unwrap_or(false);
                let Some(s) = BOOL_SETTINGS.iter().find(|s| s.key == key) else { return Err(msg::unknown_setting(key)) };
                if setting_on(&cfg, &s.path) != on {
                    set_at(&mut cfg, &s.path, json!(on))?;
                    diff.push(&file, format!("{} = {on}", s.path.join(".")), on);
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Gemini CLI 同时只用一个供应商，请用「设为当前」", "Gemini CLI uses one provider at a time; use \"Set as current\""))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    // Bring .env and settings.json in line with the active provider.
    let switching = cur != before;
    let target = match cur.as_str() {
        GOOGLE if switching => Some(Target { base: None, key: None, auth: OAUTH, model: None }),
        UNMANAGED if switching => Some(Target { base: dotenv::get(&env0, BASE), key: dotenv::get(&env0, KEY), auth: API_KEY_AUTH, model: None }),
        GOOGLE | UNMANAGED => None,
        id if id.starts_with(AUTH) => None,
        id if switching || profs0.get(id) != profs.get(id) => profs.get(id).map(|p| Target {
            base: Some(str_field(p, "baseUrl")).filter(|b| !b.is_empty()),
            key: Some(str_field(p, "apiKey")).filter(|k| !k.is_empty()),
            auth: API_KEY_AUTH,
            model: default_model(p),
        }),
        _ => None,
    };
    if let Some(Target { base, key, auth, model }) = target {
        for (var, v) in [(BASE, &base), (KEY, &key)] {
            if dotenv::get(&env, var) != *v {
                env = dotenv::set(&env, var, v.as_deref());
                match v {
                    Some(val) => diff.push(&envfile, format!("{var} = {}", if var == KEY { mask_key(val) } else { val.clone() }), true),
                    None => diff.push(&envfile, tr!("{var}（删除）", "{var} (removed)"), false),
                }
            }
        }
        if switching && auth_type(&cfg).as_deref() != Some(auth) {
            set_at(&mut cfg, &["security", "auth", "selectedType"], json!(auth))?;
            diff.push(&file, format!("security.auth.selectedType = {auth}"), true);
        }
        let old_model = model_name(&cfg);
        if let Some(m) = model {
            if old_model.as_deref() != Some(m.as_str()) {
                // The older `"model": "<id>"` form becomes `"model": { "name": … }`.
                if cfg.get("model").is_some_and(|x| !x.is_object()) {
                    cfg["model"] = json!({});
                }
                set_at(&mut cfg, &["model", "name"], json!(m))?;
                diff.push(&file, format!("model.name = {m}"), true);
            }
        } else if cur == GOOGLE {
            // A relay's model id means nothing to the Google login: drop it if it came from the profile we left.
            let from_profile = profs0.get(&before).map(|p| model_list(p).iter().any(|(x, _)| Some(x) == old_model.as_ref())).unwrap_or(false);
            if from_profile {
                // The older string form goes as a whole; `model.name` goes with the object it leaves empty.
                if cfg.get("model").is_some_and(Value::is_string) {
                    if let Some(o) = cfg.as_object_mut() {
                        o.remove("model");
                    }
                } else {
                    crate::mfields::remove(&mut cfg, "/model/name");
                }
                diff.push(&file, l("model.name（删除，用 Gemini CLI 默认模型）", "model.name (removed; Gemini CLI uses its default model)"), false);
            }
        }
    }

    let cfg_dirty = cfg != cfg0;
    let env_dirty = env != env0;
    if cfg_dirty && had_comments {
        return Err(msg::comments_not_written("settings.json"));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        let mut targets = vec![];
        if cfg_dirty {
            targets.push(settings_path());
        }
        if env_dirty {
            targets.push(env_path());
        }
        if !targets.is_empty() {
            backup_dir = Some(backup(ID, &targets)?);
            std::fs::create_dir_all(dir())?;
        }
        if cfg_dirty {
            write_json(&settings_path(), &cfg, meta)?;
            written.push(settings_path());
        }
        if env_dirty {
            write_text_atomic(&env_path(), &env, env_meta)?;
            written.push(env_path());
        }
        if store_dirty {
            profiles::save(&mut root, ID, profs);
            store::save(&root)?;
        }
    }
    Ok((diff, written, backup_dir))
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    const SETTINGS: &str = "{\n  \"security\": {\n    \"auth\": {\n      \"selectedType\": \"oauth-personal\"\n    }\n  },\n  \"mcpServers\": {\n    \"vibe_kanban\": {\n      \"command\": \"npx\",\n      \"args\": [\n        \"-y\",\n        \"vibe-kanban@latest\",\n        \"--mcp\"\n      ],\n      \"timeout\": 60000\n    }\n  }\n}";
    const ENV: &str = "# gemini env\nGEMINI_API_KEY=sk-envkey-secret-1234\nGOOGLE_GEMINI_BASE_URL=https://relay.example:8080\nOTHER=keep\n";

    /// (config dir, the temp home it lives in)
    struct Tmp(PathBuf, #[allow(dead_code)] TestHome);

    fn setup(settings: Option<&str>, env: Option<&str>) -> Tmp {
        crate::env::force(crate::env::Target::Windows);
        let home = TestHome::new("gemini");
        let d = dir();
        fs::create_dir_all(&d).unwrap();
        if let Some(s) = settings {
            fs::write(d.join("settings.json"), s).unwrap();
        }
        if let Some(e) = env {
            fs::write(d.join(".env"), e).unwrap();
        }
        Tmp(d, home)
    }

    fn apply(ops: Vec<Op>) -> Result<Diff> {
        plan(&ops, false).map(|x| x.0)
    }

    fn pi(id: Option<&str>, name: &str, base: &str, key: Option<&str>, models: &[&str]) -> ProviderInput {
        ProviderInput { id: id.map(String::from), name: name.into(), base_url: base.into(), api: "gemini".into(), api_key: key.map(String::from), models: models.iter().map(|s| s.to_string()).collect(), key_from_library: None, official_auth: None }
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect::<Vec<_>>().join("\n")
    }

    fn settings(t: &Tmp) -> Value {
        serde_json::from_str(&fs::read_to_string(t.0.join("settings.json")).unwrap()).unwrap()
    }

    fn envtext(t: &Tmp) -> String {
        fs::read_to_string(t.0.join(".env")).unwrap()
    }

    #[test]
    fn unreadable_dotenv_is_never_rewritten() {
        // GBK bytes ("# 中文") as Notepad saves them on a Chinese system.
        let gbk: &[u8] = b"# \xd6\xd0\xce\xc4\nGEMINI_API_KEY=old\nOTHER=keep\n";
        let t = setup(Some(SETTINGS), None);
        fs::write(t.0.join(".env"), gbk).unwrap();
        assert!(apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r/", Some("sk-x-1234"), &["m"]) }]).is_err());
        assert_eq!(fs::read(t.0.join(".env")).unwrap(), gbk);
        let _ = settings(&t);
    }

    #[test]
    fn missing_dotenv_is_created() {
        let t = setup(Some(SETTINGS), None);
        apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r/", Some("sk-x-1234"), &["m"]) }, Op::SetCurrentProvider { provider: "r".into() }]).unwrap();
        assert!(envtext(&t).contains("sk-x-1234"));
    }

    #[test]
    fn reads_state_with_unmanaged_env() {
        let _t = setup(Some(SETTINGS), Some(ENV));
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert_eq!(st.current_model, None, "no model.name");
        assert_eq!(st.current_provider.as_deref(), Some(GOOGLE));
        let ids: Vec<&str> = st.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec![GOOGLE, UNMANAGED]);
        let e = &st.providers[1];
        assert!(e.has_key && e.api == "gemini" && e.apis == vec!["Gemini".to_string()]);
        assert_eq!(e.base_url.as_deref(), Some("https://relay.example:8080"));
        assert!(!format!("{st:?}").contains("secret"));
        assert_eq!(provider_endpoint(UNMANAGED).unwrap().1.as_deref(), Some("sk-envkey-secret-1234"));
        assert!(provider_endpoint(GOOGLE).is_err());
    }

    #[test]
    fn adopt_switch_and_back_to_google() {
        let t = setup(Some(SETTINGS), Some(ENV));
        // Adopt the .env relay as a profile (key taken over from .env).
        apply(vec![Op::UpsertProvider { provider: pi(Some(UNMANAGED), "Relay", "https://relay.example:8080/", None, &["gemini-2.5-pro"]) }]).unwrap();
        let st = state(&Install::default());
        assert!(st.providers.iter().any(|p| p.id == "relay" && p.has_key));
        assert!(!st.providers.iter().any(|p| p.id == UNMANAGED), "adopted: no longer unmanaged");
        // Switch to it.
        let d = apply(vec![Op::SetCurrentProvider { provider: "relay".into() }]).unwrap();
        let dt = diff_text(&d);
        assert!(!dt.contains("secret"), "{dt}");
        let v = settings(&t);
        assert_eq!(v.pointer("/security/auth/selectedType").and_then(|x| x.as_str()), Some(API_KEY_AUTH));
        assert_eq!(v.pointer("/model/name").and_then(|x| x.as_str()), Some("gemini-2.5-pro"));
        assert!(v.pointer("/mcpServers/vibe_kanban/args").is_some());
        assert_eq!(envtext(&t), ENV, ".env already matches the profile");
        let st = state(&Install::default());
        assert_eq!(st.current_provider.as_deref(), Some("relay"));
        assert_eq!(st.current_model.as_deref(), Some("gemini-2.5-pro"), "model.name, for the test box");
        // Back to Google login: the two variables go, the rest of .env stays.
        apply(vec![Op::SetCurrentProvider { provider: GOOGLE.into() }]).unwrap();
        assert_eq!(envtext(&t), "# gemini env\nOTHER=keep\n");
        let v = settings(&t);
        assert_eq!(v.pointer("/security/auth/selectedType").and_then(|x| x.as_str()), Some(OAUTH));
        assert!(v.pointer("/model/name").is_none(), "relay model removed for Google login");
        // And to the profile again: variables written back.
        apply(vec![Op::SetCurrentProvider { provider: "relay".into() }]).unwrap();
        assert_eq!(envtext(&t), "# gemini env\nOTHER=keep\nGOOGLE_GEMINI_BASE_URL=https://relay.example:8080\nGEMINI_API_KEY=sk-envkey-secret-1234\n");
    }

    #[test]
    fn create_edit_delete_and_models() {
        let t = setup(Some(SETTINGS), None);
        apply(vec![Op::UpsertProvider { provider: pi(None, "My Relay", "https://r/", Some("sk-new-secret-5678"), &["m1", "m2"]) }]).unwrap();
        assert!(apply(vec![Op::UpsertProvider { provider: ProviderInput { api: "chat".into(), ..pi(None, "X", "https://x", None, &[]) } }]).is_err());
        let st = state(&Install::default());
        let p = st.providers.iter().find(|p| p.id == "my-relay").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://r"));
        assert_eq!(p.models.len(), 2);
        assert!(p.models[0].tags.contains(&Tag::default_model()));
        // Models.
        apply(vec![Op::UpsertModel { provider: "my-relay".into(), model: ModelInput { id: "m3".into(), name: None, context: None, ..Default::default() } }]).unwrap();
        apply(vec![Op::SetModelVisible { provider: "my-relay".into(), model: "m2".into(), visible: false }]).unwrap();
        apply(vec![Op::DeleteModel { provider: "my-relay".into(), model: "m1".into() }]).unwrap();
        let st = state(&Install::default());
        let p = st.providers.iter().find(|p| p.id == "my-relay").unwrap();
        assert_eq!(p.models.iter().map(|m| (m.id.as_str(), m.visible)).collect::<Vec<_>>(), vec![("m2", false), ("m3", true)]);
        assert!(p.models[1].tags.contains(&Tag::default_model()), "first visible model is the default");
        apply(vec![Op::SetModelRoles { provider: "my-relay".into(), roles: BTreeMap::from([("default".into(), "m9".into())]) }]).unwrap();
        assert!(apply(vec![Op::SetModelRoles { provider: "my-relay".into(), roles: BTreeMap::from([("opus".into(), "m9".into())]) }]).is_err());
        assert!(apply(vec![Op::SetModelVisible { provider: "my-relay".into(), model: "m9".into(), visible: false }]).is_err());
        // Current: writes .env from scratch and model.name = default model.
        apply(vec![Op::SetCurrentProvider { provider: "my-relay".into() }]).unwrap();
        assert_eq!(envtext(&t), "GOOGLE_GEMINI_BASE_URL=https://r\nGEMINI_API_KEY=sk-new-secret-5678\n");
        assert_eq!(settings(&t).pointer("/model/name").and_then(|x| x.as_str()), Some("m9"));
        // Editing the current profile updates the live config.
        let d = apply(vec![Op::UpsertProvider { provider: pi(Some("my-relay"), "My Relay", "https://r2", Some("sk-rot-secret-0000"), &[]) }]).unwrap();
        assert!(!diff_text(&d).contains("secret"));
        assert_eq!(envtext(&t), "GOOGLE_GEMINI_BASE_URL=https://r2\nGEMINI_API_KEY=sk-rot-secret-0000\n");
        apply(vec![Op::SetProviderModels { provider: "my-relay".into(), models: vec!["a".into(), "b".into(), "a".into()] }]).unwrap();
        assert_eq!(settings(&t).pointer("/model/name").and_then(|x| x.as_str()), Some("a"));
        assert!(apply(vec![Op::DeleteProvider { provider: "my-relay".into() }]).is_err(), "current");
        apply(vec![Op::SetCurrentProvider { provider: GOOGLE.into() }, Op::DeleteProvider { provider: "my-relay".into() }]).unwrap();
        assert!(!state(&Install::default()).providers.iter().any(|p| p.id == "my-relay"));
        assert!(apply(vec![Op::SetProviderEnabled { provider: GOOGLE.into(), enabled: true }]).is_err());
    }

    #[test]
    fn settings_and_formatting() {
        let t = setup(Some(SETTINGS), Some(ENV));
        apply(vec![Op::SetSetting { key: "usage_stats".into(), value: json!(false) }]).unwrap();
        let raw = fs::read_to_string(t.0.join("settings.json")).unwrap();
        assert!(raw.starts_with(&SETTINGS[..SETTINGS.len() - 2]), "{raw}");
        assert!(!raw.ends_with('\n'), "no trailing newline, like the original");
        let st = state(&Install::default());
        assert_eq!(st.settings.iter().find(|s| s.key == "usage_stats").unwrap().value, json!(false));
        assert!(apply(vec![Op::SetSetting { key: "nope".into(), value: json!(true) }]).is_err());
    }

    #[test]
    fn dry_run_and_comments() {
        let t = setup(Some(SETTINGS), Some(ENV));
        let (d, written, backup) = plan(&[Op::UpsertProvider { provider: pi(Some(UNMANAGED), "R", "https://relay.example:8080", None, &[]) }, Op::SetCurrentProvider { provider: UNMANAGED.into() }], true).unwrap();
        assert!(!d.groups.is_empty() && written.is_empty() && backup.is_none());
        assert_eq!(fs::read_to_string(t.0.join("settings.json")).unwrap(), SETTINGS);
        assert!(!agentplus_dir().join("store.json").exists());
        drop(t);
        let t = setup(Some("{\n  // mine\n  \"security\": {}\n}\n"), None);
        assert!(state(&Install::default()).readonly);
        assert!(apply(vec![Op::SetSetting { key: "autoupdate".into(), value: json!(false) }]).is_err());
        assert!(fs::read_to_string(t.0.join("settings.json")).unwrap().contains("// mine"));
    }

    #[test]
    fn missing_files() {
        let t = setup(None, None);
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert_eq!(st.current_provider.as_deref(), Some(GOOGLE));
        apply(vec![Op::UpsertProvider { provider: pi(None, "Official Key", "", Some("AIza-secret-9999"), &[]) }, Op::SetCurrentProvider { provider: "official-key".into() }]).unwrap();
        assert_eq!(envtext(&t), "GEMINI_API_KEY=AIza-secret-9999\n");
        assert_eq!(settings(&t).pointer("/security/auth/selectedType").and_then(|x| x.as_str()), Some(API_KEY_AUTH));
        assert_eq!(state(&Install::default()).current_provider.as_deref(), Some("official-key"));
    }

    #[test]
    fn api_key_from_system_env_is_listed_as_current() {
        // Gemini CLI's documented setup: `export GEMINI_API_KEY`, then "Use Gemini API key" in /auth.
        let t = setup(Some(r#"{"security":{"auth":{"selectedType":"gemini-api-key"}}}"#), None);
        crate::env::set_test_vars(&[(KEY, "AIza-sys-secret-1111")]);
        let st = state(&Install::default());
        let cur = format!("{AUTH}{API_KEY_AUTH}");
        assert_eq!(st.current_provider.as_deref(), Some(cur.as_str()));
        let p = st.providers.iter().find(|p| p.id == cur).expect("current provider is listed");
        assert_eq!(p.name, "Gemini API 密钥（来自环境变量）");
        assert!(!st.providers.iter().any(|p| p.id == UNMANAGED), "nothing in .env to adopt");
        assert!(st.notes.iter().any(|n| n.contains(KEY)));
        // Switching to the Google login still works.
        apply(vec![Op::SetCurrentProvider { provider: GOOGLE.into() }]).unwrap();
        assert_eq!(settings(&t).pointer("/security/auth/selectedType").and_then(|x| x.as_str()), Some(OAUTH));
        assert_eq!(state(&Install::default()).current_provider.as_deref(), Some(GOOGLE));
    }

    #[test]
    fn non_object_parent_is_refused() {
        let raw = r#"{"privacy": "on"}"#;
        let t = setup(Some(raw), None);
        assert!(apply(vec![Op::SetSetting { key: "usage_stats".into(), value: json!(false) }]).is_err());
        assert_eq!(fs::read_to_string(t.0.join("settings.json")).unwrap(), raw);
    }

    #[test]
    fn legacy_string_model_is_dropped_for_google() {
        let t = setup(Some(r#"{"security":{"auth":{"selectedType":"gemini-api-key"}},"model":"m1"}"#), Some(ENV));
        apply(vec![Op::UpsertProvider { provider: pi(Some(UNMANAGED), "Relay", "https://relay.example:8080", None, &["m1"]) }]).unwrap();
        assert_eq!(state(&Install::default()).current_provider.as_deref(), Some("relay"));
        let d = apply(vec![Op::SetCurrentProvider { provider: GOOGLE.into() }]).unwrap();
        assert!(diff_text(&d).contains("model.name（删除"), "{}", diff_text(&d));
        assert!(settings(&t).get("model").is_none());
    }

    #[test]
    fn hiding_the_default_model() {
        let _t = setup(Some(SETTINGS), None);
        apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r", None, &["a", "b"]) }]).unwrap();
        apply(vec![Op::SetModelRoles { provider: "r".into(), roles: BTreeMap::from([("default".into(), "b".into())]) }]).unwrap();
        assert!(apply(vec![Op::SetModelVisible { provider: "r".into(), model: "b".into(), visible: false }]).is_err());
        apply(vec![Op::SetModelVisible { provider: "r".into(), model: "a".into(), visible: false }]).unwrap();
        // Deleting the default model drops the choice; the first visible model takes over.
        apply(vec![Op::DeleteModel { provider: "r".into(), model: "b".into() }]).unwrap();
        let root = store::load();
        assert_eq!(str_field(&profiles::load(&root, ID)["r"], "defaultModel"), "");
        assert!(apply(vec![Op::UpsertModel { provider: "r".into(), model: ModelInput { id: " ".into(), ..Default::default() } }]).is_err());
        assert!(apply(vec![Op::DeleteProvider { provider: "nope".into() }]).is_err());
    }

    /// Read-only look at the real Gemini CLI config on this machine (keys masked).
    #[test]
    #[ignore]
    fn dump_gemini() {
        println!("dir: {}", dir().display());
        let inst = detect();
        println!("detect: installed={} version={:?}", inst.installed, inst.version);
        let st = state(&inst);
        println!("mode={} current={:?} readonly={}", st.mode, st.current_provider, st.readonly);
        for p in &st.providers {
            println!("- {} [{}] {:?} api={} has_key={} builtin={} models={:?}", p.id, p.name, p.base_url, p.api, p.has_key, p.builtin, p.models.iter().map(|m| m.id.clone()).collect::<Vec<_>>());
            for d in &p.details {
                println!("    {}: {}", d.k, d.v);
            }
        }
        for kv in &st.current {
            println!("current {}: {}", kv.k, kv.v);
        }
        for s in &st.settings {
            println!("setting {} = {}", s.key, s.value);
        }
        for n in &st.notes {
            println!("note: {n}");
        }
        let target = st.providers.iter().find(|p| Some(&p.id) != st.current_provider.as_ref() && !p.builtin).map(|p| p.id.clone());
        if let Some(t) = target {
            let (d, written, backup) = plan(&[Op::SetCurrentProvider { provider: t.clone() }], true).unwrap();
            println!("dry-run switch to {t}:");
            for g in &d.groups {
                for l in &g.lines {
                    println!("  {} {} {}", g.file, if l.add { "+" } else { "-" }, l.text);
                }
            }
            assert!(written.is_empty() && backup.is_none());
        }
    }
}
