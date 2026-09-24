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

#[allow(dead_code)]
pub const ID: &str = "codebuddy";
#[allow(dead_code)]
pub const NAME: &str = "CodeBuddy";
#[allow(dead_code)]
pub const MARKER: &str = "settings.json";
#[allow(dead_code)]
pub const WSL_SCRIPT: &str = "codebuddy --version 2>/dev/null | head -n 1; pgrep -x codebuddy >/dev/null && echo @running; true";
#[allow(dead_code)]
pub const WSL_MARKER: &str = ".codebuddy";

const SUFFIX: &str = "/chat/completions";
const STORE_LABEL: &str = "AgentPlus · CodeBuddy";

#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests point the adapter at a temp home (config, store and backups all inside it).
fn test_home() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_HOME.with(|t| t.borrow().clone())
    }
    #[cfg(not(test))]
    {
        None
    }
}

/// `CODEBUDDY_CONFIG_DIR` (Windows side only), else `~/.codebuddy`.
#[allow(dead_code)]
pub fn default_dir() -> PathBuf {
    if let Some(h) = test_home() {
        return h.join(".codebuddy");
    }
    if !crate::env::is_wsl() {
        if let Some(d) = std::env::var_os("CODEBUDDY_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
    }
    home().join(".codebuddy")
}

fn dir() -> PathBuf {
    if test_home().is_some() {
        return default_dir();
    }
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn models_path() -> PathBuf {
    dir().join("models.json")
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn load_root() -> Value {
    match test_home() {
        Some(h) => std::fs::read_to_string(h.join("agentplus-store.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({})),
        None => store::load(),
    }
}

fn save_root(v: &Value) -> Result<()> {
    match test_home() {
        Some(h) => Ok(std::fs::write(h.join("agentplus-store.json"), serde_json::to_string_pretty(v)?)?),
        None => store::save(v),
    }
}

fn do_backup(files: &[PathBuf]) -> Result<PathBuf> {
    match test_home() {
        Some(h) => {
            let d = h.join("agentplus-backup");
            std::fs::create_dir_all(&d)?;
            for f in files.iter().filter(|f| f.exists()) {
                std::fs::copy(f, d.join(f.file_name().unwrap()))?;
            }
            Ok(d)
        }
        None => backup(ID, files),
    }
}

// ---------- detection ----------

/// (DisplayVersion, DisplayIcon, InstallLocation) of the first uninstall entry whose
/// DisplayName starts with `prefix`.
#[cfg(windows)]
fn uninstall_entry(prefix: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    use winreg::enums::*;
    use winreg::RegKey;
    let path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(root) = RegKey::predef(hive).open_subkey(path) else { continue };
        for name in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&name) else { continue };
            let dn: String = k.get_value("DisplayName").unwrap_or_default();
            if dn.starts_with(prefix) {
                return Some((k.get_value("DisplayVersion").ok(), k.get_value("DisplayIcon").ok(), k.get_value("InstallLocation").ok()));
            }
        }
    }
    None
}
#[cfg(not(windows))]
fn uninstall_entry(_: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    None
}

fn npm_version(pkg: &str) -> Option<String> {
    let p = dirs::data_dir()?.join("npm").join("node_modules").join(pkg).join("package.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

/// The desktop IDE (registry uninstall entry "CodeBuddy …"), else the npm CLI.
#[allow(dead_code)]
pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some((ver, icon, loc)) = uninstall_entry("CodeBuddy") {
        let exe = icon
            .and_then(|i| crate::process::unquote_exe(&i))
            .filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false))
            .or_else(|| loc.map(|l| PathBuf::from(l.trim()).join("CodeBuddy.exe")))
            .filter(|p| p.is_file());
        if let Some(exe) = exe {
            inst.installed = true;
            inst.version = ver;
            inst.dir = exe.parent().map(Path::to_path_buf);
            inst.exe = Some(exe);
        }
    }
    if !inst.installed {
        if let Some(v) = npm_version("@tencent-ai/codebuddy-code") {
            inst.installed = true;
            inst.version = Some(v);
        }
    }
    if let Some(d) = &inst.dir {
        let d = d.to_string_lossy().to_lowercase();
        inst.running = crate::process::any_process(|_, path| path.to_lowercase().starts_with(&d));
    }
    inst
}

// ---------- format helpers ----------

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

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

fn env_ref(k: &str) -> Option<&str> {
    let k = k.trim();
    k.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| k.strip_prefix('$')).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

fn resolve_key(k: &str) -> Option<String> {
    match env_ref(k) {
        // The variable lives in the WSL shell, not in this Windows process.
        Some(_) if test_home().is_none() && crate::env::is_wsl() => None,
        Some(var) => std::env::var(var).ok().filter(|v| !v.trim().is_empty()),
        None => Some(k.trim().to_string()).filter(|v| !v.is_empty()),
    }
}

type Key = (String, String, String); // (base, apiKey, vendor)

fn key_of(e: &Value) -> Key {
    (base_of(&s(e, "url")), s(e, "apiKey"), s(e, "vendor"))
}

fn host(base: &str) -> String {
    url::Url::parse(base).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| host_of(base))
}

