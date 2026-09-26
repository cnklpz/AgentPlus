//! CodeBuddy (Tencent; the desktop IDE and the `codebuddy` CLI): custom models in
//! `~/.codebuddy/models.json` (dir overridable with `CODEBUDDY_CONFIG_DIR`), read by both
//! and hot-reloaded within about a second:
//! `{ models: [{ id, name, vendor, url, apiKey, maxInputTokens, maxOutputTokens,
//! supportsToolCall, supportsImages, supportsReasoning }], availableModels: [ids] }`.
//!
//! Only OpenAI Chat Completions is supported and `url` is the FULL `…/chat/completions`
//! endpoint (AgentPlus shows and takes the base URL). One AgentPlus provider = the entries
//! sharing (base URL, apiKey, vendor); the vendor is the provider's name.
//! `availableModels` is the native visibility list: hiding removes the id from it and
//! keeps the entry. Disabled providers are moved out and parked in the AgentPlus store.

use super::keyref::{self, host, resolve_key, set_or_remove, Group, Key};
use super::msg;
use super::{Plan, Endpoint};
use crate::i18n::l;
use crate::mfields;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const ID: &str = "codebuddy";
pub const NAME: &str = "CodeBuddy";
pub const MARKER: &str = "settings.json";
pub const WSL_SCRIPT: &str = "codebuddy --version 2>/dev/null | head -n 1; pgrep -x codebuddy >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".codebuddy";

const SUFFIX: &str = "/chat/completions";
const STORE_LABEL: &str = "AgentPlus · CodeBuddy";

