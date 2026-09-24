//! Providers in the OpenCode config format, shared by OpenCode and MiMo Desktop (its
//! engine is an OpenCode fork): `provider.<id> = { npm, name, options: { baseURL, apiKey },
//! models: { <id>: { name, limit: { context } } } }`.
//!
//! Hidden models are removed from the config and their definitions stashed in the
//! AgentPlus store so they come back untouched. Disabled providers either use the
//! native `disabled_providers` list (OpenCode) or are stashed the same way (MiMo).
//! Keys live in `options.apiKey`, or in OpenCode's `auth.json` when that is where the
//! user keeps them.

use crate::i18n::l;
use crate::mfields;
use crate::model::*;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub struct Fmt {
    pub agent: &'static str,
    pub path: PathBuf,
    /// OpenCode's credential store (`~/.local/share/opencode/auth.json`).
    pub auth: Option<PathBuf>,
    /// Use the config's own `disabled_providers` list instead of stashing.
    pub native_disable: bool,
}

thread_local! {
    /// Ids a new provider must not take. OpenCode project configs set it to the ids the
    /// global config already uses, so a new project provider is not merged into one of them.
    pub static RESERVED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[derive(Default)]
pub struct Dirty {
    pub cfg: bool,
    pub store: bool,
    pub auth: bool,
}

/// OpenCode's schema needs `modalities.input` / `.output` and `limit.context` / `.output`
/// in pairs: fills in the missing half (text output; OpenCode's 32000-token output cap,
/// a 128K context) and drops a lone default output modality.
fn complete_model(def: &mut Value) -> Vec<String> {
    let mut out = vec![];
    if let Some(m) = def.get_mut("modalities").and_then(|m| m.as_object_mut()) {
        match (m.contains_key("input"), m.contains_key("output")) {
            (true, false) => {
                m.insert("output".into(), json!(["text"]));
                out.push("modalities.output = [\"text\"]".into());
            }
            (false, true) if m.len() == 1 && m["output"] == json!(["text"]) => {
                def.as_object_mut().unwrap().remove("modalities");
            }
            _ => {}
        }
    }
    if let Some(l) = def.get_mut("limit").and_then(|l| l.as_object_mut()) {
        match (l.contains_key("context"), l.contains_key("output")) {
            (true, false) => {
                l.insert("output".into(), json!(32000));
                out.push(tr!("limit.output = 32000（OpenCode 要求和 limit.context 一起写）", "limit.output = 32000 (OpenCode requires it together with limit.context)"));
            }
            (false, true) => {
                l.insert("context".into(), json!(128000));
                out.push(tr!("limit.context = 128000（OpenCode 要求和 limit.output 一起写）", "limit.context = 128000 (OpenCode requires it together with limit.output)"));
            }
            _ => {}
        }
    }
    out
}

pub fn api_of(npm: &str) -> &'static str {
    if npm.contains("anthropic") {
        "anthropic"
    } else if npm.ends_with("/openai") {
        "responses"
    } else {
        "chat"
    }
}

pub fn api_label(api: &str) -> &'static str {
    match api {
        "anthropic" => "Anthropic",
        "responses" => "Responses",
        _ => "Chat",
    }
}

pub fn npm_for(api: &str) -> &'static str {
    match api {
        "anthropic" => "@ai-sdk/anthropic",
        "responses" => "@ai-sdk/openai",
        _ => "@ai-sdk/openai-compatible",
    }
}

impl Fmt {
    pub fn file(&self) -> String {
        display_path(&self.path)
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// Returns (config, meta, has_comments). A missing file reads as `{}` when `allow_missing`.
    pub fn load(&self, allow_missing: bool) -> Result<(Value, TextMeta, bool)> {
        if allow_missing && !self.path.exists() {
            return Ok((json!({ "$schema": "https://opencode.ai/config.json" }), TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }, false));
        }
        let (text, meta) = read_text(&self.path)?;
        let (clean, had) = strip_jsonc(&text);
        let v = serde_json::from_str(&clean).map_err(|e| anyhow!(tr!("{} 解析失败：{e}", "Failed to parse {}: {e}", self.name())))?;
        Ok((v, meta, had))
    }

