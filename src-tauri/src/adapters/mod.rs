pub mod claude;
pub mod codebuddy;
pub mod codex;
pub mod droid;
pub mod gemini;
pub mod hermes;
mod keyref;
pub mod kilo;
pub mod kimi;
pub mod mimo;
pub mod ocfmt;
pub mod ocproject;
pub mod ocsettings;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod pimodels;
mod profiles;
pub mod qwen;
pub mod zcode;

use crate::i18n::l;
use crate::model::{bool_setting, AgentState, Diff, Op, ProviderInput, Setting};
use crate::{process, store};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Messages the adapters share, worded once per language.
pub(crate) mod msg {
    use crate::i18n::l;
    use anyhow::{anyhow, Error};

    pub fn no_provider(id: &str) -> Error {
        anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}"))
    }

    /// Deleting the provider the agent is on.
    pub fn in_use(id: &str) -> Error {
        anyhow!(tr!("「{id}」正在使用，先切换到其他供应商", "\"{id}\" is in use; switch to another provider first"))
    }

    pub fn name_required() -> Error {
        anyhow!(l("名称不能为空", "Name is required"))
    }

    pub fn name_and_url_required() -> Error {
        anyhow!(l("名称和地址不能为空", "Name and base URL are required"))
    }

    pub fn model_id_required() -> Error {
        anyhow!(l("模型 ID 不能为空", "Model ID is required"))
    }

    pub fn unknown_setting(key: &str) -> Error {
        anyhow!(tr!("未知设置 {key}", "Unknown setting: {key}"))
    }

    /// SetModelRoles on an agent without model roles.
    pub fn no_model_roles() -> Error {
        anyhow!(l("这个 Agent 没有模型角色可分配", "This agent has no model roles to assign"))
    }

    /// SetProviderModels on an agent whose providers each keep their own models.
    pub fn models_per_provider() -> Error {
        anyhow!(l("每个供应商的模型已经各自独立，请直接编辑模型", "Each provider already has its own models; edit the models directly"))
    }

    /// Note for a config file with comments: shown, but not written back.
    pub fn comments_readonly(file: &str) -> String {
        tr!("{file} 含注释，写回会丢失注释，已切换为只读。", "{file} contains comments that would be lost on write, so it's read-only.")
    }

    /// A change to a config file with comments.
    pub fn comments_not_written(file: &str) -> Error {
        anyhow!(tr!("{file} 含注释，为避免丢失注释不写入", "{file} contains comments; not writing it to avoid losing them"))
    }

    /// ` · 密钥 ••••1234` at the end of a diff line (empty without a key).
    pub fn key_suffix(key: Option<&str>) -> String {
        key.map(|k| tr!(" · 密钥 {}", " · API key {}", crate::model::mask_key(k))).unwrap_or_default()
    }
}

/// A fresh state: what detection found about the agent and where its config lives.
pub(crate) fn new_state(id: &str, name: &str, inst: &process::Install, mode: &str, config_dir: &Path, files: Vec<String>) -> AgentState {
    AgentState {
        id: id.into(),
        name: name.into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: mode.into(),
        config_dir: config_dir.to_string_lossy().to_string(),
        files,
        ..Default::default()
    }
}

/// What a plan produces: (diff, files written, backup folder).
pub type Plan = (Diff, Vec<PathBuf>, Option<PathBuf>);
/// (base_url, key, api) of a provider.
pub type Endpoint = (String, Option<String>, String);

