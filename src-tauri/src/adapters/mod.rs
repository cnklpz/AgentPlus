pub mod claude;
pub mod codebuddy;
pub mod codex;
pub mod droid;
pub mod gemini;
pub mod hermes;
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
pub mod qwen;
pub mod zcode;

use crate::i18n::l;
use crate::model::{bool_setting, AgentState, Diff, Op, ProviderInput, Setting};
use crate::{process, store};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// What a plan produces: (diff, files written, backup folder).
pub type Plan = (Diff, Vec<PathBuf>, Option<PathBuf>);
/// (base_url, key, api) of a provider.
pub type Endpoint = (String, Option<String>, String);

/// Adapters written against the common adapter API (each module brings its own detection).
pub struct Ext {
    pub id: &'static str,
    pub name: &'static str,
    pub default_dir: fn() -> PathBuf,
    pub marker: &'static str,
    pub detect: fn() -> process::Install,
    pub wsl_script: &'static str,
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

pub const EXT: &[Ext] = &[ext!(hermes), ext!(gemini), ext!(pi), ext!(openclaw), ext!(droid), ext!(kilo), ext!(codebuddy), ext!(qwen), ext!(kimi)];

pub fn ext(agent: &str) -> Option<&'static Ext> {
    EXT.iter().find(|e| e.id == agent)
}

pub const ALL: [&str; 14] = [
    codex::ID, claude::ID, opencode::ID, zcode::ID, mimo::ID,
    hermes::ID, gemini::ID, pi::ID, openclaw::ID, droid::ID, kilo::ID, codebuddy::ID, qwen::ID, kimi::ID,
];

/// Agents that also run inside WSL (CLIs); the others are Windows desktop apps.
const IN_WSL: [&str; 12] = [codex::ID, claude::ID, opencode::ID, hermes::ID, gemini::ID, pi::ID, openclaw::ID, droid::ID, kilo::ID, codebuddy::ID, qwen::ID, kimi::ID];

pub fn display_name(agent: &str) -> &'static str {
    match agent {
        codex::ID => "Codex",
        claude::ID => "Claude Code",
        opencode::ID => "OpenCode",
        zcode::ID => "ZCode",
        mimo::ID => "MiMo Desktop",
        _ => ext(agent).map(|e| e.name).unwrap_or("?"),
    }
}

/// The one protocol an agent accepts, when it accepts only one.
pub fn only_api(agent: &str) -> Option<&'static str> {
    match agent {
        codex::ID => Some("responses"),
        claude::ID => Some("anthropic"),
        codebuddy::ID => Some("chat"),
        gemini::ID => Some("gemini"),
        _ => None,
    }
}

/// AgentPlus-owned per-agent switch, stored in ~/.agentplus/store.json.
const AUTO_RESTART: &str = "auto_restart";

pub fn auto_restart(agent: &str) -> bool {
    store::get_flag(&store::load(), agent, "autoRestart")
}

/// AgentPlus-owned: which desktop copy to start when several are installed ("" = automatic).
const DESKTOP_EXE: &str = "desktop_exe";

/// Label of a desktop copy in the picker: version, folder, and whether it runs.
fn copy_label(c: &process::DesktopCopy) -> String {
    let dir = c.exe.parent().map(crate::util::display_path).unwrap_or_default();
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
    let picked = store::get_str(&store::load(), agent, process::DESKTOP_EXE).unwrap_or_default();
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
        key: DESKTOP_EXE.into(),
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

/// Config folder picked by hand in 设置 › Agent 识别 (kept per environment).
pub fn dir_override(agent: &str) -> Option<PathBuf> {
    store::get_str(&store::load(), agent, "configDir").filter(|s| !s.trim().is_empty()).map(|s| crate::env::resolve_path(&s))
}

fn default_dir(agent: &str) -> PathBuf {
    let h = crate::util::home();
    match agent {
        codex::ID => h.join(".codex"),
        claude::ID => h.join(".claude"),
        opencode::ID => h.join(".config").join("opencode"),
        zcode::ID => h.join(".zcode").join("v2"),
        mimo::ID => h.join(".config").join("mimocode"),
        _ => ext(agent).map(|e| (e.default_dir)()).unwrap_or(h),
    }
}

/// The file whose presence means "this agent is configured here".
fn marker(agent: &str) -> &'static str {
    match agent {
        codex::ID => "config.toml",
        claude::ID => "settings.json",
        opencode::ID => "opencode.json",
        zcode::ID => "provider_config.json",
        mimo::ID => "mimocode.jsonc",
        _ => ext(agent).map(|e| e.marker).unwrap_or("?"),
    }
}