/// `CODEBUDDY_CONFIG_DIR` (Windows side only), else `~/.codebuddy`.
pub fn default_dir() -> PathBuf {
    if let Some(d) = crate::env::agent_var("CODEBUDDY_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    home().join(".codebuddy")
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn models_path() -> PathBuf {
    dir().join("models.json")
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

// ---------- detection ----------

/// The desktop IDE (registry uninstall entry "CodeBuddy …" on Windows, its app bundle on
/// macOS), else the npm CLI.
pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some(c) = crate::process::app_bundles(&["com.tencent.codebuddy", "com.tencent.codebuddycn", "CodeBuddy.app", "CodeBuddy CN.app"]).first() {
        inst.installed = true;
        inst.version = c.version.clone();
        inst.dir = crate::process::app_dir(&c.exe);
        inst.exe = Some(c.exe.clone());
    } else if let Some(e) = crate::process::uninstall_entry("CodeBuddy") {
        let exe = e
            .icon
            .and_then(|i| crate::process::unquote_exe(&i))
            .filter(|p| crate::process::is_exe(p))
            .or_else(|| e.location.map(|l| PathBuf::from(l.trim()).join("CodeBuddy.exe")))
            .filter(|p| p.is_file());
        if let Some(exe) = exe {
            inst.installed = true;
            inst.version = e.version;
            inst.dir = exe.parent().map(Path::to_path_buf);
            inst.exe = Some(exe);
        }
    }
    if !inst.installed {
        // The CLI: npm, or the native installer's binary on PATH (~/.local/bin).
        let shim = crate::process::on_path(&["codebuddy.exe", "codebuddy.cmd"]);
        if let Some(v) = crate::process::npm_version_near("@tencent-ai/codebuddy-code", shim.as_deref()) {
            inst.installed = true;
            inst.version = Some(v);
        } else if let Some(p) = shim {
            inst.installed = true;
            inst.version = crate::process::cli_version(&p);
        }
    }
    // The IDE's own processes, the same ones a restart stops.
    crate::process::set_app_running(&mut inst);
    inst
}

// ---------- format helpers ----------

/// Full `…/chat/completions` URL → base URL.
fn base_of(url: &str) -> String {
    let t = url.trim().trim_end_matches('/');
    t.strip_suffix(SUFFIX).unwrap_or(t).trim_end_matches('/').to_string()
}

/// Base URL → the full URL CodeBuddy calls.
fn url_of(base: &str) -> String {
    let t = base.trim().trim_end_matches('/');
    if t.ends_with(SUFFIX) { t.to_string() } else { format!("{t}{SUFFIX}") }
}

/// Groups entries by (base URL, apiKey, vendor).
fn key_of(e: &Value) -> Key {
    (base_of(&str_field(e, "url")), str_field(e, "apiKey"), str_field(e, "vendor"))
}

/// Groups in order of first appearance (active entries first, then parked ones).
fn groups_of(entries: &[Value], parked: &[Value]) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    for e in entries.iter().chain(parked.iter().filter_map(|p| p.get("entry"))) {
        let k = key_of(e);
        if out.iter().any(|g| g.key == k) {
            continue;
        }
        let vendor = if k.2.trim().is_empty() { host(&k.0) } else { k.2.trim().to_string() };
        let id = unique_id(&slug(&vendor), |c| out.iter().any(|g| g.id == c));
        out.push(Group { id, key: k, name: vendor });
    }
    // Same vendor twice: tell them apart by host.
    let dup: Vec<String> = out.iter().filter(|g| out.iter().filter(|o| o.name == g.name).count() > 1).map(|g| g.id.clone()).collect();
    for g in out.iter_mut().filter(|g| dup.contains(&g.id)) {
        g.name = format!("{} · {}", g.name, host(&g.key.0));
    }
    out
}

fn parked_of(root: &Value) -> Vec<Value> {
    store::get_arr(root, ID, "parked")
}

/// (models.json, meta, had_comments); a missing file reads as `{"models": []}`.
fn load_models() -> Result<(Value, TextMeta, bool)> {
    read_jsonc_object_or(&models_path(), json!({ "models": [] }))
}

fn entries_of(cfg: &Value) -> Vec<Value> {
    cfg.get("models").and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

/// None = no `availableModels` (every model shows).
fn available_of(cfg: &Value) -> Option<Vec<String>> {
    str_list(cfg.get("availableModels"))
}

fn default_model() -> Option<String> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    let v: Value = serde_json::from_str(&strip_jsonc(&text).0).ok()?;
    v.get("model").and_then(|x| x.as_str()).map(String::from).filter(|m| !m.is_empty())
}

fn model_of(e: &Value, visible: bool) -> Model {
    let ctx = e.get("maxInputTokens").and_then(|x| x.as_u64());
    let mut tags = vec![];
    if e.get("supportsImages").and_then(|x| x.as_bool()) == Some(true) {
        tags.push(Tag::new("cap:image", l("Images", "图片")));
    }
    if e.get("supportsReasoning").and_then(|x| x.as_bool()) == Some(true) {
        tags.push(Tag::new("cap:reasoning", l("Reasoning", "推理")));
    }
    let name = str_field(e, "name");
    Model {
        id: str_field(e, "id"),
        visible,
        readonly: false,
        tags,
        ctx: ctx.map(fmt_ctx),
        name: (!name.trim().is_empty() && name != str_field(e, "id")).then_some(name),
        context: ctx,
        deletable: true,
        extra: mfields::read(e, mfields::CODEBUDDY),
    }
}

fn provider_of(g: &Group, entries: &[Value], parked: &[Value], avail: Option<&Vec<String>>) -> Provider {
    let mine = |e: &Value| key_of(e) == g.key;
    let active: Vec<&Value> = entries.iter().filter(|e| mine(e)).collect();
    let enabled = !active.is_empty();
    let mut models: Vec<Model> = active.iter().map(|e| model_of(e, avail.map(|a| a.contains(&str_field(e, "id"))).unwrap_or(true))).collect();
    if !enabled {
        for p in parked {
            if let Some(e) = p.get("entry").filter(|e| mine(e)) {
                models.push(model_of(e, p.get("visible").and_then(|x| x.as_bool()).unwrap_or(true)));
            }
        }
    }
    Provider {
        id: g.id.clone(),
        name: g.name.clone(),
        base_url: Some(g.key.0.clone()),
        host: host_of(&g.key.0),
        apis: vec![api_label("chat").into()],
        enabled,
        compatible: true,
        models,
        details: vec![
            Kv::mono("vendor", if g.key.2.is_empty() { "-".into() } else { g.key.2.clone() }),
            Kv::mono("url", url_of(&g.key.0)),
            Kv::text(lbl::api_key(), keyref::key_note(&g.key.1, "models.json")),
            Kv::text(lbl::status(), if enabled { l("Enabled", "已启用") } else { l("Disabled · entries parked in AgentPlus", "已停用 · 条目暂存在 AgentPlus") }),
        ],
        editable: true,
        api: "chat".into(),
        has_key: keyref::has_key(&g.key.1),
        ..Default::default()
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&models_path())]);
    let cfg = match load_models() {
        Ok((cfg, _, had)) => {
            if had {
                st.readonly = true;
                st.notes.push(msg::comments_readonly("models.json"));
            }
            cfg
        }
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let root = store::load();
    let entries = entries_of(&cfg);
    let parked = parked_of(&root);
    let avail = available_of(&cfg);
    for g in groups_of(&entries, &parked) {
        st.providers.push(provider_of(&g, &entries, &parked, avail.as_ref()));
    }
    st.current = vec![
        Kv::mono(lbl::default_model(), default_model().unwrap_or_else(|| l("- (CodeBuddy default)", "-（CodeBuddy 默认）").into())),
        Kv::text(lbl::custom_models(), tr!("{}", "{} 个", entries.len())),
        Kv::text("availableModels", match &avail {
            Some(a) => tr!("{} (only listed models are shown)", "{} 个（只显示列出的模型）", a.len()),
            None => l("Not set (all shown)", "未设置（全部显示）").into(),
        }),
        Kv::mono(lbl::config_file(), display_path(&models_path())),
    ];
    st.notes.push(l("CodeBuddy custom models only support the OpenAI Chat Completions API; other protocols can be converted through the AgentPlus local gateway.", "CodeBuddy 的自定义模型只支持 OpenAI Chat Completions 接口；其他协议可经 AgentPlus 本地网关转换后接入。").into());
    st.notes.push(l("Changes to models.json take effect in about a second (shared by the CLI and the IDE).", "models.json 改动约 1 秒后自动生效（CLI 与 IDE 共用）。").into());
    st.notes.push(l("A project's availableModels replaces the user list entirely.", "项目里的 availableModels 会整体覆盖用户列表。").into());
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _, _) = load_models()?;
    let g = groups_of(&entries_of(&cfg), &parked_of(&store::load())).into_iter().find(|g| g.id == id).ok_or_else(|| msg::no_provider(id))?;
    if g.key.0.is_empty() {
        return Err(anyhow!(tr!("Provider {id} has no url", "供应商 {id} 没有 url")));
    }
    Ok((g.key.0, resolve_key(&g.key.1), "chat".into()))
}

