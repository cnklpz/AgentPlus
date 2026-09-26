//! Kimi Code (MoonshotAI/kimi-code) and the legacy Kimi CLI: `config.toml` in
//! `~/.kimi-code` (else the legacy `~/.kimi`; `$KIMI_CODE_HOME` overrides).
//!
//! `[providers.<id>]` = { type, base_url, api_key | api_key_env } and
//! `[models.<key>]` = { provider, model, max_context_size, capabilities }; `default_model`
//! names a models key. The AgentPlus model id is the models key; `model` (the upstream
//! name) is shown as its name when it differs.
//!
//! Edited with toml_edit so comments and layout survive. Hidden models and disabled
//! providers are moved, as TOML text, into the AgentPlus store and restored from it.

use super::msg;
use super::{Plan, Endpoint};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table, TableLike};

pub const ID: &str = "kimi";
pub const NAME: &str = "Kimi Code";
/// File whose presence in the dir means "configured here".
pub const MARKER: &str = "config.toml";
pub const WSL_SCRIPT: &str = "(command -v kimi >/dev/null && kimi --version || $HOME/.kimi-code/bin/kimi --version) 2>/dev/null; pgrep -x kimi >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".kimi-code/config.toml";

fn store_label() -> &'static str {
    l("AgentPlus · Kimi Code stash", "AgentPlus · Kimi Code 暂存")
}
/// `max_context_size` is required; used when a new model has none.
const DEFAULT_CTX: u64 = 128_000;
const TYPES: [&str; 8] = ["openai", "openai_legacy", "openai_responses", "anthropic", "google-genai", "gemini", "vertexai", "kimi"];

// ---------------------------------------------------------------- paths & test hooks

/// `~/.kimi-code` when it exists, else the legacy `~/.kimi` when that exists.
fn pick_dir(h: &Path) -> PathBuf {
    let code = h.join(".kimi-code");
    let legacy = h.join(".kimi");
    if code.exists() || !legacy.exists() {
        code
    } else {
        legacy
    }
}

/// `$KIMI_CODE_HOME` (Windows side only), else `~/.kimi-code` / `~/.kimi`.
pub fn default_dir() -> PathBuf {
    if let Some(h) = crate::env::agent_var("KIMI_CODE_HOME") {
        return PathBuf::from(h);
    }
    pick_dir(&home())
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn config_path() -> PathBuf {
    dir().join(MARKER)
}

/// The legacy Kimi CLI names its types differently (openai_legacy, gemini).
fn is_legacy() -> bool {
    dir().file_name().map(|n| n == ".kimi").unwrap_or(false)
}

// ---------------------------------------------------------------- detection

/// Native binary `~/.kimi-code/bin/kimi[.exe]`, npm `@moonshot-ai/kimi-code`, or the legacy
/// PyPI `kimi-cli` (`~/.local/bin/kimi[.exe]` / PATH). A CLI: `exe` stays None.
pub fn detect() -> Install {
    let mut inst = Install::default();
    let home = dirs::home_dir().unwrap_or_default();
    let name = crate::process::exe("kimi");
    let native = home.join(".kimi-code").join("bin").join(&name);
    let on_path = crate::process::on_path(&["kimi.exe", "kimi.cmd", "kimi"]);
    if native.exists() {
        inst.installed = true;
        inst.version = crate::process::cli_version(&native);
        inst.dir = native.parent().map(Path::to_path_buf);
    } else if let Some(v) = crate::process::npm_version_near("@moonshot-ai/kimi-code", on_path.as_deref()) {
        inst.installed = true;
        inst.version = Some(v);
    } else if let Some(p) = Some(home.join(".local").join("bin").join(&name)).filter(|p| p.exists()).or(on_path) {
        inst.installed = true;
        inst.version = crate::process::cli_version(&p);
    }
    if inst.installed {
        inst.running = crate::process::any_process(|n, _| n.eq_ignore_ascii_case(&name));
    }
    inst
}

// ---------------------------------------------------------------- reading

fn load() -> Result<(DocumentMut, TextMeta)> {
    let p = config_path();
    if !p.exists() {
        return Ok((DocumentMut::new(), TextMeta::NEW));
    }
    let (text, meta) = read_text(&p)?;
    let doc = text.parse::<DocumentMut>().map_err(|e| anyhow!(tr!("Failed to parse config.toml: {e}", "config.toml 解析失败：{e}")))?;
    Ok((doc, meta))
}

fn get_str(item: &Item, k: &str) -> Option<String> {
    item.get(k).and_then(|v| v.as_str()).map(String::from)
}

fn table_items(doc: &DocumentMut, name: &str) -> Vec<(String, Item)> {
    doc.get(name)
        .and_then(|i| i.as_table_like())
        .map(|t| t.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
        .unwrap_or_default()
}

fn default_model(doc: &DocumentMut) -> Option<String> {
    doc.get("default_model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
}

fn api_of(ty: &str) -> &'static str {
    match ty {
        "openai_responses" => "responses",
        "anthropic" => "anthropic",
        "google-genai" | "gemini" | "vertexai" => "gemini",
        _ => "chat",
    }
}

fn type_for(api: &str, legacy: bool) -> &'static str {
    match (api, legacy) {
        ("responses", _) => "openai_responses",
        ("anthropic", _) => "anthropic",
        ("gemini", true) => "gemini",
        ("gemini", false) => "google-genai",
        (_, true) => "openai_legacy",
        _ => "openai",
    }
}

fn check_api(api: &str) -> Result<&str> {
    match api {
        "chat" | "responses" | "anthropic" | "gemini" => Ok(api),
        other => Err(anyhow!(tr!("Kimi Code doesn't support the {other} API", "Kimi Code 不支持 {other} 接口"))),
    }
}

fn store_obj(root: &Value, k: &str) -> Map<String, Value> {
    store::get_obj(root, ID, k)
}

/// Copies an item with every table's document position cleared, so a table moved
/// into another document is laid out next to its new siblings.
fn fresh(item: &Item) -> Item {
    match item {
        Item::Table(t) => {
            let mut n = Table::new();
            n.set_implicit(t.is_implicit());
            n.set_dotted(t.is_dotted());
            *n.decor_mut() = t.decor().clone();
            for (k, v) in t.iter() {
                let (key, _) = t.get_key_value(k).unwrap();
                n.insert_formatted(key, fresh(v));
            }
            Item::Table(n)
        }
        other => other.clone(),
    }
}

/// A standalone TOML snippet holding `[parent.key]` tables (for the stash).
fn to_text(parts: &[(&str, &str, &Item)]) -> String {
    let mut d = DocumentMut::new();
    for (parent, key, item) in parts {
        if d.get(parent).is_none() {
            let mut t = Table::new();
            t.set_implicit(true);
            d.insert(parent, Item::Table(t));
        }
        d[*parent].as_table_mut().unwrap().insert(key, fresh(item));
    }
    d.to_string()
}

fn from_text(text: &str) -> Result<DocumentMut> {
    text.parse::<DocumentMut>().map_err(|e| anyhow!(tr!("Failed to parse the definition stashed in AgentPlus: {e}", "AgentPlus 暂存的定义解析失败：{e}")))
}

fn stash_text(v: &Value) -> &str {
    v.get("toml").and_then(|x| x.as_str()).unwrap_or("")
}

fn set_val(t: &mut dyn TableLike, k: &str, v: toml_edit::Value) {
    match t.get_mut(k).and_then(|i| i.as_value_mut()) {
        Some(old) => {
            let d = old.decor().clone();
            *old = v;
            *old.decor_mut() = d;
        }
        None => {
            t.insert(k, Item::Value(v));
        }
    }
}

fn model_of(key: &str, item: &Item, visible: bool, def: Option<&str>) -> Model {
    let upstream = get_str(item, "model");
    let context = item.get("max_context_size").and_then(|v| v.as_integer()).filter(|n| *n > 0).map(|n| n as u64);
    // Capabilities travel in `extra`; the UI shows them through the model fields (mfields::KIMI).
    let caps: Option<Vec<&str>> = item.get("capabilities").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|c| c.as_str()).collect());
    Model {
        id: key.into(),
        visible,
        readonly: false,
        tags: if def == Some(key) { vec![Tag::default_model()] } else { vec![] },
        ctx: context.map(fmt_ctx),
        name: upstream.filter(|m| m != key),
        context,
        deletable: true,
        extra: caps.map(|c| [("capabilities".to_string(), json!(c))].into_iter().collect()).unwrap_or_default(),
    }
}