fn has_marker(dir: &std::path::Path, agent: &str) -> bool {
    dir.join(marker(agent)).exists()
        || (agent == zcode::ID && dir.join("v2").join(marker(agent)).exists())
        || (agent == opencode::ID && dir.join("opencode.jsonc").exists())
        || (agent == kilo::ID && ["kilo.jsonc", "opencode.json", "opencode.jsonc"].iter().any(|f| dir.join(f).exists()))
        || (agent == pi::ID && dir.join("models.json").exists())
        || (agent == codebuddy::ID && dir.join("models.json").exists())
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
    let dir = std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_default().join("Trae");
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
    let mut out: Vec<Detect> = ALL.iter()
        .map(|a| {
            let inst = process::detect(a);
            let custom = store::get_str(&store::load(), a, "configDir").filter(|s| !s.trim().is_empty());
            let dir = dir_override(a).unwrap_or_else(|| default_dir(a));
            let found = has_marker(&dir, a);
            let name = display_name(a);
            let wsl_desktop = crate::env::is_wsl() && !IN_WSL.contains(a);
            Detect {
                id: a.to_string(),
                name: name.into(),
                app_found: inst.installed,
                version: inst.version,
                running: inst.running,
                default_dir: default_dir(a).to_string_lossy().to_string(),
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
    if !ALL.contains(&agent) {
        return Err(anyhow!(tr!("未知 Agent {agent}", "Unknown agent: {agent}")));
    }
    let mut s = store::load();
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let dir = crate::env::resolve_path(p);
            if !dir.is_dir() {
                return Err(anyhow!(tr!("找不到目录 {}", "Folder not found: {}", dir.display())));
            }
            if !has_marker(&dir, agent) {
                return Err(anyhow!(tr!(
                    "这个目录里没有 {}，不像是 {agent} 的配置目录",
                    "No {} in this folder; it doesn't look like a {agent} config folder",
                    marker(agent)
                )));
            }
            store::set_str(&mut s, agent, "configDir", p);
        }
        None => store::set_value(&mut s, agent, "configDir", serde_json::Value::Null),
    }
    store::save(&s)
}

pub fn state(agent: &str) -> Result<AgentState> {
    if ocproject::is_project(agent) {
        return ocproject::state(agent).map(|mut st| {
            st.model_fields = crate::mfields::fields(crate::mfields::for_agent(agent));
            st
        });
    }
    let inst = process::detect(agent);
    let mut st = match agent {
        codex::ID => codex::state(&inst),
        claude::ID => claude::state(&inst),
        opencode::ID => opencode::state(&inst),
        zcode::ID => zcode::state(&inst),
        mimo::ID => mimo::state(&inst),
        _ => match ext(agent) {
            Some(e) => (e.state)(&inst),
            None => return Err(anyhow!(tr!("未知 Agent {agent}", "Unknown agent: {agent}"))),
        },
    };
    st.model_fields = crate::mfields::fields(crate::mfields::for_agent(agent));
    // Only desktop apps can be restarted; CLIs read the new config on their next run.
    st.restartable = !crate::env::is_wsl() && (inst.exe.is_some() || inst.aumid.is_some());
    // Key fingerprints let the UI group a relay's entries by key without seeing it.
    for p in st.providers.iter_mut().filter(|p| p.has_key && p.base_url.is_some()) {
        if let Ok((_, Some(k), _)) = provider_endpoint(agent, &p.id) {
            p.key_fp = Some(crate::model::key_fingerprint(&k));
            p.key_hint = Some(crate::model::mask_key(&k));
        }
    }
    let custom = dir_override(agent).filter(|d| has_marker(d, agent));
    if custom.is_some() {
        st.installed = true;
    }
    if crate::env::is_wsl() {
        if IN_WSL.contains(&agent) || custom.is_some() {
            if agent == codex::ID {
                // UI injection patches the desktop app; the CLI has nothing to patch.
                st.settings.retain(|s| !matches!(s.key.as_str(), "fast_inject" | "full_names" | "quota_unlock" | "hide_usage_banner"));
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
        return Ok(st);
    }
    st.settings.extend(desktop_setting(agent, &st.name, &inst));
    st.settings.push(bool_setting(
        AUTO_RESTART,
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

/// (base_url, key, api) of an existing provider; the key stays in the backend.
pub fn provider_endpoint(agent: &str, provider: &str) -> Result<Endpoint> {
    match agent {
        _ if ocproject::is_project(agent) => ocproject::endpoint(agent, provider),
        codex::ID => codex::provider_endpoint(provider),
        claude::ID => claude::provider_endpoint(provider),
        opencode::ID => opencode::provider_endpoint(provider),
        zcode::ID => zcode::provider_endpoint(provider),
        mimo::ID => mimo::provider_endpoint(provider),
        _ => match ext(agent) {
            Some(e) => (e.endpoint)(provider),
            None => Err(anyhow!(tr!("未知 Agent {agent}", "Unknown agent: {agent}"))),
        },
    }
}

/// Turns "copy provider X from agent A (or the library)" into a normal UpsertProvider.
fn resolve_import(agent: &str, from: &str, provider: &str, api: Option<&str>, name: Option<&str>) -> Result<Op> {
    let (src_name, base_url, key, src_api, models) = if from == crate::library::FROM {
        crate::library::endpoint(provider)?
    } else {
        let (base_url, key, api) = provider_endpoint(from, provider)?;
        let src = state(from)?;
        let p = src.providers.iter().find(|p| p.id == provider).ok_or_else(|| anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")))?;
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
    let models = if agent == codex::ID { vec![] } else { models };
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
                let (_, _, key, _, _) = crate::library::endpoint(p.key_from_library.as_deref().unwrap())?;
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
    let (own, rest): (Vec<&Op>, Vec<&Op>) = ops.iter().partition(|o| matches!(o, Op::SetSetting { key, .. } if key == AUTO_RESTART || key == DESKTOP_EXE));
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
    let (mut diff, written, backup) = match agent {
        _ if ocproject::is_project(agent) => ocproject::plan(agent, &rest, dry_run)?,
        codex::ID => codex::plan(&rest, dry_run)?,
        claude::ID => claude::plan(&rest, dry_run)?,
        opencode::ID => opencode::plan(&rest, dry_run)?,
        zcode::ID => zcode::plan(&rest, dry_run)?,
        mimo::ID => mimo::plan(&rest, dry_run)?,
        _ => match ext(agent) {
            Some(e) => (e.plan)(&rest, dry_run)?,
            None => return Err(anyhow!(tr!("未知 Agent {agent}", "Unknown agent: {agent}"))),
        },
    };
    for op in own {
        if let Op::SetSetting { key, value } = op {
            if key == DESKTOP_EXE {
                let v = value.as_str().unwrap_or("").to_string();
                if store::get_str(&store::load(), agent, process::DESKTOP_EXE).unwrap_or_default() != v {
                    let inst = process::detect(agent);
                    let label = match inst.copies.iter().find(|c| c.exe.to_string_lossy() == v) {
                        Some(c) => copy_label(c),
                        None => l("自动", "Automatic").to_string(),
                    };
                    diff.push(l("AgentPlus 设置", "AgentPlus settings"), &tr!("启动的桌面版 → {label}", "Desktop copy to start → {label}"), true);
                    if !dry_run {
                        let mut s = store::load();
                        store::set_str(&mut s, agent, process::DESKTOP_EXE, &v);
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
                    store::set_flag(&mut s, agent, "autoRestart", on);
                    store::save(&s)?;
                }
            }
        }
    }
    Ok((diff, written, backup))
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