fn chat_only(api: &str) -> Result<()> {
    if api == "chat" {
        Ok(())
    } else {
        Err(anyhow!(tr!("CodeBuddy custom models only support the OpenAI Chat Completions API; connect {api} relays to the AgentPlus local gateway first, then add them with the Chat API", "CodeBuddy 的自定义模型只支持 OpenAI Chat Completions 接口；{api} 协议的中转请先接入 AgentPlus 本地网关，再以 Chat 接口添加")))
    }
}

struct Work {
    entries: Vec<Value>,
    avail: Option<Vec<String>>,
    parked: Vec<Value>,
    groups: Vec<Group>,
    diff: Diff,
    file: String,
}

impl Work {
    fn group(&self, id: &str) -> Result<Group> {
        self.groups.iter().find(|g| g.id == id).cloned().ok_or_else(|| msg::no_provider(id))
    }

    fn enabled(&self, g: &Group) -> bool {
        self.entries.iter().any(|e| key_of(e) == g.key)
    }

    fn require_enabled(&self, g: &Group) -> Result<()> {
        keyref::require_enabled(g, self.enabled(g))
    }

    /// Model ids are global in CodeBuddy (selection and availableModels use them).
    fn owner_of(&self, mid: &str) -> Option<Key> {
        self.entries.iter().chain(self.parked.iter().filter_map(|p| p.get("entry"))).find(|e| str_field(e, "id") == mid).map(key_of)
    }

    fn check_free(&self, g: &Group, mid: &str) -> Result<()> {
        match self.owner_of(mid) {
            Some(k) if k != g.key => {
                let other = self.groups.iter().find(|x| x.key == k).map(|x| x.name.clone()).unwrap_or_default();
                Err(anyhow!(tr!("Model ID {mid} is already used by provider \"{other}\"; CodeBuddy model IDs must be unique", "模型 ID {mid} 已被供应商「{other}」使用；CodeBuddy 的模型 ID 必须唯一")))
            }
            _ => Ok(()),
        }
    }

    fn new_entry(key: &Key, mid: &str, name: Option<&str>, context: Option<u64>) -> Value {
        let mut e = json!({
            "id": mid,
            "name": name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(mid),
            "vendor": "",
            "url": url_of(&key.0),
            "apiKey": "",
            "maxInputTokens": context.unwrap_or(128000),
            "maxOutputTokens": 8192,
            "supportsToolCall": true,
            "supportsImages": false,
        });
        set_or_remove(&mut e, "vendor", &key.2);
        set_or_remove(&mut e, "apiKey", &key.1);
        e
    }

    fn show(&mut self, mid: &str) {
        if let Some(a) = self.avail.as_mut() {
            if !a.iter().any(|x| x == mid) {
                a.push(mid.to_string());
            }
        }
    }

    fn unlist(&mut self, mid: &str) {
        if let Some(a) = self.avail.as_mut() {
            a.retain(|x| x != mid);
        }
    }

    fn add_model(&mut self, g: &Group, mid: &str, name: Option<&str>, context: Option<u64>) -> Result<()> {
        self.check_free(g, mid)?;
        self.entries.push(Self::new_entry(&g.key, mid, name, context));
        self.show(mid);
        let file = self.file.clone();
        self.diff.push(&file, tr!("models + {mid} ({})", "models + {mid}（{}）", g.name), true);
        Ok(())
    }