#[derive(Clone, Debug)]
struct Group {
    id: String,
    key: Key,
    name: String,
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
        let b = slug(&vendor);
        let id = if out.iter().any(|g| g.id == b) { (2..).map(|n| format!("{b}-{n}")).find(|c| !out.iter().any(|g| &g.id == c)).unwrap() } else { b };
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
    store::agent_get(root, ID, "parked").and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

fn default_meta() -> TextMeta {
    TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }
}

/// (models.json, meta, had_comments); a missing file reads as `{"models": []}`.
fn load_models() -> Result<(Value, TextMeta, bool)> {
    let p = models_path();
    if !p.exists() {
        return Ok((json!({ "models": [] }), default_meta(), false));
    }
    let (text, meta) = read_text(&p)?;
    let (clean, had) = strip_jsonc(&text);
    let v: Value = serde_json::from_str(&clean).map_err(|e| anyhow!(tr!("models.json 解析失败：{e}", "Couldn't parse models.json: {e}")))?;
    if !v.is_object() {
        return Err(anyhow!(l("models.json 顶层不是对象", "models.json: top level is not an object")));
    }
    Ok((v, meta, had))
}

fn entries_of(cfg: &Value) -> Vec<Value> {
    cfg.get("models").and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

/// None = no `availableModels` (every model shows).
fn available_of(cfg: &Value) -> Option<Vec<String>> {
    cfg.get("availableModels").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
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
        tags.push(Tag::new("cap:image", l("图片", "Images")));
    }
    if e.get("supportsReasoning").and_then(|x| x.as_bool()) == Some(true) {
        tags.push(Tag::new("cap:reasoning", l("推理", "Reasoning")));
    }
    let name = s(e, "name");
    Model {
        id: s(e, "id"),
        visible,
        readonly: false,
        tags,
        ctx: ctx.map(fmt_ctx),
        name: (!name.trim().is_empty() && name != s(e, "id")).then_some(name),
        context: ctx,
        deletable: true,
        extra: mfields::read(e, mfields::CODEBUDDY),
    }
}

fn key_note(k: &str) -> String {
    match env_ref(k) {
        Some(var) => if resolve_key(k).is_some() { tr!("环境变量 ${{{var}}}（已设置）", "Environment variable ${{{var}}} (set)") } else { tr!("环境变量 ${{{var}}}（未设置）", "Environment variable ${{{var}}} (not set)") },
        None if k.trim().is_empty() => l("未填写", "Not set").into(),
        None => l("明文保存在 models.json", "Stored in plain text in models.json").into(),
    }
}

fn provider_of(g: &Group, entries: &[Value], parked: &[Value], avail: Option<&Vec<String>>) -> Provider {
    let mine = |e: &Value| key_of(e) == g.key;
    let active: Vec<&Value> = entries.iter().filter(|e| mine(e)).collect();
    let enabled = !active.is_empty();
    let mut models: Vec<Model> = active.iter().map(|e| model_of(e, avail.map(|a| a.contains(&s(e, "id"))).unwrap_or(true))).collect();
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
        apis: vec!["Chat".into()],
        builtin: false,
        enabled,
        compatible: true,
        reason: None,
        models,
        details: vec![
            Kv::mono("vendor", if g.key.2.is_empty() { "-".into() } else { g.key.2.clone() }),
            Kv::mono("url", url_of(&g.key.0)),
            Kv::text(l("密钥", "API key"), key_note(&g.key.1)),
            Kv::text(l("状态", "Status"), if enabled { l("已启用", "Enabled") } else { l("已停用 · 条目暂存在 AgentPlus", "Disabled · entries parked in AgentPlus") }),
        ],
        editable: true,
        api: "chat".into(),
        has_key: resolve_key(&g.key.1).is_some() || env_ref(&g.key.1).is_some(),
        key_fp: None,
        key_hint: None,
        official_auth: false,
    }
}

