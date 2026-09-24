//! Qwen Code (CLI): `~/.qwen/settings.json` (JSONC, `"$version": 4`).
//!
//! Models live in `modelProviders.<protocol>[]`, one entry per model at one endpoint:
//! `{ id, name, baseUrl, envKey, wireApi, generationConfig: { contextWindowSize } }`.
//! The array key is the protocol (`openai`, `anthropic`, `gemini`, `vertex-ai`, or a
//! custom key declared in `providerProtocol`). AgentPlus shows the entries that share
//! (array key, baseUrl, envKey) as one provider; its id is derived from the envKey
//! (`AGENTPLUS_MY_RELAY_API_KEY` → `my-relay`), else from protocol + host.
//!
//! Keys are never inlined: `envKey` names a variable (shell, `~/.qwen/.env`, or the
//! settings `env` block); AgentPlus writes keys into the `env` block. Hidden models,
//! disabled providers and providers without models are stashed in the AgentPlus store.
//! The active model is `security.auth.selectedType` + `model.name`; Qwen Code
//! hot-reloads `modelProviders`.

use super::msg;
use super::{Plan, Endpoint};
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const ID: &str = "qwen";
pub const NAME: &str = "Qwen Code";
/// File whose presence in the dir means "configured here".
pub const MARKER: &str = "settings.json";
/// Qwen runs on node, so the process is found by its command line (`[q]` keeps pgrep
/// from matching this very shell).
pub const WSL_SCRIPT: &str = "(command -v qwen >/dev/null && qwen --version) 2>/dev/null; pgrep -f '[q]wen-code' >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".qwen/settings.json";

const STANDARD: [&str; 4] = ["openai", "anthropic", "gemini", "vertex-ai"];
fn store_label() -> &'static str {
    l("AgentPlus · Qwen Code 暂存", "AgentPlus · Qwen Code stash")
}
const OAUTH: &str = "qwen-oauth";

// ---------------------------------------------------------------- paths & test hooks

/// `$QWEN_HOME` (Windows side only), else `~/.qwen`.
pub fn default_dir() -> PathBuf {
    if let Some(h) = crate::env::agent_var("QWEN_HOME") {
        return PathBuf::from(h);
    }
    home().join(".qwen")
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn dotenv_path() -> PathBuf {
    dir().join(".env")
}

// ---------------------------------------------------------------- detection

/// npm global `@qwen-code/qwen-code`, or the standalone installer
/// (`%LOCALAPPDATA%\qwen-code\bin\qwen.cmd`). A CLI: `exe` stays None.
pub fn detect() -> Install {
    let mut inst = Install::default();
    let pkg = |root: &Path| root.join("node_modules").join("@qwen-code").join("qwen-code").join("package.json");
    let on_path = crate::process::on_path(&["qwen.cmd", "qwen.exe", "qwen"]);
    let mut roots: Vec<PathBuf> = dirs::data_dir().map(|d| d.join("npm")).into_iter().collect();
    if let Some(d) = on_path.as_ref().and_then(|p| p.parent()) {
        roots.push(d.to_path_buf());
    }
    if let Some(v) = roots.iter().find_map(|r| crate::process::package_version(&pkg(r))) {
        inst.installed = true;
        inst.version = Some(v);
    } else if let Some(sd) = dirs::data_local_dir().map(|d| d.join("qwen-code")).filter(|d| d.join("bin").join("qwen.cmd").exists()) {
        inst.installed = true;
        inst.version = [sd.join("package.json"), pkg(&sd), pkg(&sd.join("lib"))]
            .iter()
            .find_map(|p| crate::process::package_version(p))
            .or_else(|| crate::process::cli_version(&sd.join("bin").join("qwen.cmd")));
        inst.dir = Some(sd);
    } else if let Some(p) = on_path {
        inst.installed = true;
        inst.version = crate::process::cli_version(&p);
    }
    if inst.installed {
        inst.running = crate::process::any_process(|name, path| {
            name.eq_ignore_ascii_case("qwen.exe") || path.to_lowercase().contains("\\qwen-code\\")
        });
    }
    inst
}

// ---------------------------------------------------------------- reading

/// (settings, meta, has_comments). A missing or empty file reads as fresh v4 settings.
fn load() -> Result<(Value, TextMeta, bool)> {
    let (text, meta) = read_text_or_new(&settings_path())?;
    if text.trim().is_empty() {
        return Ok((json!({ "$version": 4 }), meta, false));
    }
    let (v, had) = parse_jsonc_object(&text, "settings.json")?;
    Ok((v, meta, had))
}

/// Where a key variable is set: (value, source).
fn lookup(cfg: &Value, name: &str) -> Option<(String, &'static str)> {
    if let Some(v) = cfg.get("env").and_then(|e| e.get(name)).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
        return Some((v.to_string(), l("settings.json 的 env", "the env block of settings.json")));
    }
    if let Some(v) = crate::dotenv::get(&crate::dotenv::load(&dotenv_path()).0, name) {
        return Some((v, "~/.qwen/.env"));
    }
    if let Some(v) = crate::env::agent_var(name) {
        return Some((v, l("系统环境变量", "system environment variables")));
    }
    None
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn norm(u: Option<&str>) -> String {
    u.unwrap_or("").trim().trim_end_matches('/').to_string()
}

fn env_of(e: &Value) -> Option<&str> {
    s(e, "envKey").filter(|k| !k.trim().is_empty())
}

fn proto_of(cfg: &Value, key: &str) -> Option<String> {
    if STANDARD.contains(&key) {
        return Some(key.into());
    }
    cfg.get("providerProtocol").and_then(|p| p.get(key)).and_then(|x| x.as_str()).filter(|p| STANDARD.contains(p)).map(String::from)
}

fn api_of(proto: Option<&str>, e: &Value) -> &'static str {
    match proto {
        Some("anthropic") => "anthropic",
        Some("gemini") | Some("vertex-ai") => "gemini",
        _ if s(e, "wireApi") == Some("responses") => "responses",
        _ => "chat",
    }
}

fn key_for_api(api: &str) -> &'static str {
    match api {
        "anthropic" => "anthropic",
        "gemini" => "gemini",
        _ => "openai",
    }
}

/// Variable Qwen falls back to when an entry has no envKey.
fn default_var(proto: &str) -> &'static str {
    match proto {
        "anthropic" => "ANTHROPIC_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "vertex-ai" => "GOOGLE_API_KEY",
        _ => "OPENAI_API_KEY",
    }
}

fn agentplus_var(id: &str) -> String {
    format!("AGENTPLUS_{}_API_KEY", id.to_uppercase().replace('-', "_"))
}

fn check_api(api: &str) -> Result<&str> {
    match api {
        "chat" | "responses" | "anthropic" | "gemini" => Ok(api),
        other => Err(anyhow!(tr!("Qwen Code 不支持 {other} 接口", "Qwen Code doesn't support the {other} API"))),
    }
}

/// Entries sharing (array key, baseUrl, envKey) = one AgentPlus provider.
#[derive(Clone, Debug)]
struct Group {
    id: String,
    /// Array key under `modelProviders`.
    key: String,
    /// Resolved protocol; None = custom key without `providerProtocol` (Qwen skips it).
    proto: Option<String>,
    base: Option<String>,
    env_key: Option<String>,
    /// First entry (or the stashed template): baseUrl / envKey / wireApi for new entries.
    tmpl: Value,
    live: Vec<Value>,
    hidden: Vec<Value>,
    enabled: bool,
}

impl Group {
    fn owns(&self, key: &str, e: &Value) -> bool {
        e.is_object() && key == self.key && norm(s(e, "baseUrl")) == norm(self.base.as_deref()) && env_of(e) == self.env_key.as_deref()
    }
    fn same(&self, o: &Group) -> bool {
        self.key == o.key && norm(self.base.as_deref()) == norm(o.base.as_deref()) && self.env_key == o.env_key
    }
    fn api(&self) -> &'static str {
        api_of(self.proto.as_deref(), &self.tmpl)
    }
    /// The variable the key is read from (envKey, or the protocol default).
    fn key_var(&self) -> Option<String> {
        self.env_key.clone().or_else(|| self.proto.as_deref().map(|p| default_var(p).to_string()))
    }
    fn template(&self) -> Value {
        let mut t = Map::new();
        for k in ["baseUrl", "envKey", "wireApi"] {
            if let Some(v) = self.tmpl.get(k) {
                t.insert(k.into(), v.clone());
            }
        }
        Value::Object(t)
    }
    fn has_live(&self, id: &str) -> bool {
        self.live.iter().any(|e| s(e, "id") == Some(id))
    }
    fn has_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|e| s(e, "id") == Some(id))
    }
}