    fn apply(&mut self, op: &Op) -> Result<()> {
        let file = self.file.clone();
        match op {
            Op::UpsertProvider { provider: p } => {
                chat_only(&p.api)?;
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                let base = base_of(&p.base_url);
                let new_key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                match &p.id {
                    None => {
                        let vendor = p.name.trim().to_string();
                        let models = clean_ids(&p.models);
                        if models.is_empty() {
                            return Err(anyhow!(l("Each CodeBuddy model entry carries its own base URL and API key: add at least one model when creating a provider", "CodeBuddy 的每个模型条目自带地址和密钥：新建供应商时至少要填一个模型")));
                        }
                        let key: Key = (base.clone(), new_key.clone().unwrap_or_default(), vendor.clone());
                        let g = match self.groups.iter().find(|g| g.key == key) {
                            // Same as an existing provider: add to it, unless it is disabled
                            // (its entries are parked; adding would orphan them).
                            Some(g) => {
                                let g = g.clone();
                                self.require_enabled(&g)?;
                                g
                            }
                            None => {
                                let id = unique_id(&slug(&vendor), |c| self.groups.iter().any(|g| g.id == c));
                                let g = Group { id, key: key.clone(), name: vendor.clone() };
                                self.groups.push(g.clone());
                                g
                            }
                        };
                        for m in &models {
                            self.check_free(&g, m)?;
                        }
                        // Models the provider already has are left as they are.
                        let added: Vec<String> = models.iter().filter(|m| self.owner_of(m).is_none()).cloned().collect();
                        if !added.is_empty() {
                            self.diff.push(&file, trn!(added.len(), "+ \"{vendor}\" {n} model ({}{})", "+ \"{vendor}\" {n} models ({}{})", "+ 「{vendor}」{n} 个模型（{}{}）", url_of(&base), msg::key_suffix(new_key.as_deref())), true);
                        }
                        for m in &added {
                            self.entries.push(Self::new_entry(&key, m, None, None));
                            self.show(m);
                        }
                        for op in crate::modelinfo::seed_ops(ID, &g.id, &added) {
                            self.apply(&op)?;
                        }
                    }
                    Some(id) => {
                        let g = self.group(id)?;
                        // The name shown may not be the vendor (a host for an empty vendor,
                        // " · host" for a shared one): unchanged, it keeps the vendor as is.
                        let renamed = p.name.trim() != g.name;
                        let vendor = if renamed { p.name.trim().to_string() } else { g.key.2.clone() };
                        let key: Key = (base.clone(), new_key.clone().unwrap_or_else(|| g.key.1.clone()), vendor);
                        if key == g.key {
                            return Ok(());
                        }
                        if self.groups.iter().any(|o| o.id != g.id && o.key == key) {
                            return Err(anyhow!(l("A provider with the same name, base URL and API key already exists", "已经有名称、地址和密钥都相同的供应商")));
                        }
                        if key.0 != g.key.0 {
                            self.diff.push(&file, tr!("\"{}\" all models url = {}", "「{}」所有模型 url = {}", g.name, url_of(&key.0)), true);
                        }
                        if key.1 != g.key.1 {
                            self.diff.push(&file, tr!("\"{}\" all models apiKey = {}", "「{}」所有模型 apiKey = {}", g.name, mask_key(&key.1)), true);
                        }
                        if key.2 != g.key.2 {
                            self.diff.push(&file, tr!("\"{}\" all models vendor = {}", "「{}」所有模型 vendor = {}", g.name, key.2), true);
                        }
                        // Only the changed fields: no `"apiKey": ""` / `"vendor": ""` appears.
                        let set = |e: &mut Value| {
                            if key_of(e) == g.key {
                                if key.0 != g.key.0 {
                                    e["url"] = json!(url_of(&key.0));
                                }
                                if key.1 != g.key.1 {
                                    set_or_remove(e, "apiKey", &key.1);
                                }
                                if key.2 != g.key.2 {
                                    set_or_remove(e, "vendor", &key.2);
                                }
                            }
                        };
                        self.entries.iter_mut().for_each(set);
                        self.parked.iter_mut().filter_map(|p| p.get_mut("entry")).for_each(set);
                        if let Some(x) = self.groups.iter_mut().find(|x| x.id == g.id) {
                            if renamed {
                                x.name = key.2.clone();
                            }
                            x.key = key;
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let g = self.group(provider)?;
                let gone: Vec<String> = self.entries.iter().filter(|e| key_of(e) == g.key).map(|e| str_field(e, "id")).collect();
                self.entries.retain(|e| key_of(e) != g.key);
                for m in &gone {
                    self.unlist(m);
                }
                let n = self.parked.len();
                self.parked.retain(|p| p.get("entry").map(|e| key_of(e) != g.key).unwrap_or(true));
                let label = if gone.is_empty() && n != self.parked.len() { STORE_LABEL.to_string() } else { file.clone() };
                self.diff.push(&label, trn!(gone.len().max(n - self.parked.len()), "- \"{}\" ({n} model, with base URL and API key)", "- \"{}\" ({n} models, with base URL and API key)", "- 「{}」（{n} 个模型，含地址和密钥）", g.name), false);
                self.groups.retain(|x| x.id != g.id);
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let g = self.group(provider)?;
                if *enabled {
                    let (back, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.parked).into_iter().partition(|p| p.get("entry").map(|e| key_of(e) == g.key).unwrap_or(false));
                    self.parked = keep;
                    if !back.is_empty() {
                        self.diff.push(&file, trn!(back.len(), "+ \"{}\" {n} model (restored from AgentPlus)", "+ \"{}\" {n} models (restored from AgentPlus)", "+ 「{}」{n} 个模型（从 AgentPlus 恢复）", g.name), true);
                    }
                    for p in back {
                        let Some(e) = p.get("entry").cloned() else { continue };
                        let mid = str_field(&e, "id");
                        if self.owner_of(&mid).is_some() {
                            return Err(anyhow!(tr!("Model ID {mid} is used by another provider; can't restore \"{}\"", "模型 ID {mid} 已被其他供应商使用，无法恢复「{}」", g.name)));
                        }
                        if p.get("visible").and_then(|x| x.as_bool()).unwrap_or(true) {
                            self.show(&mid);
                        }
                        self.entries.push(e);
                    }
                } else {
                    let (out, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.entries).into_iter().partition(|e| key_of(e) == g.key);
                    self.entries = keep;
                    if !out.is_empty() {
                        self.diff.push(&file, trn!(out.len(), "- \"{}\" {n} model (parked in AgentPlus, restorable)", "- \"{}\" {n} models (parked in AgentPlus, restorable)", "- 「{}」{n} 个模型（暂存在 AgentPlus，可恢复）", g.name), false);
                    }
                    for e in out {
                        let mid = str_field(&e, "id");
                        let visible = self.avail.as_ref().map(|a| a.contains(&mid)).unwrap_or(true);
                        self.unlist(&mid);
                        self.parked.push(json!({ "visible": visible, "entry": e }));
                    }
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let g = self.group(provider)?;
                self.require_enabled(&g)?;
                if !self.entries.iter().any(|e| key_of(e) == g.key && &str_field(e, "id") == model) {
                    return Err(anyhow!(tr!("\"{}\" has no model {model}", "「{}」里没有模型 {model}", g.name)));
                }
                let now = self.avail.as_ref().map(|a| a.contains(model)).unwrap_or(true);
                if now == *visible {
                    return Ok(());
                }
                if *visible {
                    self.show(model);
                    self.diff.push(&file, format!("availableModels + {model}"), true);
                } else {
                    if self.avail.is_none() {
                        // First hide: create the list with every custom model that shows today.
                        self.avail = Some(self.entries.iter().map(|e| str_field(e, "id")).collect());
                        self.diff.push(&file, l("+ availableModels (from now on only listed models show; add built-in models by hand if you want them)", "+ availableModels（之后只显示列出的模型；内置模型如需显示请手动加入）"), true);
                    }
                    self.unlist(model);
                    self.diff.push(&file, tr!("availableModels - {model} (hidden, entry kept)", "availableModels - {model}（隐藏，条目保留）"), false);
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let g = self.group(provider)?;
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(msg::model_id_required());
                }
                self.check_free(&g, &mid)?;
                for (k, v) in &m.extra {
                    mfields::check(mfields::CODEBUDDY, k, v)?;
                }
                if self.owner_of(&mid).is_none() {
                    self.require_enabled(&g)?;
                    self.add_model(&g, &mid, m.name.as_deref(), m.context)?;
                    if let Some(e) = self.entries.last_mut() {
                        for l in mfields::write(e, mfields::CODEBUDDY, &m.extra)? {
                            self.diff.push(&file, format!("models.{mid}.{l}"), true);
                        }
                    }
                    return Ok(());
                }
                let mut lines = vec![];
                let name = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                let mut edit = |e: &mut Value| {
                    if str_field(e, "id") != mid {
                        return;
                    }
                    if let Some(n) = name.filter(|n| str_field(e, "name") != *n) {
                        e["name"] = json!(n);
                        lines.push(format!("models.{mid}.name = {n}"));
                    }
                    if let Some(c) = m.context.filter(|c| e.get("maxInputTokens").and_then(|x| x.as_u64()) != Some(*c)) {
                        e["maxInputTokens"] = json!(c);
                        lines.push(format!("models.{mid}.maxInputTokens = {c}"));
                    }
                    for l in mfields::write(e, mfields::CODEBUDDY, &m.extra).unwrap_or_default() {
                        lines.push(format!("models.{mid}.{l}"));
                    }
                };
                let in_cfg = self.entries.iter().any(|e| str_field(e, "id") == mid);
                if in_cfg {
                    self.entries.iter_mut().for_each(&mut edit);
                } else {
                    self.parked.iter_mut().filter_map(|p| p.get_mut("entry")).for_each(&mut edit);
                }
                let label = if in_cfg { file.clone() } else { STORE_LABEL.to_string() };
                for l in lines {
                    self.diff.push(&label, l, true);
                }
            }
            Op::DeleteModel { provider, model } => {
                let g = self.group(provider)?;
                let n = self.entries.len() + self.parked.len();
                self.entries.retain(|e| !(key_of(e) == g.key && &str_field(e, "id") == model));
                self.parked.retain(|p| !p.get("entry").map(|e| key_of(e) == g.key && &str_field(e, "id") == model).unwrap_or(false));
                if self.entries.len() + self.parked.len() != n {
                    self.unlist(model);
                    self.diff.push(&file, tr!("models - {model} ({}, deleted)", "models - {model}（{}，删除）", g.name), false);
                }
            }
            Op::SetProviderModels { provider, models } => {
                let g = self.group(provider)?;
                self.require_enabled(&g)?;
                let want = clean_ids(models);
                if want.is_empty() {
                    return Err(anyhow!(l("Keep at least one model; delete the provider if you don't need it", "至少保留一个模型；不要这个供应商的话请直接删除")));
                }
                for m in &want {
                    self.check_free(&g, m)?;
                }
                let gone: Vec<String> = self.entries.iter().filter(|e| key_of(e) == g.key && !want.contains(&str_field(e, "id"))).map(|e| str_field(e, "id")).collect();
                self.entries.retain(|e| key_of(e) != g.key || want.contains(&str_field(e, "id")));
                for m in gone {
                    self.unlist(&m);
                    self.diff.push(&file, tr!("models - {m} ({})", "models - {m}（{}）", g.name), false);
                }
                for m in &want {
                    if self.owner_of(m).is_none() {
                        self.add_model(&g, m, None, None)?;
                    }
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("CodeBuddy custom models can all be enabled at once; switch models inside CodeBuddy", "CodeBuddy 的自定义模型可以同时启用，在 CodeBuddy 里切换模型"))),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
            Op::SetSetting { key, .. } => return Err(msg::unknown_setting(key)),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
        Ok(())
    }
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut cfg, meta, had_comments) = load_models()?;
    let entries0 = entries_of(&cfg);
    let avail0 = available_of(&cfg);
    let mut root = store::load();
    let parked0 = parked_of(&root);
    let groups = groups_of(&entries0, &parked0);
    let mut w = Work { entries: entries0.clone(), avail: avail0.clone(), parked: parked0.clone(), groups, diff: Diff::default(), file: display_path(&models_path()) };
    for op in ops {
        w.apply(op)?;
    }
    let cfg_dirty = w.entries != entries0 || w.avail != avail0;
    let store_dirty = w.parked != parked0;
    if cfg_dirty && had_comments {
        return Err(msg::comments_not_written("models.json"));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cfg_dirty {
            let path = models_path();
            if path.exists() {
                backup_dir = Some(backup(ID, std::slice::from_ref(&path))?);
            }
            let o = cfg.as_object_mut().unwrap();
            o.insert("models".into(), Value::Array(w.entries));
            if let Some(a) = w.avail {
                o.insert("availableModels".into(), json!(a));
            }
            std::fs::create_dir_all(dir())?;
            write_json(&path, &cfg, meta)?;
            written.push(path);
        }
        if store_dirty {
            store::set_value(&mut root, ID, "parked", Value::Array(w.parked));
            store::save(&root)?;
        }
    }
    Ok((w.diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "models": [
    {
      "id": "deepseek-chat",
      "name": "DeepSeek V3",
      "vendor": "DeepSeek",
      "url": "https://api.deepseek.com/v1/chat/completions",
      "apiKey": "sk-ds-1111",
      "maxInputTokens": 128000,
      "maxOutputTokens": 8192,
      "supportsToolCall": true,
      "supportsImages": false,
      "supportsReasoning": false
    },
    {
      "id": "deepseek-reasoner",
      "name": "DeepSeek R1",
      "vendor": "DeepSeek",
      "url": "https://api.deepseek.com/v1/chat/completions",
      "apiKey": "sk-ds-1111",
      "maxInputTokens": 64000,
      "supportsReasoning": true
    },
    {
      "id": "glm-4.6",
      "name": "GLM",
      "vendor": "Custom",
      "url": "https://open.bigmodel.cn/api/paas/v4/chat/completions",
      "apiKey": "${AGENTPLUS_CB_TEST_KEY}"
    }
  ],
  "availableModels": ["deepseek-chat", "glm-4.6", "gpt-5"]
}
"#;

    type Home = TestHome;

    fn setup(name: &str, models: Option<&str>) -> Home {
        let home = TestHome::new(&format!("codebuddy-{name}"));
        crate::env::set_test_vars(&[("AGENTPLUS_CB_TEST_KEY", "sk-env-3333")]);
        let h = home.0.clone();
        std::fs::create_dir_all(h.join(".codebuddy")).unwrap();
        std::fs::write(h.join(".codebuddy/settings.json"), r#"{"enabledPlugins":{"pdf@x":true},"model":"deepseek-chat"}"#).unwrap();
        if let Some(c) = models {
            std::fs::write(h.join(".codebuddy/models.json"), c).unwrap();
        }
        home
    }

    fn cfg_of(h: &Home) -> Value {
        serde_json::from_str(&std::fs::read_to_string(h.0.join(".codebuddy/models.json")).unwrap()).unwrap()
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.clone())).collect::<Vec<_>>().join("\n")
    }

    fn upsert(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput {
                id: id.map(String::from),
                name: name.into(),
                base_url: base.into(),
                api: api.into(),
                api_key: key.map(String::from),
                models: models.iter().map(|s| s.to_string()).collect(),
                key_from_library: None,
                official_auth: None,
            },
        }
    }

    fn ids(c: &Value) -> Vec<String> {
        c["models"].as_array().unwrap().iter().map(|e| str_field(e, "id")).collect()
    }

    #[test]
    fn reads_state() {
        let _h = setup("state", Some(SAMPLE));
        let st = state(&Install::default());
        assert!(!st.readonly);
        let pids: Vec<&str> = st.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pids, ["deepseek", "custom"]);
        let ds = &st.providers[0];
        assert_eq!(ds.base_url.as_deref(), Some("https://api.deepseek.com/v1"));
        assert_eq!(ds.models.iter().map(|m| (m.id.as_str(), m.visible)).collect::<Vec<_>>(), [("deepseek-chat", true), ("deepseek-reasoner", false)]);
        assert_eq!(ds.models[1].context, Some(64000));
        assert!(st.providers[1].has_key);
        assert_eq!(st.current[0].v, "deepseek-chat");
        let (base, key, api) = provider_endpoint("custom").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://open.bigmodel.cn/api/paas/v4", Some("sk-env-3333"), "chat"));
    }

    #[test]
    fn missing_file_create() {
        let h = setup("create", None);
        let st = state(&Install::default());
        assert!(!st.readonly && st.providers.is_empty());
        let err = plan(&[upsert(None, "Relay", "https://r.example.com/v1", "anthropic", None, &["m"])], true).err().unwrap();
        assert!(err.to_string().contains("网关"));
        let op = upsert(None, "My Relay", "https://r.example.com/v1/", "chat", Some("sk-new-abcd9876"), &["gpt-5", "gpt-5-mini"]);
        let (d, w, _) = plan(std::slice::from_ref(&op), true).unwrap();
        assert!(w.is_empty() && !h.0.join(".codebuddy/models.json").exists());
        let t = diff_text(&d);
        assert!(t.contains("••••9876") && !t.contains("abcd9876"), "{t}");
        let (_, w, _) = plan(&[op], false).unwrap();
        assert_eq!(w.len(), 1);
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["gpt-5", "gpt-5-mini"]);
        assert_eq!(c["models"][0]["url"], "https://r.example.com/v1/chat/completions");
        assert_eq!(c["models"][0]["vendor"], "My Relay");
        assert!(c.get("availableModels").is_none());
        let st = state(&Install::default());
        assert_eq!((st.providers[0].id.as_str(), st.providers[0].name.as_str()), ("my-relay", "My Relay"));
        // settings.json untouched
        assert!(std::fs::read_to_string(h.0.join(".codebuddy/settings.json")).unwrap().contains("enabledPlugins"));
    }

    #[test]
    fn hide_show_add_delete_models() {
        let h = setup("models", Some(SAMPLE));
        let ops = vec![
            Op::SetModelVisible { provider: "deepseek".into(), model: "deepseek-reasoner".into(), visible: true },
            Op::SetModelVisible { provider: "deepseek".into(), model: "deepseek-chat".into(), visible: false },
            Op::UpsertModel { provider: "deepseek".into(), model: ModelInput { id: "deepseek-v4".into(), name: Some("DS V4".into()), context: Some(1_000_000), ..Default::default() } },
            Op::UpsertModel { provider: "deepseek".into(), model: ModelInput { id: "deepseek-reasoner".into(), name: None, context: Some(128000), ..Default::default() } },
        ];
        plan(&ops, false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["deepseek-chat", "deepseek-reasoner", "glm-4.6", "deepseek-v4"]);
        assert_eq!(c["availableModels"], json!(["glm-4.6", "gpt-5", "deepseek-reasoner", "deepseek-v4"]));
        assert_eq!(c["models"][1]["maxInputTokens"], 128000);
        assert_eq!(c["models"][3]["apiKey"], "sk-ds-1111");
        assert_eq!(c["models"][3]["url"], "https://api.deepseek.com/v1/chat/completions");
        // Duplicate id across providers is refused.
        assert!(plan(&[Op::UpsertModel { provider: "custom".into(), model: ModelInput { id: "deepseek-v4".into(), name: None, context: None, ..Default::default() } }], true).is_err());
        plan(&[Op::DeleteModel { provider: "deepseek".into(), model: "deepseek-v4".into() }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["deepseek-chat", "deepseek-reasoner", "glm-4.6"]);
        assert_eq!(c["availableModels"], json!(["glm-4.6", "gpt-5", "deepseek-reasoner"]));
        plan(&[Op::SetProviderModels { provider: "deepseek".into(), models: vec!["deepseek-chat".into(), "deepseek-v3.2".into()] }], false).unwrap();
        assert_eq!(ids(&cfg_of(&h)), ["deepseek-chat", "glm-4.6", "deepseek-v3.2"]);
    }

    #[test]
    fn first_hide_creates_available_models() {
        let h = setup("avail", Some(r#"{"models":[{"id":"a","vendor":"V","url":"https://x/v1/chat/completions"},{"id":"b","vendor":"V","url":"https://x/v1/chat/completions"}]}"#));
        let (d, _, _) = plan(&[Op::SetModelVisible { provider: "v".into(), model: "a".into(), visible: false }], false).unwrap();
        assert!(diff_text(&d).contains("+ availableModels"));
        assert_eq!(cfg_of(&h)["availableModels"], json!(["b"]));
    }

    #[test]
    fn edit_disable_enable_delete_provider() {
        let h = setup("prov", Some(SAMPLE));
        let (d, _, _) = plan(&[upsert(Some("custom"), "Zhipu", "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions", "chat", Some("sk-zp-5555"), &[])], false).unwrap();
        let t = diff_text(&d);
        assert!(t.contains("••••5555") && !t.contains("sk-zp-5555"), "{t}");
        let c = cfg_of(&h);
        assert_eq!(c["models"][2]["url"], "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions");
        assert_eq!(c["models"][2]["vendor"], "Zhipu");
        assert_eq!(c["models"][2]["apiKey"], "sk-zp-5555");
        assert!(plan(&[upsert(Some("zhipu"), "Zhipu", "https://x/v1", "responses", None, &[])], true).is_err());

        plan(&[Op::SetProviderEnabled { provider: "deepseek".into(), enabled: false }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["glm-4.6"]);
        assert_eq!(c["availableModels"], json!(["glm-4.6", "gpt-5"]));
        let st = state(&Install::default());
        let ds = st.providers.iter().find(|p| p.id == "deepseek").unwrap();
        assert!(!ds.enabled && ds.models.len() == 2 && ds.models[0].visible && !ds.models[1].visible);
        assert_eq!(provider_endpoint("deepseek").unwrap().1.as_deref(), Some("sk-ds-1111"));
        assert!(plan(&[Op::SetModelVisible { provider: "deepseek".into(), model: "deepseek-chat".into(), visible: false }], true).is_err());

        plan(&[Op::SetProviderEnabled { provider: "deepseek".into(), enabled: true }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["glm-4.6", "deepseek-chat", "deepseek-reasoner"]);
        assert_eq!(c["availableModels"], json!(["glm-4.6", "gpt-5", "deepseek-chat"]));

        plan(&[Op::DeleteProvider { provider: "deepseek".into() }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["glm-4.6"]);
        assert_eq!(c["availableModels"], json!(["glm-4.6", "gpt-5"]));
        assert!(plan(&[Op::SetCurrentProvider { provider: "zhipu".into() }], true).is_err());
        assert!(plan(&[Op::SetModelRoles { provider: "zhipu".into(), roles: Default::default() }], true).is_err());
    }

    #[test]
    fn dry_run_roundtrip_and_comments() {
        let h = setup("dry", Some(SAMPLE));
        let before = std::fs::read(h.0.join(".codebuddy/models.json")).unwrap();
        let (d, w, b) = plan(&[Op::SetProviderEnabled { provider: "deepseek".into(), enabled: false }], true).unwrap();
        assert!(!d.groups.is_empty() && w.is_empty() && b.is_none());
        assert_eq!(before, std::fs::read(h.0.join(".codebuddy/models.json")).unwrap());
        assert!(!agentplus_dir().join("store.json").exists());
        // Hide + show only touches availableModels; the model entries come back unchanged.
        plan(&[Op::SetModelVisible { provider: "custom".into(), model: "glm-4.6".into(), visible: false }], false).unwrap();
        plan(&[Op::SetModelVisible { provider: "custom".into(), model: "glm-4.6".into(), visible: true }], false).unwrap();
        let after: Value = cfg_of(&h);
        assert_eq!(after["models"], serde_json::from_str::<Value>(SAMPLE).unwrap()["models"]);
        let _h2 = setup("jsonc", Some("{\n  // c\n  \"models\": []\n}"));
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[upsert(None, "x", "https://x/v1", "chat", None, &["m"])], true).is_err());
    }

    #[test]
    fn edit_with_the_shown_name_keeps_vendor() {
        // Same vendor at two URLs: shown as "DeepSeek · host"; editing the key with that name
        // must not write the display name into `vendor`.
        let h = setup("vendor", Some(r#"{"models":[{"id":"a","vendor":"DeepSeek","url":"https://api.deepseek.com/v1/chat/completions","apiKey":"sk-1"},{"id":"b","vendor":"DeepSeek","url":"https://relay.example.com/v1/chat/completions","apiKey":"sk-2"},{"id":"c","url":"https://local.example.com/v1/chat/completions"}]}"#));
        let st = state(&Install::default());
        let names: Vec<(&str, &str)> = st.providers.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
        assert_eq!(names, [("deepseek", "DeepSeek · api.deepseek.com"), ("deepseek-2", "DeepSeek · relay.example.com"), ("local-example-com", "local.example.com")]);
        let ops = [
            upsert(Some("deepseek"), "DeepSeek · api.deepseek.com", "https://api.deepseek.com/v1", "chat", Some("sk-new-1"), &[]),
            // No vendor, shown as its host: a new key keeps the entry vendor-less.
            upsert(Some("local-example-com"), "local.example.com", "https://local.example.com/v1", "chat", Some("sk-new-3"), &[]),
        ];
        let (d, _, _) = plan(&ops, false).unwrap();
        assert!(!diff_text(&d).contains("vendor"), "{}", diff_text(&d));
        let c = cfg_of(&h);
        assert_eq!(c["models"][0]["vendor"], "DeepSeek");
        assert_eq!(c["models"][0]["apiKey"], "sk-new-1");
        assert!(c["models"][2].get("vendor").is_none());
        assert_eq!(c["models"][2]["apiKey"], "sk-new-3");
        let st = state(&Install::default());
        assert_eq!(st.providers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["DeepSeek · api.deepseek.com", "DeepSeek · relay.example.com", "local.example.com"]);
        // A real rename still sets the vendor.
        plan(&[upsert(Some("deepseek-2"), "Relay", "https://relay.example.com/v1", "chat", None, &[])], false).unwrap();
        assert_eq!(cfg_of(&h)["models"][1]["vendor"], "Relay");
    }

    #[test]
    fn keyless_edit_writes_no_api_key() {
        let h = setup("keyless", Some(r#"{"models":[{"id":"qwen3","vendor":"Ollama","url":"http://localhost:11434/v1/chat/completions"}]}"#));
        let ops = [
            upsert(Some("ollama"), "Ollama", "http://localhost:11435/v1", "chat", None, &[]),
            Op::UpsertModel { provider: "ollama".into(), model: ModelInput { id: "llama4".into(), ..Default::default() } },
        ];
        plan(&ops, false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(ids(&c), ["qwen3", "llama4"]);
        for e in c["models"].as_array().unwrap() {
            assert_eq!(e["url"], "http://localhost:11435/v1/chat/completions");
            assert_eq!(e["vendor"], "Ollama");
            assert!(e.get("apiKey").is_none(), "{e}");
        }
    }

    #[test]
    fn recreating_a_disabled_provider_is_refused() {
        let h = setup("recreate", Some(SAMPLE));
        plan(&[Op::SetProviderEnabled { provider: "deepseek".into(), enabled: false }], false).unwrap();
        let store0 = std::fs::read(agentplus_dir().join("store.json")).unwrap();
        let again = upsert(None, "DeepSeek", "https://api.deepseek.com/v1", "chat", Some("sk-ds-1111"), &["deepseek-v4"]);
        let err = plan(std::slice::from_ref(&again), false).err().unwrap();
        assert!(err.to_string().contains("已停用"), "{err}");
        assert_eq!(store0, std::fs::read(agentplus_dir().join("store.json")).unwrap());
        assert_eq!(ids(&cfg_of(&h)), ["glm-4.6"]);

        // An enabled one takes the new models; the diff counts only those.
        plan(&[Op::SetProviderEnabled { provider: "deepseek".into(), enabled: true }], false).unwrap();
        let (d, _, _) = plan(&[upsert(None, "DeepSeek", "https://api.deepseek.com/v1", "chat", Some("sk-ds-1111"), &["deepseek-chat", "deepseek-v4"])], false).unwrap();
        assert!(diff_text(&d).contains("「DeepSeek」1 个模型"), "{}", diff_text(&d));
        assert_eq!(ids(&cfg_of(&h)), ["glm-4.6", "deepseek-chat", "deepseek-reasoner", "deepseek-v4"]);
    }

    /// Read-only look at the real machine: state and a dry-run plan (nothing is written).
    #[test]
    #[ignore]
    fn dump_codebuddy() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={} exe={:?} dir={:?}", inst.installed, inst.version, inst.running, inst.exe, inst.dir);
        let st = state(&inst);
        println!("dir={} files={:?} readonly={} notes={:?}", st.config_dir, st.files, st.readonly, st.notes);
        for p in &st.providers {
            println!("  provider {} ({}) api={} enabled={} has_key={} models={:?}", p.id, p.name, p.api, p.enabled, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        for kv in &st.current {
            println!("  {} = {}", kv.k, kv.v);
        }
        let (d, w, b) = plan(&[upsert(None, "Dry Run", "https://dry.example.com/v1", "chat", Some("sk-dryrun-0000"), &["m1"])], true).unwrap();
        println!("dry-run diff:\n{}", diff_text(&d));
        assert!(w.is_empty() && b.is_none());
    }
}
