//! Providers in the OpenCode config format, shared by OpenCode and MiMo Desktop (its
//! engine is an OpenCode fork): `provider.<id> = { npm, name, options: { baseURL, apiKey },
//! models: { <id>: { name, limit: { context } } } }`.
//!
//! Hidden models are removed from the config and their definitions stashed in the
//! AgentPlus store so they come back untouched. Disabled providers either use the
//! native `disabled_providers` list (OpenCode) or are stashed the same way (MiMo).
//! Keys live in `options.apiKey`, or in OpenCode's `auth.json` when that is where the
//! user keeps them.

use super::msg;
use super::Endpoint;
use crate::i18n::l;
use crate::mfields;
use crate::model::*;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct Fmt {
    /// The agent id, which also names its AgentPlus store section (stashed models and providers).
    pub agent: String,
    pub path: PathBuf,
    /// OpenCode's credential store (`~/.local/share/opencode/auth.json`).
    pub auth: Option<PathBuf>,
    /// Use the config's own `disabled_providers` list instead of stashing.
    pub native_disable: bool,
    /// Ids a new provider must not take. OpenCode project configs set it to the ids the
    /// global config already uses, so a new project provider is not merged into one of them.
    pub reserved: Vec<String>,
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

fn api_of(npm: &str) -> &'static str {
    if npm.contains("anthropic") {
        "anthropic"
    } else if npm.ends_with("/openai") {
        "responses"
    } else {
        "chat"
    }
}

/// A provider's non-empty `options.apiKey` as written (it may hold references, see `key_ref`).
pub(super) fn cfg_key(def: &Value) -> Option<&str> {
    def.pointer("/options/apiKey")?.as_str().filter(|k| !k.trim().is_empty())
}

static KEY_REF: OnceLock<regex::Regex> = OnceLock::new();

/// OpenCode's config substitutions: `{env:NAME}` and `{file:path}`.
fn key_ref_re() -> regex::Regex {
    regex::Regex::new(r"\{(env|file):([^}]+)\}").unwrap()
}

/// The first reference in a key: ("env", NAME) or ("file", path).
fn key_ref(raw: &str) -> Option<(&str, &str)> {
    let c = KEY_REF.get_or_init(key_ref_re).captures(raw)?;
    Some((c.get(1)?.as_str(), c.get(2)?.as_str()))
}

fn npm_for(api: &str) -> &'static str {
    match api {
        "anthropic" => "@ai-sdk/anthropic",
        "responses" => "@ai-sdk/openai",
        _ => "@ai-sdk/openai-compatible",
    }
}

/// Read-only cards for the entries of an `auth.json` (OpenCode and its forks, pi) that no
/// config entry in `known` covers: providers built into the agent, logged in with an OAuth
/// account or given an API key. `login` is how to log in (`opencode auth`), `about` the note
/// on where their models come from; ids in `off` are disabled.
pub(crate) fn auth_cards(auth: Option<&Value>, known: &[Provider], login: &str, about: &str, off: &[String]) -> Vec<Provider> {
    let Some(obj) = auth.and_then(|a| a.as_object()) else { return vec![] };
    obj.iter()
        .filter(|(id, _)| !known.iter().any(|p| &p.id == *id))
        .map(|(id, e)| {
            let oauth = e.get("type").and_then(|t| t.as_str()) == Some("oauth");
            Provider {
                enabled: !off.contains(id),
                ..Provider::builtin(
                    id.clone(),
                    id.clone(),
                    if oauth { tr!("账号登录（{login}）", "Account sign-in ({login})") } else { l("内置供应商 · API Key", "Built-in provider · API key").into() },
                    "chat",
                    l("内置", "Built-in"),
                    vec![
                        Kv::mono(lbl::credentials(), format!("auth.json · {id} · {}", if oauth { l("OAuth 登录", "OAuth sign-in") } else { l("API Key", "API key") })),
                        Kv::text(lbl::note(), about),
                    ],
                )
            }
        })
        .collect()
}

