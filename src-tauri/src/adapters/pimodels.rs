//! Providers in the pi-ai `models.json` shape, shared by pi (`~/.pi/agent/models.json`,
//! `providers.<id>`) and OpenClaw (`openclaw.json`, `models.providers.<id>`):
//! `{ baseUrl, api, apiKey, headers, …, models: [{ id, name, contextWindow, … }] }`.
//!
//! Neither agent can switch a provider or a model off, so a hidden model / disabled provider
//! is removed from the file and its definition stashed in the AgentPlus store
//! (`hiddenModels["<provider>|<model>"]`, `disabledProviders["<provider>"]`), to come back
//! untouched. Unknown fields are always kept; only documented fields are ever added.


use super::Endpoint;
use crate::model::*;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

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

pub fn blank_meta() -> TextMeta {
    TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }
}

/// Provider ids pi-ai ships with. A config entry with one of these ids only overrides the
/// built-in, so removing it does not make "provider/model" references dangle.
pub const BUILTIN_PROVIDERS: [&str; 24] = [
    "anthropic", "openai", "google", "google-vertex", "google-gemini-cli", "google-antigravity", "amazon-bedrock",
    "azure-openai-responses", "openai-codex", "github-copilot", "mistral", "groq", "cerebras", "xai", "openrouter",
    "vercel-ai-gateway", "zai", "minimax", "minimax-cn", "huggingface", "kimi-coding", "opencode", "deepseek", "moonshot",
];

/// (AgentPlus api, label) of a pi-ai `api` value; None for protocols AgentPlus cannot speak.
pub fn api_of(raw: &str) -> Option<(&'static str, &'static str)> {
    match raw {
        "openai-completions" => Some(("chat", "Chat")),
        "openai-responses" => Some(("responses", "Responses")),
        "anthropic-messages" => Some(("anthropic", "Anthropic")),
        "google-generative-ai" => Some(("gemini", "Gemini")),
        _ => None,
    }
}

/// The pi-ai `api` value for an AgentPlus api.
pub fn raw_for(api: &str) -> Option<&'static str> {
    match api {
        "chat" => Some("openai-completions"),
        "responses" => Some("openai-responses"),
        "anthropic" => Some("anthropic-messages"),
        "gemini" => Some("google-generative-ai"),
        _ => None,
    }
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

// ---------- test hooks: tests point everything (files, store, backups, env) at a temp dir ----------