fn base_id(key: &str, base: Option<&str>, env: Option<&str>) -> String {
    if let Some(e) = env {
        let e = e.strip_prefix("AGENTPLUS_").unwrap_or(e);
        let e = e.strip_suffix("_API_KEY").or_else(|| e.strip_suffix("_KEY")).filter(|x| !x.is_empty()).unwrap_or(e);
        return slug(e);
    }
    slug(&format!("{key}-{}", base.map(host_of).unwrap_or_else(|| "default".into())))
}

fn store_list(root: &Value, k: &str) -> Vec<Value> {
    store::get_arr(root, ID, k)
}

fn store_obj(root: &Value, k: &str) -> Map<String, Value> {
    store::get_obj(root, ID, k)
}

fn attach(out: &mut Vec<Group>, cfg: &Value, key: &str, e: &Value, live: bool) {
    if let Some(g) = out.iter_mut().find(|g| g.owns(key, e)) {
        if live { g.live.push(e.clone()) } else { g.hidden.push(e.clone()) }
        return;
    }
    out.push(Group {
        id: String::new(),
        key: key.into(),
        proto: proto_of(cfg, key),
        base: s(e, "baseUrl").map(String::from),
        env_key: env_of(e).map(String::from),
        tmpl: e.clone(),
        live: if live { vec![e.clone()] } else { vec![] },
        hidden: if live { vec![] } else { vec![e.clone()] },
        enabled: true,
    });
}

/// Every provider: live entries (config order), hidden-only ones, empty ones, then disabled.
fn groups(cfg: &Value, root: &Value) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    if let Some(mp) = cfg.get("modelProviders").and_then(|x| x.as_object()) {
        for (k, arr) in mp {
            for e in arr.as_array().into_iter().flatten().filter(|e| e.is_object()) {
                attach(&mut out, cfg, k, e, true);
            }
        }
    }
    for h in store_list(root, "hiddenModels") {
        if let (Some(k), Some(e)) = (s(&h, "key"), h.get("entry").filter(|e| e.is_object())) {
            attach(&mut out, cfg, k, e, false);
        }
    }
    for sk in store_list(root, "emptyProviders") {
        if let (Some(k), Some(t)) = (s(&sk, "key"), sk.get("template").filter(|t| t.is_object())) {
            if !out.iter().any(|g| g.owns(k, t)) {
                attach(&mut out, cfg, k, t, false);
                out.last_mut().unwrap().hidden.clear();
            }
        }
    }
    let disabled = store_obj(root, "disabledProviders");
    let mut taken: HashSet<String> = disabled.keys().cloned().collect();
    taken.insert(OAUTH.into());
    // Shared ids go to the standard protocols first (openai, anthropic, …), so an id does
    // not jump to another group when a group moves between the config and the stash.
    let rank = |k: &str| STANDARD.iter().position(|x| *x == k).unwrap_or(STANDARD.len());
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|&i| rank(&out[i].key));
    for i in order {
        let g = &mut out[i];
        let b = base_id(&g.key, g.base.as_deref(), g.env_key.as_deref());
        // Before numbering, a taken id tries the protocol-qualified one.
        let by_key = format!("{b}-{}", slug(&g.key));
        let id = if taken.contains(&b) && !taken.contains(&by_key) { by_key } else { unique_id(&b, |c| taken.contains(c)) };
        taken.insert(id.clone());
        g.id = id;
    }
    for (pid, d) in disabled {
        let key = s(&d, "key").unwrap_or("openai").to_string();
        let arr = |k: &str| d.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let (live, hidden) = (arr("entries"), arr("hidden"));
        let tmpl = d.get("template").filter(|t| t.is_object()).cloned().or_else(|| live.first().cloned()).unwrap_or_else(|| json!({}));
        out.push(Group {
            id: pid,
            proto: proto_of(cfg, &key),
            base: s(&tmpl, "baseUrl").map(String::from),
            env_key: env_of(&tmpl).map(String::from),
            key,
            tmpl,
            live,
            hidden,
            enabled: false,
        });
    }
    out
}

struct Sel {
    auth: Option<String>,
    model: Option<String>,
    base: Option<String>,
}

fn selection(cfg: &Value) -> Sel {
    let g = |p: &str| cfg.pointer(p).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from);
    Sel { auth: g("/security/auth/selectedType"), model: g("/model/name"), base: g("/model/baseUrl") }
}

fn is_selected(sel: &Sel, g: &Group, id: &str) -> bool {
    g.enabled
        && sel.auth.as_deref() == Some(g.key.as_str())
        && sel.model.as_deref() == Some(id)
        && sel.base.as_deref().map(|b| norm(Some(b)) == norm(g.base.as_deref())).unwrap_or(true)
}

fn model_of(e: &Value, visible: bool, selected: bool) -> Option<Model> {
    let id = s(e, "id")?.to_string();
    let context = e.pointer("/generationConfig/contextWindowSize").and_then(|x| x.as_u64());
    Some(Model {
        visible,
        readonly: false,
        tags: if selected { vec![Tag::current()] } else { vec![] },
        ctx: context.map(fmt_ctx),
        name: s(e, "name").filter(|n| !n.is_empty()).map(String::from),
        context,
        deletable: true,
        extra: crate::mfields::read(e, crate::mfields::QWEN),
        id,
    })
}

fn fallback_name(g: &Group) -> String {
    g.base
        .as_deref()
        .map(|b| host_of(b).split('/').next().unwrap_or("").to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| tr!("{} 默认地址", "{} default URL", api_label(g.api())))
}