/// One agent's adapter: what AgentPlus knows about it and the functions that read and write
/// its config. Every adapter module has the same surface, listed in [`EXT`].
pub struct Ext {
    pub id: &'static str,
    /// Display name (a product name: not translated).
    pub name: &'static str,
    /// Where the agent keeps its config when no folder is picked in AgentPlus; never the
    /// picked folder (the settings page shows it as the default).
    pub default_dir: fn() -> PathBuf,
    /// The config file whose presence means "configured here" (see [`markers`] for the others).
    pub marker: &'static str,
    /// Detection on Windows; see `process::detect`.
    pub detect: fn() -> process::Install,
    /// Shell script that finds the CLI inside WSL; "" for a Windows-only desktop app.
    pub wsl_script: &'static str,
    /// Relative to the WSL home; its presence also means installed.
    pub wsl_marker: &'static str,
    pub state: fn(&process::Install) -> AgentState,
    pub endpoint: fn(&str) -> Result<Endpoint>,
    pub plan: fn(&[Op], bool) -> Result<Plan>,
}

macro_rules! ext {
    ($m:ident) => {
        Ext {
            id: $m::ID,
            name: $m::NAME,
            default_dir: $m::default_dir,
            marker: $m::MARKER,
            detect: $m::detect,
            wsl_script: $m::WSL_SCRIPT,
            wsl_marker: $m::WSL_MARKER,
            state: $m::state,
            endpoint: $m::provider_endpoint,
            plan: $m::plan,
        }
    };
}

/// Every agent, in sidebar order.
pub const EXT: &[Ext] = &[
    ext!(codex), ext!(claude), ext!(opencode), ext!(zcode), ext!(mimo),
    ext!(hermes), ext!(gemini), ext!(pi), ext!(openclaw), ext!(droid), ext!(kilo), ext!(codebuddy), ext!(qwen), ext!(kimi),
];

/// The ids of [`EXT`], in the same order.
pub const ALL: [&str; EXT.len()] = {
    let mut ids = [""; EXT.len()];
    let mut i = 0;
    while i < EXT.len() {
        ids[i] = EXT[i].id;
        i += 1;
    }
    ids
};

pub fn ext(agent: &str) -> Option<&'static Ext> {
    EXT.iter().find(|e| e.id == agent)
}

/// [`ext`], or the error for an id no adapter has.
fn adapter(agent: &str) -> Result<&'static Ext> {
    ext(agent).ok_or_else(|| anyhow!(tr!("未知 Agent {agent}", "Unknown agent: {agent}")))
}

/// The agent an id belongs to: OpenCode for its project configs (`opencode@<folder>`).
pub fn base_agent(agent: &str) -> &str {
    if ocproject::is_project(agent) {
        opencode::ID
    } else {
        agent
    }
}

/// Agents that also run inside WSL (CLIs); the others are Windows desktop apps.
fn in_wsl(e: &Ext) -> bool {
    !e.wsl_script.is_empty()
}

pub fn display_name(agent: &str) -> &'static str {
    ext(base_agent(agent)).map(|e| e.name).unwrap_or("?")
}

/// The one protocol an agent accepts, when it accepts only one.
pub fn only_api(agent: &str) -> Option<&'static str> {
    match base_agent(agent) {
        codex::ID => Some("responses"),
        claude::ID => Some("anthropic"),
        codebuddy::ID => Some("chat"),
        gemini::ID => Some("gemini"),
        _ => None,
    }
}

/// Setting key of the AgentPlus-owned per-agent switch "restart after applying".
const AUTO_RESTART_SETTING: &str = "auto_restart";
/// Where that switch is kept in ~/.agentplus/store.json.
const AUTO_RESTART_STORE_KEY: &str = "autoRestart";

pub fn auto_restart(agent: &str) -> bool {
    store::get_flag(&store::load(), agent, AUTO_RESTART_STORE_KEY)
}

/// Setting key of the AgentPlus-owned pick of which desktop copy to start when several are
/// installed ("" = automatic); stored under `process::DESKTOP_EXE_STORE_KEY`.
const DESKTOP_EXE_SETTING: &str = "desktop_exe";