/// Writes a credentials file in place (not tmp + rename) so it keeps its owner-only permissions.
pub(crate) fn write_auth(path: &Path, auth: &Value) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(auth)? + "\n")?;
    Ok(())
}

impl Fmt {
    pub fn new(agent: &str, path: PathBuf, auth: Option<PathBuf>, native_disable: bool) -> Self {
        Fmt { agent: agent.into(), path, auth, native_disable, reserved: vec![] }
    }

    pub fn file(&self) -> String {
        display_path(&self.path)
    }

    /// The config's file name (`opencode.json`).
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// Returns (config, meta, has_comments). A missing file reads as `{}` when `allow_missing`.
    pub fn load(&self, allow_missing: bool) -> Result<(Value, TextMeta, bool)> {
        if allow_missing {
            return read_jsonc_object_or(&self.path, json!({ "$schema": "https://opencode.ai/config.json" }));
        }
        let (text, meta) = read_text(&self.path)?;
        let (v, had) = parse_jsonc_object(&text, &self.name())?;
        Ok((v, meta, had))
    }

    /// The credentials file: `{}` when it doesn't exist yet, None when it can't be read or
    /// parsed (it is then left alone, never rewritten from scratch).
    pub fn load_auth(&self) -> Option<(Value, TextMeta)> {
        let p = self.auth.as_ref()?;
        if !p.exists() {
            return Some((json!({}), TextMeta::NEW));
        }
        read_json(p).ok().filter(|(v, _)| v.is_object())
    }

    /// The config's own `disabled_providers` list.
    pub fn disabled(&self, cfg: &Value) -> Vec<String> {
        str_list(cfg.get("disabled_providers")).unwrap_or_default()
    }

    /// The real key behind an `options.apiKey` value: `{env:NAME}` / `{file:path}` references
    /// are substituted the way OpenCode does (a relative path is relative to the config's
    /// folder). None when a reference can't be resolved here, so it is never sent literally.
    fn resolve_key(&self, raw: &str) -> Option<String> {
        let mut missing = false;
        let out = KEY_REF.get_or_init(key_ref_re).replace_all(raw, |c: &regex::Captures| {
            let v = if &c[1] == "env" {
                crate::env::agent_var(&c[2])
            } else {
                let p = crate::env::resolve_path(&c[2]);
                let p = if p.is_absolute() { p } else { self.path.parent().map(|d| d.join(&p)).unwrap_or(p) };
                std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
            };
            v.unwrap_or_else(|| {
                missing = true;
                String::new()
            })
        });
        (!missing).then(|| out.trim().to_string()).filter(|k| !k.is_empty())
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
        let in_cfg = cfg_key(def);
        let in_auth = Self::auth_key(auth, id).is_some();
        let key_note = if let Some(k) = in_cfg {
            match key_ref(k) {
                Some(("env", n)) => tr!("API Key · 环境变量 {n}", "API key · environment variable {n}"),
                Some((_, p)) => tr!("API Key · 读取文件 {p}", "API key · read from file {p}"),
                None => tr!("API Key · 明文保存在 {}", "API key · stored in plain text in {}", self.name()),
            }
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
            enabled,
            compatible: true,
            models,
            details: vec![
                Kv::mono(lbl::config_id(), format!("provider.{id}")),
                Kv::mono("npm", if npm.is_empty() { "-".into() } else { npm.to_string() }),
                Kv::text(lbl::api_key(), key_note),
                Kv::text(lbl::status(), if enabled { l("已启用", "Enabled") } else { parked }),
            ],
            editable: true,
            api: api.into(),
            has_key: in_cfg.is_some() || in_auth,
            ..Default::default()
        }
    }

    /// Custom providers in the config (enabled, natively disabled, or stashed).
    pub fn providers(&self, cfg: &Value, root: &Value) -> Vec<Provider> {
        self.providers_with(cfg, root, &self.disabled(cfg))
    }

