//! Claude Code: providers are AgentPlus profiles (address, key, model list, and which
//! model each role uses). Switching writes the profile into the `env` block of
//! `~/.claude/settings.json`; only the variables below are touched, everything else in
//! the file stays as it is. "Claude 官方账号" means none of them are set.
//!
//! Claude Code speaks the Anthropic Messages protocol only; relays with other protocols
//! go through the local gateway.

use super::{Plan, Endpoint};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const ID: &str = "claude";
const OFFICIAL: &str = "official";
/// The env block points at a relay that is not an AgentPlus profile (yet).
const UNMANAGED: &str = "settings-env";

const BASE: &str = "ANTHROPIC_BASE_URL";
const TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";
/// (role, env var, (label zh, label en))
const ROLES: [(&str, &str, (&str, &str)); 5] = [
    ("default", "ANTHROPIC_MODEL", ("默认", "Default")),
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL", ("Opus", "Opus")),
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL", ("Sonnet", "Sonnet")),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL", ("Haiku", "Haiku")),
    ("subagent", "CLAUDE_CODE_SUBAGENT_MODEL", ("子代理", "Subagent")),
];
/// Older name of the Haiku slot, kept in step with it.
const SMALL_FAST: &str = "ANTHROPIC_SMALL_FAST_MODEL";
const QUIET: &str = "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC";

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(|| home().join(".claude"))
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn load() -> Result<(Value, TextMeta)> {
    if !settings_path().exists() {
        return Ok((json!({}), TextMeta::NEW));
    }
    read_json_object(&settings_path())
}

fn env_of(cfg: &Value) -> Map<String, Value> {
    cfg.get("env").and_then(|e| e.as_object()).cloned().unwrap_or_default()
}

fn env_str(env: &Map<String, Value>, k: &str) -> Option<String> {
    env.get(k).and_then(|v| v.as_str()).map(String::from).filter(|s| !s.is_empty())
}

fn profiles(root: &Value) -> Map<String, Value> {
    store::get_obj(root, ID, "profiles")
}

fn save_profiles(root: &mut Value, p: Map<String, Value>) {
    store::set_value(root, ID, "profiles", Value::Object(p));
}

/// Claude Code appends /v1/messages itself, so a stored base never ends in /v1.
fn claude_base(u: &str) -> String {
    let t = u.trim().trim_end_matches('/');
    t.strip_suffix("/v1").unwrap_or(t).to_string()
}



/// Which provider the env block currently reflects.
fn current(env: &Map<String, Value>, profs: &Map<String, Value>) -> String {
    let Some(base) = env_str(env, BASE) else { return OFFICIAL.into() };
    let key = env_str(env, TOKEN).or_else(|| env_str(env, API_KEY));
    profs
        .iter()
        .filter(|(_, p)| norm_url(&str_field(p, "baseUrl")) == norm_url(&claude_base(&base)))
        .find(|(_, p)| key.is_none() || str_field(p, "apiKey") == key.clone().unwrap_or_default())
        .or_else(|| profs.iter().find(|(_, p)| norm_url(&str_field(p, "baseUrl")) == norm_url(&claude_base(&base))))
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| UNMANAGED.into())
}

fn roles_of(p: &Value) -> BTreeMap<String, String> {
    p.get("roles")
        .and_then(|r| r.as_object())
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().filter(|x| !x.is_empty()).map(|x| (k.clone(), x.to_string()))).collect())
        .unwrap_or_default()
}

fn model_list(p: &Value) -> Vec<(String, bool)> {
    p.get("models")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|m| Some((m.get("id")?.as_str()?.to_string(), m.get("visible").and_then(|v| v.as_bool()).unwrap_or(true)))).collect())
        .unwrap_or_default()
}

fn set_model_list(p: &mut Value, list: &[(String, bool)]) {
    p["models"] = Value::Array(list.iter().map(|(id, v)| json!({ "id": id, "visible": v })).collect());
}

fn models_with_roles(list: &[(String, bool)], roles: &BTreeMap<String, String>) -> Vec<Model> {
    let mut out: Vec<Model> = list
        .iter()
        .map(|(id, visible)| Model { id: id.clone(), visible: *visible, deletable: true, ..Default::default() })
        .collect();
    // Role models that are not in the list still show up.
    for m in roles.values() {
        if !out.iter().any(|x| &x.id == m) {
            out.push(Model { id: m.clone(), visible: true, deletable: true, ..Default::default() });
        }
    }
    for m in out.iter_mut() {
        for (role, _, (zh, en)) in ROLES {
            if roles.get(role) == Some(&m.id) {
                m.tags.push(Tag::new(format!("role:{role}"), l(zh, en)));
            }
        }
    }
    out
}