/// Models of `pid` in `doc` (visible) plus its hidden ones from the stash.
fn models_for(doc: &DocumentMut, pid: &str, hidden: &Map<String, Value>, def: Option<&str>) -> Vec<Model> {
    let mut out: Vec<Model> = table_items(doc, "models")
        .iter()
        .filter(|(_, m)| get_str(m, "provider").as_deref() == Some(pid))
        .map(|(k, m)| model_of(k, m, true, def))
        .collect();
    for (k, h) in hidden {
        if h.get("provider").and_then(|x| x.as_str()) != Some(pid) {
            continue;
        }
        if let Ok(d) = from_text(stash_text(h)) {
            if let Some(m) = d.get("models").and_then(|t| t.get(k)) {
                out.push(model_of(k, m, false, def));
            }
        }
    }
    out
}

fn provider_of(pid: &str, item: &Item, models: Vec<Model>, enabled: bool, names: &Map<String, Value>) -> Provider {
    let ty = get_str(item, "type").unwrap_or_default();
    let base = get_str(item, "base_url").filter(|b| !b.is_empty());
    let api = api_of(&ty);
    let inline = get_str(item, "api_key").filter(|k| !k.is_empty());
    let var = get_str(item, "api_key_env").filter(|k| !k.is_empty());
    let from_env = var.as_deref().and_then(crate::env::agent_var).is_some();
    let key_note = match (&var, &inline) {
        (Some(v), _) => tr!(
            "Environment variable {v} (api_key_env) · {}",
            "环境变量 {v}（api_key_env）· {}",
            if from_env { l("set", "已设置") } else { l("AgentPlus can't see it; make sure Kimi has this variable when it runs", "AgentPlus 没读到，确认 Kimi 运行时有这个变量") }
        ),
        (None, Some(_)) => l("api_key · stored in plain text in config.toml", "api_key · 明文保存在 config.toml").into(),
        _ => l("Not set", "未填写").into(),
    };
    let known = TYPES.contains(&ty.as_str());
    let mut details = vec![
        Kv::mono(lbl::config_id(), format!("[providers.{pid}]")),
        Kv::mono("type", format!("\"{ty}\"")),
        Kv::text(lbl::api_key(), key_note),
        Kv::text(lbl::status(), if enabled { l("Enabled", "已启用") } else { l("Disabled · definition kept in AgentPlus", "已停用 · 定义暂存在 AgentPlus") }),
    ];
    if ty == "kimi" {
        details.push(Kv::text(lbl::note(), l("Kimi's official API (treated as Chat); /login rewrites this section", "Kimi 官方接口（按 Chat 处理）；/login 会改写这一段")));
    }
    Provider {
        id: pid.into(),
        name: names.get(pid).and_then(|x| x.as_str()).unwrap_or(pid).to_string(),
        host: base.as_deref().map(host_of).unwrap_or_else(|| "-".into()),
        base_url: base,
        apis: vec![api_label(api).into()],
        builtin: ty == "kimi",
        enabled,
        compatible: known,
        reason: (!known).then(|| tr!("Unknown type = \"{ty}\"", "未知的 type = \"{ty}\"")),
        models,
        details,
        editable: known,
        api: api.into(),
        has_key: inline.is_some() || from_env,
        ..Default::default()
    }
}

pub fn state(inst: &Install) -> AgentState {
    let legacy = is_legacy();
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&config_path())]);
    let (doc, _) = match load() {
        Ok(x) => x,
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let root = store::load();
    let names = store_obj(&root, "names");
    let hidden = store_obj(&root, "hiddenModels");
    let def = default_model(&doc);
    for (pid, item) in table_items(&doc, "providers") {
        let models = models_for(&doc, &pid, &hidden, def.as_deref());
        st.providers.push(provider_of(&pid, &item, models, true, &names));
    }
    for (pid, rec) in store_obj(&root, "disabledProviders") {
        let Ok(d) = from_text(stash_text(&rec)) else { continue };
        let Some(item) = d.get("providers").and_then(|t| t.get(&pid)) else { continue };
        let models = models_for(&d, &pid, &hidden, None);
        st.providers.push(provider_of(&pid, item, models, false, &names));
    }

    let def_item = def.as_deref().and_then(|k| doc.get("models").and_then(|t| t.get(k)));
    let def_prov = def_item.and_then(|m| get_str(m, "provider"));
    let vis: usize = st.providers.iter().filter(|p| p.enabled).map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::mono("default_model", def.clone().unwrap_or_else(|| "-".into())),
        Kv::text(lbl::provider(), def_prov.as_deref().map(|p| st.providers.iter().find(|x| x.id == p).map(|x| x.name.clone()).unwrap_or_else(|| p.to_string())).unwrap_or_else(|| "-".into())),
        Kv::mono(l("Upstream model", "上游模型"), def_item.and_then(|m| get_str(m, "model")).unwrap_or_else(|| "-".into())),
        Kv::text(lbl::visible_models(), tr!("{vis}", "{vis} 个")),
        Kv::text(l("Config", "配置"), if legacy { l("Legacy Kimi CLI (~/.kimi)", "旧版 Kimi CLI（~/.kimi）") } else { "Kimi Code" }),
    ];
    st.notes.push(l("/login and /model rewrite config.toml and drop its comments", "/login 和 /model 会重写 config.toml 并丢掉注释").into());
    st.notes.push(l("Changes apply to new Kimi sessions; switch the default model with /model in Kimi.", "改动对新开的 Kimi 会话生效；默认模型在 Kimi 里用 /model 切换。").into());
    if legacy {
        st.notes.push(
            l(
                "This is a legacy Kimi CLI config (~/.kimi); new Chat / Gemini providers are written as openai_legacy / gemini types.",
                "这是旧版 Kimi CLI 的配置（~/.kimi）；新建的 Chat / Gemini 供应商写成 openai_legacy / gemini 类型。",
            )
            .into(),
        );
    }
    if def.is_some() && def_item.is_none() {
        st.notes.push(tr!("default_model = \"{}\" isn't in [models].", "default_model = \"{}\" 在 [models] 里找不到。", def.unwrap_or_default()));
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (doc, _) = load()?;
    let stashed = store_obj(&store::load(), "disabledProviders").get(id).map(|r| from_text(stash_text(r))).transpose()?;
    let item = doc
        .get("providers")
        .and_then(|t| t.get(id))
        .cloned()
        .or_else(|| stashed.as_ref().and_then(|d| d.get("providers").and_then(|t| t.get(id)).cloned()))
        .ok_or_else(|| msg::no_provider(id))?;
    let base = get_str(&item, "base_url").filter(|b| !b.is_empty()).ok_or_else(|| anyhow!(tr!("Provider {id} has no base_url", "供应商 {id} 没有 base_url")))?;
    let key = get_str(&item, "api_key")
        .filter(|k| !k.is_empty())
        .or_else(|| get_str(&item, "api_key_env").and_then(|v| crate::env::agent_var(&v)));
    Ok((base, key, api_of(&get_str(&item, "type").unwrap_or_default()).into()))
}

