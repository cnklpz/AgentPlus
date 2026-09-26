//! Providers in the pi-ai `models.json` shape, shared by pi (`~/.pi/agent/models.json`,
//! `providers.<id>`) and OpenClaw (`openclaw.json`, `models.providers.<id>`):
//! `{ baseUrl, api, apiKey, headers, …, models: [{ id, name, contextWindow, … }] }`.
//!
//! Neither agent can switch a provider or a model off, so a hidden model / disabled provider
//! is removed from the file and its definition stashed in the AgentPlus store
//! (`hiddenModels["<provider>|<model>"]`, `disabledProviders["<provider>"]`), to come back
//! untouched. Unknown fields are always kept; only documented fields are ever added.


use super::msg;
use super::Endpoint;
use crate::model::*;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flavor {
    /// pi: provider `name` field, `!cmd` / `$VAR` / `${VAR}` keys, keys may live in auth.json.
    Pi,
    /// OpenClaw: no provider `name` (strict schema), model `name` required, `${VAR}` / SecretRef keys.
    OpenClaw,
}

pub struct Fmt {
    pub agent: &'static str,
    pub flavor: Flavor,
    /// File that holds the providers.
    pub path: PathBuf,
    /// JSON pointer of the providers object in that file ("/providers" or "/models/providers").
    pub ptr: &'static str,
    /// pi's credential store (`~/.pi/agent/auth.json`).
    pub auth: Option<PathBuf>,
    /// Extra `KEY=VALUE` file consulted for `${VAR}` keys (OpenClaw's `~/.openclaw/.env`).
    pub env_file: Option<PathBuf>,
}

#[derive(Default)]
pub struct Dirty {
    pub cfg: bool,
    pub store: bool,
    pub auth: bool,
}

/// Provider ids pi-ai ships with. A config entry with one of these ids only overrides the
/// built-in, so removing it does not make "provider/model" references dangle.
pub const BUILTIN_PROVIDERS: [&str; 24] = [
    "anthropic", "openai", "google", "google-vertex", "google-gemini-cli", "google-antigravity", "amazon-bedrock",
    "azure-openai-responses", "openai-codex", "github-copilot", "mistral", "groq", "cerebras", "xai", "openrouter",
    "vercel-ai-gateway", "zai", "minimax", "minimax-cn", "huggingface", "kimi-coding", "opencode", "deepseek", "moonshot",
];

/// (pi-ai `api` value, AgentPlus api) for the protocols AgentPlus can speak.
const APIS: [(&str, &str); 4] = [
    ("openai-completions", "chat"),
    ("openai-responses", "responses"),
    ("anthropic-messages", "anthropic"),
    ("google-generative-ai", "gemini"),
];

/// The AgentPlus api of a pi-ai `api` value; None for protocols AgentPlus cannot speak.
fn api_of(raw: &str) -> Option<&'static str> {
    APIS.iter().find(|a| a.0 == raw).map(|a| a.1)
}

/// The pi-ai `api` value for an AgentPlus api.
fn raw_for(api: &str) -> Option<&'static str> {
    APIS.iter().find(|a| a.1 == api).map(|a| a.0)
}

/// The AgentPlus api of a pi-ai `api` value, or the value itself for other protocols.
fn family(raw: &str) -> String {
    api_of(raw).map(String::from).unwrap_or_else(|| raw.to_string())
}

/// The provider's `api`: its own field, else its first model's, else the built-in's protocol.
pub fn raw_api(id: &str, def: &Value) -> String {
    if let Some(a) = def.get("api").and_then(|x| x.as_str()) {
        return a.to_string();
    }
    if let Some(a) = def.get("models").and_then(|m| m.as_array()).and_then(|a| a.iter().find_map(|m| m.get("api").and_then(|x| x.as_str()))) {
        return a.to_string();
    }
    match id {
        "anthropic" => "anthropic-messages",
        "openai" => "openai-responses",
        "google" => "google-generative-ai",
        _ => "openai-completions",
    }
    .to_string()
}

fn is_env_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.chars().next().unwrap().is_ascii_digit()
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