/// Label of a desktop copy in the picker: version, folder, and whether it runs.
fn copy_label(c: &process::DesktopCopy) -> String {
    let dir = process::app_dir(&c.exe).map(|d| crate::util::display_path(&d)).unwrap_or_default();
    let ver = c.version.as_deref().unwrap_or("?");
    if c.running {
        tr!("{ver} · {dir}（运行中）", "{ver} · {dir} (running)")
    } else {
        format!("{ver} · {dir}")
    }
}

/// The picker for agents with more than one desktop copy installed.
fn desktop_setting(agent: &str, name: &str, inst: &process::Install) -> Option<Setting> {
    if inst.copies.len() < 2 {
        return None;
    }
    let picked = store::get_str(&store::load(), agent, process::DESKTOP_EXE_STORE_KEY).unwrap_or_default();
    let current = inst.copies.iter().find(|c| Some(&c.exe) == inst.exe.as_ref());
    let auto = match current {
        Some(c) => tr!("自动（现在是 {}）", "Automatic (now {})", c.version.as_deref().unwrap_or("?")),
        None => l("自动", "Automatic").to_string(),
    };
    let mut options = vec![String::new()];
    let mut hints = vec![auto];
    for c in &inst.copies {
        options.push(c.exe.to_string_lossy().to_string());
        hints.push(copy_label(c));
    }
    // A pick whose copy is gone shows as automatic, which is what detection does with it.
    let value = if options.contains(&picked) { picked } else { String::new() };
    Some(Setting {
        key: DESKTOP_EXE_SETTING.into(),
        group: "AgentPlus".into(),
        label: tr!("启动哪个 {}", "Which {} to start", name),
        desc: tr!(
            "检测到 {} 个桌面版。自动：优先用正在运行的那个，都没运行时用版本最新的。启动、重启和自动重启都按这里来。",
            "{} desktop copies found. Automatic uses the one that's running, or the newest when none is. Start, Restart and restart-after-applying all follow this.",
            inst.copies.len()
        ),
        kind: "select".into(),
        value: serde_json::Value::from(value),
        options,
        hints,
    })
}

/// Store key of the config folder picked by hand in 设置 › Agent 识别 (kept per environment).
const CONFIG_DIR_STORE_KEY: &str = "configDir";

/// The picked folder as typed (None when unset or blank).
fn custom_dir(root: &serde_json::Value, agent: &str) -> Option<String> {
    store::get_str(root, agent, CONFIG_DIR_STORE_KEY).filter(|s| !s.trim().is_empty())
}

/// Config folder picked by hand in 设置 › Agent 识别 (kept per environment).
pub fn dir_override(agent: &str) -> Option<PathBuf> {
    custom_dir(&store::load(), agent).map(|s| crate::env::resolve_path(&s))
}