// ---------------------------------------------------------------- writing

struct Ctx {
    doc: DocumentMut,
    root: Value,
    diff: Diff,
    file: String,
    legacy: bool,
    cfg_dirty: bool,
    store_dirty: bool,
}

fn edit_provider_in(doc: &mut DocumentMut, pid: &str, p: &ProviderInput, legacy: bool) -> Result<Vec<String>> {
    let t = doc
        .get_mut("providers")
        .and_then(|x| x.get_mut(pid))
        .and_then(|x| x.as_table_like_mut())
        .ok_or_else(|| msg::no_provider(pid))?;
    let mut lines = vec![];
    let base = p.base_url.trim();
    if t.get("base_url").and_then(|v| v.as_str()) != Some(base) {
        set_val(t, "base_url", base.into());
        lines.push(format!("[providers.{pid}] base_url = \"{base}\""));
    }
    let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if api_of(&ty) != p.api {
        let nt = type_for(&p.api, legacy);
        set_val(t, "type", nt.into());
        lines.push(format!("[providers.{pid}] type = \"{ty}\" → \"{nt}\""));
    }
    if let Some(k) = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        if let Some(var) = t.get("api_key_env").and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
            return Err(anyhow!(tr!(
                "\"{pid}\" reads its API key from api_key_env = \"{var}\"; set it in the {var} environment variable. AgentPlus doesn't write it",
                "「{pid}」用 api_key_env = \"{var}\" 读取密钥，请在环境变量 {var} 里设置，AgentPlus 不写入"
            )));
        }
        if t.get("api_key").and_then(|v| v.as_str()) != Some(k) {
            set_val(t, "api_key", k.into());
            lines.push(format!("[providers.{pid}] api_key = {}", mask_key(k)));
        }
    }
    Ok(lines)
}

fn edit_model_in(doc: &mut DocumentMut, key: &str, name: Option<&str>, ctx: Option<u64>, extra: &crate::mfields::Extra) -> Vec<String> {
    let Some(t) = doc.get_mut("models").and_then(|x| x.get_mut(key)).and_then(|x| x.as_table_like_mut()) else { return vec![] };
    let mut lines = vec![];
    if let Some(n) = name {
        if t.get("model").and_then(|v| v.as_str()) != Some(n) {
            set_val(t, "model", n.into());
            lines.push(format!("[models.\"{key}\"] model = \"{n}\""));
        }
    }
    if let Some(c) = ctx {
        if t.get("max_context_size").and_then(|v| v.as_integer()) != Some(c as i64) {
            set_val(t, "max_context_size", (c as i64).into());
            lines.push(format!("[models.\"{key}\"] max_context_size = {c}"));
        }
    }
    match extra.get("capabilities") {
        Some(Value::Null) => {
            if t.remove("capabilities").is_some() {
                lines.push(tr!("[models.\"{key}\"] capabilities removed", "[models.\"{key}\"] capabilities 删除"));
            }
        }
        Some(Value::Array(want)) => {
            let want: Vec<&str> = want.iter().filter_map(|c| c.as_str()).collect();
            let have: Option<Vec<String>> = t.get("capabilities").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect());
            if have.as_deref().map(|h| h.iter().map(String::as_str).collect::<Vec<_>>()) != Some(want.clone()) {
                set_val(t, "capabilities", toml_edit::Value::Array(want.iter().copied().collect()));
                lines.push(format!("[models.\"{key}\"] capabilities = {want:?}"));
            }
        }
        _ => {}
    }
    lines
}

impl Ctx {
    fn has_provider(&self, pid: &str) -> bool {
        self.doc.get("providers").and_then(|t| t.get(pid)).is_some()
    }

    fn live_provider(&self, pid: &str) -> Result<()> {
        if self.has_provider(pid) {
            Ok(())
        } else if store_obj(&self.root, "disabledProviders").contains_key(pid) {
            Err(anyhow!(tr!("Provider {pid} is disabled; enable it before changing its models", "供应商 {pid} 已停用，先启用再调整模型")))
        } else {
            Err(msg::no_provider(pid))
        }
    }

    /// (key, upstream model) of the provider's models in the config.
    fn live_models(&self, pid: &str) -> Vec<(String, Option<String>)> {
        table_items(&self.doc, "models")
            .into_iter()
            .filter(|(_, m)| get_str(m, "provider").as_deref() == Some(pid))
            .map(|(k, m)| (k, get_str(&m, "model")))
            .collect()
    }

    /// (key, upstream model) of the provider's hidden models.
    fn hidden_models(&self, pid: &str) -> Vec<(String, Option<String>)> {
        store_obj(&self.root, "hiddenModels")
            .iter()
            .filter(|(_, h)| h.get("provider").and_then(|x| x.as_str()) == Some(pid))
            .map(|(k, h)| (k.clone(), from_text(stash_text(h)).ok().and_then(|d| d.get("models").and_then(|t| t.get(k)).and_then(|m| get_str(m, "model")))))
            .collect()
    }

    fn is_live_model(&self, pid: &str, key: &str) -> bool {
        self.doc.get("models").and_then(|t| t.get(key)).and_then(|m| get_str(m, "provider")).as_deref() == Some(pid)
    }

    /// Every models key in use: config, hidden stash, disabled providers.
    fn all_keys(&self) -> HashSet<String> {
        let mut out: HashSet<String> = table_items(&self.doc, "models").into_iter().map(|(k, _)| k).collect();
        out.extend(store_obj(&self.root, "hiddenModels").keys().cloned());
        for rec in store_obj(&self.root, "disabledProviders").values() {
            if let Ok(d) = from_text(stash_text(rec)) {
                out.extend(table_items(&d, "models").into_iter().map(|(k, _)| k));
            }
        }
        out
    }

    /// The default_model, when it is one of `keys`.
    fn default_in(&self, keys: &[String]) -> Option<String> {
        default_model(&self.doc).filter(|d| keys.contains(d))
    }