/// OpenClaw's `${VAR}` reference.
fn env_ref_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}").unwrap())
}

/// Where a config `apiKey` string takes the key from.
enum KeySrc<'a> {
    /// pi's `!command`, which AgentPlus never runs.
    Cmd,
    /// One environment variable.
    Env(&'a str),
    /// OpenClaw: text with `${VAR}` references inside.
    Template,
    /// The key itself.
    Literal,
}

impl Fmt {
    pub fn file(&self) -> String {
        display_path(&self.path)
    }

    fn cfg_prefix(&self) -> &'static str {
        match self.flavor {
            Flavor::Pi => "providers",
            Flavor::OpenClaw => "models.providers",
        }
    }

    fn lookup(&self, name: &str) -> Option<String> {
        crate::env::agent_var(name).or_else(|| self.env_file.as_deref().and_then(|p| crate::dotenv::get(&crate::dotenv::load(p).0, name)))
    }

    /// How a (trimmed, non-empty) `apiKey` string is read. `resolve` and `key_desc` both go
    /// through this so the detail panel describes the key that is actually used.
    fn key_source<'a>(&self, raw: &'a str) -> KeySrc<'a> {
        match self.flavor {
            Flavor::Pi => {
                if raw.starts_with('!') {
                    KeySrc::Cmd
                } else if let Some(n) = raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| raw.strip_prefix('$')) {
                    KeySrc::Env(n)
                } else if is_env_name(raw) && self.lookup(raw).is_some() {
                    // pi also treats a bare string as an env var name when that variable exists.
                    KeySrc::Env(raw)
                } else {
                    KeySrc::Literal
                }
            }
            Flavor::OpenClaw => match env_ref_re().captures(raw) {
                Some(c) if c.get(0).map(|m| m.len()) == Some(raw.len()) => KeySrc::Env(c.get(1).unwrap().as_str()),
                Some(_) => KeySrc::Template,
                None => KeySrc::Literal,
            },
        }
    }

    /// The real key behind a config value, when it can be known without running anything.
    pub fn resolve(&self, v: &Value) -> Option<String> {
        let raw = v.as_str()?.trim();
        if raw.is_empty() {
            return None;
        }
        match self.key_source(raw) {
            KeySrc::Cmd => None,
            KeySrc::Env(n) => self.lookup(n),
            KeySrc::Literal => Some(raw.to_string()),
            KeySrc::Template => {
                let mut missing = false;
                let out = env_ref_re().replace_all(raw, |c: &regex::Captures| self.lookup(&c[1]).unwrap_or_else(|| {
                    missing = true;
                    String::new()
                }));
                (!missing).then(|| out.to_string()).filter(|k| !k.is_empty())
            }
        }
    }

    /// Where the key comes from, for the detail panel (never the key itself).
    fn key_desc(&self, v: Option<&Value>, in_auth: bool) -> String {
        let fname = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let cfg = match v {
            Some(Value::Object(o)) => Some(tr!("SecretRef（{}）", "SecretRef ({})", o.get("source").and_then(|x| x.as_str()).unwrap_or("?"))),
            Some(Value::String(k)) if !k.trim().is_empty() => Some(match self.key_source(k.trim()) {
                KeySrc::Cmd => l("shell 命令（AgentPlus 不执行）", "Shell command (AgentPlus doesn't run it)").to_string(),
                KeySrc::Env(n) => tr!("环境变量 {n}{}", "Environment variable {n}{}", if self.lookup(n).is_some() { "" } else { l("（当前未设置）", " (not set now)") }),
                KeySrc::Template => l("含环境变量引用", "Contains environment variable references").to_string(),
                KeySrc::Literal => tr!("明文保存在 {fname}", "Stored in plain text in {fname}"),
            }),
            _ => None,
        };
        match (in_auth, cfg) {
            (true, Some(c)) => tr!("auth.json（优先）；{fname} 里另有：{c}", "auth.json (takes precedence); {fname} also has: {c}"),
            (true, None) => l("保存在 auth.json", "Stored in auth.json").into(),
            (false, Some(c)) => c,
            (false, None) => l("未填写", "Not set").into(),
        }
    }

    pub fn load_auth(&self) -> Option<(Value, TextMeta)> {
        let p = self.auth.as_ref()?;
        if !p.exists() {
            return Some((json!({}), TextMeta::NEW));
        }
        read_json(p).ok()
    }

    pub fn auth_key<'a>(auth: Option<&'a Value>, id: &str) -> Option<&'a Value> {
        auth?.get(id).filter(|e| s(e, "type") == Some("api_key")).and_then(|e| e.get("key")).filter(|k| k.as_str().map(|k| !k.is_empty()).unwrap_or(false))
    }

    /// A map this agent keeps in the AgentPlus store, borrowed rather than copied.
    fn stash<'a>(&self, root: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
        store::agent_get(root, self.agent, key).and_then(|x| x.as_object())
    }

    /// Definitions of hidden models, keyed "<provider>|<model>".
    fn hidden<'a>(&self, root: &'a Value) -> Option<&'a Map<String, Value>> {
        self.stash(root, "hiddenModels")
    }

    /// The definition of a disabled provider, parked in the store.
    fn parked<'a>(&self, root: &'a Value, id: &str) -> Option<&'a Value> {
        self.stash(root, "disabledProviders")?.get(id)
    }

    /// Display name: pi keeps it in the config, OpenClaw's schema has no room so it lives in the store.
    fn display_name(&self, id: &str, def: &Value, root: &Value) -> String {
        match self.flavor {
            Flavor::Pi => s(def, "name").filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
            Flavor::OpenClaw => self.stash(root, "names").and_then(|n| n.get(id)).and_then(|x| x.as_str()).unwrap_or(id).to_string(),
        }
    }

    pub fn providers_of<'a>(&self, cfg: &'a Value) -> Option<&'a Map<String, Value>> {
        cfg.pointer(self.ptr)?.as_object()
    }

    pub fn providers_mut<'a>(&self, cfg: &'a mut Value) -> Result<&'a mut Map<String, Value>> {
        let mut cur = cfg;
        for seg in self.ptr.trim_start_matches('/').split('/') {
            cur = cur
                .as_object_mut()
                .ok_or_else(|| anyhow!(tr!("{} 的结构不对：{seg} 的上一级不是对象", "{} has an unexpected structure: the parent of {seg} is not an object", self.file())))?
                .entry(seg)
                .or_insert_with(|| json!({}));
        }
        cur.as_object_mut().ok_or_else(|| anyhow!(tr!("{} 不是对象", "{} is not an object", self.cfg_prefix())))
    }

    /// Per-model fields this flavor's schema has.
    fn specs(&self) -> &'static [crate::mfields::Spec] {
        match self.flavor {
            Flavor::Pi => crate::mfields::PI,
            Flavor::OpenClaw => crate::mfields::OPENCLAW,
        }
    }

    fn model_from(&self, def: &Value, visible: bool) -> Option<Model> {
        let id = s(def, "id")?.to_string();
        let context = def.get("contextWindow").and_then(|x| x.as_u64());
        let mut tags = vec![];
        if def.get("reasoning").and_then(|x| x.as_bool()) == Some(true) {
            tags.push(Tag::new("cap:reasoning", l("推理", "Reasoning")));
        }
        if def.get("input").and_then(|x| x.as_array()).map(|a| a.iter().any(|i| i.as_str() == Some("image"))).unwrap_or(false) {
            tags.push(Tag::new("cap:image", l("图片", "Images")));
        }
        Some(Model {
            id,
            visible,
            readonly: false,
            tags,
            ctx: context.map(fmt_ctx),
            name: s(def, "name").map(String::from),
            context,
            deletable: true,
            extra: crate::mfields::read(def, self.specs()),
        })
    }

    fn provider_from(&self, id: &str, def: &Value, enabled: bool, root: &Value, hidden: Option<&Map<String, Value>>, auth: Option<&Value>) -> Provider {
        let base = s(def, "baseUrl").map(String::from).filter(|b| !b.is_empty());
        let raw = raw_api(id, def);
        let known = api_of(&raw);
        let mut models: Vec<Model> = def.get("models").and_then(|m| m.as_array()).map(|a| a.iter().filter_map(|m| self.model_from(m, true)).collect()).unwrap_or_default();
        let prefix = format!("{id}|");
        for (k, d) in hidden.into_iter().flatten() {
            if k.starts_with(&prefix) {
                if let Some(m) = self.model_from(d, false) {
                    models.push(m);
                }
            }
        }
        let key_v = def.get("apiKey");
        let in_cfg = match key_v {
            Some(Value::String(k)) => !k.trim().is_empty(),
            Some(Value::Object(_)) => true,
            _ => false,
        };
        let in_auth = Self::auth_key(auth, id).is_some();
        let mut details = vec![
            Kv::mono(lbl::config_id(), format!("{}.{id}", self.cfg_prefix())),
            Kv::mono("api", raw.clone()),
            Kv::text(lbl::api_key(), self.key_desc(key_v, in_auth)),
            Kv::text(lbl::status(), if enabled { l("已启用", "Enabled") } else { l("已停用 · 定义暂存在 AgentPlus", "Disabled · definition kept in AgentPlus") }),
        ];
        if BUILTIN_PROVIDERS.contains(&id) {
            details.push(Kv::text(lbl::note(), l("与内置供应商同名：这里的设置会合并到内置供应商上", "Same id as a built-in provider: these settings are merged into the built-in one")));
        }
        if let Some(h) = def.get("headers").and_then(|x| x.as_object()).filter(|h| !h.is_empty()) {
            details.push(Kv::mono("headers", h.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        if let Some(n) = def.get("modelOverrides").and_then(|x| x.as_object()).filter(|o| !o.is_empty()) {
            details.push(Kv::text("modelOverrides", tr!("{} 个（原样保留）", "{} (kept as is)", n.len())));
        }
        Provider {
            id: id.into(),
            name: self.display_name(id, def, root),
            host: base.as_deref().map(host_of).unwrap_or_default(),
            base_url: base,
            apis: vec![known.map(|k| api_label(k).to_string()).unwrap_or_else(|| raw.clone())],
            enabled,
            compatible: known.is_some(),
            reason: known.is_none().then(|| tr!("{raw} 协议，AgentPlus 不能测速或转接", "{raw} protocol: AgentPlus can't test speed or relay it")),
            models,
            details,
            editable: true,
            api: family(&raw),
            has_key: in_cfg || in_auth,
            ..Default::default()
        }
    }

    /// Configured providers (enabled) plus the ones parked in the store (disabled).
    pub fn providers(&self, cfg: &Value, root: &Value, auth: Option<&Value>) -> Vec<Provider> {
        let mut out = vec![];
        let hidden = self.hidden(root);
        if let Some(p) = self.providers_of(cfg) {
            for (id, def) in p {
                out.push(self.provider_from(id, def, true, root, hidden, auth));
            }
        }
        for (id, def) in self.stash(root, "disabledProviders").into_iter().flatten() {
            if !out.iter().any(|p| &p.id == id) {
                out.push(self.provider_from(id, def, false, root, hidden, auth));
            }
        }
        out
    }

    /// The state summary: custom providers, the default model, `extra`, visible models and
    /// the config file.
    pub fn summary(&self, providers: &[Provider], default_model: String, extra: Option<Kv>) -> Vec<Kv> {
        let on: Vec<&Provider> = providers.iter().filter(|p| p.enabled && !p.builtin).collect();
        let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
        let mut rows = vec![
            Kv::text(lbl::custom_providers(), lbl::names_or_none(on.iter().map(|p| &p.name))),
            Kv::mono(lbl::default_model(), default_model),
        ];
        rows.extend(extra);
        rows.push(Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")));
        rows.push(Kv::mono(lbl::config_file(), self.file()));
        rows
    }

    /// Base URL, key and api of a provider (auth.json wins over the config, as in pi).
    pub fn endpoint(&self, id: &str, cfg: &Value, root: &Value) -> Result<Endpoint> {
        let def = self.providers_of(cfg).and_then(|p| p.get(id)).or_else(|| self.parked(root, id)).cloned().ok_or_else(|| msg::no_provider(id))?;
        let base = s(&def, "baseUrl").filter(|b| !b.is_empty()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 baseUrl（沿用内置地址）", "Provider {id} has no baseUrl (uses the built-in URL)")))?.to_string();
        let auth = self.load_auth().map(|a| a.0);
        let key = Self::auth_key(auth.as_ref(), id).and_then(|k| self.resolve(k)).or_else(|| def.get("apiKey").and_then(|k| self.resolve(k)));
        let raw = raw_api(id, &def);
        Ok((base, key, family(&raw)))
    }

    /// Whether `provider/model` is still defined in the config (built-in providers always count).
    pub fn has_model(&self, cfg: &Value, provider: &str, model: Option<&str>) -> bool {
        let Some(def) = self.providers_of(cfg).and_then(|p| p.get(provider)) else {
            return BUILTIN_PROVIDERS.contains(&provider);
        };
        match model {
            None => true,
            Some(m) => {
                BUILTIN_PROVIDERS.contains(&provider)
                    || def.get("models").and_then(|x| x.as_array()).map(|a| a.iter().any(|d| s(d, "id") == Some(m))).unwrap_or(false)
            }
        }
    }

    /// Replaces the key of `id`'s API-key entry in pi's auth.json. False when it has none.
    fn set_auth_key(&self, auth: &mut Option<(Value, TextMeta)>, id: &str, key: &str, diff: &mut Diff, dirty: &mut Dirty) -> bool {
        if Self::auth_key(auth.as_ref().map(|a| &a.0), id).is_none() {
            return false;
        }
        let (Some((a, _)), Some(path)) = (auth.as_mut(), self.auth.as_ref()) else { return false };
        a[id]["key"] = json!(key);
        diff.push(&display_path(path), format!("{id}.key = {}", mask_key(key)), true);
        dirty.auth = true;
        true
    }

    fn set_key(&self, cfg: &mut Value, auth: &mut Option<(Value, TextMeta)>, id: &str, key: &str, diff: &mut Diff, dirty: &mut Dirty) -> Result<()> {
        // Keep the key where it already is: pi's auth.json entry, else inline apiKey.
        if !self.set_auth_key(auth, id, key, diff, dirty) {
            let prefix = self.cfg_prefix();
            let def = self.providers_mut(cfg)?.get_mut(id).ok_or_else(|| msg::no_provider(id))?;
            def["apiKey"] = json!(key);
            diff.push(&self.file(), format!("{prefix}.{id}.apiKey = {}", mask_key(key)), true);
            dirty.cfg = true;
        }
        Ok(())
    }

    fn new_model(&self, id: &str, name: Option<&str>, context: Option<u64>) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(id));
        match (self.flavor, name) {
            (_, Some(n)) => {
                m.insert("name".into(), json!(n));
            }
            // OpenClaw requires a name.
            (Flavor::OpenClaw, None) => {
                m.insert("name".into(), json!(id));
            }
            (Flavor::Pi, None) => {}
        }
        if let Some(c) = context {
            m.insert("contextWindow".into(), json!(c));
        }
        Value::Object(m)
    }

    fn models_mut<'a>(&self, cfg: &'a mut Value, provider: &str, what: &str) -> Result<&'a mut Vec<Value>> {
        let def = self
            .providers_mut(cfg)?
            .get_mut(provider)
            .and_then(|p| p.as_object_mut())
            .ok_or_else(|| anyhow!(tr!("供应商 {provider} 未启用，先启用再{what}", "Provider {provider} is disabled. Enable it before {what}.")))?;
        let m = def.entry("models").or_insert_with(|| json!([]));
        if !m.is_array() {
            return Err(anyhow!(tr!("{provider}.models 不是数组", "{provider}.models is not an array")));
        }
        Ok(m.as_array_mut().unwrap())
    }

    /// Applies one provider / model op. Returns false for ops this module does not handle.
    pub fn apply(&self, op: &Op, cfg: &mut Value, root: &mut Value, auth: &mut Option<(Value, TextMeta)>, diff: &mut Diff, dirty: &mut Dirty) -> Result<bool> {
        let ef = self.file();
        let pre = self.cfg_prefix();
        let agent = self.agent;
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                let name = p.name.trim();
                let base_url = p.base_url.trim();
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
                match &p.id {
                    None => {
                        let raw = raw_for(&p.api).ok_or_else(|| anyhow!(tr!("不支持的协议 {}", "Unsupported protocol: {}", p.api)))?;
                        let providers = self.providers_mut(cfg)?;
                        let id = unique_id(&slug(name), |c| providers.contains_key(c) || self.parked(root, c).is_some() || BUILTIN_PROVIDERS.contains(&c));
                        let ids = clean_ids(&p.models);
                        let models: Vec<Value> = ids.iter().map(|m| self.new_model(m, None, None)).collect();
                        let mut def = Map::new();
                        if self.flavor == Flavor::Pi {
                            def.insert("name".into(), json!(name));
                        }
                        def.insert("baseUrl".into(), json!(base_url));
                        def.insert("api".into(), json!(raw));
                        def.insert("models".into(), Value::Array(models));
                        providers.insert(id.clone(), Value::Object(def));
                        diff.push(&ef, trn!(ids.len(), "+ {pre}.{id}（{base_url} · {raw} · {n} 个模型）", "+ {pre}.{id} ({base_url} · {raw} · {n} model)", "+ {pre}.{id} ({base_url} · {raw} · {n} models)"), true);
                        dirty.cfg = true;
                        if self.flavor == Flavor::OpenClaw && name != id {
                            store::section(root, agent, "names").insert(id.clone(), json!(name));
                            dirty.store = true;
                        }
                        match key {
                            Some(k) => self.set_key(cfg, auth, &id, k, diff, dirty)?,
                            None if self.flavor == Flavor::Pi => diff.push(&ef, tr!("（{id} 没填密钥：pi 里它的模型用不了，可以填 $环境变量名）", "({id} has no API key: its models won't work in pi. You can enter $ENV_VAR_NAME.)"), false),
                            None => {}
                        }
                        for op in crate::modelinfo::seed_ops(agent, &id, &ids) {
                            self.apply(&op, cfg, root, auth, diff, dirty)?;
                        }
                    }
                    Some(id) => {
                        let in_cfg = self.providers_of(cfg).map(|p| p.contains_key(id)).unwrap_or(false);
                        let flavor = self.flavor;
                        let cur_name = self.display_name(id, self.providers_of(cfg).and_then(|p| p.get(id)).or_else(|| self.parked(root, id)).unwrap_or(&Value::Null), root);
                        if flavor == Flavor::OpenClaw && cur_name != name {
                            let names = store::section(root, agent, "names");
                            if name == id { names.remove(id) } else { names.insert(id.clone(), json!(name)) };
                            diff.push(l("AgentPlus · 显示名称", "AgentPlus · display names"), tr!("{id} → 「{name}」", "{id} → \"{name}\""), true);
                            dirty.store = true;
                        }
                        let def = if in_cfg {
                            self.providers_mut(cfg)?.get_mut(id).unwrap()
                        } else {
                            store::section(root, agent, "disabledProviders").get_mut(id).ok_or_else(|| msg::no_provider(id))?
                        };
                        if !def.is_object() {
                            return Err(anyhow!(tr!("{pre}.{id} 不是对象", "{pre}.{id} is not an object")));
                        }
                        let mut changed = vec![];
                        if flavor == Flavor::Pi && s(def, "name").unwrap_or(id) != name {
                            def["name"] = json!(name);
                            changed.push(format!("name = \"{name}\""));
                        }
                        if s(def, "baseUrl") != Some(base_url) {
                            def["baseUrl"] = json!(base_url);
                            changed.push(format!("baseUrl = \"{base_url}\""));
                        }
                        let raw_now = raw_api(id, def);
                        if p.api != family(&raw_now) {
                            let raw = raw_for(&p.api).ok_or_else(|| anyhow!(tr!("不支持的协议 {}", "Unsupported protocol: {}", p.api)))?;
                            if api_of(&raw_now).is_none() {
                                return Err(anyhow!(tr!("{id} 用的是 {raw_now} 协议，AgentPlus 不改它的协议", "{id} uses the {raw_now} protocol; AgentPlus doesn't change its protocol")));
                            }
                            def["api"] = json!(raw);
                            changed.push(format!("api = \"{raw}\""));
                        }
                        for c in changed {
                            diff.push(&ef, format!("{pre}.{id}.{c}"), true);
                            if in_cfg { dirty.cfg = true } else { dirty.store = true }
                        }
                        if let Some(k) = key {
                            if in_cfg {
                                self.set_key(cfg, auth, id, k, diff, dirty)?;
                            } else if !self.set_auth_key(auth, id, k, diff, dirty) {
                                // Disabled, key inline: it goes back into the parked definition.
                                if let Some(def) = store::section(root, agent, "disabledProviders").get_mut(id) {
                                    def["apiKey"] = json!(k);
                                    diff.push(&ef, tr!("{pre}.{id}.apiKey = {}（停用中，启用时写入）", "{pre}.{id}.apiKey = {} (disabled; written when enabled)", mask_key(k)), true);
                                    dirty.store = true;
                                }
                            }
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let removed_cfg = self.providers_mut(cfg)?.remove(provider).is_some();
                let removed_stash = store::section(root, agent, "disabledProviders").remove(provider).is_some();
                if !removed_cfg && !removed_stash {
                    return Err(msg::no_provider(provider));
                }
                let prefix = format!("{provider}|");
                store::section(root, agent, "hiddenModels").retain(|k, _| !k.starts_with(&prefix));
                store::section(root, agent, "names").remove(provider);
                let mut kept_key = Self::auth_key(auth.as_ref().map(|a| &a.0), provider).is_some();
                // The key of a provider pi ships stays in auth.json (pi keeps using it); any other
                // auth.json entry would come back as a built-in provider that can't be removed.
                if kept_key && !BUILTIN_PROVIDERS.contains(&provider.as_str()) {
                    if let Some(o) = auth.as_mut().and_then(|a| a.0.as_object_mut()) {
                        o.remove(provider);
                        dirty.auth = true;
                        kept_key = false;
                    }
                }
                let line = if kept_key {
                    tr!("- {pre}.{provider}（含它的模型；auth.json 里的密钥保留）", "- {pre}.{provider} (with its models; the API key in auth.json is kept)")
                } else {
                    tr!("- {pre}.{provider}（含它的模型和密钥）", "- {pre}.{provider} (with its models and API key)")
                };
                diff.push(&ef, line, false);
                dirty.cfg |= removed_cfg;
                dirty.store = true;
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let providers = self.providers_mut(cfg)?;
                let parked = store::section(root, agent, "disabledProviders");
                if *enabled {
                    if let Some(def) = parked.remove(provider) {
                        providers.insert(provider.clone(), def);
                        diff.push(&ef, tr!("+ {pre}.{provider}（启用）", "+ {pre}.{provider} (enabled)"), true);
                        dirty.cfg = true;
                        dirty.store = true;
                    }
                } else if let Some(def) = providers.remove(provider) {
                    parked.insert(provider.clone(), def);
                    diff.push(&ef, tr!("- {pre}.{provider}（停用：定义暂存在 AgentPlus，可恢复）", "- {pre}.{provider} (disabled: definition kept in AgentPlus, can be restored)"), false);
                    dirty.cfg = true;
                    dirty.store = true;
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let key = format!("{provider}|{model}");
                let fresh = self.new_model(model, None, None);
                let models = self.models_mut(cfg, provider, l("调整模型", "changing its models"))?;
                let hidden = store::section(root, agent, "hiddenModels");
                let idx = models.iter().position(|m| s(m, "id") == Some(model.as_str()));
                match (visible, idx) {
                    (true, None) => {
                        models.push(hidden.remove(&key).unwrap_or(fresh));
                        diff.push(&ef, format!("{pre}.{provider}.models + \"{model}\""), true);
                        dirty.cfg = true;
                        dirty.store = true;
                    }
                    (false, Some(i)) => {
                        hidden.insert(key, models.remove(i));
                        diff.push(&ef, tr!("{pre}.{provider}.models - \"{model}\"（隐藏，定义暂存在 AgentPlus）", "{pre}.{provider}.models - \"{model}\" (hidden, definition kept in AgentPlus)"), false);
                        dirty.cfg = true;
                        dirty.store = true;
                    }
                    _ => {}
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(msg::model_id_required());
                }
                let name = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                let key = format!("{provider}|{mid}");
                let in_stash = self.hidden(root).is_some_and(|h| h.contains_key(&key));
                let mut fresh = self.new_model(&mid, name, m.context);
                crate::mfields::write(&mut fresh, self.specs(), &m.extra)?;
                let def: &mut Value = if in_stash {
                    store::section(root, agent, "hiddenModels").get_mut(&key).unwrap()
                } else {
                    let models = self.models_mut(cfg, provider, l("添加模型", "adding models"))?;
                    match models.iter().position(|d| s(d, "id") == Some(mid.as_str())) {
                        Some(i) => &mut models[i],
                        None => {
                            models.push(fresh);
                            diff.push(&ef, format!("{pre}.{provider}.models + \"{mid}\""), true);
                            dirty.cfg = true;
                            return Ok(true);
                        }
                    }
                };
                let mut changed = vec![];
                if let Some(n) = name {
                    if s(def, "name") != Some(n) {
                        def["name"] = json!(n);
                        changed.push(format!("name = \"{n}\""));
                    }
                }
                if let Some(c) = m.context.filter(|c| *c > 0) {
                    if def.get("contextWindow").and_then(|x| x.as_u64()) != Some(c) {
                        def["contextWindow"] = json!(c);
                        changed.push(format!("contextWindow = {c}"));
                    }
                }
                changed.extend(crate::mfields::write(def, self.specs(), &m.extra)?);
                for c in changed {
                    diff.push(&ef, format!("{pre}.{provider}.models[\"{mid}\"].{c}"), true);
                    if in_stash { dirty.store = true } else { dirty.cfg = true }
                }
            }
            Op::DeleteModel { provider, model } => {
                let mut removed = false;
                if let Some(models) = self.providers_mut(cfg)?.get_mut(provider).and_then(|p| p.get_mut("models")).and_then(|m| m.as_array_mut()) {
                    let n = models.len();
                    models.retain(|d| s(d, "id") != Some(model.as_str()));
                    removed = models.len() != n;
                }
                let stashed = store::section(root, agent, "hiddenModels").remove(&format!("{provider}|{model}")).is_some();
                if removed || stashed {
                    diff.push(&ef, tr!("{pre}.{provider}.models - \"{model}\"（删除）", "{pre}.{provider}.models - \"{model}\" (deleted)"), false);
                    dirty.cfg |= removed;
                    dirty.store |= stashed;
                }
            }
            Op::SetProviderModels { provider, models: ids } => {
                let hidden = self.hidden(root);
                let want = clean_ids(ids);
                let fresh: Vec<Value> = want.iter().map(|m| self.new_model(m, None, None)).collect();
                let models = self.models_mut(cfg, provider, l("调整模型", "changing its models"))?;
                let old = models.clone();
                let next: Vec<Value> = want
                    .iter()
                    .zip(fresh)
                    .map(|(id, f)| old.iter().find(|d| s(d, "id") == Some(id.as_str())).or_else(|| hidden.and_then(|h| h.get(&format!("{provider}|{id}")))).cloned().unwrap_or(f))
                    .collect();
                let old_ids: Vec<String> = old.iter().filter_map(|d| s(d, "id").map(String::from)).collect();
                if old_ids != want {
                    for id in old_ids.iter().filter(|x| !want.contains(x)) {
                        diff.push(&ef, format!("{pre}.{provider}.models - \"{id}\""), false);
                    }
                    for id in want.iter().filter(|x| !old_ids.contains(x)) {
                        diff.push(&ef, format!("{pre}.{provider}.models + \"{id}\""), true);
                    }
                    *models = next;
                    dirty.cfg = true;
                    let sec = store::section(root, agent, "hiddenModels");
                    for id in &want {
                        if sec.remove(&format!("{provider}|{id}")).is_some() {
                            dirty.store = true;
                        }
                    }
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