/// Config files (relative, `/`-separated) whose presence in a folder means "this agent is
/// configured here": every file its adapter reads there, the marker among them.
fn markers(e: &'static Ext) -> &'static [&'static str] {
    match e.id {
        opencode::ID => &opencode::CONFIG_FILES,
        kilo::ID => &kilo::CONFIG_FILES,
        // ~/.zcode is accepted as well as ~/.zcode/v2 (see zcode::dir).
        zcode::ID => &[zcode::MARKER, "v2/provider_config.json"],
        pi::ID => &[pi::MARKER, "models.json"],
        codebuddy::ID => &[codebuddy::MARKER, "models.json"],
        _ => std::slice::from_ref(&e.marker),
    }
}

fn has_marker(dir: &Path, e: &'static Ext) -> bool {
    markers(e).iter().any(|m| m.split('/').fold(dir.to_path_buf(), |d, part| d.join(part)).exists())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detect {
    pub id: String,
    pub name: String,
    /// The app / CLI itself was found.
    pub app_found: bool,
    pub version: Option<String>,
    pub running: bool,
    pub default_dir: String,
    pub custom_dir: Option<String>,
    pub config_dir: String,
    pub config_found: bool,
    /// Shown in the sidebar.
    pub enabled: bool,
    pub note: Option<String>,
    /// Found but not editable by AgentPlus; the note says how to set it up by hand.
    pub manual: bool,
}

/// Agents AgentPlus can only detect: their provider settings live somewhere it cannot write.
fn detect_manual() -> Vec<Detect> {
    if crate::env::is_wsl() {
        return vec![];
    }
    let trae = process::detect_trae();
    // %APPDATA%\Trae on Windows, ~/Library/Application Support/Trae on macOS.
    let dir = dirs::config_dir().unwrap_or_default().join("Trae");
    if !trae.installed {
        return vec![];
    }
    vec![Detect {
        id: "trae".into(),
        name: "Trae".into(),
        app_found: true,
        version: trae.version,
        running: trae.running,
        default_dir: dir.to_string_lossy().to_string(),
        custom_dir: None,
        config_found: dir.is_dir(),
        config_dir: dir.to_string_lossy().to_string(),
        enabled: false,
        note: Some(
            l(
                "Trae 的自定义模型登记在账号云端，密钥加密保存在本地数据库里，AgentPlus 无法代为写入。\
                 手动添加：Trae 右上角设置 → 模型 → 添加模型，服务商选「自定义」或对应厂商，填入供应商页里复制的地址和密钥。\
                 需要协议转换时，地址可以填本地网关的统一入口。",
                "Trae keeps custom models in your cloud account and encrypts API keys in a local database, so AgentPlus can't write them. \
                 To add one by hand: in Trae, open Settings (top right) → Models → Add model, choose \"Custom\" or the matching vendor, \
                 and paste the base URL and API key copied from the provider page. \
                 If you need protocol conversion, use the local gateway's unified endpoint as the base URL.",
            )
            .into(),
        ),
        manual: true,
    }]
}

pub fn detect_all() -> Vec<Detect> {
    let root = store::load();
    let mut out: Vec<Detect> = EXT
        .iter()
        .map(|e| {
            let inst = process::detect(e.id);
            let custom = custom_dir(&root, e.id);
            let default_dir = (e.default_dir)();
            let dir = custom.as_deref().map(crate::env::resolve_path).unwrap_or_else(|| default_dir.clone());
            let found = has_marker(&dir, e);
            let name = e.name;
            let wsl_desktop = crate::env::is_wsl() && !in_wsl(e);
            Detect {
                id: e.id.to_string(),
                name: name.into(),
                app_found: inst.installed,
                version: inst.version,
                running: inst.running,
                default_dir: default_dir.to_string_lossy().to_string(),
                enabled: (inst.installed && !wsl_desktop) || (custom.is_some() && found),
                custom_dir: custom,
                config_dir: dir.to_string_lossy().to_string(),
                config_found: found,
                note: wsl_desktop.then(|| tr!("{name} 是 Windows 应用；在 WSL 里需要手动指定配置目录", "{name} is a Windows app; in WSL, set its config folder by hand")),
                manual: false,
            }
        })
        .collect();
    out.extend(detect_manual());
    out
}

pub fn set_dir(agent: &str, path: Option<&str>) -> Result<()> {
    let e = adapter(agent)?;
    let mut s = store::load();
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let dir = crate::env::resolve_path(p);
            crate::util::require_dir(&dir)?;
            if !has_marker(&dir, e) {
                return Err(anyhow!(tr!(
                    "这个目录里没有 {}，不像是 {} 的配置目录",
                    "No {} in this folder; it doesn't look like a {} config folder",
                    e.marker,
                    e.name
                )));
            }
            store::set_str(&mut s, agent, CONFIG_DIR_STORE_KEY, p);
        }
        None => store::set_value(&mut s, agent, CONFIG_DIR_STORE_KEY, serde_json::Value::Null),
    }
    store::save(&s)
}