    /// `providers`, with `off` as the disabled list (a project's effective one).
    pub fn providers_with(&self, cfg: &Value, root: &Value, off: &[String]) -> Vec<Provider> {
        let hidden = store::get_obj(root, &self.agent, "hiddenModels");
        let auth = self.load_auth().map(|x| x.0);
        let mut out = vec![];
        if let Some(p) = cfg.get("provider").and_then(|x| x.as_object()) {
            for (id, def) in p {
                out.push(self.provider_from(id, def, !off.contains(id), &hidden, auth.as_ref()));
            }
        }
        if !self.native_disable {
            for (id, def) in &store::get_obj(root, &self.agent, "disabledProviders") {
                out.push(self.provider_from(id, def, false, &hidden, auth.as_ref()));
            }
        }
        out
    }

    /// Base URL, key and API kind of a provider.
    pub fn endpoint(&self, id: &str) -> Result<Endpoint> {
        let (cfg, _, _) = self.load(true)?;
        // Only a config without a native disabled list parks providers in the store.
        let parked = || if self.native_disable { None } else { store::get_obj(&store::load(), &self.agent, "disabledProviders").get(id).cloned() };
        let def = cfg.pointer(&jptr(&["provider", id])).cloned().or_else(parked).ok_or_else(|| msg::no_provider(id))?;
        let base = def.pointer("/options/baseURL").and_then(|x| x.as_str()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 baseURL", "Provider {id} has no baseURL")))?.to_string();
        let auth = self.load_auth().map(|x| x.0);
        let key = cfg_key(&def).and_then(|k| self.resolve_key(k)).or_else(|| Self::auth_key(auth.as_ref(), id).map(String::from));
        Ok((base, key, api_of(def.get("npm").and_then(|x| x.as_str()).unwrap_or("")).into()))
    }

    /// Read-only cards for the auth.json logins without a config entry (see `auth_cards`).
    pub fn auth_only(&self, known: &[Provider], login: &str, about: &str, off: &[String]) -> Vec<Provider> {
        auth_cards(self.load_auth().as_ref().map(|a| &a.0), known, login, about, off)
    }

    /// Loads the config for `state()`: comments make the agent read-only (with a note), an
    /// error fails the state and gives None. `allow_missing` as in `load`.
    pub fn load_for_state(&self, st: &mut AgentState, allow_missing: bool) -> Option<Value> {
        match self.load(allow_missing) {
            Ok((cfg, _, had_comments)) => {
                if had_comments {
                    st.readonly = true;
                    st.notes.push(msg::comments_readonly(&self.name()));
                }
                Some(cfg)
            }
            Err(e) => {
                st.fail(e);
                None
            }
        }
    }

    /// The "current" rows of a global config: custom providers, default and small model,
    /// visible models, config file.
    pub fn summary(&self, cfg: &Value, providers: &[Provider]) -> Vec<Kv> {
        let get_s = |k: &str| cfg.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string();
        let on: Vec<&Provider> = providers.iter().filter(|p| p.enabled && !p.builtin).collect();
        let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
        vec![
            Kv::text(lbl::custom_providers(), lbl::names_or_none(on.iter().map(|p| &p.name))),
            Kv::mono(lbl::default_model(), get_s("model")),
            Kv::mono(lbl::small_model(), get_s("small_model")),
            Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")),
            Kv::mono(lbl::config_file(), self.file()),
        ]
    }

    /// A config with comments is never written back (they would be lost).
    pub fn guard_comments(&self, dirty: &Dirty, had_comments: bool) -> Result<()> {
        if dirty.cfg && had_comments {
            return Err(msg::comments_not_written(&self.name()));
        }
        Ok(())
    }

    /// Writes what `dirty` says changed: backs up the config and auth.json (with `backup`),
    /// then writes them. Returns (written files, backup folder); nothing on a dry run.
    /// The caller saves the store.
    pub fn commit(&self, cfg: &Value, meta: TextMeta, auth: &Option<(Value, TextMeta)>, dirty: &Dirty, dry_run: bool, backup: impl FnOnce(&[PathBuf]) -> Result<PathBuf>) -> Result<(Vec<PathBuf>, Option<PathBuf>)> {
        let auth_out = self.auth.as_ref().zip(auth.as_ref()).filter(|_| dirty.auth);
        if dry_run || (!dirty.cfg && auth_out.is_none()) {
            return Ok((vec![], None));
        }
        let mut targets = vec![];
        if dirty.cfg {
            targets.push(self.path.clone());
        }
        if let Some((p, _)) = auth_out {
            targets.push(p.clone());
        }
        let backup_dir = backup(&targets)?;
        if dirty.cfg {
            if let Some(d) = self.path.parent() {
                std::fs::create_dir_all(d)?;
            }
            write_json(&self.path, cfg, meta)?;
        }
        if let Some((p, (a, _))) = auth_out {
            write_auth(p, a)?;
        }
        Ok((targets, Some(backup_dir)))
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
        if cfg.pointer(&jptr(&["provider", pid, "models", mid])).is_some() {
            return cfg.pointer_mut(&jptr(&["provider", pid, "models", mid]));
        }
        store::section(root, &self.agent, "hiddenModels").get_mut(&format!("{pid}|{mid}"))
    }

    fn set_key(&self, cfg: &mut Value, auth: &mut Option<(Value, TextMeta)>, id: &str, key: &str, diff: &mut Diff, dirty: &mut Dirty) -> Result<()> {
        // Keep the key where the user already keeps it; new ones go to auth.json when there is one.
        let in_cfg = cfg.pointer(&jptr(&["provider", id])).and_then(cfg_key).is_some();
        // The agent keeps keys in auth.json but it can't be read: never fall back to the config
        // (a project's opencode.json is usually committed).
        if let (Some(p), None, false) = (&self.auth, auth.as_ref(), in_cfg) {
            return Err(anyhow!(tr!("{} 无法读取，没有写入密钥", "{} can't be read, so the API key was not written", display_path(p))));
        }
        if let (Some((a, _)), false) = (auth.as_mut(), in_cfg) {
            a[id] = json!({ "type": "api", "key": key });
            diff.push(&display_path(self.auth.as_ref().unwrap()), format!("{id}.key = {}", mask_key(key)), true);
            dirty.auth = true;
        } else if let Some(def) = cfg.pointer_mut(&jptr(&["provider", id])) {
            if !def.get("options").map(|o| o.is_object()).unwrap_or(false) {
                def["options"] = json!({});
            }
            def["options"]["apiKey"] = json!(key);
            diff.push(&self.file(), format!("provider.{id}.options.apiKey = {}", mask_key(key)), true);
            dirty.cfg = true;
        }
        Ok(())
    }

    /// Applies one provider / model op. Returns false for ops this module does not handle.
    pub fn apply(&self, op: &Op, cfg: &mut Value, root: &mut Value, auth: &mut Option<(Value, TextMeta)>, diff: &mut Diff, dirty: &mut Dirty) -> Result<bool> {
        let ef = self.file();
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                match &p.id {
                    None => {
                        let parked = store::get_obj(root, &self.agent, "disabledProviders");
                        // An id already in auth.json (an OAuth login, a built-in provider's key)
                        // would have that entry overwritten by the new provider's key.
                        let in_auth = |c: &str| auth.as_ref().is_some_and(|(a, _)| a.get(c).is_some());
                        let providers = self.providers_obj(cfg)?;
                        let id = unique_id(&slug(&p.name), |c| providers.contains_key(c) || parked.contains_key(c) || self.reserved.iter().any(|x| x == c) || in_auth(c));
                        let models: Map<String, Value> = clean_ids(&p.models).into_iter().map(|m| (m, json!({}))).collect();
                        let n = models.len();
                        // The label of what gets written (an api OpenCode lacks falls back to Chat).
                        let label = api_label(api_of(npm_for(&p.api)));
                        providers.insert(id.clone(), json!({
                            "npm": npm_for(&p.api),
                            "name": p.name.trim(),
                            "options": { "baseURL": p.base_url.trim() },
                            "models": models,
                        }));
                        diff.push(
                            &ef,
                            trn!(n, "+ provider.{id}（{} · {label} · {n} 个模型）", "+ provider.{id} ({} · {label} · {n} model)", "+ provider.{id} ({} · {label} · {n} models)", p.base_url.trim()),
                            true,
                        );
                        dirty.cfg = true;
                        if let Some(k) = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                            self.set_key(cfg, auth, &id, k, diff, dirty)?;
                        }
                    }
                    Some(id) => {
                        let in_cfg = cfg.pointer(&jptr(&["provider", id])).is_some();
                        let def = if in_cfg {
                            cfg.pointer_mut(&jptr(&["provider", id])).unwrap()
                        } else {
                            store::section(root, &self.agent, "disabledProviders").get_mut(id).ok_or_else(|| msg::no_provider(id))?
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
                                self.set_key(cfg, auth, id, k, diff, dirty)?;
                            } else if let Some(def) = store::section(root, &self.agent, "disabledProviders").get_mut(id) {
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
                let removed_stash = store::section(root, &self.agent, "disabledProviders").remove(provider).is_some();
                let prefix = format!("{provider}|");
                store::section(root, &self.agent, "hiddenModels").retain(|k, _| !k.starts_with(&prefix));
                if !removed_cfg && !removed_stash {
                    return Err(msg::no_provider(provider));
                }
                if self.native_disable {
                    if let Some(list) = cfg.get_mut("disabled_providers").and_then(|x| x.as_array_mut()) {
                        list.retain(|x| x.as_str() != Some(provider.as_str()));
                    }
                }
                // The auth.json entry stays (it may be a login OpenCode uses without a config entry).
                let line = if Self::auth_key(auth.as_ref().map(|a| &a.0), provider).is_some() {
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
                    let parked = store::section(root, &self.agent, "disabledProviders");
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
                    .pointer_mut(&jptr(&["provider", provider]))
                    .and_then(|p| p.as_object_mut())
                    .ok_or_else(|| anyhow!(tr!("供应商 {provider} 未启用，先启用再调整模型", "Provider {provider} is disabled; enable it before changing its models")))?
                    .entry("models")
                    .or_insert_with(|| json!({}));
                let models = models.as_object_mut().ok_or_else(|| anyhow!(l("models 不是对象", "models is not an object")))?;
                let hidden = store::section(root, &self.agent, "hiddenModels");
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
                    return Err(msg::model_id_required());
                }
                for (k, v) in &m.extra {
                    mfields::check(mfields::OPENCODE, k, v)?;
                }
                if self.model_def_mut(cfg, root, provider, &mid).is_none() {
                    let models = cfg
                        .pointer_mut(&jptr(&["provider", provider]))
                        .and_then(|p| p.as_object_mut())
                        .ok_or_else(|| anyhow!(tr!("供应商 {provider} 未启用，先启用再添加模型", "Provider {provider} is disabled; enable it before adding models")))?
                        .entry("models")
                        .or_insert_with(|| json!({}));
                    models.as_object_mut().ok_or_else(|| anyhow!(l("models 不是对象", "models is not an object")))?.insert(mid.clone(), json!({}));
                    diff.push(&ef, format!("provider.{provider}.models + \"{mid}\""), true);
                    dirty.cfg = true;
                }
                let in_cfg = cfg.pointer(&jptr(&["provider", provider, "models", &mid])).is_some();
                let def = self.model_def_mut(cfg, root, provider, &mid).ok_or_else(|| anyhow!(tr!("找不到模型 {mid}", "Model not found: {mid}")))?;
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
                    .pointer_mut(&jptr(&["provider", provider, "models"]))
                    .and_then(|m| m.as_object_mut())
                    .and_then(|m| m.remove(model))
                    .is_some();
                let stashed = store::section(root, &self.agent, "hiddenModels").remove(&format!("{provider}|{model}")).is_some();
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

    fn write_cfg(h: &TestHome, cfg: Value) -> Fmt {
        let path = h.0.join("cfg").join("opencode.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, cfg.to_string()).unwrap();
        Fmt::new("opencode", path, Some(h.0.join("auth.json")), true)
    }

    #[test]
    fn key_references_are_resolved_not_sent_literally() {
        let h = TestHome::new("ocfmt-keyref");
        crate::env::set_test_vars(&[("OC_TEST_KEY", "sk-from-env-1111")]);
        std::fs::write(h.0.join("cfg-key.txt"), "sk-from-file-2222\n").unwrap();
        let prov = |key: &str| json!({ "npm": "@ai-sdk/openai-compatible", "options": { "baseURL": "https://r.example.com/v1", "apiKey": key } });
        let f = write_cfg(&h, json!({ "provider": {
            "env": prov("{env:OC_TEST_KEY}"),
            "unset": prov("{env:OC_NOT_SET}"),
            "rel": prov("{file:../cfg-key.txt}"),
            "home": prov("{file:~/cfg-key.txt}"),
            "plain": prov("sk-plain-3333"),
        }}));
        let key = |id: &str| f.endpoint(id).unwrap().1;
        assert_eq!(key("env").as_deref(), Some("sk-from-env-1111"));
        assert_eq!(key("unset"), None);
        assert_eq!(key("rel").as_deref(), Some("sk-from-file-2222"));
        assert_eq!(key("home").as_deref(), Some("sk-from-file-2222"));
        assert_eq!(key("plain").as_deref(), Some("sk-plain-3333"));
        let (cfg, _, _) = f.load(true).unwrap();
        let ps = f.providers(&cfg, &json!({}));
        let note = |id: &str| ps.iter().find(|p| p.id == id).unwrap().details.iter().find(|kv| kv.k == lbl::api_key()).unwrap().v.clone();
        assert_eq!(note("env"), "API Key · 环境变量 OC_TEST_KEY");
        assert_eq!(note("rel"), "API Key · 读取文件 ../cfg-key.txt");
        assert_eq!(note("plain"), "API Key · 明文保存在 opencode.json");
        assert!(ps.iter().all(|p| p.has_key));
    }

    #[test]
    fn delete_says_where_the_key_was() {
        let h = TestHome::new("ocfmt-delete");
        std::fs::write(h.0.join("auth.json"), r#"{"a":{"type":"api","key":"sk-a"}}"#).unwrap();
        let prov = json!({ "options": { "baseURL": "https://r.example.com/v1" } });
        let f = write_cfg(&h, json!({ "provider": { "a": prov, "b": prov } }));
        let (mut cfg, _, _) = f.load(true).unwrap();
        let mut auth = f.load_auth();
        let mut diff = Diff::default();
        for id in ["a", "b"] {
            f.apply(&Op::DeleteProvider { provider: id.into() }, &mut cfg, &mut json!({}), &mut auth, &mut diff, &mut Dirty::default()).unwrap();
        }
        let lines: Vec<&str> = diff.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.as_str())).collect();
        assert_eq!(lines, ["- provider.a（含它的模型；auth.json 里的密钥保留）", "- provider.b（含它的模型和密钥）"]);
    }

    #[test]
    fn auth_cards_skip_known_and_honor_disabled() {
        let auth = json!({ "known": { "type": "api", "key": "k" }, "anthropic": { "type": "oauth" }, "openai": { "type": "api", "key": "k" } });
        let known = vec![Provider { id: "known".into(), ..Default::default() }];
        let cards = auth_cards(Some(&auth), &known, "opencode auth", "about", &["openai".into()]);
        let got: Vec<(&str, &str, bool)> = cards.iter().map(|p| (p.id.as_str(), p.host.as_str(), p.enabled)).collect();
        assert_eq!(got, [("anthropic", "账号登录（opencode auth）", true), ("openai", "内置供应商 · API Key", false)]);
        assert!(cards.iter().all(|p| p.builtin && !p.editable));
    }
}