    fn parent(&mut self, name: &str) -> Result<&mut Table> {
        if self.doc.get(name).is_none() {
            let mut t = Table::new();
            t.set_implicit(true);
            self.doc.insert(name, Item::Table(t));
        }
        self.doc.get_mut(name).and_then(|i| i.as_table_mut()).ok_or_else(|| anyhow!(tr!("{name} in config.toml isn't written as a [{name}] table; AgentPlus won't change it", "config.toml 的 {name} 不是 [{name}] 表的写法，AgentPlus 不修改它")))
    }

    fn remove(&mut self, parent: &str, key: &str) -> Option<Item> {
        self.doc.get_mut(parent).and_then(|t| t.as_table_like_mut()).and_then(|t| t.remove(key))
    }

    fn set_store(&mut self, k: &str, v: impl Into<Value>) {
        store::set_value(&mut self.root, ID, k, v.into());
        self.store_dirty = true;
    }

    fn set_name(&mut self, id: &str, name: &str) {
        let mut names = store_obj(&self.root, "names");
        let now = names.get(id).and_then(|x| x.as_str()).unwrap_or(id);
        if now != name {
            names.insert(id.into(), json!(name));
            self.set_store("names", names);
            self.diff.push(store_label(), tr!("\"{id}\" name = {name}", "「{id}」名称 = {name}"), true);
        }
    }

    /// Adds `[models.<key>]` sending `upstream` as the model id; returns the key used.
    fn add_model(&mut self, pid: &str, key: &str, upstream: &str, ctx: Option<u64>) -> Result<String> {
        let all = self.all_keys();
        // A models key taken by another provider's model gets the provider as prefix.
        let key = if !all.contains(key) { key.to_string() } else { unique_id(&format!("{pid}/{key}"), |c| all.contains(c)) };
        let c = ctx.unwrap_or(DEFAULT_CTX);
        let mut t = Table::new();
        t.insert("provider", value(pid));
        t.insert("model", value(upstream));
        t.insert("max_context_size", value(c as i64));
        self.parent("models")?.insert(&key, Item::Table(t));
        self.diff.push(&self.file, format!("+ [models.\"{key}\"] provider = \"{pid}\", model = \"{upstream}\", max_context_size = {c}"), true);
        self.cfg_dirty = true;
        Ok(key)
    }

    fn create_provider(&mut self, p: &ProviderInput) -> Result<()> {
        let api = check_api(&p.api)?;
        let mut taken: HashSet<String> = table_items(&self.doc, "providers").into_iter().map(|(k, _)| k).collect();
        taken.extend(store_obj(&self.root, "disabledProviders").keys().cloned());
        let id = unique_id(&slug(p.name.trim()), |c| taken.contains(c));
        let ty = type_for(api, self.legacy);
        let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let mut t = Table::new();
        t.insert("type", value(ty));
        t.insert("base_url", value(p.base_url.trim()));
        t.insert("api_key", value(key.unwrap_or("")));
        self.parent("providers")?.insert(&id, Item::Table(t));
        self.diff.push(&self.file, format!("+ [providers.{id}] type = \"{ty}\", base_url = \"{}\"", p.base_url.trim()), true);
        if let Some(k) = key {
            self.diff.push(&self.file, format!("[providers.{id}] api_key = {}", mask_key(k)), true);
        }
        self.cfg_dirty = true;
        self.set_name(&id, p.name.trim());
        let models = clean_ids(&p.models);
        let seeds = crate::modelinfo::seed_ops(ID, &id, &models);
        for m in models {
            match seeds.iter().find_map(|o| match o {
                Op::UpsertModel { model, .. } if model.id == m => Some(model),
                _ => None,
            }) {
                Some(seeded) => self.upsert_model(&id, seeded)?,
                None => {
                    self.add_model(&id, &m, &m, None)?;
                }
            }
        }
        Ok(())
    }

    fn edit_provider(&mut self, id: &str, p: &ProviderInput) -> Result<()> {
        check_api(&p.api)?;
        let legacy = self.legacy;
        if self.has_provider(id) {
            for l in edit_provider_in(&mut self.doc, id, p, legacy)? {
                self.diff.push(&self.file, l, true);
                self.cfg_dirty = true;
            }
        } else {
            let mut d = store_obj(&self.root, "disabledProviders");
            let rec = d.get_mut(id).ok_or_else(|| msg::no_provider(id))?;
            let mut sd = from_text(stash_text(rec))?;
            let lines = edit_provider_in(&mut sd, id, p, legacy)?;
            if !lines.is_empty() {
                rec["toml"] = json!(sd.to_string());
                self.set_store("disabledProviders", d);
                for l in lines {
                    self.diff.push(store_label(), tr!("{l} (disabled)", "{l}（已停用）"), true);
                }
            }
        }
        if !p.name.trim().is_empty() {
            self.set_name(id, p.name.trim());
        }
        Ok(())
    }

    fn delete_provider(&mut self, id: &str) -> Result<()> {
        if self.has_provider(id) {
            let keys: Vec<String> = self.live_models(id).into_iter().map(|(k, _)| k).collect();
            if let Some(d) = self.default_in(&keys) {
                return Err(anyhow!(tr!(
                    "\"{d}\" is the default_model; switch to another default model with /model in Kimi before deleting this provider",
                    "「{d}」是 default_model，删除这个供应商前先在 Kimi 里用 /model 换一个默认模型"
                )));
            }
            self.remove("providers", id);
            for k in &keys {
                self.remove("models", k);
            }
            self.diff.push(&self.file, trn!(keys.len(), "- [providers.{id}] and its {n} model (API key included)", "- [providers.{id}] and its {n} models (API key included)", "- [providers.{id}] 和它的 {n} 个模型（含密钥）"), false);
            self.cfg_dirty = true;
        } else {
            let mut d = store_obj(&self.root, "disabledProviders");
            if d.remove(id).is_none() {
                return Err(msg::no_provider(id));
            }
            self.set_store("disabledProviders", d);
            self.diff.push(store_label(), tr!("- \"{id}\" (disabled; its stashed definition is deleted too)", "- 「{id}」（已停用，暂存的定义一并删除）"), false);
        }
        let mut hidden = store_obj(&self.root, "hiddenModels");
        let n = hidden.len();
        hidden.retain(|_, h| h.get("provider").and_then(|x| x.as_str()) != Some(id));
        if hidden.len() != n {
            self.diff.push(store_label(), trn!(n - hidden.len(), "- {n} hidden model stashed for \"{id}\"", "- {n} hidden models stashed for \"{id}\"", "- 「{id}」暂存的 {n} 个隐藏模型"), false);
            self.set_store("hiddenModels", hidden);
        }
        let mut names = store_obj(&self.root, "names");
        if names.remove(id).is_some() {
            self.set_store("names", names);
        }
        Ok(())
    }