fn provider_of(g: &Group, cfg: &Value, names: &Map<String, Value>, sel: &Sel) -> Provider {
    let api = g.api();
    let mut models: Vec<Model> = g.live.iter().filter_map(|e| model_of(e, true, s(e, "id").map(|id| is_selected(sel, g, id)).unwrap_or(false))).collect();
    models.extend(g.hidden.iter().filter_map(|e| model_of(e, false, false)));
    let var = g.key_var();
    let found = var.as_deref().and_then(|v| lookup(cfg, v));
    let key_note = match (&var, &found) {
        (Some(v), Some((_, src))) => tr!("环境变量 {v} · 已在{src}设置", "Environment variable {v} · set in {src}"),
        (Some(v), None) => tr!("环境变量 {v} · 未设置，请求会失败", "Environment variable {v} · not set, requests will fail"),
        _ => l("未设置", "Not set").into(),
    };
    let mut details = vec![
        Kv::mono(lbl::config_location(), tr!("modelProviders.{}（{} 个条目）", "modelProviders.{} ({} entries)", g.key, g.live.len())),
        Kv::mono("envKey", g.env_key.clone().unwrap_or_else(|| tr!("-（默认 {}）", "- (default {})", var.clone().unwrap_or_default()))),
        Kv::text(lbl::api_key(), key_note),
        Kv::text(lbl::status(), if g.enabled { l("已启用", "Enabled") } else { l("已停用 · 条目暂存在 AgentPlus", "Disabled · entries kept in AgentPlus") }),
    ];
    if g.proto.as_deref().is_some_and(|p| p != g.key) {
        details.push(Kv::mono("providerProtocol", format!("{} → {}", g.key, g.proto.as_deref().unwrap_or(""))));
    }
    Provider {
        id: g.id.clone(),
        name: names.get(&g.id).and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| fallback_name(g)),
        base_url: g.base.clone(),
        host: g.base.as_deref().map(host_of).unwrap_or_else(|| l("默认地址", "Default URL").into()),
        apis: vec![api_label(api).into()],
        enabled: g.enabled,
        compatible: g.proto.is_some(),
        reason: g.proto.is_none().then(|| tr!("modelProviders.{} 没有在 providerProtocol 里声明协议，Qwen Code 会忽略它", "modelProviders.{} has no protocol declared in providerProtocol, so Qwen Code ignores it", g.key)),
        models,
        details,
        editable: g.proto.is_some(),
        api: api.into(),
        has_key: found.is_some(),
        ..Default::default()
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&settings_path()), display_path(&dotenv_path())]);
    let cfg = match load() {
        Ok((cfg, _, had)) => {
            if had {
                st.readonly = true;
                st.notes.push(msg::comments_readonly("settings.json"));
            }
            cfg
        }
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let root = store::load();
    let names = store_obj(&root, "names");
    let sel = selection(&cfg);
    let gs = groups(&cfg, &root);

    if sel.auth.as_deref() == Some(OAUTH) || dir().join("oauth_creds.json").exists() {
        st.providers.push(Provider::builtin(
            OAUTH,
            l("Qwen 账号（OAuth）", "Qwen account (OAuth)"),
            l("Qwen OAuth 登录", "Qwen OAuth sign-in"),
            "chat",
            l("账号", "Account"),
            vec![
                Kv::text(lbl::auth(), l("qwen-oauth（~/.qwen/oauth_creds.json）", "qwen-oauth (~/.qwen/oauth_creds.json)")),
                Kv::text(lbl::note(), l("Qwen Code 内置的账号登录，模型由 Qwen Code 管理，用 /auth 切换", "Qwen Code's built-in account sign-in. Models are managed by Qwen Code; switch with /auth.")),
            ],
        ));
    }
    let mut provs: Vec<Provider> = gs.iter().map(|g| provider_of(g, &cfg, &names, &sel)).collect();
    // Two unnamed providers on the same host: tell them apart by protocol.
    let dup: Vec<String> = provs.iter().map(|p| p.name.clone()).collect();
    for (p, g) in provs.iter_mut().zip(&gs) {
        if !names.contains_key(&p.id) && dup.iter().filter(|n| **n == p.name).count() > 1 {
            p.name = format!("{} · {}", p.name, g.key);
        }
    }
    st.providers.extend(provs);

    st.settings = vec![bool_setting(
        "usage_stats",
        "Qwen Code",
        l("发送使用统计", "Send usage statistics"),
        l("privacy.usageStatisticsEnabled：向 Qwen Code 发送匿名使用统计", "privacy.usageStatisticsEnabled: send anonymous usage statistics to Qwen Code"),
        cfg.pointer("/privacy/usageStatisticsEnabled").and_then(|x| x.as_bool()).unwrap_or(true),
    )];

    let cur = gs.iter().find(|g| sel.model.as_deref().map(|m| is_selected(&sel, g, m) && g.has_live(m)).unwrap_or(false));
    let cur_name = cur.and_then(|g| st.providers.iter().find(|p| p.id == g.id)).map(|p| p.name.clone());
    let vis: usize = st.providers.iter().filter(|p| p.enabled && !p.builtin).map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::mono(lbl::auth(), sel.auth.clone().unwrap_or_else(|| l("-（未选择）", "- (not selected)").into())),
        Kv::mono(lbl::current_model(), sel.model.clone().unwrap_or_else(|| "-".into())),
        Kv::text(lbl::provider(), cur_name.unwrap_or_else(|| if sel.auth.as_deref() == Some(OAUTH) { l("Qwen 账号（OAuth）", "Qwen account (OAuth)").into() } else { "-".into() })),
        Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")),
        Kv::mono(lbl::config_file(), display_path(&settings_path())),
    ];
    st.notes.push(l("Qwen Code 会热加载 modelProviders；在 Qwen Code 里用 /model 选择模型。", "Qwen Code hot-reloads modelProviders; pick models with /model in Qwen Code.").into());
    st.notes.push(l("密钥写入 settings.json 的 env 块（按 envKey 命名的变量），不会写进模型条目。", "API keys are written to the env block of settings.json (in the variable named by envKey), never into model entries.").into());
    if let (Some(m), Some(a)) = (&sel.model, &sel.auth) {
        if a != OAUTH && cur.is_none() && gs.iter().any(|g| g.key == *a) {
            st.notes.push(tr!("当前选中的模型 {m} 不在已启用的 modelProviders.{a} 里（可能已隐藏或停用）。", "The selected model {m} is not in the enabled modelProviders.{a} (it may be hidden or disabled)."));
        }
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _, _) = load()?;
    let g = groups(&cfg, &store::load()).into_iter().find(|g| g.id == id).ok_or_else(|| msg::no_provider(id))?;
    let base = g.base.clone().filter(|b| !b.trim().is_empty()).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 baseUrl", "Provider {id} has no baseUrl")))?;
    let key = g.key_var().and_then(|v| lookup(&cfg, &v)).map(|x| x.0);
    Ok((base, key, g.api().into()))
}

// ---------------------------------------------------------------- writing

struct Ctx {
    cfg: Value,
    root: Value,
    diff: Diff,
    file: String,
    cfg_dirty: bool,
    store_dirty: bool,
}

impl Ctx {
    fn groups(&self) -> Vec<Group> {
        groups(&self.cfg, &self.root)
    }

    fn group(&self, id: &str) -> Result<Group> {
        self.groups().into_iter().find(|g| g.id == id).ok_or_else(|| msg::no_provider(id))
    }

    fn enabled_group(&self, id: &str) -> Result<Group> {
        let g = self.group(id)?;
        if !g.enabled {
            return Err(anyhow!(tr!("供应商 {id} 已停用，先启用再调整模型", "Provider {id} is disabled. Enable it before changing its models.")));
        }
        if g.proto.is_none() {
            return Err(anyhow!(tr!("modelProviders.{} 没有声明协议，AgentPlus 不修改它", "modelProviders.{} has no declared protocol; AgentPlus won't modify it", g.key)));
        }
        Ok(g)
    }

