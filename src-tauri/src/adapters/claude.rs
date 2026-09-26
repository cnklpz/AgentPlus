//! Claude Code: providers are AgentPlus profiles (address, key, model list, and which
//! model each role uses). Switching writes the profile into the `env` block of
//! `~/.claude/settings.json`; only the variables below are touched, everything else in
//! the file stays as it is. "Claude official account" means none of them are set.
//!
//! Claude Code speaks the Anthropic Messages protocol only; relays with other protocols
//! go through the local gateway.

use super::msg;
use super::profiles::{self, model_list};
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
pub const NAME: &str = "Claude Code";
pub const MARKER: &str = "settings.json";
pub const WSL_SCRIPT: &str = "claude --version 2>/dev/null; pgrep -x claude >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".claude/settings.json";
const OFFICIAL: &str = "official";
/// The env block points at a relay that is not an AgentPlus profile (yet).
const UNMANAGED: &str = "settings-env";

const BASE: &str = "ANTHROPIC_BASE_URL";
const TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";
/// (role, env var, (label en, label zh))
const ROLES: [(&str, &str, (&str, &str)); 5] = [
    ("default", "ANTHROPIC_MODEL", ("Default", "默认")),
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL", ("Opus", "Opus")),
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL", ("Sonnet", "Sonnet")),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL", ("Haiku", "Haiku")),
    ("subagent", "CLAUDE_CODE_SUBAGENT_MODEL", ("Subagent", "子代理")),
];
/// Older name of the Haiku slot, kept in step with it.
const SMALL_FAST: &str = "ANTHROPIC_SMALL_FAST_MODEL";
const QUIET: &str = "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC";

