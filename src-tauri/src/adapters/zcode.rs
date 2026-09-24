//! ZCode: providers live in `~/.zcode/v2/provider_config.json`
//! (`config.providerConfigRules.providerRules[]`). A model shows in the picker
//! when its id is in `personalModelIds`; `modelOrder` keeps the full known list.
//! Per-model context windows live in `config.modelConfigRules.providerModelRules`.

use super::msg;
use super::{Plan, Endpoint};
use crate::mfields;
use crate::model::*;
use crate::process::Install;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const ID: &str = "zcode";
pub const NAME: &str = "ZCode";

fn dir() -> PathBuf {
    match super::dir_override(ID) {
        // Accept either ~/.zcode or ~/.zcode/v2.
        Some(d) if d.join("v2").join("provider_config.json").exists() => d.join("v2"),
        Some(d) => d,
        None => home().join(".zcode").join("v2"),
    }
}
fn provider_path() -> PathBuf {
    dir().join("provider_config.json")
}
fn setting_path() -> PathBuf {
    dir().join("setting.json")
}
fn legacy_path() -> PathBuf {
    dir().join("config.json")
}

/// (key, group, label, desc); group and label are (zh, en), picked with `l()`.
type Text2 = (&'static str, &'static str);
const G_CHAT: Text2 = ("对话", "Chat");
const G_WINDOW: Text2 = ("窗口与数据", "Window & data");
const SETTINGS: [(&str, Text2, Text2, &str); 6] = [
    ("messageStreamShowReasoning", G_CHAT, ("显示推理过程", "Show reasoning"), "messageStreamShowReasoning"),
    ("messageStreamShowTodos", G_CHAT, ("显示待办清单", "Show to-do list"), "messageStreamShowTodos"),
    ("memoryEnabled", G_CHAT, ("记忆", "Memory"), "memoryEnabled"),
    ("closeToTrayOnWindows", G_WINDOW, ("关闭窗口时最小化到托盘", "Minimize to tray when closing the window"), "closeToTrayOnWindows"),
    ("modelIoFullRetentionEnabled", G_WINDOW, ("保留完整的模型输入输出", "Keep full model input and output"), "modelIoFullRetentionEnabled"),
    ("taskAutoArchiveEnabled", G_WINDOW, ("自动归档旧任务", "Auto-archive old tasks"), "taskAutoArchiveEnabled"),
];

fn api_type(api: &str) -> &'static str {
    match api {
        "responses" => "openai-responses",
        "anthropic" => "anthropic-messages",
        _ => "openai-chat-completions",
    }
}

fn api_short(t: &str) -> &'static str {
    match t {
        "openai-responses" => "responses",
        "anthropic-messages" => "anthropic",
        _ => "chat",
    }
}

fn rules(v: &Value) -> Vec<Value> {
    v.pointer("/config/providerConfigRules/providerRules")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
}

fn is_rule(r: &Value, pid: &str, mid: &str) -> bool {
    r.get("providerId").and_then(|x| x.as_str()) == Some(pid) && r.get("modelId").and_then(|x| x.as_str()) == Some(mid)
}

/// The per-model rule's `config` (context window, input formats, max output…).
fn rule_config<'a>(v: &'a Value, pid: &str, mid: &str) -> Option<&'a Value> {
    v.pointer("/config/modelConfigRules/providerModelRules")?.as_array()?.iter().find(|r| is_rule(r, pid, mid))?.get("config")
}

fn ctx_for(v: &Value, pid: &str, mid: &str) -> Option<u64> {
    rule_config(v, pid, mid)?.pointer("/properties/contextWindow")?.as_u64()
}

fn model_of(pc: &Value, pid: &str, mid: &str, visible: bool) -> Model {
    let context = ctx_for(pc, pid, mid);
    let extra = rule_config(pc, pid, mid).map(|c| mfields::read(c, mfields::ZCODE)).unwrap_or_default();
    let tags = [("supportsImage", "cap:image", "图片", "Images"), ("supportsPdf", "cap:pdf", "PDF", "PDF"), ("supportsVideo", "cap:video", "视频", "Video")]
        .iter()
        .filter(|(k, ..)| extra.get(&format!("/properties/inputFormat/{k}")) == Some(&json!(true)))
        .map(|(_, id, zh, en)| Tag::new(*id, l(zh, en)))
        .collect();
    Model { id: mid.into(), visible, tags, ctx: context.map(fmt_ctx), context, deletable: true, extra, ..Default::default() }
}