    fn set_enabled(&mut self, id: &str, on: bool) -> Result<()> {
        let mut d = store_obj(&self.root, "disabledProviders");
        if !on {
            if !self.has_provider(id) {
                return if d.contains_key(id) { Ok(()) } else { Err(msg::no_provider(id)) };
            }
            let keys: Vec<String> = self.live_models(id).into_iter().map(|(k, _)| k).collect();
            if let Some(d) = self.default_in(&keys) {
                return Err(anyhow!(tr!(
                    "\"{d}\" is the default_model; switch to another default model with /model in Kimi before disabling this provider",
                    "「{d}」是 default_model，停用这个供应商前先在 Kimi 里用 /model 换一个默认模型"
                )));
            }
            let prov = self.remove("providers", id).unwrap();
            let models: Vec<(String, Item)> = keys.iter().filter_map(|k| self.remove("models", k).map(|m| (k.clone(), m))).collect();
            let mut parts: Vec<(&str, &str, &Item)> = vec![("providers", id, &prov)];
            parts.extend(models.iter().map(|(k, m)| ("models", k.as_str(), m)));
            d.insert(id.into(), json!({ "toml": to_text(&parts) }));
            self.set_store("disabledProviders", d);
            self.diff.push(&self.file, trn!(models.len(), "- [providers.{id}] and its {n} model (stashed in AgentPlus; can be restored)", "- [providers.{id}] and its {n} models (stashed in AgentPlus; can be restored)", "- [providers.{id}] 和它的 {n} 个模型（暂存在 AgentPlus，可恢复）"), false);
            self.cfg_dirty = true;
        } else {
            let Some(rec) = d.remove(id) else {
                return if self.has_provider(id) { Ok(()) } else { Err(msg::no_provider(id)) };
            };
            // A [providers.<id>] written meanwhile (by hand, /login) would be replaced, api_key and all.
            if self.has_provider(id) {
                return Err(anyhow!(tr!(
                    "config.toml already has [providers.{id}]; can't restore the disabled \"{id}\"",
                    "config.toml 里已经有 [providers.{id}]，无法恢复停用的「{id}」"
                )));
            }
            let sd = from_text(stash_text(&rec))?;
            let prov = sd.get("providers").and_then(|t| t.get(id)).ok_or_else(|| anyhow!(tr!("Stashed provider {id} is incomplete", "暂存的供应商 {id} 不完整")))?;
            let models = table_items(&sd, "models");
            let live: HashSet<String> = table_items(&self.doc, "models").into_iter().map(|(k, _)| k).collect();
            if let Some((k, _)) = models.iter().find(|(k, _)| live.contains(k)) {
                return Err(anyhow!(tr!("Model key {k} is already used by another model; can't restore \"{id}\"", "模型键 {k} 已被其他模型占用，无法恢复「{id}」")));
            }
            self.parent("providers")?.insert(id, fresh(prov));
            for (k, m) in &models {
                self.parent("models")?.insert(k, fresh(m));
            }
            self.set_store("disabledProviders", d);
            self.diff.push(&self.file, trn!(models.len(), "+ [providers.{id}] and its {n} model", "+ [providers.{id}] and its {n} models", "+ [providers.{id}] 和它的 {n} 个模型"), true);
            self.cfg_dirty = true;
        }
        Ok(())
    }

    fn set_visible(&mut self, pid: &str, key: &str, visible: bool) -> Result<()> {
        self.live_provider(pid)?;
        let mut hidden = store_obj(&self.root, "hiddenModels");
        if !visible {
            if !self.is_live_model(pid, key) {
                return if hidden.contains_key(key) { Ok(()) } else { Err(anyhow!(tr!("Model not found: {key}", "找不到模型 {key}"))) };
            }
            if let Some(d) = self.default_in(&[key.to_string()]) {
                return Err(anyhow!(tr!(
                    "\"{d}\" is the default_model; switch to another default model with /model in Kimi before hiding it",
                    "「{d}」是 default_model，隐藏它前先在 Kimi 里用 /model 换一个默认模型"
                )));
            }
            // The stash is keyed by models key: never overwrite another model stashed under it
            // (config.toml can bring a key back behind AgentPlus's back: /login, /model, by hand).
            if hidden.contains_key(key) {
                return Err(anyhow!(tr!(
                    "Model key {key} already has a hidden model stashed in AgentPlus; give this model a different key in config.toml first",
                    "模型键 {key} 在 AgentPlus 里已暂存了一个隐藏模型，先在 config.toml 里给这个模型换个键"
                )));
            }
            let item = self.remove("models", key).unwrap();
            hidden.insert(key.into(), json!({ "provider": pid, "toml": to_text(&[("models", key, &item)]) }));
            self.set_store("hiddenModels", hidden);
            self.diff.push(&self.file, tr!("- [models.\"{key}\"] (stashed in AgentPlus)", "- [models.\"{key}\"]（暂存在 AgentPlus）"), false);
            self.cfg_dirty = true;
        } else {
            if self.is_live_model(pid, key) {
                return Ok(());
            }
            let rec = hidden.remove(key).filter(|h| h.get("provider").and_then(|x| x.as_str()) == Some(pid)).ok_or_else(|| anyhow!(tr!("Model not found: {key}", "找不到模型 {key}")))?;
            if self.doc.get("models").and_then(|t| t.get(key)).is_some() {
                return Err(anyhow!(tr!("Model key {key} is already used by another model", "模型键 {key} 已被其他模型占用")));
            }
            let sd = from_text(stash_text(&rec))?;
            let item = sd.get("models").and_then(|t| t.get(key)).ok_or_else(|| anyhow!(tr!("Stashed model {key} is incomplete", "暂存的模型 {key} 不完整")))?;
            self.parent("models")?.insert(key, fresh(item));
            self.set_store("hiddenModels", hidden);
            self.diff.push(&self.file, format!("+ [models.\"{key}\"]"), true);
            self.cfg_dirty = true;
        }
        Ok(())
    }

    fn upsert_model(&mut self, pid: &str, m: &ModelInput) -> Result<()> {
        self.live_provider(pid)?;
        let mid = m.id.trim();
        if mid.is_empty() {
            return Err(msg::model_id_required());
        }
        let name = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        for (k, v) in &m.extra {
            crate::mfields::check(crate::mfields::KIMI, k, v)?;
        }
        if self.is_live_model(pid, mid) {
            for l in edit_model_in(&mut self.doc, mid, name, m.context, &m.extra) {
                self.diff.push(&self.file, l, true);
                self.cfg_dirty = true;
            }
            return Ok(());
        }
        let mut hidden = store_obj(&self.root, "hiddenModels");
        if let Some(rec) = hidden.get_mut(mid).filter(|h| h.get("provider").and_then(|x| x.as_str()) == Some(pid)) {
            let mut sd = from_text(stash_text(rec))?;
            let lines = edit_model_in(&mut sd, mid, name, m.context, &m.extra);
            if !lines.is_empty() {
                rec["toml"] = json!(sd.to_string());
                self.set_store("hiddenModels", hidden);
                for l in lines {
                    self.diff.push(store_label(), tr!("{l} (hidden)", "{l}（已隐藏）"), true);
                }
            }
            return Ok(());
        }
        // Kimi's per-model "name" is the upstream model id (`model = …`), as on edit.
        let key = self.add_model(pid, mid, name.unwrap_or(mid), m.context)?;
        for l in edit_model_in(&mut self.doc, &key, None, None, &m.extra) {
            self.diff.push(&self.file, l, true);
        }
        Ok(())
    }

