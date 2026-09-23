//! ZCode: providers live in `~/.zcode/v2/provider_config.json`
//! (`config.providerConfigRules.providerRules[]`). A model shows in the picker
//! when its id is in `personalModelIds`; `modelOrder` keeps the full known list.
//! Per-model context windows live in `config.modelConfigRules.providerModelRules`.

use crate::model::*;
use crate::process::Install;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const ID: &str = "zcode";

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

const SETTINGS: [(&str, &str, &str, &str); 6] = [
    ("messageStreamShowReasoning", "对话", "显示推理过程", "messageStreamShowReasoning"),
    ("messageStreamShowTodos", "对话", "显示待办清单", "messageStreamShowTodos"),
    ("memoryEnabled", "对话", "记忆", "memoryEnabled"),
    ("closeToTrayOnWindows", "窗口与数据", "关闭窗口时最小化到托盘", "closeToTrayOnWindows"),
    ("modelIoFullRetentionEnabled", "窗口与数据", "保留完整的模型输入输出", "modelIoFullRetentionEnabled"),
    ("taskAutoArchiveEnabled", "窗口与数据", "自动归档旧任务", "taskAutoArchiveEnabled"),
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

fn api_label(t: &str) -> &'static str {
    match t {
        "openai-responses" => "Responses",
        "anthropic-messages" => "Anthropic",
        _ => "Chat",
    }
}

fn rules(v: &Value) -> Vec<Value> {
    v.pointer("/config/providerConfigRules/providerRules")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
}

fn str_vec(v: Option<&Value>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn ctx_for(v: &Value, pid: &str, mid: &str) -> Option<u64> {
    v.pointer("/config/modelConfigRules/providerModelRules")?
        .as_array()?
        .iter()
        .find(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(pid) && r.get("modelId").and_then(|x| x.as_str()) == Some(mid))?
        .pointer("/config/properties/contextWindow")?
        .as_u64()
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
                id: key.into(),
                name: b.get("name").and_then(|x| x.as_str()).unwrap_or("Z.ai").to_string(),
                base_url: None,
                host: if oauth { "Z.ai 账号登录".into() } else { "Z.ai API Key".into() },
                apis: vec!["套餐".into()],
                builtin: true,
                enabled: true,
                compatible: true,
                reason: None,
                models: models.into_iter().map(|id| Model { id, visible: true, readonly: true, ..Default::default() }).collect(),
                details: vec![
                    Kv::text("认证方式", if oauth { "Z.ai 账号（OAuth）" } else { "Z.ai API Key" }),
                    Kv::mono("配置 ID", key),
                    Kv::text("说明", "ZCode 内置，模型列表由 ZCode 管理"),
                ],
                editable: false,
                api: "chat".into(),
                has_key: true,
            });
        }
    }
    for r in rules(pc) {
        let Some(api) = r.pointer("/config/api") else { continue };
        let pid = r.get("providerId").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let base = api.get("baseUrl").and_then(|x| x.as_str()).map(String::from);
        let atype = api.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let visible = str_vec(r.pointer("/config/personalModelIds"));
        let mut order = str_vec(r.pointer("/config/modelOrder"));
        for m in &visible {
            if !order.contains(m) {
                order.push(m.clone());
            }
        }
        let access = r.pointer("/config/access/type").and_then(|x| x.as_str()).unwrap_or("-");
        let has_key = r.pointer("/config/access/apiKey").and_then(|x| x.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        let details = vec![
            Kv::mono("配置 ID", pid.clone()),
            Kv::mono("api.type", if atype.is_empty() { "-".into() } else { atype.to_string() }),
            Kv::text("密钥", match (access, has_key) {
                ("api-key", true) => "API Key · 明文保存在 provider_config.json".to_string(),
                ("api-key", false) => "API Key · 未填写".to_string(),
                (other, _) => other.to_string(),
            }),
            Kv::mono("分组", r.pointer("/config/group").and_then(|x| x.as_str()).unwrap_or("-").to_string()),
        ];
        out.push(Provider {
            details,
            name: r.get("providerName").and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| pid.clone()),
            host: base.as_deref().map(host_of).unwrap_or_default(),
            base_url: base,
            apis: vec![api_label(atype).into()],
            builtin: false,
            enabled: r.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
            compatible: true,
            reason: None,
            models: order
                .iter()
                .map(|m| {
                    let context = ctx_for(pc, &pid, m);
                    Model { id: m.clone(), visible: visible.contains(m), ctx: context.map(fmt_ctx), context, deletable: true, ..Default::default() }
                })
                .collect(),
            editable: true,
            api: api_short(atype).into(),
            has_key,
            id: pid,
        });
    }
    out
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: "ZCode".into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "multi".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![display_path(&provider_path()), display_path(&setting_path())],
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
    let pc = match read_json(&provider_path()) {
        Ok((v, _)) => v,
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    let setting = read_json(&setting_path()).ok().map(|x| x.0);
    let legacy = read_json(&legacy_path()).ok().map(|x| x.0);
    st.providers = provider_list(&pc, legacy.as_ref(), setting.as_ref());
    let get_b = |k: &str| setting.as_ref().and_then(|s| s.get(k)).and_then(|x| x.as_bool()).unwrap_or(false);
    st.settings = SETTINGS.iter().map(|(k, g, l, d)| bool_setting(k, g, l, d, get_b(k))).collect();

    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled).collect();
    let off: Vec<&str> = st.providers.iter().filter(|p| !p.enabled).map(|p| p.name.as_str()).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    let mode = setting.as_ref().and_then(|s| s.pointer("/modelProviderFamilyModes/zai")).and_then(|x| x.as_str()).unwrap_or("-");
    st.current = vec![
        Kv::text("登录方式", if mode == "oauth" { "Z.ai 账号（OAuth）".to_string() } else { mode.to_string() }),
        Kv::text("启用供应商", format!("{} 个", on.len())),
        Kv::text("停用供应商", if off.is_empty() { "无".into() } else { off.join("、") }),
        Kv::text("可见模型", format!("{vis} 个")),
        Kv::text("记忆", if get_b("memoryEnabled") { "开" } else { "关" }),
        Kv::text("最小化到托盘", if get_b("closeToTrayOnWindows") { "开" } else { "关" }),
    ];
    st
}