#[allow(dead_code)]
pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: NAME.into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "multi".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![display_path(&models_path())],
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
    let cfg = match load_models() {
        Ok((cfg, _, had)) => {
            if had {
                st.readonly = true;
                st.notes.push(l("models.json 含注释，写回会丢失注释，已切换为只读。", "models.json contains comments that would be lost on write, so it's read-only.").into());
            }
            cfg
        }
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    let root = load_root();
    let entries = entries_of(&cfg);
    let parked = parked_of(&root);
    let avail = available_of(&cfg);
    for g in groups_of(&entries, &parked) {
        st.providers.push(provider_of(&g, &entries, &parked, avail.as_ref()));
    }
    st.current = vec![
        Kv::mono(l("默认模型", "Default model"), default_model().unwrap_or_else(|| l("-（CodeBuddy 默认）", "- (CodeBuddy default)").into())),
        Kv::text(l("自定义模型", "Custom models"), tr!("{} 个", "{}", entries.len())),
        Kv::text("availableModels", match &avail {
            Some(a) => tr!("{} 个（只显示列出的模型）", "{} (only listed models are shown)", a.len()),
            None => l("未设置（全部显示）", "Not set (all shown)").into(),
        }),
        Kv::mono(l("配置文件", "Config file"), display_path(&models_path())),
    ];
    st.notes.push(l("CodeBuddy 的自定义模型只支持 OpenAI Chat Completions 接口；其他协议可经 AgentPlus 本地网关转换后接入。", "CodeBuddy custom models only support the OpenAI Chat Completions API; other protocols can be converted through the AgentPlus local gateway.").into());
    st.notes.push(l("models.json 改动约 1 秒后自动生效（CLI 与 IDE 共用）。", "Changes to models.json take effect in about a second (shared by the CLI and the IDE).").into());
    st.notes.push(l("项目里的 availableModels 会整体覆盖用户列表。", "A project's availableModels replaces the user list entirely.").into());
    st
}

#[allow(dead_code)]
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _, _) = load_models()?;
    let g = groups_of(&entries_of(&cfg), &parked_of(&load_root())).into_iter().find(|g| g.id == id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
    if g.key.0.is_empty() {
        return Err(anyhow!(tr!("供应商 {id} 没有 url", "Provider {id} has no url")));
    }
    Ok((g.key.0, resolve_key(&g.key.1), "chat".into()))
}