pub fn state(agent: &str) -> Result<AgentState> {
    let (mut st, found) = if ocproject::is_project(agent) {
        (ocproject::state(agent)?, None)
    } else {
        let e = adapter(agent)?;
        let inst = process::detect(agent);
        ((e.state)(&inst), Some((e, inst)))
    };
    st.model_fields = crate::mfields::fields(crate::mfields::for_agent(agent));
    // Key fingerprints let the UI group a relay's entries by key without seeing it.
    for p in st.providers.iter_mut().filter(|p| p.has_key && p.base_url.is_some()) {
        if let Ok((_, Some(k), _)) = provider_endpoint(agent, &p.id) {
            p.key_fp = Some(crate::model::key_fingerprint(&k));
            p.key_hint = Some(crate::model::mask_key(&k));
        }
    }
    // A project folder is neither installed nor started: nothing below applies.
    let Some((e, inst)) = found else { return Ok(st) };
    // Only desktop apps can be restarted; CLIs read the new config on their next run.
    st.restartable = !crate::env::is_wsl() && (inst.exe.is_some() || inst.aumid.is_some());
    let custom = dir_override(agent).filter(|d| has_marker(d, e));
    if custom.is_some() {
        st.installed = true;
    }
    if crate::env::is_wsl() {
        if in_wsl(e) || custom.is_some() {
            if agent == codex::ID {
                // UI injection patches the desktop app; the CLI has nothing to patch.
                st.settings.retain(|s| !codex::INJECTIONS.iter().any(|i| i.key == s.key));
                st.notes.insert(0, l("WSL 里是 Codex CLI：改动写入后，新开的 codex 会话就会读取。", "In WSL this is the Codex CLI: new codex sessions pick up changes once they're written.").into());
            }
        } else {
            st.installed = false;
            st.running = false;
            st.readonly = true;
            st.providers.clear();
            st.catalog = None;
            st.settings.clear();
            st.current.clear();
            st.notes = vec![tr!(
                "{} 是 Windows 应用，{} 里没有它的配置。切回「本机 · Windows」即可管理。",
                "{} is a Windows app and has no config in {}. Switch back to \"This PC · Windows\" to manage it.",
                st.name,
                crate::env::label()
            )];
            return Ok(st);
        }
        return Ok(st);
    }
    if !st.restartable {
        // On macOS without Codex.app, detection found the Codex CLI.
        if agent == codex::ID && inst.installed {
            st.settings.retain(|s| !codex::INJECTIONS.iter().any(|i| i.key == s.key));
            st.notes.insert(0, l("没有找到 Codex 桌面版，这里管理的是 Codex CLI：改动写入后，新开的 codex 会话就会读取。", "The Codex desktop app wasn't found, so this is the Codex CLI: new codex sessions pick up changes once they're written.").into());
        }
        return Ok(st);
    }
    st.settings.extend(desktop_setting(agent, &st.name, &inst));
    st.settings.push(bool_setting(
        AUTO_RESTART_SETTING,
        "AgentPlus",
        &tr!("应用后自动重启 {}", "Restart {} after applying", st.name),
        l(
            "写入配置后自动重启，让改动马上生效（没在运行时不会启动它）。Codex 开启了 Fast 注入时会一并注入。",
            "Restart after writing the config so changes take effect right away (it won't be started if it isn't running). If Codex has Fast injection on, it's injected as well.",
        ),
        auto_restart(agent),
    ));
    Ok(st)
}

/// (base_url, key, api) of an existing provider: an agent's, or a library entry's (agent
/// `library::FROM`); the key stays in the backend.
pub fn provider_endpoint(agent: &str, provider: &str) -> Result<Endpoint> {
    if agent == crate::library::FROM {
        let e = crate::library::endpoint(provider)?;
        return Ok((e.base_url, e.key, e.api));
    }
    if ocproject::is_project(agent) {
        return ocproject::endpoint(agent, provider);
    }
    (adapter(agent)?.endpoint)(provider)
}