/// `~/.claude`.
pub fn default_dir() -> PathBuf {
    home().join(".claude")
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

/// The CLI (npm or the native installer).
pub fn detect() -> Install {
    crate::process::detect_claude()
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn load() -> Result<(Value, TextMeta)> {
    read_json_object_or_new(&settings_path())
}

fn env_of(cfg: &Value) -> Map<String, Value> {
    cfg.get("env").and_then(|e| e.as_object()).cloned().unwrap_or_default()
}

fn env_str(env: &Map<String, Value>, k: &str) -> Option<String> {
    env.get(k).and_then(|v| v.as_str()).map(String::from).filter(|s| !s.is_empty())
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
        for (role, _, (en, zh)) in ROLES {
            if roles.get(role) == Some(&m.id) {
                m.tags.push(Tag::new(format!("role:{role}"), l(en, zh)));
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

/// `managed`: an AgentPlus profile (not the hand-set settings.json entry).
fn provider_of(id: &str, p: &Value, managed: bool) -> Provider {
    let base = str_field(p, "baseUrl");
    let roles = roles_of(p);
    let key = str_field(p, "apiKey");
    let mut details = vec![
        Kv::mono(lbl::base_url(), base.clone()),
        Kv::text(lbl::api_key(), if key.is_empty() { l("Not set", "未填写").into() } else { format!("{} · {}", if str_field(p, "keyEnv") == API_KEY { API_KEY } else { TOKEN }, mask_key(&key)) }),
    ];
    for (role, _, (en, zh)) in ROLES {
        if let Some(m) = roles.get(role) {
            let label = l(en, zh);
            details.push(Kv::mono(&tr!("{label} model", "{label}模型"), m.clone()));
        }
    }
    if managed {
        details.push(Kv::text(l("Stored in", "保存位置"), l("AgentPlus profile (written to the env block of settings.json on switch)", "AgentPlus 配置档（切换时写入 settings.json 的 env）")));
    }
    Provider {
        id: id.into(),
        name: str_field(p, "name"),
        host: host_of(&base),
        base_url: Some(base),
        apis: vec![api_label("anthropic").into()],
        enabled: true,
        compatible: true,
        models: models_with_roles(&model_list(p), &roles),
        details,
        editable: true,
        api: "anthropic".into(),
        has_key: !key.is_empty(),
        ..Default::default()
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "single", &dir(), vec![display_path(&settings_path())]);
    let (cfg, _) = match load() {
        Ok(x) => x,
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let root = store::load();
    let env = env_of(&cfg);
    let profs = profiles::load(&root, ID);
    let cur = current(&env, &profs);
    st.current_provider = Some(cur.clone());

    st.providers.push(Provider::builtin(
        OFFICIAL,
        l("Claude official account", "Claude 官方账号"),
        l("Claude.ai / Console sign-in", "Claude.ai / Console 登录"),
        "anthropic",
        api_label("anthropic"),
        vec![
            Kv::text(lbl::auth(), l("claude sign-in (~/.claude/.credentials.json)", "claude 登录（~/.claude/.credentials.json）")),
            Kv::text(lbl::note(), l("Leaves ANTHROPIC_BASE_URL and related variables unset; uses the official account and official models", "不设置 ANTHROPIC_BASE_URL 等环境变量，用官方账号和官方模型")),
        ],
    ));
    for (id, p) in &profs {
        st.providers.push(provider_of(id, p, true));
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
            "name": l("Config in settings.json", "settings.json 里的配置"),
            "baseUrl": env_str(&env, BASE).unwrap_or_default(),
            "apiKey": env_str(&env, TOKEN).or_else(|| env_str(&env, API_KEY)).unwrap_or_default(),
            "keyEnv": if env_str(&env, API_KEY).is_some() && env_str(&env, TOKEN).is_none() { API_KEY } else { TOKEN },
            "roles": roles,
        });
        let mut prov = provider_of(UNMANAGED, &p, false);
        prov.details.push(Kv::text(lbl::note(), l("Not saved by AgentPlus; edit and save it once and AgentPlus will manage it", "不是 AgentPlus 保存的配置；编辑并保存一次后就会由 AgentPlus 管理")));
        st.providers.push(prov);
        st.notes.push(l("settings.json has a hand-set ANTHROPIC_BASE_URL, shown as \"Config in settings.json\". Edit and save it once to let AgentPlus manage it.", "settings.json 里有手动设置的 ANTHROPIC_BASE_URL，已显示为「settings.json 里的配置」；编辑保存一次即可由 AgentPlus 管理。").into());
    }

    st.settings = vec![
        bool_setting("coauthor", "Claude Code", l("Credit Claude in commits", "提交里署名 Claude"), l("includeCoAuthoredBy: append Co-Authored-By: Claude to git commit messages", "includeCoAuthoredBy：git 提交信息末尾加 Co-Authored-By: Claude"), cfg.get("includeCoAuthoredBy").and_then(|x| x.as_bool()).unwrap_or(true)),
        bool_setting("quiet", "Claude Code", l("Disable nonessential traffic", "关闭非必要网络请求"), l("env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = 1: no telemetry, error reports or auto-update checks (recommended with a relay)", "env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = 1：不发遥测、错误报告和自动更新检查（用中转时推荐）"), env_str(&env, QUIET).is_some()),
    ];
    let cur_name = st.providers.iter().find(|p| p.id == cur).map(|p| p.name.clone()).unwrap_or_default();
    st.current = vec![
        Kv::text(lbl::provider(), cur_name),
        Kv::mono(BASE, env_str(&env, BASE).unwrap_or_else(|| l("- (official)", "-（官方）").into())),
    ];
    for (_, k, (en, zh)) in ROLES {
        if let Some(m) = env_str(&env, k) {
            let label = l(en, zh);
            st.current.push(Kv::mono(&tr!("{label} model", "{label}模型"), m));
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
        profiles::load(&store::load(), ID).get(id).cloned().ok_or_else(|| msg::no_provider(id))?
    };
    let base = str_field(&p, "baseUrl");
    if base.is_empty() {
        return Err(anyhow!(l("The official account has no usable base URL", "官方账号没有可用的地址")));
    }
    let key = str_field(&p, "apiKey");
    // Claude Code appends /v1/messages to the base; callers append /messages.
    let base = if norm_url(&base).ends_with("/v1") { base } else { format!("{}/v1", base.trim_end_matches('/')) };
    Ok((base, (!key.is_empty()).then_some(key), "anthropic".into()))
}

/// The profile whose models an op edits; the built-in entries have none.
fn profile_mut<'a>(profs: &'a mut Map<String, Value>, id: &str) -> Result<&'a mut Value> {
    if id == OFFICIAL {
        return Err(anyhow!(l("The official account has no model list to edit", "官方账号没有模型列表可编辑")));
    }
    if id == UNMANAGED {
        return Err(anyhow!(l("Edit and save \"Config in settings.json\" once so AgentPlus takes it over, then change its models", "先编辑并保存一次「settings.json 里的配置」，让 AgentPlus 接管后再改模型")));
    }
    profiles::get_mut(profs, id)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut cfg, meta) = load()?;
    let mut root = store::load();
    let mut profs = profiles::load(&root, ID);
    let env0 = env_of(&cfg);
    let before = current(&env0, &profs);
    let mut cur = before.clone();
    let file = display_path(&settings_path());
    let store_label = l("AgentPlus · Claude Code profiles", "AgentPlus · Claude Code 配置档");
    let mut diff = Diff::default();
    let (mut cfg_dirty, mut store_dirty) = (false, false);

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.api != "anthropic" {
                    return Err(anyhow!(l("Claude Code only supports the Anthropic protocol; connect relays using other protocols through the local gateway", "Claude Code 只支持 Anthropic 协议；其他协议的中转请经本地网关接入")));
                }
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                match p.id.as_deref() {
                    None | Some(UNMANAGED) => {
                        let id = profiles::new_id(&profs, &p.name, &[OFFICIAL, UNMANAGED]);
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
                        profs.insert(id.clone(), json!({
                            "name": p.name.trim(), "baseUrl": claude_base(&p.base_url), "apiKey": key.clone().unwrap_or_default(),
                            "keyEnv": key_env, "models": profiles::models_value(&p.models), "roles": roles,
                        }));
                        let adopt = if adopting { l(" (takes over the config in settings.json)", "（接管 settings.json 里的配置）") } else { "" };
                        profiles::push_added(&mut diff, store_label, p.name.trim(), adopt, &claude_base(&p.base_url), key.as_deref());
                        if adopting && cur == UNMANAGED {
                            cur = id;
                        }
                        store_dirty = true;
                    }
                    Some(OFFICIAL) => return Err(anyhow!(l("The official account can't be edited", "官方账号不能编辑"))),
                    Some(id) => {
                        let e = profiles::get_mut(&mut profs, id)?;
                        store_dirty |= profiles::edit(e, id, p.name.trim(), &claude_base(&p.base_url), key.as_deref(), &mut diff, store_label);
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                if provider == OFFICIAL || provider == UNMANAGED {
                    return Err(anyhow!(l("This entry can't be deleted", "这一项不能删除")));
                }
                if provider == &cur {
                    return Err(msg::in_use(provider));
                }
                profiles::delete(&mut profs, provider, &mut diff, store_label)?;
                store_dirty = true;
            }
            Op::SetCurrentProvider { provider } => {
                if provider != OFFICIAL && provider != UNMANAGED && !profs.contains_key(provider) {
                    return Err(msg::no_provider(provider));
                }
                cur = provider.clone();
            }
            Op::SetModelVisible { provider, model, visible } => {
                let p = profile_mut(&mut profs, provider)?;
                store_dirty |= profiles::set_visible(p, provider, model, *visible, &mut diff, store_label);
            }
            Op::UpsertModel { provider, model: m } => {
                let p = profile_mut(&mut profs, provider)?;
                store_dirty |= profiles::add_model(p, provider, &m.id, &mut diff, store_label)?;
            }
            Op::DeleteModel { provider, model } => {
                let p = profile_mut(&mut profs, provider)?;
                store_dirty |= profiles::delete_model(p, provider, model, &mut diff, store_label);
            }
            Op::SetProviderModels { provider, models } => {
                let p = profile_mut(&mut profs, provider)?;
                profiles::set_models(p, provider, models, &mut diff, store_label);
                store_dirty = true;
            }
            Op::SetModelRoles { provider, roles } => {
                let p = profile_mut(&mut profs, provider)?;
                let old = roles_of(p);
                let new: BTreeMap<String, String> = roles.iter().filter(|(k, v)| ROLES.iter().any(|(r, ..)| r == k) && !v.trim().is_empty()).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
                if old != new {
                    for (role, _, (en, zh)) in ROLES {
                        if old.get(role) != new.get(role) {
                            let label = l(en, zh);
                            diff.push(store_label, tr!("\"{provider}\" {label} model = {}", "「{provider}」{label}模型 = {}", new.get(role).map(String::as_str).unwrap_or(l("(unset)", "（不指定）"))), new.contains_key(role));
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
                            let env = obj_at(&mut cfg, &["env"])?;
                            if on { env.insert(QUIET.into(), json!("1")); } else { env.remove(QUIET); }
                            diff.push(&file, format!("env.{QUIET} {}", if on { "= 1" } else { l("(removed)", "（删除）") }), on);
                            cfg_dirty = true;
                        }
                    }
                    other => return Err(msg::unknown_setting(other)),
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Claude Code uses one provider at a time; use \"Set as current\"", "Claude Code 同时只用一个供应商，请用「设为当前」"))),
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
        let env = obj_at(&mut cfg, &["env"])?;
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
                    diff.push(&file, tr!("env.{k} (removed)", "env.{k}（删除）"), false);
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
            // Profile-only edits leave settings.json alone and need no backup; a settings
            // backup carries the profiles it was written from (see history::PROFILE_AGENTS).
            let b = backup(ID, &[settings_path()])?;
            crate::history::backup_profiles(&b, &store::scoped(ID), &root)?;
            backup_dir = Some(b);
            std::fs::create_dir_all(dir())?;
            write_json(&settings_path(), &cfg, meta)?;
            written.push(settings_path());
        }
        if store_dirty {
            profiles::save(&mut root, ID, profs);
            store::save(&root)?;
        }
    }
    Ok((diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn setup(settings: Option<&str>) -> TestHome {
        crate::env::force(crate::env::Target::Windows);
        let home = TestHome::new("claude");
        fs::create_dir_all(dir()).unwrap();
        if let Some(s) = settings {
            fs::write(settings_path(), s).unwrap();
        }
        home
    }

    fn apply(ops: Vec<Op>) -> Result<Diff> {
        plan(&ops, false).map(|x| x.0)
    }

    fn pi(id: Option<&str>, name: &str, base: &str, key: Option<&str>, models: &[&str]) -> ProviderInput {
        ProviderInput { id: id.map(String::from), name: name.into(), base_url: base.into(), api: "anthropic".into(), api_key: key.map(String::from), models: models.iter().map(|s| s.to_string()).collect(), key_from_library: None, official_auth: None }
    }

    fn settings() -> Value {
        serde_json::from_str(&fs::read_to_string(settings_path()).unwrap()).unwrap()
    }

    fn models(id: &str) -> Vec<(String, bool)> {
        model_list(&profiles::load(&store::load(), ID)[id])
    }

    fn model(id: &str) -> ModelInput {
        ModelInput { id: id.into(), ..Default::default() }
    }

    #[test]
    fn create_switch_and_back_to_official() {
        let _h = setup(Some("{\n  \"includeCoAuthoredBy\": false\n}\n"));
        let d = apply(vec![Op::UpsertProvider { provider: pi(None, "My Relay", "https://r/v1/", Some("sk-new-secret-5678"), &["m1", "m1", " m2 "]) }]).unwrap();
        assert_eq!(d.groups[0].lines[0].text, "+ 「My Relay」（https://r · 密钥 ••••5678）");
        assert_eq!(models("my-relay"), vec![("m1".into(), true), ("m2".into(), true)], "cleaned and deduped");
        apply(vec![Op::SetModelRoles { provider: "my-relay".into(), roles: BTreeMap::from([("opus".into(), "m2".into())]) }, Op::SetCurrentProvider { provider: "my-relay".into() }]).unwrap();
        let v = settings();
        assert_eq!(v.pointer("/env/ANTHROPIC_BASE_URL").and_then(|x| x.as_str()), Some("https://r"));
        assert_eq!(v.pointer("/env/ANTHROPIC_AUTH_TOKEN").and_then(|x| x.as_str()), Some("sk-new-secret-5678"));
        assert_eq!(v.pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL").and_then(|x| x.as_str()), Some("m2"));
        assert_eq!(v["includeCoAuthoredBy"], json!(false));
        assert_eq!(state(&Install::default()).current_provider.as_deref(), Some("my-relay"));
        apply(vec![Op::SetCurrentProvider { provider: OFFICIAL.into() }]).unwrap();
        assert!(settings().get("env").is_none(), "our variables removed, empty env dropped");
        apply(vec![Op::DeleteProvider { provider: "my-relay".into() }]).unwrap();
        assert!(profiles::load(&store::load(), ID).is_empty());
    }

    #[test]
    fn unchanged_edits_are_no_ops() {
        let _h = setup(None);
        apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r", Some("sk-same-secret-1234"), &["m"]) }]).unwrap();
        let store0 = fs::read_to_string(agentplus_dir().join("store.json")).unwrap();
        // Re-entering the same key (and name / address) changes nothing.
        let (d, written, backup) = plan(&[Op::UpsertProvider { provider: pi(Some("r"), "R", "https://r/", Some("sk-same-secret-1234"), &[]) }], false).unwrap();
        assert!(d.groups.is_empty() && written.is_empty() && backup.is_none());
        assert!(apply(vec![Op::UpsertModel { provider: "r".into(), model: model("m") }]).unwrap().groups.is_empty());
        assert!(apply(vec![Op::SetModelVisible { provider: "r".into(), model: "m".into(), visible: true }]).unwrap().groups.is_empty());
        assert_eq!(fs::read_to_string(agentplus_dir().join("store.json")).unwrap(), store0);
    }

    #[test]
    fn rollback_restores_profiles_and_survives_unrelated_edits() {
        let _h = setup(Some("{}"));
        apply(vec![Op::UpsertProvider { provider: pi(None, "Relay", "https://r", Some("dummy-key"), &["old", "new"]) }]).unwrap();
        let role = |model: &str| Op::SetModelRoles { provider: "relay".into(), roles: BTreeMap::from([("default".into(), model.into())]) };
        apply(vec![role("old"), Op::SetCurrentProvider { provider: "relay".into() }]).unwrap();
        let (_, _, dir) = plan(&[role("new")], false).unwrap();
        let dir = dir.unwrap();
        let id = format!("{}/claude", dir.parent().unwrap().file_name().unwrap().to_string_lossy());
        store::update(|root| {
            root["library"] = json!([{"id":"keep-library"}]);
            root["gatewayKeys"] = json!({"codex":"keep-key"});
            root["claude"]["autoRestart"] = json!(true);
            root["claude@wsl:Ubuntu"] = json!({"profiles":{"keep-wsl":{}}});
            Ok(())
        }).unwrap();
        crate::history::restore(&id).unwrap();
        assert_eq!(settings()["env"]["ANTHROPIC_MODEL"], "old");
        apply(vec![Op::SetSetting { key: "quiet".into(), value: json!(true) }]).unwrap();
        assert_eq!(settings()["env"]["ANTHROPIC_MODEL"], "old");
        let root = store::load();
        assert_eq!(root["claude"]["profiles"]["relay"]["roles"]["default"], "old");
        assert_eq!(root["library"][0]["id"], "keep-library");
        assert_eq!(root["gatewayKeys"]["codex"], "keep-key");
        assert_eq!(root["claude"]["autoRestart"], true);
        assert!(root["claude@wsl:Ubuntu"]["profiles"]["keep-wsl"].is_object());
        // A profile-only edit leaves settings.json alone and adds no backup.
        let (d, written, backup) = plan(&[Op::UpsertModel { provider: "relay".into(), model: model("extra") }], false).unwrap();
        assert!(!d.groups.is_empty() && written.is_empty() && backup.is_none());
    }

    #[test]
    fn invalid_ops_are_errors() {
        let _h = setup(None);
        apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r", None, &[]) }]).unwrap();
        assert!(apply(vec![Op::UpsertModel { provider: "r".into(), model: model("  ") }]).is_err(), "blank model id");
        assert!(apply(vec![Op::DeleteProvider { provider: "nope".into() }]).is_err(), "unknown provider");
        assert!(apply(vec![Op::UpsertModel { provider: OFFICIAL.into(), model: model("m") }]).is_err());
        apply(vec![Op::SetProviderModels { provider: "r".into(), models: vec!["a".into(), "b".into(), "a".into()] }]).unwrap();
        assert_eq!(models("r"), vec![("a".into(), true), ("b".into(), true)]);
    }

    #[test]
    fn hand_set_relay_is_not_labelled_a_profile() {
        let _h = setup(Some("{\n  \"env\": {\"ANTHROPIC_BASE_URL\": \"https://hand\", \"ANTHROPIC_AUTH_TOKEN\": \"sk-hand-1234\"}\n}\n"));
        apply(vec![Op::UpsertProvider { provider: pi(None, "R", "https://r", None, &[]) }]).unwrap();
        let st = state(&Install::default());
        assert_eq!(st.current_provider.as_deref(), Some(UNMANAGED));
        let stored_in = |id: &str| st.providers.iter().find(|p| p.id == id).unwrap().details.iter().any(|d| d.k == "保存位置");
        assert!(!stored_in(UNMANAGED), "the settings.json entry is not an AgentPlus profile");
        assert!(stored_in("r"));
    }

    #[test]
    fn non_object_env_is_refused() {
        let raw = "{\n  \"env\": \"oops\"\n}\n";
        let _h = setup(Some(raw));
        assert!(apply(vec![Op::SetSetting { key: "quiet".into(), value: json!(true) }]).is_err());
        assert_eq!(fs::read_to_string(settings_path()).unwrap(), raw);
    }
}