    pub fn load_auth(&self) -> Option<(Value, TextMeta)> {
        let p = self.auth.as_ref()?;
        match read_json(p) {
            Ok(x) => Some(x),
            Err(_) => Some((json!({}), TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 })),
        }
    }

    fn stash(&self, root: &Value, key: &str) -> Map<String, Value> {
        store::agent_get(root, self.agent, key).and_then(|x| x.as_object()).cloned().unwrap_or_default()
    }

    fn disabled(&self, cfg: &Value) -> Vec<String> {
        cfg.get("disabled_providers").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    fn auth_key<'a>(auth: Option<&'a Value>, id: &str) -> Option<&'a str> {
        auth?.get(id).filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("api")).and_then(|e| e.get("key")).and_then(|k| k.as_str()).filter(|k| !k.is_empty())
    }

    fn model_from(id: &str, def: &Value, visible: bool) -> Model {
        let context = def.pointer("/limit/context").and_then(|x| x.as_u64());
        Model {
            id: id.into(),
            visible,
            ctx: context.map(fmt_ctx),
            context,
            name: def.get("name").and_then(|x| x.as_str()).map(String::from),
            deletable: true,
            extra: mfields::read(def, mfields::OPENCODE),
            ..Default::default()
        }
    }

    fn provider_from(&self, id: &str, def: &Value, enabled: bool, hidden: &Map<String, Value>, auth: Option<&Value>) -> Provider {
        let base = def.pointer("/options/baseURL").and_then(|x| x.as_str()).map(String::from);
        let npm = def.get("npm").and_then(|x| x.as_str()).unwrap_or("");
        let api = api_of(npm);
        let mut models: Vec<Model> = def
            .get("models")
            .and_then(|m| m.as_object())
            .map(|m| m.iter().map(|(mid, d)| Self::model_from(mid, d, true)).collect())
            .unwrap_or_default();
        let prefix = format!("{id}|");
        for (k, d) in hidden {
            if let Some(mid) = k.strip_prefix(&prefix) {
                models.push(Self::model_from(mid, d, false));
            }
        }
        let in_cfg = def.pointer("/options/apiKey").and_then(|x| x.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        let in_auth = Self::auth_key(auth, id).is_some();
        let key_note = if in_cfg {
            tr!("API Key · 明文保存在 {}", "API key · stored in plain text in {}", self.name())
        } else if in_auth {
            l("API Key · 保存在 auth.json", "API key · stored in auth.json").into()
        } else {
            l("未填写", "Not set").into()
        };
        let parked = if self.native_disable {
            l("已停用（disabled_providers）", "Disabled (disabled_providers)")
        } else {
            l("已停用 · 定义暂存在 AgentPlus", "Disabled · definition kept in AgentPlus")
        };
        Provider {
            id: id.into(),
            name: def.get("name").and_then(|x| x.as_str()).unwrap_or(id).to_string(),
            host: base.as_deref().map(host_of).unwrap_or_default(),
            base_url: base,
            apis: vec![api_label(api).into()],
            builtin: false,
            enabled,
            compatible: true,
            reason: None,
            models,
            details: vec![
                Kv::mono(l("配置 ID", "Config ID"), format!("provider.{id}")),
                Kv::mono("npm", if npm.is_empty() { "-".into() } else { npm.to_string() }),
                Kv::text(l("密钥", "API key"), key_note),
                Kv::text(l("状态", "Status"), if enabled { l("已启用", "Enabled") } else { parked }),
            ],
            editable: true,
            api: api.into(),
            has_key: in_cfg || in_auth,
            key_fp: None,
            key_hint: None,
            official_auth: false,
        }
    }

    /// Custom providers in the config (enabled, natively disabled, or stashed).
    pub fn providers(&self, cfg: &Value, root: &Value) -> Vec<Provider> {
        let hidden = self.stash(root, "hiddenModels");
        let auth = self.load_auth().map(|x| x.0);
        let off = self.disabled(cfg);
        let mut out = vec![];
        if let Some(p) = cfg.get("provider").and_then(|x| x.as_object()) {
            for (id, def) in p {
                out.push(self.provider_from(id, def, !off.contains(id), &hidden, auth.as_ref()));
            }
        }
        if !self.native_disable {
            for (id, def) in &self.stash(root, "disabledProviders") {
                out.push(self.provider_from(id, def, false, &hidden, auth.as_ref()));
            }
        }
        out
    }

    /// Base URL, key and API kind of a provider.
    pub fn endpoint(&self, id: &str) -> Result<(String, Option<String>, String)> {
        let (cfg, _, _) = self.load(true)?;
        let parked = self.stash(&store::load(), "disabledProviders");
        let def = cfg.pointer(&format!("/provider/{id}")).cloned().or_else(|| parked.get(id).cloned()).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
        let base = def.pointer("/options/baseURL").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 baseURL", "Provider {id} has no baseURL")))?.to_string();
        let auth = self.load_auth().map(|x| x.0);
        let key = def
            .pointer("/options/apiKey")
            .and_then(|x| x.as_str())
            .filter(|k| !k.is_empty())
            .map(String::from)
            .or_else(|| Self::auth_key(auth.as_ref(), id).map(String::from));
        Ok((base, key, api_of(def.get("npm").and_then(|x| x.as_str()).unwrap_or("")).into()))
    }

    fn providers_obj<'a>(&self, cfg: &'a mut Value) -> Result<&'a mut Map<String, Value>> {
        cfg.as_object_mut()
            .ok_or_else(|| anyhow!(tr!("{} 顶层不是对象", "The top level of {} is not an object", self.name())))?
            .entry("provider")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!(l("provider 不是对象", "provider is not an object")))
    }

    /// Edits a model definition wherever it lives (active config or the hidden stash).
    fn model_def_mut<'a>(&self, cfg: &'a mut Value, root: &'a mut Value, pid: &str, mid: &str) -> Option<&'a mut Value> {
        if cfg.pointer(&format!("/provider/{pid}/models/{mid}")).is_some() {
            return cfg.pointer_mut(&format!("/provider/{pid}/models/{mid}"));
        }
        store::section(root, self.agent, "hiddenModels").get_mut(&format!("{pid}|{mid}"))
    }

    fn set_key(&self, cfg: &mut Value, auth: &mut Option<(Value, TextMeta)>, id: &str, key: &str, diff: &mut Diff, dirty: &mut Dirty) {
        // Keep the key where the user already keeps it; new ones go to auth.json when there is one.
        let in_cfg = cfg.pointer(&format!("/provider/{id}/options/apiKey")).and_then(|x| x.as_str()).map(|k| !k.is_empty()).unwrap_or(false);
        if let (Some((a, _)), false) = (auth.as_mut(), in_cfg) {
            a[id] = json!({ "type": "api", "key": key });
            diff.push(&display_path(self.auth.as_ref().unwrap()), format!("{id}.key = {}", mask_key(key)), true);
            dirty.auth = true;
        } else if let Some(def) = cfg.pointer_mut(&format!("/provider/{id}")) {
            if !def.get("options").map(|o| o.is_object()).unwrap_or(false) {
                def["options"] = json!({});
            }
            def["options"]["apiKey"] = json!(key);
            diff.push(&self.file(), format!("provider.{id}.options.apiKey = {}", mask_key(key)), true);
            dirty.cfg = true;
        }
    }

    /// Applies one provider / model op. Returns false for ops this module does not handle.
    pub fn apply(&self, op: &Op, cfg: &mut Value, root: &mut Value, auth: &mut Option<(Value, TextMeta)>, diff: &mut Diff, dirty: &mut Dirty) -> Result<bool> {
        let ef = self.file();
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                match &p.id {
                    None => {
                        let parked = self.stash(root, "disabledProviders");
                        let providers = self.providers_obj(cfg)?;
                        let base = slug(&p.name);
                        let free = |c: &String| !providers.contains_key(c) && !parked.contains_key(c) && !RESERVED.with(|r| r.borrow().contains(c));
                        let id = if free(&base) { base.clone() } else { (2..).map(|n| format!("{base}-{n}")).find(free).unwrap() };
                        let models: Map<String, Value> = p.models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()).map(|m| (m.to_string(), json!({}))).collect();
                        let n = models.len();
                        providers.insert(id.clone(), json!({
                            "npm": npm_for(&p.api),
                            "name": p.name.trim(),
                            "options": { "baseURL": p.base_url.trim() },
                            "models": models,
                        }));
                        diff.push(
                            &ef,
                            if crate::i18n::is_en() && n == 1 {
                                format!("+ provider.{id} ({} · {} · 1 model)", p.base_url.trim(), api_label(&p.api))
                            } else {
                                tr!("+ provider.{id}（{} · {} · {n} 个模型）", "+ provider.{id} ({} · {} · {n} models)", p.base_url.trim(), api_label(&p.api))
                            },
                            true,
                        );
                        dirty.cfg = true;
                        if let Some(k) = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                            self.set_key(cfg, auth, &id, k, diff, dirty);
                        }
                    }
                    Some(id) => {
                        let in_cfg = cfg.pointer(&format!("/provider/{id}")).is_some();
                        let def = if in_cfg {
                            cfg.pointer_mut(&format!("/provider/{id}")).unwrap()
                        } else {
                            store::section(root, self.agent, "disabledProviders").get_mut(id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?
                        };
                        let mut changed = vec![];
                        if def.get("name").and_then(|x| x.as_str()) != Some(p.name.trim()) {
                            def["name"] = json!(p.name.trim());
                            changed.push(format!("name = \"{}\"", p.name.trim()));
                        }
                        if !def.get("options").map(|o| o.is_object()).unwrap_or(false) {
                            def["options"] = json!({});
                        }
                        if def.pointer("/options/baseURL").and_then(|x| x.as_str()) != Some(p.base_url.trim()) {
                            def["options"]["baseURL"] = json!(p.base_url.trim());
                            changed.push(format!("options.baseURL = \"{}\"", p.base_url.trim()));
                        }
                        let npm = npm_for(&p.api);
                        if api_of(def.get("npm").and_then(|x| x.as_str()).unwrap_or("")) != p.api {
                            def["npm"] = json!(npm);
                            changed.push(format!("npm = \"{npm}\""));
                        }
                        for c in changed {
                            diff.push(&ef, format!("provider.{id}.{c}"), true);
                            if in_cfg { dirty.cfg = true } else { dirty.store = true }
                        }
                        if let Some(k) = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                            if in_cfg {
                                self.set_key(cfg, auth, id, k, diff, dirty);
                            } else if let Some(def) = store::section(root, self.agent, "disabledProviders").get_mut(id) {
                                def["options"]["apiKey"] = json!(k);
                                diff.push(&ef, format!("provider.{id}.options.apiKey = {}", mask_key(k)), true);
                                dirty.store = true;
                            }
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let removed_cfg = self.providers_obj(cfg)?.remove(provider).is_some();
                let removed_stash = store::section(root, self.agent, "disabledProviders").remove(provider).is_some();
                let prefix = format!("{provider}|");
                store::section(root, self.agent, "hiddenModels").retain(|k, _| !k.starts_with(&prefix));
                if !removed_cfg && !removed_stash {
                    return Err(anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")));
                }
                if self.native_disable {
                    if let Some(list) = cfg.get_mut("disabled_providers").and_then(|x| x.as_array_mut()) {
                        list.retain(|x| x.as_str() != Some(provider.as_str()));
                    }
                }
                let line = if self.auth.is_some() {
                    tr!("- provider.{provider}（含它的模型；auth.json 里的密钥保留）", "- provider.{provider} (with its models; the API key in auth.json is kept)")
                } else {
                    tr!("- provider.{provider}（含它的模型和密钥）", "- provider.{provider} (with its models and API key)")
                };
                diff.push(&ef, line, false);
                dirty.cfg |= removed_cfg;
                dirty.store = true;
            }
            Op::SetProviderEnabled { provider, enabled } => {
                if self.native_disable {
                    let mut off = self.disabled(cfg);
                    let was = !off.contains(provider);
                    if was != *enabled {
                        if *enabled { off.retain(|x| x != provider) } else { off.push(provider.clone()) }
                        cfg["disabled_providers"] = json!(off);
                        diff.push(&ef, format!("disabled_providers {} \"{provider}\"", if *enabled { "-" } else { "+" }), *enabled);
                        dirty.cfg = true;
                    }
                } else {
                    let providers = self.providers_obj(cfg)?;
                    let parked = store::section(root, self.agent, "disabledProviders");
                    if *enabled {
                        if let Some(def) = parked.remove(provider) {
                            providers.insert(provider.clone(), def);
                            diff.push(&ef, format!("+ provider.{provider}"), true);
                            dirty.cfg = true;
                            dirty.store = true;
                        }
                    } else if let Some(def) = providers.remove(provider) {
                        parked.insert(provider.clone(), def);
                        diff.push(&ef, tr!("- provider.{provider}（定义暂存在 AgentPlus，可恢复）", "- provider.{provider} (definition kept in AgentPlus; can be restored)"), false);
                        dirty.cfg = true;
                        dirty.store = true;
                    }
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let key = format!("{provider}|{model}");
                let models = cfg
                    .pointer_mut(&format!("/provider/{provider}"))
                    .and_then(|p| p.as_object_mut())
                    .ok_or_else(|| anyhow!(tr!("供应商 {provider} 未启用，先启用再调整模型", "Provider {provider} is disabled; enable it before changing its models")))?
                    .entry("models")
                    .or_insert_with(|| json!({}));
                let models = models.as_object_mut().ok_or_else(|| anyhow!(l("models 不是对象", "models is not an object")))?;
                let hidden = store::section(root, self.agent, "hiddenModels");
                if *visible {
                    if !models.contains_key(model) {
                        models.insert(model.clone(), hidden.remove(&key).unwrap_or_else(|| json!({})));
                        diff.push(&ef, format!("provider.{provider}.models + \"{model}\""), true);
                        dirty.cfg = true;
                        dirty.store = true;
                    }
                } else if let Some(def) = models.remove(model) {
                    hidden.insert(key, def);
                    diff.push(&ef, format!("provider.{provider}.models - \"{model}\""), false);
                    dirty.cfg = true;
                    dirty.store = true;
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
                }
                for (k, v) in &m.extra {
                    mfields::check(mfields::OPENCODE, k, v)?;
                }
                if self.model_def_mut(cfg, root, provider, &mid).is_none() {
                    let models = cfg
                        .pointer_mut(&format!("/provider/{provider}"))
                        .and_then(|p| p.as_object_mut())
                        .ok_or_else(|| anyhow!(tr!("供应商 {provider} 未启用，先启用再添加模型", "Provider {provider} is disabled; enable it before adding models")))?
                        .entry("models")
                        .or_insert_with(|| json!({}));
                    models.as_object_mut().ok_or_else(|| anyhow!(l("models 不是对象", "models is not an object")))?.insert(mid.clone(), json!({}));
                    diff.push(&ef, format!("provider.{provider}.models + \"{mid}\""), true);
                    dirty.cfg = true;
                }
                let in_cfg = cfg.pointer(&format!("/provider/{provider}/models/{mid}")).is_some();
                let def = self.model_def_mut(cfg, root, provider, &mid).unwrap();
                let mut changed = vec![];
                if let Some(n) = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    if def.get("name").and_then(|x| x.as_str()) != Some(n) {
                        def["name"] = json!(n);
                        changed.push(format!("name = \"{n}\""));
                    }
                }
                if let Some(c) = m.context {
                    if def.pointer("/limit/context").and_then(|x| x.as_u64()) != Some(c) {
                        if !def.get("limit").map(|l| l.is_object()).unwrap_or(false) {
                            def["limit"] = json!({});
                        }
                        def["limit"]["context"] = json!(c);
                        changed.push(format!("limit.context = {c}"));
                    }
                }
                changed.extend(mfields::write(def, mfields::OPENCODE, &m.extra)?);
                changed.extend(complete_model(def));
                for c in changed {
                    diff.push(&ef, format!("provider.{provider}.models.\"{mid}\".{c}"), true);
                    if in_cfg { dirty.cfg = true } else { dirty.store = true }
                }
            }
            Op::DeleteModel { provider, model } => {
                let removed = cfg
                    .pointer_mut(&format!("/provider/{provider}/models"))
                    .and_then(|m| m.as_object_mut())
                    .and_then(|m| m.remove(model))
                    .is_some();
                let stashed = store::section(root, self.agent, "hiddenModels").remove(&format!("{provider}|{model}")).is_some();
                if removed || stashed {
                    diff.push(&ef, tr!("provider.{provider}.models - \"{model}\"（删除）", "provider.{provider}.models - \"{model}\" (deleted)"), false);
                    dirty.cfg |= removed;
                    dirty.store |= stashed;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_pairs_are_completed() {
        let mut d = json!({ "limit": { "context": 200000 }, "modalities": { "input": ["text", "image"] } });
        assert_eq!(complete_model(&mut d).len(), 2);
        assert_eq!(d["limit"], json!({ "context": 200000, "output": 32000 }));
        assert_eq!(d["modalities"]["output"], json!(["text"]));
        let mut d = json!({ "limit": { "output": 8192 }, "modalities": { "output": ["text"] } });
        complete_model(&mut d);
        assert_eq!(d["limit"], json!({ "context": 128000, "output": 8192 }));
        assert!(d.get("modalities").is_none());
    }
}
