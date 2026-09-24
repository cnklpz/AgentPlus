//! Factory Droid (CLI): custom models live in `~/.factory/settings.json` → `customModels`,
//! one entry per model at one endpoint:
//! `{ model, displayName, baseUrl, apiKey, provider, maxOutputTokens, … }`, with
//! `provider` = `generic-chat-completion-api` (Chat) | `openai` (Responses) | `anthropic`.
//! The selected model is `"model"`; for custom entries it is `custom:<displayName, spaces
//! → dashes>-<array index>`, so any reorder / removal shifts it — AgentPlus fixes it up.
//!
//! AgentPlus shows one provider per (baseUrl, provider, apiKey) group. Hidden models and
//! disabled providers are moved out of the file and parked in the AgentPlus store.
//! Droid rewrites settings.json while running: writes re-read the file right before
//! writing and replace only `customModels` and `model` (atomic tmp + rename).
//! The legacy `~/.factory/config.json` (`custom_models`, snake_case) is shown read-only.

use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub const ID: &str = "droid";
#[allow(dead_code)]
pub const NAME: &str = "Droid";
#[allow(dead_code)]
pub const MARKER: &str = "settings.json";
#[allow(dead_code)]
pub const WSL_SCRIPT: &str = "droid --version 2>/dev/null | head -n 1; pgrep -x droid >/dev/null && echo @running; true";
#[allow(dead_code)]
pub const WSL_MARKER: &str = ".factory/settings.json";

/// Temporary markers carried on entries during a plan (never written).
const SEL: &str = "__agentplus_sel";
const IDFMT: &str = "__agentplus_idfmt";
const STORE_LABEL: &str = "AgentPlus · Droid";

#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests point the adapter at a temp home (settings, store and backups all inside it).
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

/// Droid's default config dir, `~/.factory`.
#[allow(dead_code)]
pub fn default_dir() -> PathBuf {
    test_home().unwrap_or_else(home).join(".factory")
}

fn dir() -> PathBuf {
    if test_home().is_some() {
        return default_dir();
    }
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn legacy_path() -> PathBuf {
    dir().join("config.json")
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

#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}
#[cfg(not(windows))]
fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

fn npm_version(pkg: &str) -> Option<String> {
    let p = dirs::data_dir()?.join("npm").join("node_modules").join(pkg).join("package.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn exe_version(exe: &Path) -> Option<String> {
    let out = no_window(std::process::Command::new(exe).arg("--version")).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && w.contains('.'))
        .map(String::from)
}

/// The `droid` CLI: npm global package, or the native installer's `droid.exe`.
#[allow(dead_code)]
pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some(v) = npm_version("droid").or_else(|| npm_version("@factory/cli")) {
        inst.installed = true;
        inst.version = Some(v);
    } else {
        let h = dirs::home_dir().unwrap_or_default();
        let native = [h.join(".local").join("bin").join("droid.exe"), h.join("bin").join("droid.exe"), h.join(".factory").join("bin").join("droid.exe")];
        if let Some(exe) = native.into_iter().find(|p| p.is_file()).or_else(|| on_path(&["droid.exe", "droid.cmd"])) {
            inst.installed = true;
            if exe.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
                static V: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
                inst.version = V.get_or_init(|| exe_version(&exe)).clone();
            }
        }
    }
    inst.running = inst.installed && crate::process::any_process(|name, _| name.eq_ignore_ascii_case("droid.exe"));
    inst
}

// ---------- format helpers ----------

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

fn norm_base(u: &str) -> String {
    u.trim().trim_end_matches('/').to_string()
}

fn api_of(provider: &str) -> &'static str {
    match provider {
        "openai" => "responses",
        "anthropic" => "anthropic",
        _ => "chat",
    }
}

fn api_label(api: &str) -> &'static str {
    match api {
        "anthropic" => "Anthropic",
        "responses" => "Responses",
        _ => "Chat",
    }
}

fn provider_for(api: &str) -> Result<&'static str> {
    match api {
        "chat" => Ok("generic-chat-completion-api"),
        "responses" => Ok("openai"),
        "anthropic" => Ok("anthropic"),
        other => Err(anyhow!(tr!("Droid 不支持 {other} 接口（只有 Chat / Responses / Anthropic）", "Droid doesn't support the {other} API (only Chat / Responses / Anthropic)"))),
    }
}

/// `${VAR}` / `$VAR` → the variable name.
fn env_ref(k: &str) -> Option<&str> {
    let k = k.trim();
    k.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| k.strip_prefix('$')).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// The key a request would use: a literal, or the value of the referenced env variable.
fn resolve_key(k: &str) -> Option<String> {
    match env_ref(k) {
        // The variable lives in the WSL shell, not in this Windows process.
        Some(_) if test_home().is_none() && crate::env::is_wsl() => None,
        Some(var) => std::env::var(var).ok().filter(|v| !v.trim().is_empty()),
        None => Some(k.trim().to_string()).filter(|v| !v.is_empty()),
    }
}

/// What Droid shows / selects: displayName, else the model id.
fn display(e: &Value) -> String {
    let d = s(e, "displayName");
    if d.trim().is_empty() { s(e, "model") } else { d }
}

/// Droid's selection id of the entry at `i`.
fn sel_id(e: &Value, i: usize) -> String {
    format!("custom:{}-{i}", display(e).replace(' ', "-"))
}

/// The id Droid uses for an entry (an explicit `id` field wins).
fn entry_id(e: &Value, i: usize) -> String {
    e.get("id").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| sel_id(e, i))
}

type Key = (String, String, String);

fn key_of(e: &Value) -> Key {
    (norm_base(&s(e, "baseUrl")), s(e, "provider"), s(e, "apiKey"))
}