    fn delete_model(&mut self, pid: &str, key: &str) -> Result<()> {
        self.live_provider(pid)?;
        if self.is_live_model(pid, key) {
            if let Some(d) = self.default_in(&[key.to_string()]) {
                return Err(anyhow!(tr!(
                    "\"{d}\" is the default_model; switch to another default model with /model in Kimi before deleting it",
                    "「{d}」是 default_model，删除它前先在 Kimi 里用 /model 换一个默认模型"
                )));
            }
            self.remove("models", key);
            self.diff.push(&self.file, tr!("- [models.\"{key}\"] (deleted)", "- [models.\"{key}\"]（删除）"), false);
            self.cfg_dirty = true;
            return Ok(());
        }
        let mut hidden = store_obj(&self.root, "hiddenModels");
        if hidden.get(key).and_then(|h| h.get("provider")).and_then(|x| x.as_str()) == Some(pid) {
            hidden.remove(key);
            self.set_store("hiddenModels", hidden);
            self.diff.push(store_label(), tr!("- \"{key}\" (hidden; deleted)", "- 「{key}」（已隐藏，删除）"), false);
        }
        Ok(())
    }

    /// Makes exactly `want` the provider's visible models; entries match by key or upstream name.
    fn set_models(&mut self, pid: &str, want: &[String]) -> Result<()> {
        self.live_provider(pid)?;
        let list = clean_ids(want);
        let keep = |k: &str, up: &Option<String>| list.iter().any(|w| w == k || Some(w) == up.as_ref());
        let def = default_model(&self.doc);
        let (live, hidden) = (self.live_models(pid), self.hidden_models(pid));
        for (k, up) in &live {
            if !keep(k, up) && def.as_deref() != Some(k.as_str()) {
                self.delete_model(pid, k)?;
            }
        }
        for (k, up) in &hidden {
            if !keep(k, up) {
                self.delete_model(pid, k)?;
            }
        }
        for w in &list {
            let hit = |(k, up): &(String, Option<String>)| k == w || up.as_deref() == Some(w.as_str());
            if live.iter().any(hit) {
                continue;
            }
            match hidden.iter().find(|x| hit(x)) {
                Some((k, _)) => self.set_visible(pid, k, true)?,
                None => {
                    self.add_model(pid, w, w, None)?;
                }
            }
        }
        Ok(())
    }
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (doc, meta) = load()?;
    let mut cx = Ctx { doc, root: store::load(), diff: Diff::default(), file: display_path(&config_path()), legacy: is_legacy(), cfg_dirty: false, store_dirty: false };

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                match p.id.as_deref() {
                    None => cx.create_provider(p)?,
                    Some(id) => cx.edit_provider(id, p)?,
                }
            }
            Op::DeleteProvider { provider } => cx.delete_provider(provider)?,
            Op::SetProviderEnabled { provider, enabled } => cx.set_enabled(provider, *enabled)?,
            Op::SetModelVisible { provider, model, visible } => cx.set_visible(provider, model, *visible)?,
            Op::UpsertModel { provider, model } => cx.upsert_model(provider, model)?,
            Op::DeleteModel { provider, model } => cx.delete_model(provider, model)?,
            Op::SetProviderModels { provider, models } => cx.set_models(provider, models)?,
            Op::SetSetting { key, .. } => return Err(msg::unknown_setting(key)),
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("Kimi Code can have several providers at once; switch the default model with /model in Kimi", "Kimi Code 可以同时配置多个供应商，默认模型在 Kimi 里用 /model 切换"))),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cx.cfg_dirty {
            let p = config_path();
            backup_dir = Some(backup(ID, std::slice::from_ref(&p))?);
            std::fs::create_dir_all(dir())?;
            write_text_atomic(&p, &cx.doc.to_string(), meta)?;
            written.push(p);
        }
        if cx.store_dirty {
            store::save(&cx.root)?;
        }
    }
    Ok((cx.diff, written, backup_dir))
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SECRET: &str = "sk-kimi-secret-4242";

    const SAMPLE: &str = r#"# Kimi Code config
default_model = "kimi-k2"

[providers.moonshot]
type = "kimi"
base_url = "https://api.moonshot.cn/v1"
api_key = "sk-moonshot-0001"

[providers.relay] # my relay
type = "openai"
base_url = "https://relay.example.com/v1" # primary endpoint
api_key = "sk-relay-9999"

[providers.envy]
type = "anthropic"
base_url = "https://anthropic.example.com"
api_key_env = "AGENTPLUS_TEST_KIMI_UNSET_VAR"

[models.kimi-k2]
provider = "moonshot"
model = "kimi-k2-0905-preview"
max_context_size = 262144
capabilities = ["thinking"]

# GPT via relay
[models."gpt-4.1"]
provider = "relay"
model = "gpt-4.1"
max_context_size = 128000

[models.relay-mini]
provider = "relay"
model = "gpt-4.1-mini"
max_context_size = 128000