/// Writes model fields into the model's rule, creating the rule if needed and dropping it
/// when nothing is left in it.
/// `config.modelConfigRules.providerModelRules`, created when missing.
fn model_rules(pc: &mut Value) -> Result<&mut Vec<Value>> {
    let rules = obj_at(pc, &["config", "modelConfigRules"])?.entry("providerModelRules").or_insert(Value::Null);
    if rules.is_null() {
        *rules = json!([]);
    }
    rules.as_array_mut().ok_or_else(|| anyhow!(l("providerModelRules 不是数组", "providerModelRules is not an array")))
}

fn set_fields(pc: &mut Value, pid: &str, mid: &str, extra: &mfields::Extra) -> Result<Vec<String>> {
    let list = model_rules(pc)?;
    let i = match list.iter().position(|r| is_rule(r, pid, mid)) {
        Some(i) => i,
        None => {
            list.push(json!({ "modelId": mid, "config": {}, "providerId": pid }));
            list.len() - 1
        }
    };
    obj_at(&mut list[i], &["config"])?;
    let lines = mfields::write(&mut list[i]["config"], mfields::ZCODE, extra)?;
    if list[i]["config"].as_object().map(|c| c.is_empty()).unwrap_or(false) {
        list.remove(i);
    }
    Ok(lines)
}