/// Turns "copy provider X from agent A (or the library)" into a normal UpsertProvider.
fn resolve_import(agent: &str, from: &str, provider: &str, api: Option<&str>, name: Option<&str>) -> Result<Op> {
    let (src_name, base_url, key, src_api, models) = if from == crate::library::FROM {
        let e = crate::library::endpoint(provider)?;
        (e.name, e.base_url, e.key, e.api, e.models)
    } else {
        let (base_url, key, api) = provider_endpoint(from, provider)?;
        let src = state(from)?;
        let p = src.providers.iter().find(|p| p.id == provider).ok_or_else(|| msg::no_provider(provider))?;
        (p.name.clone(), base_url, key, api, p.models.iter().filter(|m| m.visible).map(|m| m.id.clone()).collect())
    };
    let api = api.map(String::from).unwrap_or(src_api);
    match only_api(agent) {
        Some(only) if only != api && only == "gemini" => {
            return Err(anyhow!(tr!("{} 只支持 Gemini 协议，这个供应商是 {api}", "{} only supports the Gemini protocol; this provider uses {api}", display_name(agent))))
        }
        Some(only) if only != api => {
            return Err(anyhow!(tr!(
                "{} 只支持 {only} 接口，这个供应商是 {api}；可以经本地网关转换",
                "{} only supports the {only} API; this provider uses {api}. You can convert it through the local gateway",
                display_name(agent)
            )))
        }
        None if api == "gemini" => return Err(anyhow!(tr!("{} 不支持 Gemini 协议", "{} doesn't support the Gemini protocol", display_name(agent)))),
        _ => {}
    }
    Ok(Op::UpsertProvider {
        provider: ProviderInput { id: None, name: name.map(String::from).unwrap_or(src_name), base_url, api, api_key: key, models, key_from_library: None, official_auth: None },
    })
}

pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    plan_resolved(agent, &resolve(agent, ops)?, dry_run)
}

/// Turns copies (another agent's provider, a library entry's key) into plain ops. Only reads,
/// but reading another agent's state runs its detection (`--version`, PowerShell, WSL), so
/// callers do this before taking the store lock for `plan_resolved`.
pub fn resolve(agent: &str, ops: &[Op]) -> Result<Vec<Op>> {
    ops.iter()
        .map(|o| match o {
            Op::ImportProvider { from_agent, provider, api, name } => resolve_import(agent, from_agent, provider, api.as_deref(), name.as_deref()),
            Op::UpsertProvider { provider: p } if p.key_from_library.is_some() => {
                let key = crate::library::endpoint(p.key_from_library.as_deref().unwrap())?.key;
                let mut p = p.clone();
                p.api_key = key;
                p.key_from_library = None;
                Ok(Op::UpsertProvider { provider: p })
            }
            other => Ok(other.clone()),
        })
        .collect()
}