    fn label(&self, g: &Group) -> String {
        store_obj(&self.root, "names").get(&g.id).and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| g.id.clone())
    }

    fn arr(&mut self, key: &str) -> Result<&mut Vec<Value>> {
        let root = self.cfg.as_object_mut().ok_or_else(|| anyhow!(l("settings.json 顶层不是对象", "settings.json is not a JSON object at the top level")))?;
        let mp = root.entry("modelProviders").or_insert_with(|| json!({}));
        let mp = mp.as_object_mut().ok_or_else(|| anyhow!(l("modelProviders 不是对象", "modelProviders is not an object")))?;
        mp.entry(key.to_string()).or_insert_with(|| json!([])).as_array_mut().ok_or_else(|| anyhow!(tr!("modelProviders.{key} 不是数组", "modelProviders.{key} is not an array")))
    }

    fn live_arr(&mut self, key: &str) -> Option<&mut Vec<Value>> {
        self.cfg.get_mut("modelProviders").and_then(|m| m.get_mut(key)).and_then(|a| a.as_array_mut())
    }

    /// Removes the entries of `g` matching `pred` from the config.
    fn take_live(&mut self, g: &Group, pred: &dyn Fn(&Value) -> bool) -> Vec<Value> {
        let Some(a) = self.live_arr(&g.key) else { return vec![] };
        let (taken, keep): (Vec<Value>, Vec<Value>) = std::mem::take(a).into_iter().partition(|e| g.owns(&g.key, e) && pred(e));
        *a = keep;
        if !taken.is_empty() {
            self.cfg_dirty = true;
        }
        taken
    }

    /// Drops `modelProviders.<key>` once it is empty.
    fn prune(&mut self, key: &str) {
        if self.live_arr(key).map(|a| a.is_empty()).unwrap_or(false) {
            if let Some(mp) = self.cfg.get_mut("modelProviders").and_then(|m| m.as_object_mut()) {
                mp.remove(key);
            }
        }
    }

    /// Inserts entries right after the group's last entry (or at the end of the array).
    fn put_live(&mut self, g: &Group, entries: Vec<Value>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let a = self.arr(&g.key)?;
        let pos = a.iter().rposition(|e| g.owns(&g.key, e)).map(|i| i + 1).unwrap_or(a.len());
        for (i, e) in entries.into_iter().enumerate() {
            a.insert(pos + i, e);
        }
        self.cfg_dirty = true;
        Ok(())
    }

    fn take_hidden(&mut self, g: &Group, pred: &dyn Fn(&Value) -> bool) -> Vec<Value> {
        let (taken, keep): (Vec<Value>, Vec<Value>) = store_list(&self.root, "hiddenModels")
            .into_iter()
            .partition(|h| s(h, "key").map(|k| h.get("entry").map(|e| g.owns(k, e) && pred(e)).unwrap_or(false)).unwrap_or(false));
        if !taken.is_empty() {
            store::set_value(&mut self.root, ID, "hiddenModels", Value::Array(keep));
            self.store_dirty = true;
        }
        taken.into_iter().filter_map(|mut h| h.get_mut("entry").map(Value::take)).collect()
    }

    fn push_hidden(&mut self, key: &str, e: Value) {
        let mut l = store_list(&self.root, "hiddenModels");
        l.push(json!({ "key": key, "entry": e }));
        store::set_value(&mut self.root, ID, "hiddenModels", Value::Array(l));
        self.store_dirty = true;
    }

    fn take_skeleton(&mut self, g: &Group) -> bool {
        let (taken, keep): (Vec<Value>, Vec<Value>) = store_list(&self.root, "emptyProviders")
            .into_iter()
            .partition(|x| s(x, "key").map(|k| x.get("template").map(|t| g.owns(k, t)).unwrap_or(false)).unwrap_or(false));
        if taken.is_empty() {
            return false;
        }
        store::set_value(&mut self.root, ID, "emptyProviders", Value::Array(keep));
        self.store_dirty = true;
        true
    }

    fn push_skeleton(&mut self, key: &str, template: Value) {
        let mut l = store_list(&self.root, "emptyProviders");
        l.push(json!({ "key": key, "template": template }));
        store::set_value(&mut self.root, ID, "emptyProviders", Value::Array(l));
        self.store_dirty = true;
    }

    /// Keeps a provider whose last model went away (Qwen has no provider-level entry).
    fn keep_if_gone(&mut self, g: &Group) {
        if !self.groups().iter().any(|x| x.enabled && x.same(g)) {
            self.push_skeleton(&g.key, g.template());
        }
    }

    fn set_env(&mut self, var: &str, key: &str) -> Result<()> {
        let root = self.cfg.as_object_mut().ok_or_else(|| anyhow!(l("settings.json 顶层不是对象", "settings.json is not a JSON object at the top level")))?;
        let env = root.entry("env").or_insert_with(|| json!({}));
        let env = env.as_object_mut().ok_or_else(|| anyhow!(l("env 不是对象", "env is not an object")))?;
        if env.get(var).and_then(|v| v.as_str()) != Some(key) {
            env.insert(var.into(), json!(key));
            self.diff.push(&self.file, format!("env.{var} = {}", mask_key(key)), true);
            self.cfg_dirty = true;
        }
        Ok(())
    }

    fn set_name(&mut self, id: &str, name: &str) {
        let mut names = store_obj(&self.root, "names");
        if names.get(id).and_then(|x| x.as_str()) != Some(name) {
            names.insert(id.into(), json!(name));
            store::set_value(&mut self.root, ID, "names", Value::Object(names));
            self.diff.push(store_label(), tr!("「{id}」名称 = {name}", "\"{id}\" name = {name}"), true);
            self.store_dirty = true;
        }
    }

    /// Applies `f` to every entry of `g` (config, hidden stash, skeleton, disabled stash),
    /// moving live entries to `modelProviders.<new_key>` when the protocol changes.
    fn transform(&mut self, g: &Group, new_key: &str, f: &dyn Fn(&mut Value)) -> Result<()> {
        if !g.enabled {
            let mut d = store_obj(&self.root, "disabledProviders");
            let rec = d.get_mut(&g.id).ok_or_else(|| msg::no_provider(&g.id))?;
            for k in ["entries", "hidden"] {
                if let Some(a) = rec.get_mut(k).and_then(|x| x.as_array_mut()) {
                    a.iter_mut().filter(|e| e.is_object()).for_each(f);
                }
            }
            if let Some(t) = rec.get_mut("template").filter(|t| t.is_object()) {
                f(t);
            }
            rec["key"] = json!(new_key);
            store::set_value(&mut self.root, ID, "disabledProviders", Value::Object(d));
            self.store_dirty = true;
            return Ok(());
        }
        if new_key == g.key {
            if let Some(a) = self.live_arr(&g.key) {
                for e in a.iter_mut() {
                    if g.owns(&g.key, e) {
                        f(e);
                    }
                }
            }
            self.cfg_dirty |= !g.live.is_empty();
        } else {
            let mut moved = self.take_live(g, &|_| true);
            self.prune(&g.key);
            moved.iter_mut().for_each(f);
            if !moved.is_empty() {
                self.arr(new_key)?.extend(moved);
            }
        }
        for (list, field) in [("hiddenModels", "entry"), ("emptyProviders", "template")] {
            let mut l = store_list(&self.root, list);
            let mut any = false;
            for item in l.iter_mut() {
                let mine = s(item, "key").map(|k| item.get(field).map(|e| g.owns(k, e)).unwrap_or(false)).unwrap_or(false);
                if mine {
                    f(&mut item[field]);
                    item["key"] = json!(new_key);
                    any = true;
                }
            }
            if any {
                store::set_value(&mut self.root, ID, list, Value::Array(l));
                self.store_dirty = true;
            }
        }
        Ok(())
    }

    fn create_provider(&mut self, p: &ProviderInput) -> Result<()> {
        let api = check_api(&p.api)?;
        let gs = self.groups();
        let taken: HashSet<String> = gs.iter().map(|g| g.id.clone()).chain([OAUTH.to_string()]).collect();
        let used_env: HashSet<String> = gs.iter().filter_map(|g| g.env_key.clone()).collect();
        let id = unique_id(&slug(p.name.trim()), |c| taken.contains(c) || used_env.contains(&agentplus_var(c)));
        let var = agentplus_var(&id);
        let key = key_for_api(api);
        let mut tmpl = json!({ "baseUrl": p.base_url.trim(), "envKey": var });
        if key == "openai" {
            tmpl["wireApi"] = json!(if api == "responses" { "responses" } else { "chat-completions" });
        }
        let models = clean_ids(&p.models);
        if models.is_empty() {
            self.push_skeleton(key, tmpl.clone());
            self.diff.push(store_label(), tr!("+ 「{}」{}（{} · 还没有模型，添加模型后写入 modelProviders.{key}）", "+ \"{}\" {} ({} · no models yet; written to modelProviders.{key} once you add models)", p.name.trim(), p.base_url.trim(), api_label(api)), true);
        } else {
            let entries: Vec<Value> = models.iter().map(|m| new_entry(&tmpl, m, None, None)).collect();
            self.arr(key)?.extend(entries);
            self.diff.push(&self.file, tr!("+ modelProviders.{key}：「{}」{} · {} · {} 个模型（envKey = {var}）", "+ modelProviders.{key}: \"{}\" {} · {} · {} models (envKey = {var})", p.name.trim(), p.base_url.trim(), api_label(api), models.len()), true);
            self.cfg_dirty = true;
        }
        self.set_name(&id, p.name.trim());
        if let Some(k) = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            self.set_env(&var, k)?;
        }
        Ok(())
    }

    fn edit_provider(&mut self, id: &str, p: &ProviderInput) -> Result<()> {
        let api = check_api(&p.api)?;
        let g = self.group(id)?;
        let proto = g.proto.clone().ok_or_else(|| anyhow!(tr!("modelProviders.{} 没有声明协议，AgentPlus 不修改它", "modelProviders.{} has no declared protocol; AgentPlus won't modify it", g.key)))?;
        let same_family = g.api() == api || (proto == "openai" && (api == "chat" || api == "responses"));
        let new_key = if same_family { g.key.clone() } else { key_for_api(api).to_string() };
        let openai = proto_of(&self.cfg, &new_key).as_deref() == Some("openai");
        let base = p.base_url.trim().to_string();
        let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from);
        let new_env = (key.is_some() && g.env_key.is_none()).then(|| agentplus_var(id));
        let base_changed = norm(g.base.as_deref()) != norm(Some(&base));
        let api_changed = g.api() != api;
        let n = g.live.len() + g.hidden.len();
        if base_changed || api_changed || new_env.is_some() {
            let f = |e: &mut Value| {
                let Some(o) = e.as_object_mut() else { return };
                o.insert("baseUrl".into(), json!(base));
                if !openai {
                    o.remove("wireApi");
                } else if api == "responses" {
                    o.insert("wireApi".into(), json!("responses"));
                } else if o.contains_key("wireApi") {
                    o.insert("wireApi".into(), json!("chat-completions"));
                }
                if let Some(v) = &new_env {
                    o.insert("envKey".into(), json!(v));
                }
            };
            self.transform(&g, &new_key, &f)?;
            let file = if g.enabled { self.file.clone() } else { store_label().to_string() };
            if base_changed {
                self.diff.push(&file, tr!("modelProviders.{new_key}「{id}」baseUrl = \"{base}\"（{n} 个条目）", "modelProviders.{new_key} \"{id}\" baseUrl = \"{base}\" ({n} entries)"), true);
            }
            if api_changed {
                let moved = if new_key != g.key { tr!("，移到 modelProviders.{new_key}", ", moved to modelProviders.{new_key}") } else { String::new() };
                self.diff.push(&file, tr!("「{id}」协议 {} → {}{moved}", "\"{id}\" protocol {} → {}{moved}", api_label(g.api()), api_label(api)), true);
            }
            if let Some(v) = &new_env {
                self.diff.push(&file, tr!("「{id}」envKey = \"{v}\"（{n} 个条目）", "\"{id}\" envKey = \"{v}\" ({n} entries)"), true);
            }
        }
        // The id follows (key, baseUrl, envKey); carry the name over if it moved.
        let new_id = if g.enabled {
            let env = new_env.clone().or(g.env_key.clone());
            self.groups().into_iter().find(|x| x.enabled && x.key == new_key && norm(x.base.as_deref()) == norm(Some(&base)) && x.env_key == env).map(|x| x.id).unwrap_or_else(|| id.to_string())
        } else {
            id.to_string()
        };
        if new_id != id {
            let mut names = store_obj(&self.root, "names");
            if let Some(v) = names.remove(id) {
                names.insert(new_id.clone(), v);
                store::set_value(&mut self.root, ID, "names", Value::Object(names));
                self.store_dirty = true;
            }
        }
        if !p.name.trim().is_empty() {
            self.set_name(&new_id, p.name.trim());
        }
        if let Some(k) = key {
            let var = new_env.or(g.key_var()).ok_or_else(|| anyhow!(l("没有可写入密钥的变量", "No variable to write the API key to")))?;
            self.set_env(&var, &k)?;
        }
        Ok(())
    }

    fn delete_provider(&mut self, id: &str) -> Result<()> {
        let g = self.group(id)?;
        let name = self.label(&g);
        if g.enabled {
            let n = self.take_live(&g, &|_| true).len();
            self.prune(&g.key);
            let h = self.take_hidden(&g, &|_| true).len();
            self.take_skeleton(&g);
            if n > 0 {
                self.diff.push(&self.file, tr!("- modelProviders.{}：「{name}」的 {n} 个条目", "- modelProviders.{}: {n} entries of \"{name}\"", g.key), false);
            }
            if h > 0 || n == 0 {
                self.diff.push(store_label(), tr!("- 「{name}」暂存的 {h} 个隐藏模型", "- {h} stashed hidden models of \"{name}\""), false);
            }
        } else {
            let mut d = store_obj(&self.root, "disabledProviders");
            d.remove(id);
            store::set_value(&mut self.root, ID, "disabledProviders", Value::Object(d));
            self.store_dirty = true;
            self.diff.push(store_label(), tr!("- 「{name}」（已停用，暂存的条目一并删除）", "- \"{name}\" (disabled; stashed entries deleted too)"), false);
        }
        let mut names = store_obj(&self.root, "names");
        if names.remove(id).is_some() {
            store::set_value(&mut self.root, ID, "names", Value::Object(names));
            self.store_dirty = true;
        }
        // Drop the key variable once nothing uses it any more.
        if let Some(var) = g.env_key.as_deref() {
            let used = self.groups().iter().any(|o| o.env_key.as_deref() == Some(var));
            let has = self.cfg.get("env").and_then(|e| e.get(var)).is_some();
            if !used && has {
                self.cfg["env"].as_object_mut().unwrap().remove(var);
                self.diff.push(&self.file, tr!("env.{var}（删除）", "env.{var} (deleted)"), false);
                self.cfg_dirty = true;
            }
        }
        Ok(())
    }

    fn set_enabled(&mut self, id: &str, on: bool) -> Result<()> {
        let g = self.group(id)?;
        if g.enabled == on {
            return Ok(());
        }
        let name = self.label(&g);
        let mut d = store_obj(&self.root, "disabledProviders");
        if !on {
            let entries = self.take_live(&g, &|_| true);
            self.prune(&g.key);
            let hidden = self.take_hidden(&g, &|_| true);
            self.take_skeleton(&g);
            self.diff.push(&self.file, tr!("- modelProviders.{}：「{name}」的 {} 个条目（暂存在 AgentPlus，可恢复）", "- modelProviders.{}: {} entries of \"{name}\" (kept in AgentPlus, can be restored)", g.key, entries.len()), false);
            d.insert(g.id.clone(), json!({ "key": g.key, "entries": entries, "hidden": hidden, "template": g.template() }));
        } else {
            let rec = d.remove(id).unwrap_or_default();
            let key = s(&rec, "key").unwrap_or(&g.key).to_string();
            let arr = |k: &str| rec.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let (entries, hidden) = (arr("entries"), arr("hidden"));
            if !entries.is_empty() {
                self.diff.push(&self.file, tr!("+ modelProviders.{key}：「{name}」的 {} 个条目", "+ modelProviders.{key}: {} entries of \"{name}\"", entries.len()), true);
                self.arr(&key)?.extend(entries.clone());
                self.cfg_dirty = true;
            } else {
                self.diff.push(store_label(), tr!("+ 「{name}」（还没有可见模型）", "+ \"{name}\" (no visible models yet)"), true);
            }
            for e in hidden.iter().cloned() {
                self.push_hidden(&key, e);
            }
            if entries.is_empty() && hidden.is_empty() {
                self.push_skeleton(&key, rec.get("template").cloned().unwrap_or_else(|| g.template()));
            }
        }
        store::set_value(&mut self.root, ID, "disabledProviders", Value::Object(d));
        self.store_dirty = true;
        Ok(())
    }

    fn set_visible(&mut self, provider: &str, model: &str, visible: bool) -> Result<()> {
        let g = self.enabled_group(provider)?;
        if !visible {
            if !g.has_live(model) {
                return if g.has_hidden(model) { Ok(()) } else { Err(anyhow!(tr!("找不到模型 {model}", "Model not found: {model}"))) };
            }
            let taken = self.take_live(&g, &|e| s(e, "id") == Some(model));
            for e in taken {
                self.push_hidden(&g.key, e);
            }
            self.diff.push(&self.file, tr!("modelProviders.{} - \"{model}\"（暂存在 AgentPlus）", "modelProviders.{} - \"{model}\" (kept in AgentPlus)", g.key), false);
        } else {
            if g.has_live(model) {
                return Ok(());
            }
            let back = self.take_hidden(&g, &|e| s(e, "id") == Some(model));
            if back.is_empty() {
                return Err(anyhow!(tr!("找不到模型 {model}", "Model not found: {model}")));
            }
            self.put_live(&g, back)?;
            self.diff.push(&self.file, format!("modelProviders.{} + \"{model}\"", g.key), true);
        }
        Ok(())
    }

    fn upsert_model(&mut self, provider: &str, m: &ModelInput) -> Result<()> {
        let g = self.enabled_group(provider)?;
        let mid = m.id.trim().to_string();
        if mid.is_empty() {
            return Err(msg::model_id_required());
        }
        let name = m.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(String::from);
        let ctx = m.context;
        for (k, v) in &m.extra {
            crate::mfields::check(crate::mfields::QWEN, k, v)?;
        }
        let edit = |e: &mut Value, changed: &mut Vec<String>| {
            if let Some(n) = &name {
                if s(e, "name") != Some(n.as_str()) {
                    e["name"] = json!(n);
                    changed.push(format!("name = \"{n}\""));
                }
            }
            if let Some(c) = ctx {
                if e.pointer("/generationConfig/contextWindowSize").and_then(|x| x.as_u64()) != Some(c) {
                    if !e.get("generationConfig").map(|x| x.is_object()).unwrap_or(false) {
                        e["generationConfig"] = json!({});
                    }
                    e["generationConfig"]["contextWindowSize"] = json!(c);
                    changed.push(format!("generationConfig.contextWindowSize = {c}"));
                }
            }
            changed.extend(crate::mfields::write(e, crate::mfields::QWEN, &m.extra).unwrap_or_default());
        };
        let mut changed = vec![];
        if g.has_live(&mid) {
            if let Some(a) = self.live_arr(&g.key) {
                for e in a.iter_mut().filter(|e| g.owns(&g.key, e) && s(e, "id") == Some(mid.as_str())) {
                    edit(e, &mut changed);
                }
            }
            for c in &changed {
                self.diff.push(&self.file, tr!("modelProviders.{}「{mid}」{c}", "modelProviders.{} \"{mid}\" {c}", g.key), true);
            }
            self.cfg_dirty |= !changed.is_empty();
        } else if g.has_hidden(&mid) {
            let mut l = store_list(&self.root, "hiddenModels");
            for h in l.iter_mut() {
                let mine = s(h, "key").map(|k| h.get("entry").map(|e| g.owns(k, e) && s(e, "id") == Some(mid.as_str())).unwrap_or(false)).unwrap_or(false);
                if mine {
                    edit(&mut h["entry"], &mut changed);
                }
            }
            if !changed.is_empty() {
                store::set_value(&mut self.root, ID, "hiddenModels", Value::Array(l));
                self.store_dirty = true;
                for c in &changed {
                    self.diff.push(store_label(), tr!("「{mid}」（已隐藏）{c}", "\"{mid}\" (hidden) {c}"), true);
                }
            }
        } else {
            let mut e = new_entry(&g.template(), &mid, name.as_deref(), ctx);
            crate::mfields::write(&mut e, crate::mfields::QWEN, &m.extra)?;
            self.put_live(&g, vec![e])?;
            self.take_skeleton(&g);
            self.diff.push(&self.file, format!("modelProviders.{} + \"{mid}\"{}", g.key, ctx.map(|c| tr!("（上下文 {}）", " (context {})", fmt_ctx(c))).unwrap_or_default()), true);
        }
        Ok(())
    }

    fn delete_model(&mut self, provider: &str, model: &str) -> Result<()> {
        let g = self.enabled_group(provider)?;
        let n = self.take_live(&g, &|e| s(e, "id") == Some(model)).len();
        let h = self.take_hidden(&g, &|e| s(e, "id") == Some(model)).len();
        if n + h == 0 {
            return Ok(());
        }
        if n > 0 {
            self.diff.push(&self.file, tr!("modelProviders.{} - \"{model}\"（删除）", "modelProviders.{} - \"{model}\" (deleted)", g.key), false);
        } else {
            self.diff.push(store_label(), tr!("- 「{model}」（已隐藏，删除）", "- \"{model}\" (hidden, deleted)"), false);
        }
        self.keep_if_gone(&g);
        Ok(())
    }

    fn set_models(&mut self, provider: &str, models: &[String]) -> Result<()> {
        let g = self.enabled_group(provider)?;
        let want = clean_ids(models);
        let have: Vec<String> = g.live.iter().chain(&g.hidden).filter_map(|e| s(e, "id").map(String::from)).collect();
        for id in have.iter().filter(|h| !want.contains(h)) {
            self.delete_model(provider, id)?;
        }
        for id in &want {
            if g.has_hidden(id) {
                self.set_visible(provider, id, true)?;
            } else if !g.has_live(id) {
                self.upsert_model(provider, &ModelInput { id: id.clone(), name: None, context: None, ..Default::default() })?;
            }
        }
        Ok(())
    }
}