fn provider_list(pc: &Value, legacy: Option<&Value>, setting: Option<&Value>) -> Vec<Provider> {
    let mut out = vec![];
    // Built-in Z.ai plan (signed in through the ZCode account).
    if let Some(lg) = legacy {
        let oauth = setting.and_then(|s| s.pointer("/modelProviderFamilyModes/zai")).and_then(|x| x.as_str()) == Some("oauth");
        let key = if oauth { "builtin:zai-coding-plan" } else { "builtin:zai" };
        if let Some(b) = lg.get("provider").and_then(|p| p.get(key)) {
            let models = b.get("models").and_then(|m| m.as_object()).map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
            out.push(Provider {
                models: models.into_iter().map(|id| Model { id, visible: true, readonly: true, ..Default::default() }).collect(),
                ..Provider::builtin(
                    key,
                    b.get("name").and_then(|x| x.as_str()).unwrap_or("Z.ai").to_string(),
                    if oauth { l("Z.ai 账号登录", "Z.ai account sign-in") } else { "Z.ai API Key" },
                    "chat",
                    l("套餐", "Plan"),
                    vec![
                        Kv::text(lbl::auth(), if oauth { l("Z.ai 账号（OAuth）", "Z.ai account (OAuth)") } else { "Z.ai API Key" }),
                        Kv::mono(lbl::config_id(), key),
                        Kv::text(lbl::note(), l("ZCode 内置，模型列表由 ZCode 管理", "Built into ZCode; its model list is managed by ZCode")),
                    ],
                )
            });
        }
    }
    for r in rules(pc) {
        let Some(api) = r.pointer("/config/api") else { continue };
        let pid = r.get("providerId").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let base = api.get("baseUrl").and_then(|x| x.as_str()).map(String::from);
        let atype = api.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let visible = str_list(r.pointer("/config/personalModelIds")).unwrap_or_default();
        let mut order = str_list(r.pointer("/config/modelOrder")).unwrap_or_default();
        for m in &visible {
            if !order.contains(m) {
                order.push(m.clone());
            }
        }
        let access = r.pointer("/config/access/type").and_then(|x| x.as_str()).unwrap_or("-");
        let has_key = r.pointer("/config/access/apiKey").and_then(|x| x.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        let details = vec![
            Kv::mono(lbl::config_id(), pid.clone()),
            Kv::mono("api.type", if atype.is_empty() { "-".into() } else { atype.to_string() }),
            Kv::text(lbl::api_key(), match (access, has_key) {
                ("api-key", true) => l("API Key · 明文保存在 provider_config.json", "API key · stored in plain text in provider_config.json").to_string(),
                ("api-key", false) => l("API Key · 未填写", "API key · not set").to_string(),
                (other, _) => other.to_string(),
            }),
            Kv::mono(l("分组", "Group"), r.pointer("/config/group").and_then(|x| x.as_str()).unwrap_or("-").to_string()),
        ];
        out.push(Provider {
            details,
            name: r.get("providerName").and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| pid.clone()),
            host: base.as_deref().map(host_of).unwrap_or_default(),
            base_url: base,
            apis: vec![api_label(api_short(atype)).into()],
            enabled: r.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
            compatible: true,
            models: order
                .iter()
                .map(|m| model_of(pc, &pid, m, visible.contains(m)))
                .collect(),
            editable: true,
            api: api_short(atype).into(),
            has_key,
            id: pid,
            ..Default::default()
        });
    }
    out
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&provider_path()), display_path(&setting_path())]);
    let pc = match read_json(&provider_path()) {
        Ok((v, _)) => v,
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let setting = read_json(&setting_path()).ok().map(|x| x.0);
    let legacy = read_json(&legacy_path()).ok().map(|x| x.0);
    st.providers = provider_list(&pc, legacy.as_ref(), setting.as_ref());
    let get_b = |k: &str| setting.as_ref().and_then(|s| s.get(k)).and_then(|x| x.as_bool()).unwrap_or(false);
    st.settings = SETTINGS.iter().map(|(k, g, lb, d)| bool_setting(k, l(g.0, g.1), l(lb.0, lb.1), d, get_b(k))).collect();

    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled).collect();
    let off: Vec<&str> = st.providers.iter().filter(|p| !p.enabled).map(|p| p.name.as_str()).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    let mode = setting.as_ref().and_then(|s| s.pointer("/modelProviderFamilyModes/zai")).and_then(|x| x.as_str()).unwrap_or("-");
    st.current = vec![
        Kv::text(l("登录方式", "Sign-in method"), if mode == "oauth" { l("Z.ai 账号（OAuth）", "Z.ai account (OAuth)").to_string() } else { mode.to_string() }),
        Kv::text(l("启用供应商", "Enabled providers"), tr!("{} 个", "{}", on.len())),
        Kv::text(l("停用供应商", "Disabled providers"), lbl::names_or_none(&off)),
        Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")),
        Kv::text(l("记忆", "Memory"), if get_b("memoryEnabled") { l("开", "On") } else { l("关", "Off") }),
        Kv::text(l("最小化到托盘", "Minimize to tray"), if get_b("closeToTrayOnWindows") { l("开", "On") } else { l("关", "Off") }),
    ];
    st
}

/// Base URL, key and API kind of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (pc, _) = read_json(&provider_path())?;
    let r = rules(&pc)
        .into_iter()
        .find(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(id))
        .ok_or_else(|| msg::no_provider(id))?;
    let base = r.pointer("/config/api/baseUrl").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有地址", "Provider {id} has no base URL")))?.to_string();
    let key = r.pointer("/config/access/apiKey").and_then(|x| x.as_str()).filter(|k| !k.is_empty()).map(String::from);
    let api = api_short(r.pointer("/config/api/type").and_then(|x| x.as_str()).unwrap_or("")).to_string();
    Ok((base, key, api))
}

fn rules_mut(pc: &mut Value) -> Result<&mut Vec<Value>> {
    pc.pointer_mut("/config/providerConfigRules/providerRules")
        .and_then(|r| r.as_array_mut())
        .ok_or_else(|| anyhow!(l("provider_config.json 格式不对", "provider_config.json has an unexpected format")))
}

fn rule_mut<'a>(pc: &'a mut Value, pid: &str) -> Result<&'a mut Value> {
    rules_mut(pc)?
        .iter_mut()
        .find(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(pid))
        .ok_or_else(|| msg::no_provider(pid))
}

fn rule_name(r: &Value, pid: &str) -> String {
    r.get("providerName").and_then(|x| x.as_str()).unwrap_or(pid).to_string()
}