/// Base URL, key and API kind of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<(String, Option<String>, String)> {
    let (pc, _) = read_json(&provider_path())?;
    let r = rules(&pc)
        .into_iter()
        .find(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(id))
        .ok_or_else(|| anyhow!("找不到供应商 {id}"))?;
    let base = r.pointer("/config/api/baseUrl").and_then(|x| x.as_str()).ok_or_else(|| anyhow!("供应商 {id} 没有地址"))?.to_string();
    let key = r.pointer("/config/access/apiKey").and_then(|x| x.as_str()).filter(|k| !k.is_empty()).map(String::from);
    let api = api_short(r.pointer("/config/api/type").and_then(|x| x.as_str()).unwrap_or("")).to_string();
    Ok((base, key, api))
}

fn rules_mut(pc: &mut Value) -> Result<&mut Vec<Value>> {
    pc.pointer_mut("/config/providerConfigRules/providerRules")
        .and_then(|r| r.as_array_mut())
        .ok_or_else(|| anyhow!("provider_config.json 格式不对"))
}

fn rule_mut<'a>(pc: &'a mut Value, pid: &str) -> Result<&'a mut Value> {
    rules_mut(pc)?
        .iter_mut()
        .find(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(pid))
        .ok_or_else(|| anyhow!("找不到供应商 {pid}"))
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
fn set_context(pc: &mut Value, pid: &str, mid: &str, ctx: Option<u64>) {
    if pc.pointer("/config/modelConfigRules/providerModelRules").is_none() {
        pc["config"]["modelConfigRules"]["providerModelRules"] = json!([]);
    }
    let list = pc.pointer_mut("/config/modelConfigRules/providerModelRules").and_then(|x| x.as_array_mut()).unwrap();
    let pos = list.iter().position(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(pid) && r.get("modelId").and_then(|x| x.as_str()) == Some(mid));
    match (pos, ctx) {
        (Some(i), Some(c)) => list[i]["config"]["properties"]["contextWindow"] = json!(c),
        (None, Some(c)) => list.push(json!({ "modelId": mid, "config": { "properties": { "contextWindow": c } }, "providerId": pid })),
        (Some(i), None) => {
            list.remove(i);
        }
        (None, None) => {}
    }
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    let (mut pc, pc_meta) = read_json(&provider_path())?;
    let (mut st, st_meta) = read_json(&setting_path())?;
    let (pf, sf) = (display_path(&provider_path()), display_path(&setting_path()));
    let mut diff = Diff::default();
    let (mut pc_dirty, mut st_dirty) = (false, false);

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!("名称和地址不能为空"));
                }
                match &p.id {
                    None => {
                        let pid = new_uuid();
                        let models: Vec<String> = p.models.iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
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
                        diff.push(&pf, format!("+ 供应商「{}」{} · {}（{} 个模型）", p.name.trim(), p.base_url.trim(), api_label(api_type(&p.api)), models.len()), true);
                        if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                            diff.push(&pf, format!("「{}」.apiKey = {}", p.name.trim(), mask_key(k.trim())), true);
                        }
                    }
                    Some(pid) => {
                        let r = rule_mut(&mut pc, pid)?;
                        let name = rule_name(r, pid);
                        if r.get("providerName").and_then(|x| x.as_str()) != Some(p.name.trim()) {
                            r["providerName"] = json!(p.name.trim());
                            diff.push(&pf, format!("「{name}」.providerName = \"{}\"", p.name.trim()), true);
                        }
                        if r.pointer("/config/api/baseUrl").and_then(|x| x.as_str()) != Some(p.base_url.trim()) {
                            r["config"]["api"]["baseUrl"] = json!(p.base_url.trim());
                            diff.push(&pf, format!("「{name}」.baseUrl = \"{}\"", p.base_url.trim()), true);
                        }
                        let t = api_type(&p.api);
                        if r.pointer("/config/api/type").and_then(|x| x.as_str()) != Some(t) {
                            r["config"]["api"]["type"] = json!(t);
                            diff.push(&pf, format!("「{name}」.api.type = \"{t}\""), true);
                        }
                        if let Some(k) = p.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
                            r["config"]["access"]["type"] = json!("api-key");
                            r["config"]["access"]["apiKey"] = json!(k.trim());
                            diff.push(&pf, format!("「{name}」.apiKey = {}", mask_key(k.trim())), true);
                        }
                    }
                }
                pc_dirty = true;
            }
            Op::DeleteProvider { provider } => {
                let list = rules_mut(&mut pc)?;
                let Some(i) = list.iter().position(|r| r.get("providerId").and_then(|x| x.as_str()) == Some(provider)) else {
                    return Err(anyhow!("找不到供应商 {provider}"));
                };
                let name = rule_name(&list[i], provider);
                list.remove(i);
                if let Some(order) = pc.pointer_mut("/config/providerOrder").and_then(|x| x.as_array_mut()) {
                    order.retain(|x| x.as_str() != Some(provider));
                }
                if let Some(m) = pc.pointer_mut("/config/modelConfigRules/providerModelRules").and_then(|x| x.as_array_mut()) {
                    m.retain(|r| r.get("providerId").and_then(|x| x.as_str()) != Some(provider));
                }
                diff.push(&pf, format!("- 供应商「{name}」（含它的模型和密钥）"), false);
                pc_dirty = true;
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                if r.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) != *enabled {
                    r["enabled"] = json!(enabled);
                    diff.push(&pf, format!("「{name}」.enabled = {enabled}"), *enabled);
                    pc_dirty = true;
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!("供应商 {provider} 缺少 config"))?;
                let mut ids = str_vec(cfg.get("personalModelIds"));
                let mut order = str_vec(cfg.get("modelOrder"));
                if ids.contains(model) != *visible {
                    if *visible { ids.push(model.clone()) } else { ids.retain(|m| m != model) }
                    // keep the model in modelOrder so hiding never forgets it
                    if !order.contains(model) {
                        order.push(model.clone());
                    }
                    ids.sort_by_key(|m| order.iter().position(|o| o == m).unwrap_or(usize::MAX));
                    cfg["personalModelIds"] = json!(ids);
                    cfg["modelOrder"] = json!(order);
                    diff.push(&pf, format!("「{name}」.personalModelIds {} \"{model}\"", if *visible { "+" } else { "-" }), *visible);
                    pc_dirty = true;
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(anyhow!("模型 ID 不能为空"));
                }
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!("供应商 {provider} 缺少 config"))?;
                let mut order = str_vec(cfg.get("modelOrder"));
                let mut ids = str_vec(cfg.get("personalModelIds"));
                if !order.contains(&mid) && !ids.contains(&mid) {
                    order.push(mid.clone());
                    ids.push(mid.clone());
                    cfg["modelOrder"] = json!(order);
                    cfg["personalModelIds"] = json!(ids);
                    diff.push(&pf, format!("「{name}」+ 模型 \"{mid}\""), true);
                    pc_dirty = true;
                }
                if let Some(c) = m.context {
                    if ctx_for(&pc, provider, &mid) != Some(c) {
                        set_context(&mut pc, provider, &mid, Some(c));
                        diff.push(&pf, format!("「{name}」{mid} contextWindow = {c}"), true);
                        pc_dirty = true;
                    }
                }
            }
            Op::DeleteModel { provider, model } => {
                let r = rule_mut(&mut pc, provider)?;
                let name = rule_name(r, provider);
                let cfg = r.get_mut("config").ok_or_else(|| anyhow!("供应商 {provider} 缺少 config"))?;
                let mut ids = str_vec(cfg.get("personalModelIds"));
                let mut order = str_vec(cfg.get("modelOrder"));
                ids.retain(|m| m != model);
                order.retain(|m| m != model);
                cfg["personalModelIds"] = json!(ids);
                cfg["modelOrder"] = json!(order);
                set_context(&mut pc, provider, model, None);
                diff.push(&pf, format!("「{name}」- 模型 \"{model}\""), false);
                pc_dirty = true;
            }
            Op::SetSetting { key, value } => {
                if !SETTINGS.iter().any(|(k, ..)| k == key) {
                    return Err(anyhow!("未知设置 {key}"));
                }
                let on = value.as_bool().unwrap_or(false);
                if st.get(key).and_then(|x| x.as_bool()).unwrap_or(false) != on {
                    st[key.as_str()] = json!(on);
                    diff.push(&sf, format!("{key} = {on}"), on);
                    st_dirty = true;
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!("ZCode 按启用/停用管理供应商")),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
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