fn chat_only(api: &str) -> Result<()> {
    if api == "chat" {
        Ok(())
    } else {
        Err(anyhow!(tr!("CodeBuddy 的自定义模型只支持 OpenAI Chat Completions 接口；{api} 协议的中转请先接入 AgentPlus 本地网关，再以 Chat 接口添加", "CodeBuddy custom models only support the OpenAI Chat Completions API; connect {api} relays to the AgentPlus local gateway first, then add them with the Chat API")))
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
        self.groups.iter().find(|g| g.id == id).cloned().ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))
    }

    fn enabled(&self, g: &Group) -> bool {
        self.entries.iter().any(|e| key_of(e) == g.key)
    }

    fn require_enabled(&self, g: &Group) -> Result<()> {
        if self.enabled(g) { Ok(()) } else { Err(anyhow!(tr!("供应商「{}」已停用，先启用再调整模型", "Provider \"{}\" is disabled; enable it before changing its models", g.name))) }
    }

    /// Model ids are global in CodeBuddy (selection and availableModels use them).
    fn owner_of(&self, mid: &str) -> Option<Key> {
        self.entries.iter().chain(self.parked.iter().filter_map(|p| p.get("entry"))).find(|e| s(e, "id") == mid).map(key_of)
    }

    fn check_free(&self, g: &Group, mid: &str) -> Result<()> {
        match self.owner_of(mid) {
            Some(k) if k != g.key => {
                let other = self.groups.iter().find(|x| x.key == k).map(|x| x.name.clone()).unwrap_or_default();
                Err(anyhow!(tr!("模型 ID {mid} 已被供应商「{other}」使用；CodeBuddy 的模型 ID 必须唯一", "Model ID {mid} is already used by provider \"{other}\"; CodeBuddy model IDs must be unique")))
            }
            _ => Ok(()),
        }
    }

    fn new_entry(key: &Key, mid: &str, name: Option<&str>, context: Option<u64>) -> Value {
        let mut e = json!({
            "id": mid,
            "name": name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(mid),
            "vendor": key.2,
            "url": url_of(&key.0),
            "apiKey": key.1,
            "maxInputTokens": context.unwrap_or(128000),
            "maxOutputTokens": 8192,
            "supportsToolCall": true,
            "supportsImages": false,
        });
        if key.1.is_empty() {
            e.as_object_mut().unwrap().remove("apiKey");
        }
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
        self.diff.push(&file, tr!("models + {mid}（{}）", "models + {mid} ({})", g.name), true);
        Ok(())
    }

    fn apply(&mut self, op: &Op) -> Result<()> {
        let file = self.file.clone();
        match op {
            Op::UpsertProvider { provider: p } => {
                chat_only(&p.api)?;
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                let base = base_of(&p.base_url);
                let new_key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                let vendor = p.name.trim().to_string();
                match &p.id {
                    None => {
                        let mut models: Vec<&str> = vec![];
                        for m in p.models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
                            if !models.contains(&m) {
                                models.push(m);
                            }
                        }
                        if models.is_empty() {
                            return Err(anyhow!(l("CodeBuddy 的每个模型条目自带地址和密钥：新建供应商时至少要填一个模型", "Each CodeBuddy model entry carries its own base URL and API key: add at least one model when creating a provider")));
                        }
                        let key: Key = (base.clone(), new_key.clone().unwrap_or_default(), vendor.clone());
                        let g = match self.groups.iter().find(|g| g.key == key) {
                            Some(g) => g.clone(),
                            None => {
                                let b = slug(&vendor);
                                let id = if self.groups.iter().any(|g| g.id == b) { (2..).map(|n| format!("{b}-{n}")).find(|c| !self.groups.iter().any(|g| &g.id == c)).unwrap() } else { b };
                                let g = Group { id, key: key.clone(), name: vendor.clone() };
                                self.groups.push(g.clone());
                                g
                            }
                        };
                        for m in &models {
                            self.check_free(&g, m)?;
                        }
                        let key_part = new_key.as_deref().map(|k| tr!(" · 密钥 {}", " · API key {}", mask_key(k))).unwrap_or_default();
                        self.diff.push(&file, tr!("+ 「{vendor}」{} 个模型（{}{}）", "+ \"{vendor}\" {} model(s) ({}{})", models.len(), url_of(&base), key_part), true);
                        for m in models {
                            if self.owner_of(m).is_none() {
                                self.entries.push(Self::new_entry(&key, m, None, None));
                                self.show(m);
                            }
                        }
                    }
                    Some(id) => {
                        let g = self.group(id)?;
                        let key: Key = (base.clone(), new_key.clone().unwrap_or_else(|| g.key.1.clone()), vendor.clone());
                        if key == g.key {
                            return Ok(());
                        }
                        if self.groups.iter().any(|o| o.id != g.id && o.key == key) {
                            return Err(anyhow!(l("已经有名称、地址和密钥都相同的供应商", "A provider with the same name, base URL and API key already exists")));
                        }
                        if key.0 != g.key.0 {
                            self.diff.push(&file, tr!("「{}」所有模型 url = {}", "\"{}\" all models url = {}", g.name, url_of(&key.0)), true);
                        }
                        if key.1 != g.key.1 {
                            self.diff.push(&file, tr!("「{}」所有模型 apiKey = {}", "\"{}\" all models apiKey = {}", g.name, mask_key(&key.1)), true);
                        }
                        if key.2 != g.key.2 {
                            self.diff.push(&file, tr!("「{}」所有模型 vendor = {}", "\"{}\" all models vendor = {}", g.name, key.2), true);
                        }
                        let set = |e: &mut Value| {
                            if key_of(e) == g.key {
                                e["url"] = json!(url_of(&key.0));
                                e["apiKey"] = json!(key.1);
                                e["vendor"] = json!(key.2);
                            }
                        };
                        self.entries.iter_mut().for_each(set);
                        self.parked.iter_mut().filter_map(|p| p.get_mut("entry")).for_each(set);
                        if let Some(x) = self.groups.iter_mut().find(|x| x.id == g.id) {
                            x.key = key;
                            x.name = vendor;
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let g = self.group(provider)?;
                let gone: Vec<String> = self.entries.iter().filter(|e| key_of(e) == g.key).map(|e| s(e, "id")).collect();
                self.entries.retain(|e| key_of(e) != g.key);
                for m in &gone {
                    self.unlist(m);
                }
                let n = self.parked.len();
                self.parked.retain(|p| p.get("entry").map(|e| key_of(e) != g.key).unwrap_or(true));
                let label = if gone.is_empty() && n != self.parked.len() { STORE_LABEL.to_string() } else { file.clone() };
                self.diff.push(&label, tr!("- 「{}」（{} 个模型，含地址和密钥）", "- \"{}\" ({} model(s), with base URL and API key)", g.name, gone.len().max(n - self.parked.len())), false);
                self.groups.retain(|x| x.id != g.id);
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let g = self.group(provider)?;
                if *enabled {
                    let (back, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.parked).into_iter().partition(|p| p.get("entry").map(|e| key_of(e) == g.key).unwrap_or(false));
                    self.parked = keep;
                    if !back.is_empty() {
                        self.diff.push(&file, tr!("+ 「{}」{} 个模型（从 AgentPlus 恢复）", "+ \"{}\" {} model(s) (restored from AgentPlus)", g.name, back.len()), true);
                    }
                    for p in back {
                        let Some(e) = p.get("entry").cloned() else { continue };
                        let mid = s(&e, "id");
                        if self.owner_of(&mid).is_some() {
                            return Err(anyhow!(tr!("模型 ID {mid} 已被其他供应商使用，无法恢复「{}」", "Model ID {mid} is used by another provider; can't restore \"{}\"", g.name)));
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
                        self.diff.push(&file, tr!("- 「{}」{} 个模型（暂存在 AgentPlus，可恢复）", "- \"{}\" {} model(s) (parked in AgentPlus, restorable)", g.name, out.len()), false);
                    }
                    for e in out {
                        let mid = s(&e, "id");
                        let visible = self.avail.as_ref().map(|a| a.contains(&mid)).unwrap_or(true);
                        self.unlist(&mid);
                        self.parked.push(json!({ "visible": visible, "entry": e }));
                    }
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let g = self.group(provider)?;
                self.require_enabled(&g)?;
                if !self.entries.iter().any(|e| key_of(e) == g.key && &s(e, "id") == model) {
                    return Err(anyhow!(tr!("「{}」里没有模型 {model}", "\"{}\" has no model {model}", g.name)));
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
                        self.avail = Some(self.entries.iter().map(|e| s(e, "id")).collect());
                        self.diff.push(&file, l("+ availableModels（之后只显示列出的模型；内置模型如需显示请手动加入）", "+ availableModels (from now on only listed models show; add built-in models by hand if you want them)"), true);
                    }
                    self.unlist(model);
                    self.diff.push(&file, tr!("availableModels - {model}（隐藏，条目保留）", "availableModels - {model} (hidden, entry kept)"), false);
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let g = self.group(provider)?;
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
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
                    if s(e, "id") != mid {
                        return;
                    }
                    if let Some(n) = name.filter(|n| s(e, "name") != *n) {
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
                let in_cfg = self.entries.iter().any(|e| s(e, "id") == mid);
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
                self.entries.retain(|e| !(key_of(e) == g.key && &s(e, "id") == model));
                self.parked.retain(|p| !p.get("entry").map(|e| key_of(e) == g.key && &s(e, "id") == model).unwrap_or(false));
                if self.entries.len() + self.parked.len() != n {
                    self.unlist(model);
                    self.diff.push(&file, tr!("models - {model}（{}，删除）", "models - {model} ({}, deleted)", g.name), false);
                }
            }
            Op::SetProviderModels { provider, models } => {
                let g = self.group(provider)?;
                self.require_enabled(&g)?;
                let mut want: Vec<String> = vec![];
                for m in models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
                    if !want.iter().any(|w| w == m) {
                        want.push(m.to_string());
                    }
                }
                if want.is_empty() {
                    return Err(anyhow!(l("至少保留一个模型；不要这个供应商的话请直接删除", "Keep at least one model; delete the provider if you don't need it")));
                }
                for m in &want {
                    self.check_free(&g, m)?;
                }
                let gone: Vec<String> = self.entries.iter().filter(|e| key_of(e) == g.key && !want.contains(&s(e, "id"))).map(|e| s(e, "id")).collect();
                self.entries.retain(|e| key_of(e) != g.key || want.contains(&s(e, "id")));
                for m in gone {
                    self.unlist(&m);
                    self.diff.push(&file, tr!("models - {m}（{}）", "models - {m} ({})", g.name), false);
                }
                for m in &want {
                    if self.owner_of(m).is_none() {
                        self.add_model(&g, m, None, None)?;
                    }
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("CodeBuddy 的自定义模型可以同时启用，在 CodeBuddy 里切换模型", "CodeBuddy custom models can all be enabled at once; switch models inside CodeBuddy"))),
            Op::SetModelRoles { .. } => return Err(anyhow!(l("只有 Claude Code 需要分配模型角色", "Only Claude Code uses model roles"))),
            Op::SetSetting { key, .. } => return Err(anyhow!(tr!("未知设置 {key}", "Unknown setting: {key}"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (mut cfg, meta, had_comments) = load_models()?;
    let entries0 = entries_of(&cfg);
    let avail0 = available_of(&cfg);
    let mut root = load_root();
    let parked0 = parked_of(&root);
    let groups = groups_of(&entries0, &parked0);
    let mut w = Work { entries: entries0.clone(), avail: avail0.clone(), parked: parked0.clone(), groups, diff: Diff::default(), file: display_path(&models_path()) };
    for op in ops {
        w.apply(op)?;
    }
    let cfg_dirty = w.entries != entries0 || w.avail != avail0;
    let store_dirty = w.parked != parked0;
    if cfg_dirty && had_comments {
        return Err(anyhow!(l("models.json 含注释，为避免丢失注释不写入", "models.json contains comments; not writing it to avoid losing them")));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cfg_dirty {
            let path = models_path();
            if path.exists() {
                backup_dir = Some(do_backup(std::slice::from_ref(&path))?);
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
            save_root(&root)?;
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

    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            TEST_HOME.with(|t| *t.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str, models: Option<&str>) -> Home {
        std::env::set_var("AGENTPLUS_CB_TEST_KEY", "sk-env-3333");
        let h = std::env::temp_dir().join(format!("agentplus-codebuddy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join(".codebuddy")).unwrap();
        std::fs::write(h.join(".codebuddy/settings.json"), r#"{"enabledPlugins":{"pdf@x":true},"model":"deepseek-chat"}"#).unwrap();
        if let Some(c) = models {
            std::fs::write(h.join(".codebuddy/models.json"), c).unwrap();
        }
        TEST_HOME.with(|t| *t.borrow_mut() = Some(h.clone()));
        Home(h)
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
        c["models"].as_array().unwrap().iter().map(|e| s(e, "id")).collect()
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
        assert!(!h.0.join("agentplus-store.json").exists());
        // Hide + show only touches availableModels; the model entries come back unchanged.
        plan(&[Op::SetModelVisible { provider: "custom".into(), model: "glm-4.6".into(), visible: false }], false).unwrap();
        plan(&[Op::SetModelVisible { provider: "custom".into(), model: "glm-4.6".into(), visible: true }], false).unwrap();
        let after: Value = cfg_of(&h);
        assert_eq!(after["models"], serde_json::from_str::<Value>(SAMPLE).unwrap()["models"]);
        let _h2 = setup("jsonc", Some("{\n  // c\n  \"models\": []\n}"));
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[upsert(None, "x", "https://x/v1", "chat", None, &["m"])], true).is_err());
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