fn new_entry(tmpl: &Value, id: &str, name: Option<&str>, ctx: Option<u64>) -> Value {
    let mut e = Map::new();
    e.insert("id".into(), json!(id));
    e.insert("name".into(), json!(name.unwrap_or(id)));
    for k in ["baseUrl", "envKey", "wireApi"] {
        if let Some(v) = tmpl.get(k) {
            e.insert(k.into(), v.clone());
        }
    }
    if let Some(c) = ctx {
        e.insert("generationConfig".into(), json!({ "contextWindowSize": c }));
    }
    Value::Object(e)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (cfg, meta, had_comments) = load()?;
    let mut cx = Ctx { cfg, root: store::load(), diff: Diff::default(), file: display_path(&settings_path()), cfg_dirty: false, store_dirty: false };

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                if p.name.trim().is_empty() || p.base_url.trim().is_empty() {
                    return Err(msg::name_and_url_required());
                }
                match p.id.as_deref() {
                    None => cx.create_provider(p)?,
                    Some(OAUTH) => return Err(anyhow!(l("Qwen 账号登录不能编辑", "The Qwen account sign-in can't be edited"))),
                    Some(id) => cx.edit_provider(id, p)?,
                }
            }
            Op::DeleteProvider { provider } => {
                if provider == OAUTH {
                    return Err(anyhow!(l("Qwen 账号登录不能删除，在 Qwen Code 里用 /auth 切换", "The Qwen account sign-in can't be deleted; switch with /auth in Qwen Code")));
                }
                cx.delete_provider(provider)?
            }
            Op::SetProviderEnabled { provider, enabled } => cx.set_enabled(provider, *enabled)?,
            Op::SetModelVisible { provider, model, visible } => cx.set_visible(provider, model, *visible)?,
            Op::UpsertModel { provider, model } => cx.upsert_model(provider, model)?,
            Op::DeleteModel { provider, model } => cx.delete_model(provider, model)?,
            Op::SetProviderModels { provider, models } => cx.set_models(provider, models)?,
            Op::SetSetting { key, value } => match key.as_str() {
                "usage_stats" => {
                    let on = value.as_bool().unwrap_or(false);
                    if cx.cfg.pointer("/privacy/usageStatisticsEnabled").and_then(|x| x.as_bool()).unwrap_or(true) != on {
                        if !cx.cfg.get("privacy").map(|x| x.is_object()).unwrap_or(false) {
                            cx.cfg["privacy"] = json!({});
                        }
                        cx.cfg["privacy"]["usageStatisticsEnabled"] = json!(on);
                        cx.diff.push(&cx.file, format!("privacy.usageStatisticsEnabled = {on}"), on);
                        cx.cfg_dirty = true;
                    }
                }
                other => return Err(msg::unknown_setting(other)),
            },
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("Qwen Code 可以同时配置多个供应商，在 Qwen Code 里用 /model 选择模型", "Qwen Code can have several providers configured at once; pick models with /model in Qwen Code."))),
            Op::SetModelRoles { .. } => return Err(msg::roles_claude_only()),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    if cx.cfg_dirty && had_comments {
        return Err(msg::comments_not_written("settings.json"));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        if cx.cfg_dirty {
            let p = settings_path();
            backup_dir = Some(backup(ID, std::slice::from_ref(&p))?);
            std::fs::create_dir_all(dir())?;
            write_json(&p, &cx.cfg, meta)?;
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

    const SECRET: &str = "sk-qwen-secret-7788";

    const SAMPLE: &str = r#"{
  "$version": 4,
  "ui": {
    "theme": "GitHub"
  },
  "modelProviders": {
    "openai": [
      {
        "id": "qwen3-coder-plus",
        "name": "Qwen3 Coder Plus",
        "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "envKey": "DASHSCOPE_API_KEY",
        "generationConfig": {
          "contextWindowSize": 1000000
        }
      },
      {
        "id": "qwen3-max",
        "name": "Qwen3 Max",
        "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "envKey": "DASHSCOPE_API_KEY"
      },
      {
        "id": "gpt-5",
        "name": "GPT-5",
        "baseUrl": "https://relay.example.com/v1",
        "envKey": "RELAY_KEY",
        "wireApi": "responses",
        "generationConfig": {
          "contextWindowSize": 400000
        }
      }
    ],
    "anthropic": [
      {
        "id": "claude-sonnet-4-5",
        "name": "Claude Sonnet 4.5",
        "baseUrl": "https://relay.example.com",
        "envKey": "RELAY_KEY"
      }
    ]
  },
  "env": {
    "DASHSCOPE_API_KEY": "sk-dash-1111"
  },
  "security": {
    "auth": {
      "selectedType": "openai"
    }
  },
  "model": {
    "name": "qwen3-coder-plus"
  },
  "mcpServers": {
    "fs": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem"
      ]
    }
  }
}
"#;

    fn setup(tag: &str, sample: Option<&str>) -> TestHome {
        let home = TestHome::new(&format!("qwen-{tag}"));
        let t = home.0.clone();
        fs::create_dir_all(t.join(".qwen")).unwrap();
        if let Some(s) = sample {
            fs::write(t.join(".qwen").join("settings.json"), s).unwrap();
        }
        home
    }

    fn cfg_now() -> Value {
        load().unwrap().0
    }

    fn st() -> AgentState {
        state(&Install::default())
    }

    fn prov(st: &AgentState, id: &str) -> Provider {
        st.providers.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| panic!("no provider {id}: {:?}", st.providers.iter().map(|p| &p.id).collect::<Vec<_>>()))
    }

    fn apply(ops: Vec<Op>) -> (Diff, Vec<PathBuf>) {
        let (d, w, _) = plan(&ops, false).unwrap();
        (d, w)
    }

    fn lines(d: &Diff) -> Vec<String> {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect()
    }

    fn input(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> ProviderInput {
        ProviderInput {
            id: id.map(String::from),
            name: name.into(),
            base_url: base.into(),
            api: api.into(),
            api_key: key.map(String::from),
            models: models.iter().map(|m| m.to_string()).collect(),
            key_from_library: None,
            official_auth: None,
        }
    }

    #[test]
    fn reads_groups_and_selection() {
        let _home = setup("read", Some(SAMPLE));
        let s = st();
        assert_eq!(s.mode, "multi");
        assert!(!s.readonly);
        let ids: Vec<&str> = s.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["dashscope", "relay", "relay-anthropic"]);
        let d = prov(&s, "dashscope");
        assert_eq!(d.api, "chat");
        assert!(d.has_key, "key in env block");
        assert_eq!(d.models.len(), 2);
        assert_eq!(d.models[0].ctx.as_deref(), Some("1M"));
        assert!(d.models[0].tags.contains(&Tag::current()));
        assert_eq!(d.name, "dashscope.aliyuncs.com");
        let r = prov(&s, "relay");
        assert_eq!(r.api, "responses");
        assert!(!r.has_key);
        assert_eq!(prov(&s, "relay-anthropic").api, "anthropic");
        assert!(s.current.iter().any(|kv| kv.v == "qwen3-coder-plus"));
        assert!(s.current.iter().any(|kv| kv.v == "dashscope.aliyuncs.com"));
        let (base, key, api) = provider_endpoint("dashscope").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://dashscope.aliyuncs.com/compatible-mode/v1", Some("sk-dash-1111"), "chat"));
    }

    #[test]
    fn missing_file_and_create_provider() {
        let _home = setup("create", None);
        let s = st();
        assert!(s.providers.is_empty() && !s.readonly);
        let (d, w) = apply(vec![Op::UpsertProvider { provider: input(None, "My Relay", "https://my.relay/v1", "chat", Some(SECRET), &["m1", "m2"]) }]);
        assert_eq!(w.len(), 1);
        let all = lines(&d).join("\n");
        assert!(!all.contains(SECRET), "secret leaked: {all}");
        assert!(all.contains(&mask_key(SECRET)));
        let cfg = cfg_now();
        assert_eq!(cfg["$version"], 4);
        let arr = cfg["modelProviders"]["openai"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["envKey"], "AGENTPLUS_MY_RELAY_API_KEY");
        assert_eq!(arr[0]["wireApi"], "chat-completions");
        assert!(arr[0].get("apiKey").is_none());
        assert_eq!(cfg["env"]["AGENTPLUS_MY_RELAY_API_KEY"], SECRET);
        let p = prov(&st(), "my-relay");
        assert_eq!(p.name, "My Relay");
        assert!(p.has_key);
        // A provider without models survives in the stash.
        apply(vec![Op::UpsertProvider { provider: input(None, "Empty", "https://e.x/v1", "anthropic", None, &[]) }]);
        let p = prov(&st(), "empty");
        assert!(p.models.is_empty() && p.api == "anthropic");
        let (_, _, backup) = plan(&[Op::UpsertModel { provider: "empty".into(), model: ModelInput { id: "claude-x".into(), name: None, context: Some(200000), ..Default::default() } }], false).unwrap();
        let cfg = cfg_now();
        assert_eq!(cfg["modelProviders"]["anthropic"][0]["envKey"], "AGENTPLUS_EMPTY_API_KEY");
        assert_eq!(cfg["modelProviders"]["anthropic"][0]["generationConfig"]["contextWindowSize"], 200000);
        assert!(store_list(&store::load(), "emptyProviders").is_empty());
        assert!(backup.unwrap().join("settings.json").exists());
    }

    #[test]
    fn edit_provider_keeps_id_and_moves_protocol() {
        let _home = setup("edit", Some(SAMPLE));
        let (d, _) = apply(vec![Op::UpsertProvider { provider: input(Some("relay"), "Relay", "https://relay2.example.com/v1", "chat", Some(SECRET), &[]) }]);
        assert!(!lines(&d).join("\n").contains(SECRET));
        let cfg = cfg_now();
        let e = &cfg["modelProviders"]["openai"][2];
        assert_eq!(e["baseUrl"], "https://relay2.example.com/v1");
        assert_eq!(e["wireApi"], "chat-completions");
        assert_eq!(cfg["env"]["RELAY_KEY"], SECRET);
        // The anthropic group with the same key var is untouched.
        assert_eq!(cfg["modelProviders"]["anthropic"][0]["baseUrl"], "https://relay.example.com");
        let s = st();
        let p = prov(&s, "relay");
        assert_eq!((p.name.as_str(), p.api.as_str()), ("Relay", "chat"));
        assert!(prov(&s, "relay-anthropic").has_key, "shares RELAY_KEY");
        // Switch the dashscope group to Anthropic: entries move to modelProviders.anthropic.
        apply(vec![Op::UpsertProvider { provider: input(Some("dashscope"), "DashScope", "https://dashscope.aliyuncs.com/apps/anthropic", "anthropic", None, &[]) }]);
        let cfg = cfg_now();
        assert_eq!(cfg["modelProviders"]["openai"].as_array().unwrap().len(), 1);
        let an = cfg["modelProviders"]["anthropic"].as_array().unwrap();
        assert_eq!(an.len(), 3);
        assert!(an.iter().filter(|e| e["envKey"] == "DASHSCOPE_API_KEY").all(|e| e.get("wireApi").is_none()));
        let p = prov(&st(), "dashscope");
        assert_eq!((p.api.as_str(), p.name.as_str(), p.models.len()), ("anthropic", "DashScope", 2));
    }

    #[test]
    fn delete_provider_removes_entries_and_key() {
        let _home = setup("delete", Some(SAMPLE));
        apply(vec![Op::DeleteProvider { provider: "dashscope".into() }]);
        let cfg = cfg_now();
        assert_eq!(cfg["modelProviders"]["openai"].as_array().unwrap().len(), 1);
        assert!(cfg["env"].get("DASHSCOPE_API_KEY").is_none());
        assert!(st().providers.iter().all(|p| p.id != "dashscope"));
        // Shared key var survives while another group still uses it.
        apply(vec![Op::DeleteProvider { provider: "relay".into() }]);
        assert!(cfg_now()["modelProviders"].get("openai").is_none());
        assert!(prov(&st(), "relay").models.len() == 1);
    }

    #[test]
    fn hide_show_add_delete_models() {
        let _home = setup("models", Some(SAMPLE));
        apply(vec![Op::SetModelVisible { provider: "dashscope".into(), model: "qwen3-max".into(), visible: false }]);
        let cfg = cfg_now();
        assert_eq!(cfg["modelProviders"]["openai"].as_array().unwrap().len(), 2);
        let p = prov(&st(), "dashscope");
        assert_eq!(p.models.iter().map(|m| (m.id.as_str(), m.visible)).collect::<Vec<_>>(), [("qwen3-coder-plus", true), ("qwen3-max", false)]);
        // Edit while hidden, then show: comes back after its siblings with the edit.
        apply(vec![Op::UpsertModel { provider: "dashscope".into(), model: ModelInput { id: "qwen3-max".into(), name: Some("Max".into()), context: Some(262144), ..Default::default() } }]);
        apply(vec![Op::SetModelVisible { provider: "dashscope".into(), model: "qwen3-max".into(), visible: true }]);
        let cfg = cfg_now();
        let e = &cfg["modelProviders"]["openai"][1];
        assert_eq!((e["id"].as_str(), e["name"].as_str()), (Some("qwen3-max"), Some("Max")));
        assert_eq!(e["generationConfig"]["contextWindowSize"], 262144);
        assert!(store_list(&store::load(), "hiddenModels").is_empty());
        // Add copies the group's baseUrl / envKey / wireApi.
        apply(vec![Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: "gpt-5-mini".into(), name: None, context: None, ..Default::default() } }]);
        let cfg = cfg_now();
        let e = &cfg["modelProviders"]["openai"][3];
        assert_eq!((e["id"].as_str(), e["envKey"].as_str(), e["wireApi"].as_str()), (Some("gpt-5-mini"), Some("RELAY_KEY"), Some("responses")));
        // Deleting every model keeps the provider (as a stashed skeleton).
        apply(vec![Op::DeleteModel { provider: "relay".into(), model: "gpt-5".into() }, Op::DeleteModel { provider: "relay".into(), model: "gpt-5-mini".into() }]);
        let p = prov(&st(), "relay");
        assert!(p.models.is_empty());
        assert_eq!(p.api, "responses");
        // Replace the list.
        apply(vec![Op::SetProviderModels { provider: "dashscope".into(), models: vec!["qwen3-max".into(), "qwen-flash".into()] }]);
        let ids: Vec<String> = prov(&st(), "dashscope").models.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, ["qwen3-max", "qwen-flash"]);
    }

    #[test]
    fn disable_enable_provider() {
        let _home = setup("enable", Some(SAMPLE));
        apply(vec![Op::SetModelVisible { provider: "dashscope".into(), model: "qwen3-max".into(), visible: false }]);
        let (d, _) = apply(vec![Op::SetProviderEnabled { provider: "dashscope".into(), enabled: false }]);
        assert!(!lines(&d).is_empty());
        let cfg = cfg_now();
        assert!(cfg["modelProviders"]["openai"].as_array().unwrap().iter().all(|e| e["envKey"] != "DASHSCOPE_API_KEY"));
        let p = prov(&st(), "dashscope");
        assert!(!p.enabled);
        assert_eq!(p.models.len(), 2);
        assert!(plan(&[Op::SetModelVisible { provider: "dashscope".into(), model: "qwen3-max".into(), visible: true }], true).is_err());
        // Editing while disabled edits the stash.
        apply(vec![Op::UpsertProvider { provider: input(Some("dashscope"), "DS", "https://ds.example/v1", "chat", None, &[]) }]);
        apply(vec![Op::SetProviderEnabled { provider: "dashscope".into(), enabled: true }]);
        let cfg = cfg_now();
        let back: Vec<&Value> = cfg["modelProviders"]["openai"].as_array().unwrap().iter().filter(|e| e["envKey"] == "DASHSCOPE_API_KEY").collect();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0]["baseUrl"], "https://ds.example/v1");
        let p = prov(&st(), "dashscope");
        assert!(p.enabled);
        assert_eq!(p.name, "DS");
        assert_eq!(p.models.iter().filter(|m| !m.visible).count(), 1);
        assert!(store_obj(&store::load(), "disabledProviders").is_empty());
    }

    #[test]
    fn dry_run_writes_nothing_and_roundtrip_keeps_rest() {
        let t = setup("dry", Some(SAMPLE));
        let path = t.0.join(".qwen").join("settings.json");
        let (d, w, b) = plan(&[Op::UpsertProvider { provider: input(None, "X", "https://x/v1", "chat", Some(SECRET), &["a"]) }, Op::SetSetting { key: "usage_stats".into(), value: json!(false) }], true).unwrap();
        assert!(w.is_empty() && b.is_none() && !lines(&d).is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), SAMPLE);
        assert!(!agentplus_dir().join("store.json").exists());
        // A real no-op-ish write keeps everything else byte-identical.
        apply(vec![Op::SetSetting { key: "usage_stats".into(), value: json!(false) }]);
        let out = fs::read_to_string(&path).unwrap();
        let expect = SAMPLE.replacen("\n  }\n}\n", "\n  },\n  \"privacy\": {\n    \"usageStatisticsEnabled\": false\n  }\n}\n", 1);
        assert_eq!(out, expect);
        let v = cfg_now();
        assert_eq!(v["ui"]["theme"], "GitHub");
        assert_eq!(v["mcpServers"]["fs"]["command"], "npx");
    }

    #[test]
    fn comments_make_it_readonly() {
        let _home = setup("jsonc", Some("{\n  // my settings\n  \"modelProviders\": {}\n}\n"));
        let s = st();
        assert!(s.readonly);
        assert!(plan(&[Op::UpsertProvider { provider: input(None, "X", "https://x/v1", "chat", None, &["a"]) }], false).is_err());
        assert!(plan(&[Op::SetCurrentProvider { provider: "x".into() }], true).is_err());
        assert!(plan(&[Op::SetModelRoles { provider: "x".into(), roles: Default::default() }], true).is_err());
    }

    #[test]
    fn custom_key_needs_protocol() {
        let _home = setup("custom", Some(r#"{"modelProviders":{"mine":[{"id":"a","baseUrl":"https://a/v1","envKey":"A_KEY"}],"ok":[{"id":"b","baseUrl":"https://b/v1","envKey":"B_KEY"}]},"providerProtocol":{"ok":"openai"}}"#));
        let s = st();
        let a = prov(&s, "a");
        assert!(!a.compatible && !a.editable);
        let b = prov(&s, "b");
        assert!(b.compatible && b.api == "chat");
        apply(vec![Op::UpsertModel { provider: "b".into(), model: ModelInput { id: "b2".into(), name: None, context: None, ..Default::default() } }]);
        assert_eq!(cfg_now()["modelProviders"]["ok"].as_array().unwrap().len(), 2);
    }

    /// Read-only: `cargo test --lib dump_qwen -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_qwen() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={} dir={:?}", inst.installed, inst.version, inst.running, inst.dir);
        let s = state(&inst);
        println!("dir={} files={:?} readonly={} notes={:?}", s.config_dir, s.files, s.readonly, s.notes);
        for p in &s.providers {
            println!("  {} [{}] {:?} api={} enabled={} has_key={} models={:?}", p.id, p.name, p.base_url, p.api, p.enabled, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        println!("current={:?}", s.current.iter().map(|k| format!("{}={}", k.k, k.v)).collect::<Vec<_>>());
        let op = match s.providers.iter().find(|p| p.editable) {
            Some(p) => Op::SetProviderEnabled { provider: p.id.clone(), enabled: !p.enabled },
            None => Op::SetSetting { key: "usage_stats".into(), value: json!(false) },
        };
        if s.readonly {
            println!("readonly: skip dry-run plan");
            return;
        }
        let (d, written, backup) = plan(&[op], true).unwrap();
        assert!(written.is_empty() && backup.is_none());
        println!("dry-run diff: {:?}", lines(&d));
    }
}