/// Store key for a group's display name (a one-way fingerprint; the key never lands in it).
fn fp(k: &Key) -> String {
    key_fingerprint(&format!("{}\n{}\n{}", k.0, k.1, k.2))
}

fn host(base: &str) -> String {
    url::Url::parse(base).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| host_of(base))
}

fn strip_markers(e: &mut Value) {
    if let Some(o) = e.as_object_mut() {
        o.retain(|k, _| !k.starts_with("__agentplus_"));
    }
}

#[derive(Clone, Debug)]
struct Group {
    id: String,
    key: Key,
    name: String,
}

/// Groups in order of first appearance (active entries first, then parked ones).
fn groups_of(entries: &[Value], parked: &[Value], names: &Map<String, Value>) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    let all = entries.iter().chain(parked.iter().filter_map(|p| p.get("entry")));
    for e in all {
        let k = key_of(e);
        if out.iter().any(|g| g.key == k) {
            continue;
        }
        let name = names.get(&fp(&k)).and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| host(&k.0));
        let base = slug(&name);
        let id = if out.iter().any(|g| g.id == base) { (2..).map(|n| format!("{base}-{n}")).find(|c| !out.iter().any(|g| &g.id == c)).unwrap() } else { base };
        out.push(Group { id, key: k, name });
    }
    out
}

fn parked_of(root: &Value) -> Vec<Value> {
    store::agent_get(root, ID, "parked").and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

fn names_of(root: &Value) -> Map<String, Value> {
    store::agent_get(root, ID, "names").and_then(|x| x.as_object()).cloned().unwrap_or_default()
}

fn parked_why(p: &Value) -> &str {
    p.get("why").and_then(|x| x.as_str()).unwrap_or("hidden")
}

fn default_meta() -> TextMeta {
    TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 }
}

/// (settings, meta, had_comments); a missing file reads as `{}`.
fn load_settings() -> Result<(Value, TextMeta, bool)> {
    let p = settings_path();
    if !p.exists() {
        return Ok((json!({}), default_meta(), false));
    }
    let (text, meta) = read_text(&p)?;
    let (clean, had) = strip_jsonc(&text);
    let v: Value = serde_json::from_str(&clean).map_err(|e| anyhow!(tr!("settings.json 解析失败：{e}", "Couldn't parse settings.json: {e}")))?;
    if !v.is_object() {
        return Err(anyhow!(l("settings.json 顶层不是对象", "settings.json: top level is not an object")));
    }
    Ok((v, meta, had))
}