/// Random v4-style id, the same shape ZCode uses for user providers.
fn new_uuid() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let a = h.finish();
    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(a);
    let b = h2.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16 & 0xfff,
        8 + (b >> 62) as u8,
        (b >> 48) as u16 & 0xfff,
        b & 0xffff_ffff_ffff
    )
}

/// Sets or clears a model's context window in modelConfigRules.
fn set_context(pc: &mut Value, pid: &str, mid: &str, ctx: Option<u64>) -> Result<()> {
    let list = model_rules(pc)?;
    let pos = list.iter().position(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(pid) && r.get("modelId").and_then(|x| x.as_str()) == Some(mid));
    match (pos, ctx) {
        (Some(i), Some(c)) => {
            obj_at(&mut list[i], &["config", "properties"])?.insert("contextWindow".into(), json!(c));
        }
        (None, Some(c)) => list.push(json!({ "modelId": mid, "config": { "properties": { "contextWindow": c } }, "providerId": pid })),
        (Some(i), None) => {
            list.remove(i);
        }
        (None, None) => {}
    }
    Ok(())
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut pc, pc_meta) = read_json_object(&provider_path())?;
    let (mut st, st_meta) = read_json_object(&setting_path())?;
    let (pf, sf) = (display_path(&provider_path()), display_path(&setting_path()));
    let mut diff = Diff::default();
    let (mut pc_dirty, mut st_dirty) = (false, false);

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                match &p.id {
                    None => {
                        let pid = new_uuid();
                        let models = clean_ids(&p.models);
                        let rule = json!({
                            "providerId": pid,
                            "providerName": p.name.trim(),
                            "enabled": true,
                            "config": {
                                "group": "standard-personal",
                                "access": { "type": "api-key", "apiKey": p.api_key.clone().unwrap_or_default().trim() },
                                "api": { "type": api_type(&p.api), "baseUrl": p.base_url.trim() },
                                "personalModelIds": models,
                                "modelOrder": models,
                            }
                        });
                        rules_mut(&mut pc)?.push(rule);
                        if let Some(order) = pc.pointer_mut("/config/providerOrder").and_then(|x| x.as_array_mut()) {
                            order.push(json!(pid));
                        }
                        diff.push(&pf, tr!("+ 供应商「{}」{} · {}（{} 个模型）", "+ Provider \"{}\" {} · {} ({} models)", p.name.trim(), p.base_url.trim(), api_label(api_short(api_type(&p.api))), models.len()), true);
                        if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                            diff.push(&pf, tr!("「{}」.apiKey = {}", "\"{}\".apiKey = {}", p.name.trim(), mask_key(k.trim())), true);
                        }
                    }
                    Some(pid) => {
                        let r = rule_mut(&mut pc, pid)?;
                        let name = rule_name(r, pid);
                        if r.get("providerName").and_then(|x| x.as_str()) != Some(p.name.trim()) {
                            r["providerName"] = json!(p.name.trim());
                            diff.push(&pf, tr!("「{name}」.providerName = \"{}\"", "\"{name}\".providerName = \"{}\"", p.name.trim()), true);
                        }
                        if r.pointer("/config/api/baseUrl").and_then(|x| x.as_str()) != Some(p.base_url.trim()) {
                            r["config"]["api"]["baseUrl"] = json!(p.base_url.trim());
                            diff.push(&pf, tr!("「{name}」.baseUrl = \"{}\"", "\"{name}\".baseUrl = \"{}\"", p.base_url.trim()), true);
                        }
                        let t = api_type(&p.api);
                        if r.pointer("/config/api/type").and_then(|x| x.as_str()) != Some(t) {
                            r["config"]["api"]["type"] = json!(t);
                            diff.push(&pf, tr!("「{name}」.api.type = \"{t}\"", "\"{name}\".api.type = \"{t}\""), true);
                        }
                        if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                            r["config"]["access"]["type"] = json!("api-key");
                            r["config"]["access"]["apiKey"] = json!(k.trim());
                            diff.push(&pf, tr!("「{name}」.apiKey = {}", "\"{name}\".apiKey = {}", mask_key(k.trim())), true);
                        }
                    }
                }
                pc_dirty = true;
            }
            Op::DeleteProvider { provider } => {
                let list = rules_mut(&mut pc)?;
                let Some(i) = list.iter().position(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(provider)) else {
                    return Err(msg::no_provider(provider));
                };
                let name = rule_name(&list[i], provider);
                list.remove(i);
                if let Some(order) = pc.pointer_mut("/config/providerOrder").and_then(|x| x.as_array_mut()) {
                    order.retain(|x| x.as_str() != Some(provider));
                }
                if let Some(m) = pc.pointer_mut("/config/modelConfigRules/providerModelRules").and_then(|x| x.as_array_mut()) {
                    m.retain(|r| r.get("providerId").and_then(|x| x.as_str()) != Some(provider));
                }
                diff.push(&pf, tr!("- 供应商「{name}」（含它的模型和密钥）", "- Provider \"{name}\" (with its models and API key)"), false);
                pc_dirty = true;
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                if r.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) != *enabled {
                    r["enabled"] = json!(enabled);
                    diff.push(&pf, tr!("「{name}」.enabled = {enabled}", "\"{name}\".enabled = {enabled}"), *enabled);
                    pc_dirty = true;
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!(tr!("供应商 {provider} 缺少 config", "Provider {provider} has no config")))?;
                let mut ids = str_list(cfg.get("personalModelIds")).unwrap_or_default();
                let mut order = str_list(cfg.get("modelOrder")).unwrap_or_default();
                if ids.contains(model) != *visible {
                    if *visible { ids.push(model.clone()) } else { ids.retain(|m| m != model) }
                    // keep the model in modelOrder so hiding never forgets it
                    if !order.contains(model) {
                        order.push(model.clone());
                    }
                    ids.sort_by_key(|m| order.iter().position(|o| o == m).unwrap_or(usize::MAX));
                    cfg["personalModelIds"] = json!(ids);
                    cfg["modelOrder"] = json!(order);
                    diff.push(&pf, tr!("「{name}」.personalModelIds {} \"{model}\"", "\"{name}\".personalModelIds {} \"{model}\"", if *visible { "+" } else { "-" }), *visible);
                    pc_dirty = true;
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(msg::model_id_required());
                }
                for (k, v) in &m.extra {
                    mfields::check(mfields::ZCODE, k, v)?;
                }
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!(tr!("供应商 {provider} 缺少 config", "Provider {provider} has no config")))?;
                let mut order = str_list(cfg.get("modelOrder")).unwrap_or_default();
                let mut ids = str_list(cfg.get("personalModelIds")).unwrap_or_default();
                if !order.contains(&mid) && !ids.contains(&mid) {
                    order.push(mid.clone());
                    ids.push(mid.clone());
                    cfg["modelOrder"] = json!(order);
                    cfg["personalModelIds"] = json!(ids);
                    diff.push(&pf, tr!("「{name}」+ 模型 \"{mid}\"", "\"{name}\" + model \"{mid}\""), true);
                    pc_dirty = true;
                }
                if let Some(c) = m.context {
                    if ctx_for(&pc, provider, &mid) != Some(c) {
                        set_context(&mut pc, provider, &mid, Some(c))?;
                        diff.push(&pf, tr!("「{name}」{mid} contextWindow = {c}", "\"{name}\" {mid} contextWindow = {c}"), true);
                        pc_dirty = true;
                    }
                }
                for l in set_fields(&mut pc, provider, &mid, &m.extra)? {
                    diff.push(&pf, tr!("「{name}」{mid} {l}", "\"{name}\" {mid} {l}"), true);
                    pc_dirty = true;
                }
            }
            Op::DeleteModel { provider, model } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!(tr!("供应商 {provider} 缺少 config", "Provider {provider} has no config")))?;
                let mut ids = str_list(cfg.get("personalModelIds")).unwrap_or_default();
                let mut order = str_list(cfg.get("modelOrder")).unwrap_or_default();
                ids.retain(|m| m != model);
                order.retain(|m| m != model);
                cfg["personalModelIds"] = json!(ids);
                cfg["modelOrder"] = json!(order);
                set_context(&mut pc, provider, model, None)?;
                diff.push(&pf, tr!("「{name}」- 模型 \"{model}\"", "\"{name}\" - model \"{model}\""), false);
                pc_dirty = true;
            }
            Op::SetSetting { key, value } => {
                if !SETTINGS.iter().any(|(k, ..)| k == key) {
                    return Err(msg::unknown_setting(key));
                }
                let on = value.as_bool().unwrap_or(false);
                if st.get(key).and_then(|x| x.as_bool()).unwrap_or(false) != on {
                    st[key.as_str()] = json!(on);
                    diff.push(&sf, format!("{key} = {on}"), on);
                    st_dirty = true;
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("ZCode 按启用/停用管理供应商", "ZCode manages providers by enabling/disabling them"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            Op::SetProviderModels { .. } => return Err(msg::models_per_provider()),
            Op::SetModelRoles { .. } => return Err(msg::roles_claude_only()),
        }
    }

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (pc_dirty || st_dirty) {
        let mut targets = vec![];
        if pc_dirty { targets.push(provider_path()) }
        if st_dirty { targets.push(setting_path()) }
        backup_dir = Some(backup(ID, &targets)?);
        if pc_dirty {
            write_json(&provider_path(), &pc, pc_meta)?;
            written.push(provider_path());
        }
        if st_dirty {
            write_json(&setting_path(), &st, st_meta)?;
            written.push(setting_path());
        }
    }
    Ok((diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_fields_live_in_the_rule() {
        let mut pc = json!({ "config": { "modelConfigRules": { "providerModelRules": [
            { "modelId": "m", "config": { "properties": { "contextWindow": 1000 } }, "providerId": "p" }
        ] } } });
        let on: mfields::Extra = [("/properties/inputFormat/supportsImage".to_string(), json!(true))].into_iter().collect();
        assert_eq!(set_fields(&mut pc, "p", "m", &on).unwrap().len(), 1);
        assert_eq!(rule_config(&pc, "p", "m").unwrap()["properties"]["inputFormat"], json!({ "supportsImage": true }));
        assert_eq!(model_of(&pc, "p", "m", true).tags, vec![Tag::new("cap:image", "图片")]);
        // A model without a rule gets one; clearing its only field drops it again.
        set_fields(&mut pc, "p", "n", &on).unwrap();
        let off: mfields::Extra = [("/properties/inputFormat/supportsImage".to_string(), Value::Null)].into_iter().collect();
        set_fields(&mut pc, "p", "n", &off).unwrap();
        assert!(rule_config(&pc, "p", "n").is_none());
        // Clearing on "m" keeps its context window.
        set_fields(&mut pc, "p", "m", &off).unwrap();
        assert_eq!(rule_config(&pc, "p", "m").unwrap(), &json!({ "properties": { "contextWindow": 1000 } }));
    }

    #[test]
    fn context_rules_on_odd_shapes() {
        // Missing or null levels are created.
        for mut pc in [json!({}), json!({ "config": null }), json!({ "config": { "modelConfigRules": { "providerModelRules": null } } })] {
            set_context(&mut pc, "p", "m", Some(8000)).unwrap();
            assert_eq!(ctx_for(&pc, "p", "m"), Some(8000));
            set_context(&mut pc, "p", "m", None).unwrap();
            assert_eq!(ctx_for(&pc, "p", "m"), None);
        }
        // Something else in the way is an error, never a panic or an overwrite.
        for mut pc in [json!({ "config": "x" }), json!({ "config": { "modelConfigRules": { "providerModelRules": {} } } }), json!([])] {
            let before = pc.clone();
            assert!(set_context(&mut pc, "p", "m", Some(1)).is_err());
            assert!(set_fields(&mut pc, "p", "m", &Default::default()).is_err());
            assert_eq!(pc, before);
        }
        // A rule whose config is a string is refused rather than replaced.
        let mut pc = json!({ "config": { "modelConfigRules": { "providerModelRules": [{ "modelId": "m", "providerId": "p", "config": "?" }] } } });
        assert!(set_context(&mut pc, "p", "m", Some(1)).is_err());
    }
}