[loop_control]
max_steps_per_run = 100 # keep
"#;

    fn setup(tag: &str, legacy: bool, sample: Option<&str>) -> TestHome {
        let home = TestHome::new(&format!("kimi-{tag}"));
        let d = home.0.join(if legacy { ".kimi" } else { ".kimi-code" });
        fs::create_dir_all(&d).unwrap();
        if let Some(s) = sample {
            fs::write(d.join("config.toml"), s).unwrap();
        }
        home
    }

    fn text() -> String {
        fs::read_to_string(config_path()).unwrap()
    }

    fn st() -> AgentState {
        state(&Install::default())
    }

    fn prov(st: &AgentState, id: &str) -> Provider {
        st.providers.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| panic!("no provider {id}"))
    }

    fn apply(ops: Vec<Op>) -> Diff {
        plan(&ops, false).unwrap().0
    }

    fn lines(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect::<Vec<_>>().join("\n")
    }

    fn input(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> ProviderInput {
        ProviderInput {
            id: id.map(String::from),
            name: name.into(),
            base_url: base.into(),
            api: api.into(),
            api_key: key.map(String::from),
            models: models.iter().map(|m| m.to_string()).collect(),
            key_from_library: None, key_from_sync: None,
            official_auth: None,
        }
    }

    fn model(id: &str, name: Option<&str>, ctx: Option<u64>) -> ModelInput {
        ModelInput { id: id.into(), name: name.map(String::from), context: ctx, ..Default::default() }
    }

    #[test]
    fn reads_providers_and_models() {
        let _home = setup("read", false, Some(SAMPLE));
        let s = st();
        assert_eq!(s.mode, "multi");
        assert!(!s.readonly);
        assert_eq!(s.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["moonshot", "relay", "envy"]);
        let m = prov(&s, "moonshot");
        assert!(m.builtin && m.has_key && m.api == "chat");
        assert!(m.models[0].tags.contains(&Tag::default_model()));
        assert_eq!(m.models[0].name.as_deref(), Some("kimi-k2-0905-preview"));
        assert_eq!(m.models[0].ctx.as_deref(), Some("262K"));
        assert_eq!(m.models[0].extra.get("capabilities"), Some(&json!(["thinking"])));
        assert_eq!(m.models[0].tags, [Tag::default_model()]);
        let r = prov(&s, "relay");
        assert!(r.models[0].extra.is_empty());
        assert!(!r.builtin && r.has_key);
        assert_eq!(r.models.iter().map(|m| (m.id.as_str(), m.name.as_deref())).collect::<Vec<_>>(), [("gpt-4.1", None), ("relay-mini", Some("gpt-4.1-mini"))]);
        let e = prov(&s, "envy");
        assert!(!e.has_key && e.api == "anthropic");
        assert!(s.current.iter().any(|kv| kv.v == "kimi-k2"));
        assert!(s.notes.iter().any(|n| n.contains("/login 和 /model")));
        let (base, key, api) = provider_endpoint("relay").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://relay.example.com/v1", Some("sk-relay-9999"), "chat"));
    }

    #[test]
    fn create_provider_with_models() {
        let _home = setup("create", false, Some(SAMPLE));
        let d = apply(vec![Op::UpsertProvider { provider: input(None, "My Relay", "https://my.relay/v1", "responses", Some(SECRET), &["gpt-4.1", "o3"]) }]);
        let all = lines(&d);
        assert!(!all.contains(SECRET), "{all}");
        assert!(all.contains(&mask_key(SECRET)));
        let t = text();
        // Every original line survives, in order; the new provider sits after the others.
        let mut rest = t.lines();
        assert!(SAMPLE.lines().all(|l| rest.any(|x| x == l)), "existing content changed:\n{t}");
        assert!(t.find("[providers.envy]").unwrap() < t.find("[providers.my-relay]").unwrap());
        assert!(t.find("[providers.my-relay]").unwrap() < t.find("[models.kimi-k2]").unwrap());
        let doc: DocumentMut = t.parse().unwrap();
        assert_eq!(doc["providers"]["my-relay"]["type"].as_str(), Some("openai_responses"));
        assert_eq!(doc["providers"]["my-relay"]["api_key"].as_str(), Some(SECRET));
        // "gpt-4.1" is taken by relay: the new one gets a prefixed key.
        assert_eq!(doc["models"]["my-relay/gpt-4.1"]["model"].as_str(), Some("gpt-4.1"));
        // Filled in from the model catalog (not the DEFAULT_CTX a bare id gets).
        assert_ne!(doc["models"]["o3"]["max_context_size"].as_integer(), Some(DEFAULT_CTX as i64));
        let p = prov(&st(), "my-relay");
        assert_eq!((p.name.as_str(), p.api.as_str(), p.models.len()), ("My Relay", "responses", 2));
    }

    #[test]
    fn missing_file_and_legacy_types() {
        let t = setup("legacy", true, None);
        let s = st();
        assert!(s.providers.is_empty() && !s.readonly);
        assert!(s.config_dir.ends_with(".kimi"));
        apply(vec![
            Op::UpsertProvider { provider: input(None, "Chat", "https://c/v1", "chat", Some(SECRET), &["m1"]) },
            Op::UpsertProvider { provider: input(None, "Gem", "https://g", "gemini", None, &[]) },
        ]);
        let doc: DocumentMut = fs::read_to_string(t.0.join(".kimi").join("config.toml")).unwrap().parse().unwrap();
        assert_eq!(doc["providers"]["chat"]["type"].as_str(), Some("openai_legacy"));
        assert_eq!(doc["providers"]["gem"]["type"].as_str(), Some("gemini"));
        assert_eq!(doc["models"]["m1"]["provider"].as_str(), Some("chat"));
        assert!(text().find("[providers.chat]").unwrap() < text().find("[models.m1]").unwrap());
    }

    #[test]
    fn edit_provider_keeps_comments() {
        let _home = setup("edit", false, Some(SAMPLE));
        let d = apply(vec![Op::UpsertProvider { provider: input(Some("relay"), "Relay", "https://relay2.example.com/v1", "responses", Some(SECRET), &[]) }]);
        assert!(!lines(&d).contains(SECRET));
        let t = text();
        assert!(t.contains("[providers.relay] # my relay\n"));
        assert!(t.contains("base_url = \"https://relay2.example.com/v1\" # primary endpoint\n"));
        assert!(t.contains("type = \"openai_responses\"\n"));
        assert!(t.contains(&format!("api_key = \"{SECRET}\"")));
        assert_eq!(prov(&st(), "relay").name, "Relay");
        // api_key_env providers keep reading from the environment.
        assert!(plan(&[Op::UpsertProvider { provider: input(Some("envy"), "envy", "https://anthropic.example.com", "anthropic", Some(SECRET), &[]) }], true).is_err());
        assert!(plan(&[Op::UpsertProvider { provider: input(Some("envy"), "Envy", "https://a2.example.com", "anthropic", None, &[]) }], false).is_ok());
        assert!(text().contains("api_key_env = \"AGENTPLUS_TEST_KIMI_UNSET_VAR\""));
    }

    #[test]
    fn hide_show_keeps_table_and_comment() {
        let _home = setup("hide", false, Some(SAMPLE));
        apply(vec![Op::SetModelVisible { provider: "relay".into(), model: "gpt-4.1".into(), visible: false }]);
        let t = text();
        assert!(!t.contains("gpt-4.1\"]") && !t.contains("# GPT via relay"));
        assert!(t.contains("[models.relay-mini]"));
        let p = prov(&st(), "relay");
        assert_eq!(p.models.iter().map(|m| (m.id.as_str(), m.visible)).collect::<Vec<_>>(), [("relay-mini", true), ("gpt-4.1", false)]);
        // Edit while hidden, then bring it back.
        apply(vec![Op::UpsertModel { provider: "relay".into(), model: model("gpt-4.1", None, Some(1_000_000)) }]);
        apply(vec![Op::SetModelVisible { provider: "relay".into(), model: "gpt-4.1".into(), visible: true }]);
        let t = text();
        assert!(t.contains("# GPT via relay\n[models.\"gpt-4.1\"]\nprovider = \"relay\"\nmodel = \"gpt-4.1\"\nmax_context_size = 1000000\n"), "{t}");
        assert!(t.contains("[loop_control]\nmax_steps_per_run = 100 # keep\n"));
        assert!(store_obj(&store::load(), "hiddenModels").is_empty());
        // The default model cannot be hidden or deleted.
        assert!(plan(&[Op::SetModelVisible { provider: "moonshot".into(), model: "kimi-k2".into(), visible: false }], true).is_err());
        assert!(plan(&[Op::DeleteModel { provider: "moonshot".into(), model: "kimi-k2".into() }], true).is_err());
    }

    #[test]
    fn add_edit_delete_models() {
        let _home = setup("models", false, Some(SAMPLE));
        apply(vec![
            Op::UpsertModel { provider: "relay".into(), model: model("o3", None, Some(200_000)) },
            Op::UpsertModel { provider: "relay".into(), model: model("relay-mini", Some("gpt-4.1-nano"), Some(64_000)) },
        ]);
        let doc: DocumentMut = text().parse().unwrap();
        assert_eq!(doc["models"]["o3"]["max_context_size"].as_integer(), Some(200_000));
        assert_eq!(doc["models"]["relay-mini"]["model"].as_str(), Some("gpt-4.1-nano"));
        assert_eq!(doc["models"]["relay-mini"]["max_context_size"].as_integer(), Some(64_000));
        apply(vec![Op::DeleteModel { provider: "relay".into(), model: "o3".into() }]);
        assert!(!text().contains("[models.o3]"));
        // Replace the list: match by key or upstream name, keep the default model.
        apply(vec![Op::SetProviderModels { provider: "relay".into(), models: vec!["gpt-4.1-nano".into(), "gpt-5".into()] }]);
        let ids: Vec<String> = prov(&st(), "relay").models.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, ["relay-mini", "gpt-5"]);
    }

    #[test]
    fn added_model_name_is_its_upstream_model() {
        let _home = setup("add-upstream", false, Some(SAMPLE));
        let d = apply(vec![
            Op::UpsertModel { provider: "relay".into(), model: model("k", Some("gpt-x"), None) },
            Op::UpsertModel { provider: "relay".into(), model: model("plain", Some("  "), None) },
            // Key taken by another provider: prefixed key, the name still goes upstream.
            Op::UpsertModel { provider: "envy".into(), model: model("gpt-4.1", Some("claude-x"), None) },
        ]);
        let doc: DocumentMut = text().parse().unwrap();
        assert_eq!(doc["models"]["k"]["model"].as_str(), Some("gpt-x"));
        assert_eq!(doc["models"]["k"]["provider"].as_str(), Some("relay"));
        assert_eq!(doc["models"]["plain"]["model"].as_str(), Some("plain"));
        assert_eq!(doc["models"]["envy/gpt-4.1"]["model"].as_str(), Some("claude-x"));
        assert_eq!(doc["models"]["gpt-4.1"]["model"].as_str(), Some("gpt-4.1"));
        assert!(lines(&d).contains("+ [models.\"k\"] provider = \"relay\", model = \"gpt-x\""), "{}", lines(&d));
        let m = prov(&st(), "relay").models.into_iter().find(|m| m.id == "k").unwrap();
        assert_eq!(m.name.as_deref(), Some("gpt-x"));
    }

    #[test]
    fn disable_enable_and_delete_provider() {
        let _home = setup("enable", false, Some(SAMPLE));
        apply(vec![Op::SetModelVisible { provider: "relay".into(), model: "relay-mini".into(), visible: false }]);
        apply(vec![Op::SetProviderEnabled { provider: "relay".into(), enabled: false }]);
        let t = text();
        assert!(!t.contains("[providers.relay]") && !t.contains("gpt-4.1\"]"));
        let p = prov(&st(), "relay");
        assert!(!p.enabled && p.has_key);
        assert_eq!(p.models.len(), 2);
        assert!(plan(&[Op::UpsertModel { provider: "relay".into(), model: model("x", None, None) }], true).is_err());
        assert_eq!(provider_endpoint("relay").unwrap().1.as_deref(), Some("sk-relay-9999"));
        apply(vec![Op::UpsertProvider { provider: input(Some("relay"), "R", "https://r2/v1", "chat", None, &[]) }]);
        apply(vec![Op::SetProviderEnabled { provider: "relay".into(), enabled: true }]);
        let t = text();
        assert!(t.contains("[providers.relay] # my relay\ntype = \"openai\"\nbase_url = \"https://r2/v1\" # primary endpoint\n"), "{t}");
        assert!(t.contains("# GPT via relay\n[models.\"gpt-4.1\"]"));
        let p = prov(&st(), "relay");
        assert!(p.enabled);
        assert_eq!(p.models.iter().filter(|m| !m.visible).count(), 1);
        // The default model's provider can't be disabled or deleted.
        assert!(plan(&[Op::SetProviderEnabled { provider: "moonshot".into(), enabled: false }], true).is_err());
        assert!(plan(&[Op::DeleteProvider { provider: "moonshot".into() }], true).is_err());
        apply(vec![Op::DeleteProvider { provider: "relay".into() }]);
        let t = text();
        assert!(!t.contains("relay"), "{t}");
        assert!(store_obj(&store::load(), "hiddenModels").is_empty());
        assert!(st().providers.iter().all(|p| p.id != "relay"));
    }

    #[test]
    fn stash_never_overwrites_a_hand_written_key() {
        let _home = setup("clash", false, Some(SAMPLE));
        apply(vec![Op::SetModelVisible { provider: "relay".into(), model: "relay-mini".into(), visible: false }]);
        // The same key comes back by hand, now for another provider: hiding it must not
        // replace relay's stashed model.
        fs::write(config_path(), format!("{}\n[models.relay-mini]\nprovider = \"moonshot\"\nmodel = \"moon-mini\"\nmax_context_size = 8000\n", text())).unwrap();
        assert!(plan(&[Op::SetModelVisible { provider: "moonshot".into(), model: "relay-mini".into(), visible: false }], false).is_err());
        let hidden = store_obj(&store::load(), "hiddenModels");
        assert_eq!(hidden["relay-mini"]["provider"], "relay");
        assert!(stash_text(&hidden["relay-mini"]).contains("gpt-4.1-mini"));
        // A disabled provider whose [providers.<id>] was written again by hand stays disabled.
        apply(vec![Op::SetProviderEnabled { provider: "envy".into(), enabled: false }]);
        let by_hand = format!("{}\n[providers.envy]\ntype = \"openai\"\nbase_url = \"https://hand.example/v1\"\napi_key = \"sk-hand\"\n", text());
        fs::write(config_path(), &by_hand).unwrap();
        assert!(plan(&[Op::SetProviderEnabled { provider: "envy".into(), enabled: true }], false).is_err());
        assert_eq!(text(), by_hand);
        assert!(store_obj(&store::load(), "disabledProviders").contains_key("envy"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let t = setup("dry", false, Some(SAMPLE));
        let (d, w, b) = plan(
            &[
                Op::UpsertProvider { provider: input(None, "X", "https://x/v1", "chat", Some(SECRET), &["a"]) },
                Op::SetModelVisible { provider: "relay".into(), model: "gpt-4.1".into(), visible: false },
                Op::SetProviderEnabled { provider: "envy".into(), enabled: false },
            ],
            true,
        )
        .unwrap();
        assert!(w.is_empty() && b.is_none() && !lines(&d).is_empty());
        assert!(!lines(&d).contains(SECRET));
        assert_eq!(text(), SAMPLE);
        assert!(!t.0.join(".agentplus").exists(), "no store or backup");
        assert!(plan(&[Op::SetCurrentProvider { provider: "relay".into() }], true).is_err());
        assert!(plan(&[Op::SetModelRoles { provider: "relay".into(), roles: Default::default() }], true).is_err());
        assert!(plan(&[Op::SetSetting { key: "x".into(), value: json!(true) }], true).is_err());
    }

    /// Read-only: `cargo test --lib dump_kimi -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_kimi() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={} dir={:?}", inst.installed, inst.version, inst.running, inst.dir);
        let s = state(&inst);
        println!("dir={} files={:?} readonly={} notes={:?}", s.config_dir, s.files, s.readonly, s.notes);
        for p in &s.providers {
            println!("  {} [{}] {:?} api={} enabled={} builtin={} has_key={} models={:?}", p.id, p.name, p.base_url, p.api, p.enabled, p.builtin, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        println!("current={:?}", s.current.iter().map(|k| format!("{}={}", k.k, k.v)).collect::<Vec<_>>());
        if s.readonly {
            println!("readonly: skip dry-run plan");
            return;
        }
        let op = Op::UpsertProvider { provider: input(None, "Dry Run", "https://dry.example/v1", "chat", Some("sk-dry-run-0000"), &["m"]) };
        let (d, written, backup) = plan(&[op], true).unwrap();
        assert!(written.is_empty() && backup.is_none());
        println!("dry-run diff:\n{}", lines(&d));
    }
}