#[cfg(test)]
thread_local! {
    pub static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub static TEST_ENV: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub fn test_root() -> Option<PathBuf> {
    TEST_ROOT.with(|t| t.borrow().clone())
}
#[cfg(not(test))]
pub fn test_root() -> Option<PathBuf> {
    None
}

pub fn load_store() -> Value {
    match test_root() {
        Some(r) => std::fs::read_to_string(r.join("store.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({})),
        None => store::load(),
    }
}

pub fn save_store(v: &Value) -> Result<()> {
    match test_root() {
        Some(r) => Ok(std::fs::write(r.join("store.json"), serde_json::to_string_pretty(v)?)?),
        None => store::save(v),
    }
}

pub fn backup_files(agent: &str, files: &[PathBuf]) -> Result<PathBuf> {
    match test_root() {
        Some(r) => {
            let d = r.join("backups");
            std::fs::create_dir_all(&d)?;
            for f in files.iter().filter(|f| f.exists()) {
                std::fs::copy(f, d.join(f.file_name().unwrap()))?;
            }
            Ok(d)
        }
        None => backup(agent, files),
    }
}

/// An environment variable of the agent's environment (None inside WSL: not ours to read).
pub fn env_var(name: &str) -> Option<String> {
    #[cfg(test)]
    if test_root().is_some() {
        return TEST_ENV.with(|e| e.borrow().iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()));
    }
    if crate::env::is_wsl() {
        return None;
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn is_env_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.chars().next().unwrap().is_ascii_digit()
}

fn read_env_file(p: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(p).ok()?;
    text.lines().find_map(|l| {
        let l = l.trim().strip_prefix("export ").unwrap_or(l.trim());
        let (k, v) = l.split_once('=')?;
        (k.trim() == name).then(|| v.trim().trim_matches('"').trim_matches('\'').to_string()).filter(|v| !v.is_empty())
    })
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
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
        env_var(name).or_else(|| self.env_file.as_deref().and_then(|p| read_env_file(p, name)))
    }

    /// The real key behind a config value, when it can be known without running anything.
    pub fn resolve(&self, v: &Value) -> Option<String> {
        let raw = v.as_str()?.trim();
        if raw.is_empty() {
            return None;
        }
        match self.flavor {
            Flavor::Pi => {
                if raw.starts_with('!') {
                    return None;
                }
                if let Some(n) = raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| raw.strip_prefix('$')) {
                    return self.lookup(n);
                }
                // pi also treats a bare string as an env var name when that variable exists.
                if is_env_name(raw) {
                    if let Some(v) = self.lookup(raw) {
                        return Some(v);
                    }
                }
                Some(raw.to_string())
            }
            Flavor::OpenClaw => {
                let re = regex::Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}").unwrap();
                let mut missing = false;
                let out = re.replace_all(raw, |c: &regex::Captures| self.lookup(&c[1]).unwrap_or_else(|| {
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
            Some(Value::String(k)) if !k.trim().is_empty() => {
                let k = k.trim();
                Some(if self.flavor == Flavor::Pi && k.starts_with('!') {
                    l("shell 命令（AgentPlus 不执行）", "Shell command (AgentPlus doesn't run it)").to_string()
                } else if let Some(n) = k.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| k.strip_prefix('$').filter(|_| self.flavor == Flavor::Pi)) {
                    tr!("环境变量 {n}{}", "Environment variable {n}{}", if self.lookup(n).is_some() { "" } else { l("（当前未设置）", " (not set now)") })
                } else if k.contains("${") {
                    l("含环境变量引用", "Contains environment variable references").to_string()
                } else {
                    tr!("明文保存在 {fname}", "Stored in plain text in {fname}")
                })
            }
            _ => None,
        };
        match (in_auth, cfg) {
            (true, Some(c)) => tr!("auth.json（优先）；{fname} 里另有：{c}", "auth.json (takes precedence); {fname} also has: {c}"),
            (true, None) => l("保存在 auth.json", "Stored in auth.json").into(),
            (false, Some(c)) => c,
            (false, None) => l("未填写", "Not set").into(),
        }
    }

    /// Returns (config, meta, had_comments) of a JSON / JSONC file; missing = empty object.
    pub fn load_jsonc(path: &Path, blank: Value) -> Result<(Value, TextMeta, bool)> {
        if !path.exists() {
            return Ok((blank, blank_meta(), false));
        }
        let (text, meta) = read_text(path)?;
        let (clean, had) = strip_jsonc(&text);
        let v = serde_json::from_str(&clean).map_err(|e| anyhow!(tr!("{} 解析失败：{e}", "Failed to parse {}: {e}", display_path(path))))?;
        Ok((v, meta, had))
    }

    pub fn load_auth(&self) -> Option<(Value, TextMeta)> {
        let p = self.auth.as_ref()?;
        if !p.exists() {
            return Some((json!({}), blank_meta()));
        }
        read_json(p).ok()
    }

    pub fn auth_key<'a>(auth: Option<&'a Value>, id: &str) -> Option<&'a Value> {
        auth?.get(id).filter(|e| s(e, "type") == Some("api_key")).and_then(|e| e.get("key")).filter(|k| k.as_str().map(|k| !k.is_empty()).unwrap_or(false))
    }

    fn stash(root: &Value, agent: &str, key: &str) -> Map<String, Value> {
        store::agent_get(root, agent, key).and_then(|x| x.as_object()).cloned().unwrap_or_default()
    }

    pub fn hidden(&self, root: &Value) -> Map<String, Value> {
        Self::stash(root, self.agent, "hiddenModels")
    }

    pub fn parked(&self, root: &Value) -> Map<String, Value> {
        Self::stash(root, self.agent, "disabledProviders")
    }

    /// Display name: pi keeps it in the config, OpenClaw's schema has no room so it lives in the store.
    fn display_name(&self, id: &str, def: &Value, root: &Value) -> String {
        match self.flavor {
            Flavor::Pi => s(def, "name").filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
            Flavor::OpenClaw => Self::stash(root, self.agent, "names").get(id).and_then(|x| x.as_str()).unwrap_or(id).to_string(),
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

    fn provider_from(&self, id: &str, def: &Value, enabled: bool, root: &Value, auth: Option<&Value>) -> Provider {
        let base = s(def, "baseUrl").map(String::from).filter(|b| !b.is_empty());
        let raw = raw_api(id, def);
        let known = api_of(&raw);
        let mut models: Vec<Model> = def.get("models").and_then(|m| m.as_array()).map(|a| a.iter().filter_map(|m| self.model_from(m, true)).collect()).unwrap_or_default();
        let prefix = format!("{id}|");
        for (k, d) in &self.hidden(root) {
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
            Kv::mono(l("配置 ID", "Config ID"), format!("{}.{id}", self.cfg_prefix())),
            Kv::mono("api", raw.clone()),
            Kv::text(l("密钥", "API key"), self.key_desc(key_v, in_auth)),
            Kv::text(l("状态", "Status"), if enabled { l("已启用", "Enabled") } else { l("已停用 · 定义暂存在 AgentPlus", "Disabled · definition kept in AgentPlus") }),
        ];
        if BUILTIN_PROVIDERS.contains(&id) {
            details.push(Kv::text(l("说明", "About"), l("与内置供应商同名：这里的设置会合并到内置供应商上", "Same id as a built-in provider: these settings are merged into the built-in one")));
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
            apis: vec![known.map(|k| k.1.to_string()).unwrap_or_else(|| raw.clone())],
            builtin: false,
            enabled,
            compatible: known.is_some(),
            reason: known.is_none().then(|| tr!("{raw} 协议，AgentPlus 不能测速或转接", "{raw} protocol: AgentPlus can't test speed or relay it")),
            models,
            details,
            editable: true,
            api: known.map(|k| k.0.to_string()).unwrap_or(raw),
            has_key: in_cfg || in_auth,
            key_fp: None,
            key_hint: None,
            official_auth: false,
        }
    }

    /// Configured providers (enabled) plus the ones parked in the store (disabled).
    pub fn providers(&self, cfg: &Value, root: &Value, auth: Option<&Value>) -> Vec<Provider> {
        let mut out = vec![];
        if let Some(p) = self.providers_of(cfg) {
            for (id, def) in p {
                out.push(self.provider_from(id, def, true, root, auth));
            }
        }
        for (id, def) in &self.parked(root) {
            if !out.iter().any(|p| &p.id == id) {
                out.push(self.provider_from(id, def, false, root, auth));
            }
        }
        out
    }

    /// Base URL, key and api of a provider (auth.json wins over the config, as in pi).
    pub fn endpoint(&self, id: &str, cfg: &Value, root: &Value) -> Result<Endpoint> {
        let def = self.providers_of(cfg).and_then(|p| p.get(id)).cloned().or_else(|| self.parked(root).get(id).cloned()).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
        let base = s(&def, "baseUrl").filter(|b| !b.is_empty()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 baseUrl（沿用内置地址）", "Provider {id} has no baseUrl (uses the built-in URL)")))?.to_string();
        let auth = self.load_auth().map(|a| a.0);
        let key = Self::auth_key(auth.as_ref(), id).and_then(|k| self.resolve(k)).or_else(|| def.get("apiKey").and_then(|k| self.resolve(k)));
        let raw = raw_api(id, &def);
        Ok((base, key, api_of(&raw).map(|k| k.0.to_string()).unwrap_or(raw)))
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

    fn set_key(&self, cfg: &mut Value, auth: &mut Option<(Value, TextMeta)>, id: &str, key: &str, diff: &mut Diff, dirty: &mut Dirty) -> Result<()> {
        // Keep the key where it already is: pi's auth.json entry, else inline apiKey.
        let in_auth = Self::auth_key(auth.as_ref().map(|a| &a.0), id).is_some();
        if let (true, Some((a, _))) = (in_auth, auth.as_mut()) {
            a[id]["key"] = json!(key);
            diff.push(&display_path(self.auth.as_ref().unwrap()), format!("{id}.key = {}", mask_key(key)), true);
            dirty.auth = true;
        } else {
            let prefix = self.cfg_prefix();
            let def = self.providers_mut(cfg)?.get_mut(id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
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
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                let name = p.name.trim();
                let base_url = p.base_url.trim();
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
                match &p.id {
                    None => {
                        let raw = raw_for(&p.api).ok_or_else(|| anyhow!(tr!("不支持的协议 {}", "Unsupported protocol: {}", p.api)))?;
                        let parked = self.parked(root);
                        let providers = self.providers_mut(cfg)?;
                        let base = slug(name);
                        let free = |c: &str| !providers.contains_key(c) && !parked.contains_key(c) && !BUILTIN_PROVIDERS.contains(&c);
                        let id = if free(&base) { base.clone() } else { (2..).map(|n| format!("{base}-{n}")).find(|c| free(c)).unwrap() };
                        let mut ids: Vec<&str> = vec![];
                        for m in p.models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
                            if !ids.contains(&m) {
                                ids.push(m);
                            }
                        }
                        let models: Vec<Value> = ids.iter().map(|m| self.new_model(m, None, None)).collect();
                        let mut def = Map::new();
                        if self.flavor == Flavor::Pi {
                            def.insert("name".into(), json!(name));
                        }
                        def.insert("baseUrl".into(), json!(base_url));
                        def.insert("api".into(), json!(raw));
                        def.insert("models".into(), Value::Array(models));
                        providers.insert(id.clone(), Value::Object(def));
                        diff.push(&ef, tr!("+ {pre}.{id}（{base_url} · {raw} · {} 个模型）", "+ {pre}.{id} ({base_url} · {raw} · {} models)", ids.len()), true);
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
                    }
                    Some(id) => {
                        let in_cfg = self.providers_of(cfg).map(|p| p.contains_key(id)).unwrap_or(false);
                        let flavor = self.flavor;
                        let cur_name = self.display_name(id, self.providers_of(cfg).and_then(|p| p.get(id)).or(self.parked(root).get(id)).unwrap_or(&Value::Null), root);
                        if flavor == Flavor::OpenClaw && cur_name != name {
                            let names = store::section(root, agent, "names");
                            if name == id { names.remove(id) } else { names.insert(id.clone(), json!(name)) };
                            diff.push(l("AgentPlus · 显示名称", "AgentPlus · display names"), tr!("{id} → 「{name}」", "{id} → \"{name}\""), true);
                            dirty.store = true;
                        }
                        let def = if in_cfg {
                            self.providers_mut(cfg)?.get_mut(id).unwrap()
                        } else {
                            store::section(root, agent, "disabledProviders").get_mut(id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?
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
                        let fam_now = api_of(&raw_now).map(|k| k.0.to_string()).unwrap_or_else(|| raw_now.clone());
                        if p.api != fam_now {
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
                            } else if Self::auth_key(auth.as_ref().map(|a| &a.0), id).is_some() {
                                auth.as_mut().unwrap().0[id]["key"] = json!(k);
                                diff.push(&display_path(self.auth.as_ref().unwrap()), format!("{id}.key = {}", mask_key(k)), true);
                                dirty.auth = true;
                            } else if let Some(def) = store::section(root, agent, "disabledProviders").get_mut(id) {
                                def["apiKey"] = json!(k);
                                diff.push(&ef, tr!("{pre}.{id}.apiKey = {}（停用中，启用时写入）", "{pre}.{id}.apiKey = {} (disabled; written when enabled)", mask_key(k)), true);
                                dirty.store = true;
                            }
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let removed_cfg = self.providers_mut(cfg)?.remove(provider).is_some();
                let removed_stash = store::section(root, agent, "disabledProviders").remove(provider).is_some();
                if !removed_cfg && !removed_stash {
                    return Err(anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")));
                }
                let prefix = format!("{provider}|");
                store::section(root, agent, "hiddenModels").retain(|k, _| !k.starts_with(&prefix));
                store::section(root, agent, "names").remove(provider);
                let kept_key = Self::auth_key(auth.as_ref().map(|a| &a.0), provider).is_some();
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
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
                }
                let name = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                let key = format!("{provider}|{mid}");
                let in_stash = self.hidden(root).contains_key(&key);
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
                let mut want: Vec<String> = vec![];
                for m in ids.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
                    if !want.iter().any(|w| w == m) {
                        want.push(m.to_string());
                    }
                }
                let fresh: Vec<Value> = want.iter().map(|m| self.new_model(m, None, None)).collect();
                let models = self.models_mut(cfg, provider, l("调整模型", "changing its models"))?;
                let old = models.clone();
                let next: Vec<Value> = want
                    .iter()
                    .zip(fresh)
                    .map(|(id, f)| old.iter().find(|d| s(d, "id") == Some(id.as_str())).cloned().or_else(|| hidden.get(&format!("{provider}|{id}")).cloned()).unwrap_or(f))
                    .collect();
                let ids_of = |v: &[Value]| v.iter().filter_map(|d| s(d, "id").map(String::from)).collect::<Vec<_>>();
                if ids_of(&old) != want {
                    for id in ids_of(&old).iter().filter(|x| !want.contains(x)) {
                        diff.push(&ef, format!("{pre}.{provider}.models - \"{id}\""), false);
                    }
                    for id in want.iter().filter(|x| !ids_of(&old).contains(x)) {
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