/// `plan` for ops already passed through `resolve`.
pub fn plan_resolved(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    // AgentPlus's own settings (auto-restart, desktop copy) are handled here; the adapters never see them.
    let (own, rest): (Vec<&Op>, Vec<&Op>) = ops.iter().partition(|o| matches!(o, Op::SetSetting { key, .. } if key == AUTO_RESTART_SETTING || key == DESKTOP_EXE_SETTING));
    // Entries pointing at the local gateway carry the placeholder (or, copied, another
    // agent's key): every agent gets its own, so the gateway can check and count its calls.
    let rest: Vec<Op> = rest
        .into_iter()
        .cloned()
        .map(|o| match o {
            Op::UpsertProvider { provider: mut p } if p.api_key.as_deref().is_some_and(crate::gateway::keys::is_gateway_key) => {
                p.api_key = Some(crate::gateway::keys::for_agent(agent)?);
                Ok(Op::UpsertProvider { provider: p })
            }
            other => Ok(other),
        })
        .collect::<Result<_>>()?;
    let (mut diff, written, backup) = if ocproject::is_project(agent) {
        ocproject::plan(agent, &rest, dry_run)?
    } else {
        (adapter(agent)?.plan)(&rest, dry_run)?
    };
    for op in own {
        if let Op::SetSetting { key, value } = op {
            if key == DESKTOP_EXE_SETTING {
                let v = value.as_str().unwrap_or("").to_string();
                if store::get_str(&store::load(), agent, process::DESKTOP_EXE_STORE_KEY).unwrap_or_default() != v {
                    let inst = process::detect(agent);
                    let label = match inst.copies.iter().find(|c| c.exe.to_string_lossy() == v) {
                        Some(c) => copy_label(c),
                        None => l("自动", "Automatic").to_string(),
                    };
                    diff.push(l("AgentPlus 设置", "AgentPlus settings"), &tr!("启动的桌面版 → {label}", "Desktop copy to start → {label}"), true);
                    if !dry_run {
                        let mut s = store::load();
                        store::set_str(&mut s, agent, process::DESKTOP_EXE_STORE_KEY, &v);
                        store::save(&s)?;
                    }
                }
                continue;
            }
            let on = value.as_bool().unwrap_or(false);
            if auto_restart(agent) != on {
                diff.push(
                    l("AgentPlus 设置", "AgentPlus settings"),
                    if on { l("应用后自动重启 → 开", "Restart after applying → on") } else { l("应用后自动重启 → 关", "Restart after applying → off") },
                    on,
                );
                if !dry_run {
                    let mut s = store::load();
                    store::set_flag(&mut s, agent, AUTO_RESTART_STORE_KEY, on);
                    store::save(&s)?;
                }
            }
        }
    }
    Ok((diff, written, backup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    #[test]
    fn all_lists_every_adapter_once() {
        assert_eq!(ALL.to_vec(), EXT.iter().map(|e| e.id).collect::<Vec<_>>());
        let mut ids = ALL.to_vec();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        for e in EXT {
            assert!(!e.name.is_empty() && !e.marker.is_empty(), "{}", e.id);
            assert!(markers(e).contains(&e.marker), "{}", e.id);
            // A CLI found in WSL needs a marker to look for; a desktop-only app has neither.
            assert_eq!(e.wsl_script.is_empty(), e.wsl_marker.is_empty(), "{}", e.id);
        }
        // The Windows desktop apps.
        let desktop: Vec<&str> = EXT.iter().filter(|e| !in_wsl(e)).map(|e| e.id).collect();
        assert_eq!(desktop, [zcode::ID, mimo::ID]);
    }

    #[test]
    fn unknown_agents_are_errors() {
        let _h = TestHome::new("adapters-unknown");
        for r in [state("nope").map(|_| ()), provider_endpoint("nope", "x").map(|_| ()), plan_resolved("nope", &[], true).map(|_| ()), set_dir("nope", None)] {
            assert_eq!(r.unwrap_err().to_string(), "未知 Agent nope");
        }
    }

    #[test]
    fn library_entries_have_endpoints() {
        let _h = TestHome::new("adapters-library-endpoint");
        let lib = serde_json::json!([
            { "id": "relay", "name": "Relay", "baseUrl": "https://relay.example.com/v1", "apiKey": "sk-lib-1234", "api": "chat" },
            { "id": "open", "name": "Open", "baseUrl": "https://open.example.com", "api": "responses" },
        ]);
        store::save(&serde_json::json!({ "library": lib })).unwrap();
        let from = crate::library::FROM;
        assert_eq!(provider_endpoint(from, "relay").unwrap(), ("https://relay.example.com/v1".into(), Some("sk-lib-1234".into()), "chat".into()));
        assert_eq!(provider_endpoint(from, "open").unwrap(), ("https://open.example.com".into(), None, "responses".into()));
        assert_eq!(provider_endpoint(from, "nope").unwrap_err().to_string(), "供应商库里没有 nope");
    }

    #[test]
    fn project_ids_resolve_to_opencode() {
        assert_eq!(base_agent("opencode@D:/work/x"), opencode::ID);
        assert_eq!(base_agent(kilo::ID), kilo::ID);
        assert_eq!(display_name("opencode@D:/work/x"), opencode::NAME);
        assert_eq!(display_name(codex::ID), codex::NAME);
        assert_eq!(display_name("nope"), "?");
        assert_eq!(only_api("opencode@D:/work/x"), None);
        assert_eq!(only_api(claude::ID), Some("anthropic"));
    }

    #[test]
    fn every_file_an_adapter_reads_marks_its_folder() {
        let h = TestHome::new("adapters-markers");
        let cases: Vec<(&str, &str)> = opencode::CONFIG_FILES
            .iter()
            .map(|f| (opencode::ID, *f))
            .chain(kilo::CONFIG_FILES.iter().map(|f| (kilo::ID, *f)))
            .chain([(zcode::ID, "provider_config.json"), (zcode::ID, "v2/provider_config.json"), (pi::ID, "models.json"), (codebuddy::ID, "models.json")])
            .chain(EXT.iter().map(|e| (e.id, e.marker)))
            .collect();
        for (i, (agent, file)) in cases.into_iter().enumerate() {
            let dir = h.0.join(format!("case-{i}"));
            let e = ext(agent).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            assert!(!has_marker(&dir, e), "{agent}: empty folder");
            let p = file.split('/').fold(dir.clone(), |d, part| d.join(part));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "{}").unwrap();
            assert!(has_marker(&dir, e), "{agent}: {file}");
            // set_dir takes exactly the folders detection accepts.
            set_dir(agent, Some(&dir.to_string_lossy())).unwrap();
            assert_eq!(dir_override(agent), Some(dir));
            set_dir(agent, None).unwrap();
        }
        let other = h.0.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("opencode.jsonc"), "{}").unwrap();
        let err = set_dir(codex::ID, Some(&other.to_string_lossy())).unwrap_err().to_string();
        assert_eq!(err, "这个目录里没有 config.toml，不像是 Codex 的配置目录");
        let err = set_dir(codex::ID, Some(&h.0.join("missing").to_string_lossy())).unwrap_err().to_string();
        assert!(err.starts_with("找不到文件夹：") && err.ends_with("missing"), "{err}");
    }

    #[test]
    fn the_default_folder_is_never_the_picked_one() {
        let h = TestHome::new("adapters-default-dir");
        let picked = h.0.join("picked");
        std::fs::create_dir_all(&picked).unwrap();
        let mut root = store::load();
        for a in ALL {
            store::set_str(&mut root, a, CONFIG_DIR_STORE_KEY, &picked.to_string_lossy());
        }
        store::save(&root).unwrap();
        for e in EXT {
            assert_eq!(dir_override(e.id).as_deref(), Some(picked.as_path()));
            assert_ne!((e.default_dir)(), picked, "{}", e.id);
        }
    }
}

#[cfg(test)]
mod dump {
    /// Read-only summary of every agent on this machine, with timings.
    /// `cargo test --lib adapters::dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_all_agents() {
        let t0 = std::time::Instant::now();
        for a in super::ALL {
            let t = std::time::Instant::now();
            let st = super::state(a).unwrap();
            println!(
                "{:<10} installed={} ver={:?} restartable={} readonly={} mode={} cur={:?} providers={:?} ({} ms)",
                a, st.installed, st.version, st.restartable, st.readonly, st.mode, st.current_provider,
                st.providers.iter().map(|p| format!("{}[{}·{}m·key={}]", p.id, p.api, p.models.len(), p.key_hint.as_deref().unwrap_or("-"))).collect::<Vec<_>>(),
                t.elapsed().as_millis()
            );
        }
        println!("total {} ms", t0.elapsed().as_millis());
        for d in super::detect_all() {
            println!("detect {:<10} app={} cfg={} enabled={} manual={} dir={}", d.id, d.app_found, d.config_found, d.enabled, d.manual, d.config_dir);
        }
    }
}