/// Env variables a provider needs (None = remove).
fn desired_env(p: Option<&Value>) -> Vec<(&'static str, Option<String>)> {
    let mut out = vec![];
    match p {
        None => {
            for k in [BASE, TOKEN, API_KEY, SMALL_FAST] {
                out.push((k, None));
            }
            for (_, k, _) in ROLES {
                out.push((k, None));
            }
        }
        Some(p) => {
            let key_env = if str_field(p, "keyEnv") == API_KEY { API_KEY } else { TOKEN };
            let other = if key_env == TOKEN { API_KEY } else { TOKEN };
            out.push((BASE, Some(str_field(p, "baseUrl"))));
            out.push((key_env, Some(str_field(p, "apiKey")).filter(|k| !k.is_empty())));
            out.push((other, None));
            let roles = roles_of(p);
            for (role, k, _) in ROLES {
                out.push((k, roles.get(role).cloned()));
            }
            out.push((SMALL_FAST, roles.get("haiku").cloned()));
        }
    }
    out
}

fn secret(k: &str) -> bool {
    k == TOKEN || k == API_KEY
}

fn provider_of(id: &str, p: &Value, _is_current: bool) -> Provider {
    let base = str_field(p, "baseUrl");
    let roles = roles_of(p);
    let key = str_field(p, "apiKey");
    let mut details = vec![
        Kv::mono(l("地址", "Base URL"), base.clone()),
        Kv::text(l("密钥", "API key"), if key.is_empty() { l("未填写", "Not set").into() } else { format!("{} · {}", if str_field(p, "keyEnv") == API_KEY { API_KEY } else { TOKEN }, mask_key(&key)) }),
    ];
    for (role, _, (zh, en)) in ROLES {
        if let Some(m) = roles.get(role) {
            let label = l(zh, en);
            details.push(Kv::mono(&tr!("{label}模型", "{label} model"), m.clone()));
        }
    }
    details.push(Kv::text(l("保存位置", "Stored in"), l("AgentPlus 配置档（切换时写入 settings.json 的 env）", "AgentPlus profile (written to the env block of settings.json on switch)")));
    Provider {
        id: id.into(),
        name: str_field(p, "name"),
        host: host_of(&base),
        base_url: Some(base),
        apis: vec![api_label("anthropic").into()],
        builtin: false,
        enabled: true,
        compatible: true,
        reason: None,
        models: models_with_roles(&model_list(p), &roles),
        details,
        editable: true,
        api: "anthropic".into(),
        has_key: !key.is_empty(),
        key_fp: None,
        key_hint: None,
        official_auth: false,
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: "Claude Code".into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "single".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![display_path(&settings_path())],
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
    let (cfg, _) = match load() {
        Ok(x) => x,
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    let root = store::load();
    let env = env_of(&cfg);
    let profs = profiles(&root);
    let cur = current(&env, &profs);
    st.current_provider = Some(cur.clone());

    st.providers.push(Provider {
        id: OFFICIAL.into(),
        name: l("Claude 官方账号", "Claude official account").into(),
        base_url: None,
        host: l("Claude.ai / Console 登录", "Claude.ai / Console sign-in").into(),
        apis: vec!["Anthropic".into()],
        builtin: true,
        enabled: true,
        compatible: true,
        reason: None,
        models: vec![],
        details: vec![
            Kv::text(l("认证方式", "Authentication"), l("claude 登录（~/.claude/.credentials.json）", "claude sign-in (~/.claude/.credentials.json)")),
            Kv::text(l("说明", "Note"), l("不设置 ANTHROPIC_BASE_URL 等环境变量，用官方账号和官方模型", "Leaves ANTHROPIC_BASE_URL and related variables unset; uses the official account and official models")),
        ],
        editable: false,
        api: "anthropic".into(),
        has_key: true,
        key_fp: None,
        key_hint: None,
        official_auth: false,
    });
    for (id, p) in &profs {
        st.providers.push(provider_of(id, p, id == &cur));
    }
    if cur == UNMANAGED {
        // A relay set up by hand (or by another tool): show it so it can be adopted.
        let mut roles = BTreeMap::new();
        for (role, k, _) in ROLES {
            if let Some(m) = env_str(&env, k) {
                roles.insert(role.to_string(), m);
            }
        }
        let p = json!({
            "name": l("settings.json 里的配置", "Config in settings.json"),
            "baseUrl": env_str(&env, BASE).unwrap_or_default(),
            "apiKey": env_str(&env, TOKEN).or_else(|| env_str(&env, API_KEY)).unwrap_or_default(),
            "keyEnv": if env_str(&env, API_KEY).is_some() && env_str(&env, TOKEN).is_none() { API_KEY } else { TOKEN },
            "roles": roles,
        });
        let mut prov = provider_of(UNMANAGED, &p, true);
        prov.details.push(Kv::text(l("说明", "Note"), l("不是 AgentPlus 保存的配置；编辑并保存一次后就会由 AgentPlus 管理", "Not saved by AgentPlus; edit and save it once and AgentPlus will manage it")));
        st.providers.push(prov);
        st.notes.push(l("settings.json 里有手动设置的 ANTHROPIC_BASE_URL，已显示为「settings.json 里的配置」；编辑保存一次即可由 AgentPlus 管理。", "settings.json has a hand-set ANTHROPIC_BASE_URL, shown as \"Config in settings.json\". Edit and save it once to let AgentPlus manage it.").into());
    }

    st.settings = vec![
        bool_setting("coauthor", "Claude Code", l("提交里署名 Claude", "Credit Claude in commits"), l("includeCoAuthoredBy：git 提交信息末尾加 Co-Authored-By: Claude", "includeCoAuthoredBy: append Co-Authored-By: Claude to git commit messages"), cfg.get("includeCoAuthoredBy").and_then(|x| x.as_bool()).unwrap_or(true)),
        bool_setting("quiet", "Claude Code", l("关闭非必要网络请求", "Disable nonessential traffic"), l("env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = 1：不发遥测、错误报告和自动更新检查（用中转时推荐）", "env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = 1: no telemetry, error reports or auto-update checks (recommended with a relay)"), env_str(&env, QUIET).is_some()),
    ];
    let cur_name = st.providers.iter().find(|p| p.id == cur).map(|p| p.name.clone()).unwrap_or_default();
    st.current = vec![
        Kv::text(l("供应商", "Provider"), cur_name),
        Kv::mono(BASE, env_str(&env, BASE).unwrap_or_else(|| l("-（官方）", "- (official)").into())),
    ];
    for (_, k, (zh, en)) in ROLES {
        if let Some(m) = env_str(&env, k) {
            let label = l(zh, en);
            st.current.push(Kv::mono(&tr!("{label}模型", "{label} model"), m));
        }
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _) = load()?;
    let env = env_of(&cfg);
    let p = if id == UNMANAGED {
        json!({ "baseUrl": env_str(&env, BASE).unwrap_or_default(), "apiKey": env_str(&env, TOKEN).or_else(|| env_str(&env, API_KEY)).unwrap_or_default() })
    } else {
        profiles(&store::load()).get(id).cloned().ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?
    };
    let base = str_field(&p, "baseUrl");
    if base.is_empty() {
        return Err(anyhow!(l("官方账号没有可用的地址", "The official account has no usable base URL")));
    }
    let key = str_field(&p, "apiKey");
    // Claude Code appends /v1/messages to the base; callers append /messages.
    let base = if norm_url(&base).ends_with("/v1") { base } else { format!("{}/v1", base.trim_end_matches('/')) };
    Ok((base, (!key.is_empty()).then_some(key), "anthropic".into()))
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut cfg, meta) = load()?;
    let mut root = store::load();
    let mut profs = profiles(&root);
    let env0 = env_of(&cfg);
    let before = current(&env0, &profs);
    let mut cur = before.clone();
    let file = display_path(&settings_path());
    let store_label = l("AgentPlus · Claude Code 配置档", "AgentPlus · Claude Code profiles");
    let mut diff = Diff::default();
    let (mut cfg_dirty, mut store_dirty) = (false, false);

    let profile = |profs: &mut Map<String, Value>, id: &str| -> Result<()> {
        if id == OFFICIAL {
            return Err(anyhow!(l("官方账号没有模型列表可编辑", "The official account has no model list to edit")));
        }
        if id == UNMANAGED {
            return Err(anyhow!(l("先编辑并保存一次「settings.json 里的配置」，让 AgentPlus 接管后再改模型", "Edit and save \"Config in settings.json\" once so AgentPlus takes it over, then change its models")));
        }
        profs.get(id).map(|_| ()).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))
    };

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.api != "anthropic" {
                    return Err(anyhow!(l("Claude Code 只支持 Anthropic 协议；其他协议的中转请经本地网关接入", "Claude Code only supports the Anthropic protocol; connect relays using other protocols through the local gateway")));
                }
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                match p.id.as_deref() {
                    None | Some(UNMANAGED) => {
                        let id = unique_id(&slug(&p.name), |c| profs.contains_key(c) || c == OFFICIAL || c == UNMANAGED);
                        let adopting = p.id.as_deref() == Some(UNMANAGED);
                        let mut roles = Map::new();
                        let mut key_env = TOKEN;
                        let mut key = key;
                        if adopting {
                            for (role, k, _) in ROLES {
                                if let Some(m) = env_str(&env0, k) {
                                    roles.insert(role.into(), json!(m));
                                }
                            }
                            if env_str(&env0, API_KEY).is_some() && env_str(&env0, TOKEN).is_none() {
                                key_env = API_KEY;
                            }
                            key = key.or_else(|| env_str(&env0, TOKEN).or_else(|| env_str(&env0, API_KEY)));
                        }
                        let models: Vec<Value> = clean_ids(&p.models).into_iter().map(|m| json!({ "id": m, "visible": true })).collect();
                        profs.insert(id.clone(), json!({
                            "name": p.name.trim(), "baseUrl": claude_base(&p.base_url), "apiKey": key.clone().unwrap_or_default(),
                            "keyEnv": key_env, "models": models, "roles": roles,
                        }));
                        let adopt = if adopting { l("（接管 settings.json 里的配置）", " (takes over the config in settings.json)") } else { "" };
                        let key_part = key.as_deref().map(|k| tr!(" · 密钥 {}", " · API key {}", mask_key(k))).unwrap_or_default();
                        diff.push(store_label, tr!("+ 「{}」{}（{}{}）", "+ \"{}\"{} ({}{})", p.name.trim(), adopt, claude_base(&p.base_url), key_part), true);
                        if adopting && cur == UNMANAGED {
                            cur = id;
                        }
                        store_dirty = true;
                    }
                    Some(OFFICIAL) => return Err(anyhow!(l("官方账号不能编辑", "The official account can't be edited"))),
                    Some(id) => {
                        let e = profs.get_mut(id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
                        let base = claude_base(&p.base_url);
                        for (k, v) in [("name", p.name.trim()), ("baseUrl", base.as_str())] {
                            if str_field(e, k) != v {
                                e[k] = json!(v);
                                diff.push(store_label, tr!("「{id}」{k} = {v}", "\"{id}\" {k} = {v}"), true);
                                store_dirty = true;
                            }
                        }
                        if let Some(k) = key {
                            e["apiKey"] = json!(k);
                            diff.push(store_label, tr!("「{id}」密钥 = {}", "\"{id}\" API key = {}", mask_key(&k)), true);
                            store_dirty = true;
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                if provider == OFFICIAL || provider == UNMANAGED {
                    return Err(anyhow!(l("这一项不能删除", "This entry can't be deleted")));
                }
                if provider == &cur {
                    return Err(anyhow!(tr!("「{provider}」正在使用，先切换到其他供应商", "\"{provider}\" is in use; switch to another provider first")));
                }
                if profs.remove(provider).is_some() {
                    diff.push(store_label, tr!("- 「{provider}」", "- \"{provider}\""), false);
                    store_dirty = true;
                }
            }
            Op::SetCurrentProvider { provider } => {
                if provider != OFFICIAL && provider != UNMANAGED && !profs.contains_key(provider) {
                    return Err(anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")));
                }
                cur = provider.clone();
            }
            Op::SetModelVisible { provider, model, visible } => {
                profile(&mut profs, provider)?;
                let p = profs.get_mut(provider).unwrap();
                let mut list = model_list(p);
                match list.iter_mut().find(|(id, _)| id == model) {
                    Some(e) if e.1 != *visible => e.1 = *visible,
                    Some(_) => continue,
                    None => list.push((model.clone(), *visible)),
                }
                set_model_list(p, &list);
                diff.push(store_label, if *visible { tr!("「{provider}」{model} 显示", "\"{provider}\" {model} shown") } else { tr!("「{provider}」{model} 隐藏", "\"{provider}\" {model} hidden") }, *visible);
                store_dirty = true;
            }
            Op::UpsertModel { provider, model: m } => {
                profile(&mut profs, provider)?;
                let p = profs.get_mut(provider).unwrap();
                let mut list = model_list(p);
                let id = m.id.trim().to_string();
                if !id.is_empty() && !list.iter().any(|(x, _)| x == &id) {
                    list.push((id.clone(), true));
                    set_model_list(p, &list);
                    diff.push(store_label, tr!("「{provider}」+ {id}", "\"{provider}\" + {id}"), true);
                    store_dirty = true;
                }
            }
            Op::DeleteModel { provider, model } => {
                profile(&mut profs, provider)?;
                let p = profs.get_mut(provider).unwrap();
                let mut list = model_list(p);
                let n = list.len();
                list.retain(|(x, _)| x != model);
                if list.len() != n {
                    set_model_list(p, &list);
                    diff.push(store_label, tr!("「{provider}」- {model}", "\"{provider}\" - {model}"), false);
                    store_dirty = true;
                }
            }
            Op::SetProviderModels { provider, models } => {
                profile(&mut profs, provider)?;
                let p = profs.get_mut(provider).unwrap();
                let list: Vec<(String, bool)> = clean_ids(models).into_iter().map(|m| (m, true)).collect();
                set_model_list(p, &list);
                diff.push(store_label, tr!("「{provider}」模型列表：{} 个", "\"{provider}\" model list: {}", list.len()), true);
                store_dirty = true;
            }
            Op::SetModelRoles { provider, roles } => {
                profile(&mut profs, provider)?;
                let p = profs.get_mut(provider).unwrap();
                let old = roles_of(p);
                let new: BTreeMap<String, String> = roles.iter().filter(|(k, v)| ROLES.iter().any(|(r, ..)| r == k) && !v.trim().is_empty()).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
                if old != new {
                    for (role, _, (zh, en)) in ROLES {
                        if old.get(role) != new.get(role) {
                            let label = l(zh, en);
                            diff.push(store_label, tr!("「{provider}」{label}模型 = {}", "\"{provider}\" {label} model = {}", new.get(role).map(String::as_str).unwrap_or(l("（不指定）", "(unset)"))), new.contains_key(role));
                        }
                    }
                    p["roles"] = json!(new);
                    store_dirty = true;
                }
            }
            Op::SetSetting { key, value } => {
                let on = value.as_bool().unwrap_or(false);
                match key.as_str() {
                    "coauthor" => {
                        if cfg.get("includeCoAuthoredBy").and_then(|x| x.as_bool()).unwrap_or(true) != on {
                            cfg["includeCoAuthoredBy"] = json!(on);
                            diff.push(&file, format!("includeCoAuthoredBy = {on}"), on);
                            cfg_dirty = true;
                        }
                    }
                    "quiet" => {
                        if env_str(&env_of(&cfg), QUIET).is_some() != on {
                            if !cfg.get("env").map(|e| e.is_object()).unwrap_or(false) {
                                cfg["env"] = json!({});
                            }
                            let env = cfg["env"].as_object_mut().unwrap();
                            if on { env.insert(QUIET.into(), json!("1")); } else { env.remove(QUIET); }
                            diff.push(&file, format!("env.{QUIET} {}", if on { "= 1" } else { l("（删除）", "(removed)") }), on);
                            cfg_dirty = true;
                        }
                    }
                    other => return Err(anyhow!(tr!("未知设置 {other}", "Unknown setting: {other}"))),
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Claude Code 同时只用一个供应商，请用「设为当前」", "Claude Code uses one provider at a time; use \"Set as current\""))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    // Bring the env block in line with the active provider (a profile is the source of truth;
    // the official account clears our variables only when switching to it).
    let want = match cur.as_str() {
        OFFICIAL if before != OFFICIAL => Some(desired_env(None)),
        OFFICIAL | UNMANAGED => None,
        id => profs.get(id).map(|p| desired_env(Some(p))),
    };
    if let Some(want) = want {
        if !cfg.get("env").map(|e| e.is_object()).unwrap_or(false) {
            cfg["env"] = json!({});
        }
        let env = cfg["env"].as_object_mut().unwrap();
        for (k, v) in want {
            let old = env.get(k).and_then(|x| x.as_str()).map(String::from);
            if old == v {
                continue;
            }
            let shown = |x: &str| if secret(k) { mask_key(x) } else { x.to_string() };
            match &v {
                Some(val) => {
                    env.insert(k.into(), json!(val));
                    diff.push(&file, format!("env.{k} = {}", shown(val)), true);
                }
                None => {
                    env.remove(k);
                    diff.push(&file, tr!("env.{k}（删除）", "env.{k} (removed)"), false);
                }
            }
            cfg_dirty = true;
        }
        if env.is_empty() {
            if let Some(o) = cfg.as_object_mut() {
                o.remove("env");
            }
        }
    }

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cfg_dirty {
            backup_dir = Some(backup(ID, &[settings_path()])?);
            std::fs::create_dir_all(dir())?;
            write_json(&settings_path(), &cfg, meta)?;
            written.push(settings_path());
        }
        if store_dirty {
            save_profiles(&mut root, profs);
            store::save(&root)?;
        }
    }
    Ok((diff, written, backup_dir))
}