fn custom_models(cfg: &Value) -> Vec<Value> {
    cfg.get("customModels").and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

/// Legacy `config.json` entries, mapped to the current field names.
fn legacy_entries() -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(legacy_path()) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&strip_jsonc(&text).0) else { return vec![] };
    v.get("custom_models")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .map(|e| {
                    json!({
                        "model": s(e, "model"),
                        "displayName": s(e, "model_display_name"),
                        "baseUrl": s(e, "base_url"),
                        "apiKey": s(e, "api_key"),
                        "provider": s(e, "provider"),
                        "maxOutputTokens": e.get("max_tokens").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Legacy groups not already covered by settings.json (id "legacy-…").
fn legacy_groups(active: &[Value]) -> Vec<(Group, Vec<Value>)> {
    let covered = |e: &Value| active.iter().any(|a| s(a, "model") == s(e, "model") && norm_base(&s(a, "baseUrl")) == norm_base(&s(e, "baseUrl")));
    let entries: Vec<Value> = legacy_entries().into_iter().filter(|e| !covered(e)).collect();
    let mut out: Vec<(Group, Vec<Value>)> = vec![];
    for e in entries {
        let k = key_of(&e);
        if let Some(g) = out.iter_mut().find(|(g, _)| g.key == k) {
            g.1.push(e);
            continue;
        }
        let base = format!("legacy-{}", slug(&host(&k.0)));
        let id = if out.iter().any(|(g, _)| g.id == base) { (2..).map(|n| format!("{base}-{n}")).find(|c| !out.iter().any(|(g, _)| &g.id == c)).unwrap() } else { base };
        out.push((Group { id, name: tr!("{}（旧版 config.json）", "{} (legacy config.json)", host(&k.0)), key: k }, vec![e]));
    }
    out
}

fn model_of(e: &Value, visible: bool, readonly: bool) -> Model {
    let d = s(e, "displayName");
    let out = e.get("maxOutputTokens").and_then(|x| x.as_u64());
    Model {
        id: s(e, "model"),
        visible,
        readonly,
        tags: out.map(|n| vec![tr!("输出 {}", "Output {}", fmt_ctx(n))]).unwrap_or_default(),
        name: (!d.trim().is_empty() && d != s(e, "model")).then_some(d),
        deletable: !readonly,
        extra: crate::mfields::read(e, crate::mfields::DROID),
        ..Default::default()
    }
}

fn key_note(k: &str) -> String {
    match env_ref(k) {
        Some(var) => if resolve_key(k).is_some() { tr!("环境变量 ${{{var}}}（已设置）", "Environment variable ${{{var}}} (set)") } else { tr!("环境变量 ${{{var}}}（未设置）", "Environment variable ${{{var}}} (not set)") },
        None if k.trim().is_empty() => l("未填写", "Not set").into(),
        None => l("明文保存在 settings.json", "Stored in plain text in settings.json").into(),
    }
}

fn provider_of(g: &Group, entries: &[Value], parked: &[Value], readonly: bool) -> Provider {
    let mine = |e: &Value| key_of(e) == g.key;
    let active: Vec<&Value> = entries.iter().filter(|e| mine(e)).collect();
    let disabled = active.is_empty() && parked.iter().any(|p| parked_why(p) == "disabled" && p.get("entry").map(mine).unwrap_or(false));
    let mut models: Vec<Model> = vec![];
    let mut push = |m: Model| {
        if !models.iter().any(|x| x.id == m.id) {
            models.push(m);
        }
    };
    for e in &active {
        push(model_of(e, true, readonly));
    }
    for p in parked {
        if let Some(e) = p.get("entry").filter(|e| mine(e)) {
            push(model_of(e, parked_why(p) == "disabled", readonly));
        }
    }
    let api = api_of(&g.key.1);
    Provider {
        id: g.id.clone(),
        name: g.name.clone(),
        base_url: Some(g.key.0.clone()),
        host: host_of(&g.key.0),
        apis: vec![api_label(api).into()],
        builtin: false,
        enabled: !disabled,
        compatible: true,
        reason: None,
        models,
        details: vec![
            Kv::mono("provider", if g.key.1.is_empty() { "-".into() } else { g.key.1.clone() }),
            Kv::text(l("条目", "Entries"), tr!("customModels 里 {} 个模型条目（每个条目自带地址和密钥）", "{} model entries in customModels (each carries its own base URL and API key)", active.len())),
            Kv::text(l("密钥", "API key"), key_note(&g.key.2)),
            Kv::text(l("状态", "Status"), if readonly { l("旧版 config.json · 只读", "Legacy config.json · read-only") } else if disabled { l("已停用 · 条目暂存在 AgentPlus", "Disabled · entries parked in AgentPlus") } else { l("已启用", "Enabled") }),
        ],
        editable: !readonly,
        api: api.into(),
        has_key: resolve_key(&g.key.2).is_some() || env_ref(&g.key.2).is_some(),
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
        files: vec![display_path(&settings_path())],
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
    let cfg = match load_settings() {
        Ok((cfg, _, had)) => {
            if had {
                st.readonly = true;
                st.notes.push(l("settings.json 含注释，写回会丢失注释，已切换为只读。", "settings.json contains comments that would be lost on write, so it's read-only.").into());
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
    let entries = custom_models(&cfg);
    let parked = parked_of(&root);
    let groups = groups_of(&entries, &parked, &names_of(&root));
    for g in &groups {
        st.providers.push(provider_of(g, &entries, &parked, false));
    }
    let legacy = legacy_groups(&entries);
    if !legacy.is_empty() {
        st.files.push(display_path(&legacy_path()));
        st.notes.push(l("旧版 ~/.factory/config.json 里的 custom_models 只读显示；settings.json 里有同名条目时以 settings.json 为准。", "custom_models from the legacy ~/.factory/config.json are shown read-only; when settings.json has the same entry, settings.json wins.").into());
    }
    for (g, es) in &legacy {
        st.providers.push(provider_of(g, es, &[], true));
    }

    let model = cfg.get("model").and_then(|x| x.as_str()).map(String::from);
    let selected = model.as_deref().and_then(|m| entries.iter().enumerate().find(|(i, e)| entry_id(e, *i) == m));
    st.current = vec![
        Kv::mono(l("当前模型", "Current model"), model.clone().unwrap_or_else(|| l("-（Droid 默认）", "- (Droid default)").into())),
        Kv::text(
            l("对应条目", "Matching entry"),
            match selected {
                Some((_, e)) => format!("{} · {}", display(e), groups.iter().find(|g| g.key == key_of(e)).map(|g| g.name.clone()).unwrap_or_default()),
                None if model.as_deref().map(|m| m.starts_with("custom:")).unwrap_or(false) => l("找不到对应的自定义模型", "No matching custom model").into(),
                None => l("内置模型", "Built-in model").into(),
            },
        ),
        Kv::text(l("自定义模型", "Custom models"), tr!("{} 个", "{}", entries.len())),
        Kv::mono(l("配置文件", "Config file"), display_path(&settings_path())),
    ];
    st.notes.push(l("Droid 运行时会改写 settings.json；AgentPlus 只改 customModels 和 model，改动对新会话生效。", "Droid rewrites settings.json while running; AgentPlus only changes customModels and model, and changes apply to new sessions.").into());
    st.notes.push(l("自定义模型按列表位置编号（custom:名称-序号）；增删或隐藏条目后 AgentPlus 会同步修正当前选中的 model。", "Custom models are numbered by list position (custom:name-index); after adding, removing or hiding entries AgentPlus fixes up the selected model.").into());
    st
}

#[allow(dead_code)]
pub fn provider_endpoint(id: &str) -> Result<(String, Option<String>, String)> {
    let (cfg, _, _) = load_settings()?;
    let root = load_root();
    let entries = custom_models(&cfg);
    let groups = groups_of(&entries, &parked_of(&root), &names_of(&root));
    let key = match groups.into_iter().find(|g| g.id == id) {
        Some(g) => g.key,
        None => legacy_groups(&entries).into_iter().find(|(g, _)| g.id == id).map(|(g, _)| g.key).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?,
    };
    if key.0.is_empty() {
        return Err(anyhow!(tr!("供应商 {id} 没有 baseUrl", "Provider {id} has no baseUrl")));
    }
    Ok((key.0, resolve_key(&key.2), api_of(&key.1).into()))
}

/// Everything one plan works on.
struct Work {
    entries: Vec<Value>,
    parked: Vec<Value>,
    names: Map<String, Value>,
    groups: Vec<Group>,
    legacy: Vec<String>,
    diff: Diff,
    file: String,
}

impl Work {
    fn group(&self, id: &str) -> Result<Group> {
        if self.legacy.iter().any(|l| l == id) {
            return Err(anyhow!(l("旧版 config.json 里的条目只读；请在 Droid 里迁移到 settings.json 后再编辑", "Entries in the legacy config.json are read-only; migrate them to settings.json in Droid before editing")));
        }
        self.groups.iter().find(|g| g.id == id).cloned().ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))
    }

    fn enabled(&self, g: &Group) -> bool {
        self.entries.iter().any(|e| key_of(e) == g.key) || !self.parked.iter().any(|p| parked_why(p) == "disabled" && p.get("entry").map(|e| key_of(e) == g.key).unwrap_or(false))
    }

    fn require_enabled(&self, g: &Group) -> Result<()> {
        if self.enabled(g) { Ok(()) } else { Err(anyhow!(tr!("供应商「{}」已停用，先启用再调整模型", "Provider \"{}\" is disabled; enable it before changing its models", g.name))) }
    }

    fn park(&mut self, e: Value, why: &str) {
        let mut e = e;
        strip_markers(&mut e);
        self.parked.push(json!({ "why": why, "entry": e }));
    }

    fn new_entry(g: &Group, model: &str, name: Option<&str>) -> Value {
        let mut e = json!({
            "model": model,
            "displayName": name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(model),
            "baseUrl": g.key.0,
            "apiKey": g.key.2,
            "provider": g.key.1,
        });
        if g.key.2.is_empty() {
            e.as_object_mut().unwrap().remove("apiKey");
        }
        e
    }

    fn add_model(&mut self, g: &Group, model: &str, name: Option<&str>) {
        self.entries.push(Self::new_entry(g, model, name));
        let file = self.file.clone();
        self.diff.push(&file, tr!("customModels + {model}（{}）", "customModels + {model} ({})", g.name), true);
    }

    fn apply(&mut self, op: &Op) -> Result<()> {
        let file = self.file.clone();
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(anyhow!(l("名称和地址不能为空", "Name and base URL are required")));
                }
                let prov = provider_for(&p.api)?;
                let base = norm_base(&p.base_url);
                let new_key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
                match &p.id {
                    None => {
                        let models: Vec<&str> = p.models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()).collect();
                        if models.is_empty() {
                            return Err(anyhow!(l("Droid 的每个模型条目自带地址和密钥：新建供应商时至少要填一个模型", "Each Droid model entry carries its own base URL and API key: add at least one model when creating a provider")));
                        }
                        let key: Key = (base.clone(), prov.to_string(), new_key.clone().unwrap_or_default());
                        let g = match self.groups.iter().find(|g| g.key == key) {
                            Some(g) => g.clone(),
                            None => {
                                let b = slug(p.name.trim());
                                let id = if self.groups.iter().any(|g| g.id == b) || self.legacy.contains(&b) { (2..).map(|n| format!("{b}-{n}")).find(|c| !self.groups.iter().any(|g| &g.id == c)).unwrap() } else { b };
                                let g = Group { id, key: key.clone(), name: p.name.trim().to_string() };
                                self.groups.push(g.clone());
                                g
                            }
                        };
                        self.names.insert(fp(&key), json!(p.name.trim()));
                        let key_part = new_key.as_deref().map(|k| tr!(" · 密钥 {}", " · API key {}", mask_key(k))).unwrap_or_default();
                        self.diff.push(&file, tr!("+ 「{}」{} 个模型条目（{base} · {}{}）", "+ \"{}\" {} model entries ({base} · {}{})", p.name.trim(), models.len(), api_label(&p.api), key_part), true);
                        for m in models {
                            if !self.entries.iter().any(|e| key_of(e) == key && s(e, "model") == m) {
                                self.entries.push(Self::new_entry(&g, m, None));
                            }
                        }
                    }
                    Some(id) => {
                        let g = self.group(id)?;
                        let key: Key = (base.clone(), prov.to_string(), new_key.clone().unwrap_or_else(|| g.key.2.clone()));
                        if key != g.key && self.groups.iter().any(|o| o.id != g.id && o.key == key) {
                            return Err(anyhow!(l("已经有地址、协议和密钥都相同的供应商", "A provider with the same base URL, protocol and API key already exists")));
                        }
                        let mut lines = vec![];
                        if key.0 != g.key.0 {
                            lines.push(format!("baseUrl = {}", key.0));
                        }
                        if key.1 != g.key.1 {
                            lines.push(format!("provider = {}", key.1));
                        }
                        if key.2 != g.key.2 {
                            lines.push(format!("apiKey = {}", if key.2.is_empty() { l("（空）", "(empty)").into() } else { mask_key(&key.2) }));
                        }
                        if key != g.key {
                            let set = |e: &mut Value| {
                                if key_of(e) == g.key {
                                    e["baseUrl"] = json!(key.0);
                                    e["provider"] = json!(key.1);
                                    e["apiKey"] = json!(key.2);
                                }
                            };
                            self.entries.iter_mut().for_each(set);
                            self.parked.iter_mut().filter_map(|p| p.get_mut("entry")).for_each(set);
                            for line in lines {
                                self.diff.push(&file, tr!("「{}」所有条目 {line}", "\"{}\" all entries {line}", g.name), true);
                            }
                        }
                        self.names.remove(&fp(&g.key));
                        self.names.insert(fp(&key), json!(p.name.trim()));
                        if p.name.trim() != g.name {
                            self.diff.push(STORE_LABEL, tr!("供应商名称「{}」→「{}」", "Provider name \"{}\" → \"{}\"", g.name, p.name.trim()), true);
                        }
                        if let Some(x) = self.groups.iter_mut().find(|x| x.id == g.id) {
                            x.key = key;
                            x.name = p.name.trim().to_string();
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let g = self.group(provider)?;
                let n = self.entries.len();
                self.entries.retain(|e| key_of(e) != g.key);
                let removed = n - self.entries.len();
                self.parked.retain(|p| p.get("entry").map(|e| key_of(e) != g.key).unwrap_or(true));
                self.names.remove(&fp(&g.key));
                self.groups.retain(|x| x.id != g.id);
                self.diff.push(&file, tr!("- 「{}」（{removed} 个模型条目，含地址和密钥）", "- \"{}\" ({removed} model entries, with base URL and API key)", g.name), false);
            }
            Op::SetProviderEnabled { provider, enabled } => {
                let g = self.group(provider)?;
                if *enabled {
                    let (back, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.parked).into_iter().partition(|p| parked_why(p) == "disabled" && p.get("entry").map(|e| key_of(e) == g.key).unwrap_or(false));
                    self.parked = keep;
                    if !back.is_empty() {
                        self.diff.push(&file, tr!("+ 「{}」{} 个模型条目（从 AgentPlus 恢复）", "+ \"{}\" {} model entries (restored from AgentPlus)", g.name, back.len()), true);
                        self.entries.extend(back.into_iter().filter_map(|p| p.get("entry").cloned()));
                    }
                } else {
                    let (out, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.entries).into_iter().partition(|e| key_of(e) == g.key);
                    self.entries = keep;
                    if !out.is_empty() {
                        self.diff.push(&file, tr!("- 「{}」{} 个模型条目（暂存在 AgentPlus，可恢复）", "- \"{}\" {} model entries (parked in AgentPlus, restorable)", g.name, out.len()), false);
                        for e in out {
                            self.park(e, "disabled");
                        }
                    }
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let g = self.group(provider)?;
                self.require_enabled(&g)?;
                if *visible {
                    let (back, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.parked).into_iter().partition(|p| parked_why(p) == "hidden" && p.get("entry").map(|e| key_of(e) == g.key && &s(e, "model") == model).unwrap_or(false));
                    self.parked = keep;
                    if !back.is_empty() {
                        self.entries.extend(back.into_iter().filter_map(|p| p.get("entry").cloned()));
                        self.diff.push(&file, tr!("customModels + {model}（{}，显示）", "customModels + {model} ({}, shown)", g.name), true);
                    }
                } else {
                    let (out, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.entries).into_iter().partition(|e| key_of(e) == g.key && &s(e, "model") == model);
                    self.entries = keep;
                    if !out.is_empty() {
                        self.diff.push(&file, tr!("customModels - {model}（{}，隐藏，条目暂存在 AgentPlus）", "customModels - {model} ({}, hidden, entry parked in AgentPlus)", g.name), false);
                        for e in out {
                            self.park(e, "hidden");
                        }
                    }
                }
            }
            Op::UpsertModel { provider, model: m } => {
                let g = self.group(provider)?;
                let mid = m.id.trim().to_string();
                if mid.is_empty() {
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
                }
                for (k, v) in &m.extra {
                    crate::mfields::check(crate::mfields::DROID, k, v)?;
                }
                let exists = self.entries.iter().chain(self.parked.iter().filter_map(|p| p.get("entry"))).any(|e| key_of(e) == g.key && s(e, "model") == mid);
                if !exists {
                    self.require_enabled(&g)?;
                    self.add_model(&g, &mid, m.name.as_deref());
                } else if let Some(n) = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    let mut changed = false;
                    let set = |e: &mut Value| {
                        if key_of(e) == g.key && s(e, "model") == mid && s(e, "displayName") != n {
                            e["displayName"] = json!(n);
                            true
                        } else {
                            false
                        }
                    };
                    for e in self.entries.iter_mut() {
                        changed |= set(e);
                    }
                    for e in self.parked.iter_mut().filter_map(|p| p.get_mut("entry")) {
                        changed |= set(e);
                    }
                    if changed {
                        self.diff.push(&file, tr!("customModels {mid}（{}）displayName = {n}", "customModels {mid} ({}) displayName = {n}", g.name), true);
                    }
                }
                // Droid has no context-window field for custom models; `context` is ignored.
                let mut lines = vec![];
                for e in self.entries.iter_mut().chain(self.parked.iter_mut().filter_map(|p| p.get_mut("entry"))) {
                    if key_of(e) == g.key && s(e, "model") == mid {
                        lines.extend(crate::mfields::write(e, crate::mfields::DROID, &m.extra)?);
                    }
                }
                lines.dedup();
                for line in lines {
                    self.diff.push(&file, tr!("customModels {mid}（{}）{line}", "customModels {mid} ({}) {line}", g.name), true);
                }
            }
            Op::DeleteModel { provider, model } => {
                let g = self.group(provider)?;
                let n = self.entries.len() + self.parked.len();
                self.entries.retain(|e| !(key_of(e) == g.key && &s(e, "model") == model));
                self.parked.retain(|p| !p.get("entry").map(|e| key_of(e) == g.key && &s(e, "model") == model).unwrap_or(false));
                if self.entries.len() + self.parked.len() != n {
                    self.diff.push(&file, tr!("customModels - {model}（{}，删除）", "customModels - {model} ({}, deleted)", g.name), false);
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
                let keep = |e: &Value| key_of(e) != g.key || want.contains(&s(e, "model"));
                let gone: Vec<String> = self.entries.iter().chain(self.parked.iter().filter_map(|p| p.get("entry"))).filter(|e| !keep(e)).map(|e| s(e, "model")).collect();
                self.entries.retain(|e| keep(e));
                self.parked.retain(|p| p.get("entry").map(keep).unwrap_or(true));
                for m in gone {
                    self.diff.push(&file, tr!("customModels - {m}（{}）", "customModels - {m} ({})", g.name), false);
                }
                for m in &want {
                    let has = self.entries.iter().chain(self.parked.iter().filter_map(|p| p.get("entry"))).any(|e| key_of(e) == g.key && &s(e, "model") == m);
                    if !has {
                        self.add_model(&g, m, None);
                    }
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("Droid 的自定义模型可以同时存在，在 Droid 里用 /model 切换", "Droid custom models can all coexist; switch with /model inside Droid"))),
            Op::SetModelRoles { .. } => return Err(anyhow!(l("只有 Claude Code 需要分配模型角色", "Only Claude Code uses model roles"))),
            Op::SetSetting { key, .. } => return Err(anyhow!(tr!("未知设置 {key}", "Unknown setting: {key}"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub fn plan(ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    let (cfg, _, had_comments) = load_settings()?;
    let orig_entries = custom_models(&cfg);
    let model0 = cfg.get("model").and_then(|x| x.as_str()).map(String::from);
    let mut entries = orig_entries.clone();
    // Mark the selected entry, and entries whose explicit `id` follows the positional scheme.
    let mut selected_custom = false;
    for (i, e) in entries.iter_mut().enumerate() {
        if model0.as_deref() == Some(entry_id(e, i).as_str()) && !selected_custom {
            e[SEL] = json!(true);
            selected_custom = true;
        }
        if e.get("id").and_then(|x| x.as_str()) == Some(sel_id(e, i).as_str()) {
            e[IDFMT] = json!(true);
        }
    }
    let mut root = load_root();
    let parked0 = parked_of(&root);
    let names0 = names_of(&root);
    let groups = groups_of(&entries, &parked0, &names0);
    let legacy = legacy_groups(&orig_entries).into_iter().map(|(g, _)| g.id).collect();
    let mut w = Work { entries, parked: parked0.clone(), names: names0.clone(), groups, legacy, diff: Diff::default(), file: display_path(&settings_path()) };
    for op in ops {
        w.apply(op)?;
    }

    // Re-number: keep explicit id / index fields in step, then follow the selection.
    let mut sel_pos = None;
    for (i, e) in w.entries.iter_mut().enumerate() {
        if e.get(SEL).is_some() {
            sel_pos = Some(i);
        }
        if e.get(IDFMT).is_some() {
            e["id"] = json!(sel_id(e, i));
        }
        if e.get("index").map(|x| x.is_u64()).unwrap_or(false) {
            e["index"] = json!(i);
        }
        strip_markers(e);
    }
    let file = w.file.clone();
    let mut model1 = model0.clone();
    match sel_pos {
        Some(i) => {
            let id = entry_id(&w.entries[i], i);
            if model0.as_deref() != Some(id.as_str()) {
                w.diff.push(&file, tr!("model = {id}（跟随所选模型的新位置）", "model = {id} (follows the selected model's new position)"), true);
                model1 = Some(id);
            }
        }
        None if selected_custom => {
            w.diff.push(&file, tr!("- model（所选的 {} 已移出列表，Droid 会回到默认模型）", "- model (the selected {} left the list; Droid falls back to its default model)", model0.clone().unwrap_or_default()), false);
            model1 = None;
        }
        None => {}
    }

    let cfg_dirty = w.entries != orig_entries || model1 != model0;
    let store_dirty = w.parked != parked0 || w.names != names0;
    if cfg_dirty && had_comments {
        return Err(anyhow!(l("settings.json 含注释，为避免丢失注释不写入", "settings.json contains comments; not writing it to avoid losing them")));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cfg_dirty {
            let path = settings_path();
            backup_dir = Some(do_backup(&[path.clone()])?);
            // Droid may have rewritten the file meanwhile: take the latest copy and replace
            // only our two keys.
            let (mut fresh, meta, had) = load_settings()?;
            if had {
                return Err(anyhow!(l("settings.json 含注释，为避免丢失注释不写入", "settings.json contains comments; not writing it to avoid losing them")));
            }
            let o = fresh.as_object_mut().unwrap();
            o.insert("customModels".into(), Value::Array(w.entries.clone()));
            match &model1 {
                Some(m) => {
                    o.insert("model".into(), json!(m));
                }
                None => {
                    o.remove("model");
                }
            }
            std::fs::create_dir_all(dir())?;
            write_json(&path, &fresh, meta)?;
            written.push(path);
        }
        if store_dirty {
            store::set_value(&mut root, ID, "parked", Value::Array(w.parked));
            store::set_value(&mut root, ID, "names", Value::Object(w.names));
            save_root(&root)?;
        }
    }
    Ok((w.diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "model": "custom:GLM-Coding-1",
  "reasoningEffort": "high",
  "customModels": [
    {
      "model": "kimi-k2",
      "displayName": "Kimi K2",
      "baseUrl": "https://api.moonshot.cn/v1",
      "apiKey": "sk-moon-1111",
      "provider": "generic-chat-completion-api",
      "maxOutputTokens": 32768
    },
    {
      "model": "glm-5.2",
      "displayName": "GLM Coding",
      "baseUrl": "https://open.bigmodel.cn/api/coding/paas/v4",
      "apiKey": "${AGENTPLUS_DROID_TEST_KEY}",
      "provider": "generic-chat-completion-api",
      "maxOutputTokens": 131072
    },
    {
      "model": "glm-4.6",
      "displayName": "GLM 4.6",
      "baseUrl": "https://open.bigmodel.cn/api/coding/paas/v4/",
      "apiKey": "${AGENTPLUS_DROID_TEST_KEY}",
      "provider": "generic-chat-completion-api"
    },
    {
      "model": "claude-sonnet-4-5",
      "displayName": "Relay Sonnet",
      "baseUrl": "https://relay.example.com",
      "apiKey": "sk-relay-2222",
      "provider": "anthropic"
    }
  ],
  "enableCompletionBell": true
}
"#;

    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            TEST_HOME.with(|t| *t.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str, settings: Option<&str>) -> Home {
        std::env::set_var("AGENTPLUS_DROID_TEST_KEY", "sk-env-3333");
        let h = std::env::temp_dir().join(format!("agentplus-droid-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join(".factory")).unwrap();
        if let Some(c) = settings {
            std::fs::write(h.join(".factory/settings.json"), c).unwrap();
        }
        TEST_HOME.with(|t| *t.borrow_mut() = Some(h.clone()));
        Home(h)
    }

    fn cfg_of(h: &Home) -> Value {
        serde_json::from_str(&std::fs::read_to_string(h.0.join(".factory/settings.json")).unwrap()).unwrap()
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

    fn models_of(c: &Value) -> Vec<String> {
        c["customModels"].as_array().unwrap().iter().map(|e| s(e, "model")).collect()
    }

    #[test]
    fn reads_state() {
        let _h = setup("state", Some(SAMPLE));
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert_eq!(st.mode, "multi");
        let ids: Vec<&str> = st.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["api-moonshot-cn", "open-bigmodel-cn", "relay-example-com"]);
        let glm = &st.providers[1];
        assert_eq!(glm.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["glm-5.2", "glm-4.6"]);
        assert!(glm.has_key && glm.api == "chat");
        assert_eq!(st.providers[2].api, "anthropic");
        assert!(st.current[1].v.contains("GLM Coding"), "{:?}", st.current);
        let (base, key, api) = provider_endpoint("open-bigmodel-cn").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://open.bigmodel.cn/api/coding/paas/v4", Some("sk-env-3333"), "chat"));
        assert_eq!(provider_endpoint("api-moonshot-cn").unwrap().1.as_deref(), Some("sk-moon-1111"));
    }

    #[test]
    fn missing_file_create() {
        let h = setup("create", None);
        let st = state(&Install::default());
        assert!(!st.readonly && st.providers.is_empty());
        assert!(plan(&[upsert(None, "Z", "https://z.example.com/v1", "chat", None, &[])], true).is_err());
        let op = upsert(None, "My Relay", "https://r.example.com/v1/", "responses", Some("sk-new-abcd9876"), &["gpt-5", "gpt-5-mini"]);
        let (d, w, _) = plan(&[op.clone()], true).unwrap();
        assert!(w.is_empty() && !h.0.join(".factory/settings.json").exists());
        let t = diff_text(&d);
        assert!(t.contains("••••9876") && !t.contains("abcd9876"), "{t}");
        let (_, w, _) = plan(&[op], false).unwrap();
        assert_eq!(w.len(), 1);
        let c = cfg_of(&h);
        assert_eq!(models_of(&c), ["gpt-5", "gpt-5-mini"]);
        assert_eq!(c["customModels"][0]["provider"], "openai");
        assert_eq!(c["customModels"][0]["baseUrl"], "https://r.example.com/v1");
        let st = state(&Install::default());
        assert_eq!(st.providers[0].id, "my-relay");
        assert_eq!(st.providers[0].name, "My Relay");
    }

    #[test]
    fn hide_shifts_selection_and_fixes_model() {
        let h = setup("hide", Some(SAMPLE));
        // Hiding Kimi (index 0) shifts GLM Coding from 1 to 0.
        let (d, _, _) = plan(&[Op::SetModelVisible { provider: "api-moonshot-cn".into(), model: "kimi-k2".into(), visible: false }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(c["model"], "custom:GLM-Coding-0");
        assert!(diff_text(&d).contains("custom:GLM-Coding-0"));
        assert_eq!(models_of(&c), ["glm-5.2", "glm-4.6", "claude-sonnet-4-5"]);
        // unrelated keys kept in order
        let keys: Vec<&String> = c.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "reasoningEffort", "customModels", "enableCompletionBell"]);
        let st = state(&Install::default());
        let kimi = st.providers.iter().find(|p| p.id == "api-moonshot-cn").unwrap();
        assert!(kimi.enabled && !kimi.models[0].visible);
        // Show again: appended at the end, selection unchanged.
        plan(&[Op::SetModelVisible { provider: "api-moonshot-cn".into(), model: "kimi-k2".into(), visible: true }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(models_of(&c), ["glm-5.2", "glm-4.6", "claude-sonnet-4-5", "kimi-k2"]);
        assert_eq!(c["model"], "custom:GLM-Coding-0");
        assert_eq!(c["customModels"][3]["apiKey"], "sk-moon-1111");
    }

    #[test]
    fn delete_selected_clears_model_and_rename_follows() {
        let h = setup("del", Some(SAMPLE));
        let (d, _, _) = plan(&[Op::DeleteModel { provider: "open-bigmodel-cn".into(), model: "glm-5.2".into() }], false).unwrap();
        assert!(diff_text(&d).contains("- model"));
        assert!(cfg_of(&h).get("model").is_none());

        let h = setup("rename", Some(SAMPLE));
        plan(&[Op::UpsertModel { provider: "open-bigmodel-cn".into(), model: ModelInput { id: "glm-5.2".into(), name: Some("GLM Five".into()), context: None, ..Default::default() } }], false).unwrap();
        assert_eq!(cfg_of(&h)["model"], "custom:GLM-Five-1");
    }

    #[test]
    fn edit_disable_enable_delete_provider() {
        let h = setup("prov", Some(SAMPLE));
        let ops = vec![
            upsert(Some("open-bigmodel-cn"), "Zhipu", "https://open.bigmodel.cn/api/paas/v4", "chat", Some("sk-zp-5555"), &[]),
            Op::UpsertModel { provider: "open-bigmodel-cn".into(), model: ModelInput { id: "glm-4.5-air".into(), name: None, context: None, ..Default::default() } },
        ];
        let (d, _, _) = plan(&ops, false).unwrap();
        let t = diff_text(&d);
        assert!(t.contains("••••5555") && !t.contains("sk-zp-5555"), "{t}");
        let c = cfg_of(&h);
        for i in 1..3 {
            assert_eq!(c["customModels"][i]["baseUrl"], "https://open.bigmodel.cn/api/paas/v4");
            assert_eq!(c["customModels"][i]["apiKey"], "sk-zp-5555");
        }
        assert_eq!(c["customModels"][4]["model"], "glm-4.5-air");
        assert_eq!(c["customModels"][4]["apiKey"], "sk-zp-5555");
        let st = state(&Install::default());
        let zp = st.providers.iter().find(|p| p.name == "Zhipu").unwrap();
        assert_eq!(zp.id, "zhipu");
        assert_eq!(zp.models.len(), 3);

        // Disable: entries parked, selection (GLM Coding) lost.
        let (d, _, _) = plan(&[Op::SetProviderEnabled { provider: "zhipu".into(), enabled: false }], false).unwrap();
        assert!(diff_text(&d).contains("- model"));
        let c = cfg_of(&h);
        assert_eq!(models_of(&c), ["kimi-k2", "claude-sonnet-4-5"]);
        let st = state(&Install::default());
        let zp = st.providers.iter().find(|p| p.id == "zhipu").unwrap();
        assert!(!zp.enabled && zp.models.len() == 3);
        assert!(plan(&[Op::SetModelVisible { provider: "zhipu".into(), model: "glm-4.6".into(), visible: false }], true).is_err());
        assert_eq!(provider_endpoint("zhipu").unwrap().1.as_deref(), Some("sk-zp-5555"));

        plan(&[Op::SetProviderEnabled { provider: "zhipu".into(), enabled: true }], false).unwrap();
        assert_eq!(models_of(&cfg_of(&h)), ["kimi-k2", "claude-sonnet-4-5", "glm-5.2", "glm-4.6", "glm-4.5-air"]);

        plan(&[Op::SetProviderModels { provider: "zhipu".into(), models: vec!["glm-4.6".into(), "glm-5".into()] }], false).unwrap();
        assert_eq!(models_of(&cfg_of(&h)), ["kimi-k2", "claude-sonnet-4-5", "glm-4.6", "glm-5"]);

        plan(&[Op::DeleteProvider { provider: "zhipu".into() }], false).unwrap();
        assert_eq!(models_of(&cfg_of(&h)), ["kimi-k2", "claude-sonnet-4-5"]);
        assert!(state(&Install::default()).providers.iter().all(|p| p.id != "zhipu"));
        assert!(plan(&[Op::SetCurrentProvider { provider: "x".into() }], true).is_err());
        assert!(plan(&[upsert(None, "G", "https://g/v1", "gemini", None, &["m"])], true).is_err());
    }

    #[test]
    fn explicit_ids_and_legacy() {
        let h = setup("ids", Some(r#"{"model":"custom:B-1","customModels":[{"model":"a","displayName":"A","id":"custom:A-0","index":0,"baseUrl":"https://x/v1","apiKey":"k","provider":"openai"},{"model":"b","displayName":"B","id":"custom:B-1","index":1,"baseUrl":"https://x/v1","apiKey":"k","provider":"openai"}]}"#));
        std::fs::write(h.0.join(".factory/config.json"), r#"{"custom_models":[{"model_display_name":"Old","model":"old-1","base_url":"https://old.example.com/v1","api_key":"sk-old-7777","provider":"openai","max_tokens":8192},{"model":"a","base_url":"https://x/v1","api_key":"k","provider":"openai"}]}"#).unwrap();
        let st = state(&Install::default());
        let legacy: Vec<&Provider> = st.providers.iter().filter(|p| p.id.starts_with("legacy-")).collect();
        assert_eq!(legacy.len(), 1);
        assert!(!legacy[0].editable && legacy[0].models.len() == 1);
        assert_eq!(provider_endpoint(&legacy[0].id).unwrap().1.as_deref(), Some("sk-old-7777"));
        assert!(plan(&[Op::DeleteProvider { provider: legacy[0].id.clone() }], true).is_err());

        plan(&[Op::DeleteModel { provider: "x".into(), model: "a".into() }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(c["customModels"][0]["id"], "custom:B-0");
        assert_eq!(c["customModels"][0]["index"], 0);
        assert_eq!(c["model"], "custom:B-0");
    }

    #[test]
    fn dry_run_and_comments() {
        let h = setup("dry", Some(SAMPLE));
        let before = std::fs::read(h.0.join(".factory/settings.json")).unwrap();
        let (d, w, b) = plan(&[Op::DeleteProvider { provider: "api-moonshot-cn".into() }], true).unwrap();
        assert!(!d.groups.is_empty() && w.is_empty() && b.is_none());
        assert_eq!(before, std::fs::read(h.0.join(".factory/settings.json")).unwrap());
        assert!(!h.0.join("agentplus-store.json").exists());
        let _h2 = setup("jsonc", Some("{\n  // c\n  \"customModels\": []\n}"));
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[upsert(None, "x", "https://x/v1", "chat", None, &["m"])], true).is_err());
    }

    /// Read-only look at the real machine: state and a dry-run plan (nothing is written).
    #[test]
    #[ignore]
    fn dump_droid() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={}", inst.installed, inst.version, inst.running);
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
