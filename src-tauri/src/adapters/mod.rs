pub mod codex;
pub mod mimo;
pub mod zcode;

use crate::model::{bool_setting, AgentState, Diff, Op, ProviderInput};
use crate::{process, store};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub const ALL: [&str; 3] = [codex::ID, zcode::ID, mimo::ID];

/// AgentPlus-owned per-agent switch, stored in ~/.agentplus/store.json.
const AUTO_RESTART: &str = "auto_restart";

pub fn auto_restart(agent: &str) -> bool {
    store::get_flag(&store::load(), agent, "autoRestart")
}

/// Config folder picked by hand in 设置 › Agent 识别 (kept per environment).
pub fn dir_override(agent: &str) -> Option<PathBuf> {
    store::get_str(&store::load(), agent, "configDir").filter(|s| !s.trim().is_empty()).map(|s| crate::env::resolve_path(&s))
}

fn default_dir(agent: &str) -> PathBuf {
    let h = crate::util::home();
    match agent {
        codex::ID => h.join(".codex"),
        zcode::ID => h.join(".zcode").join("v2"),
        _ => h.join(".config").join("mimocode"),
    }
}

/// The file whose presence means "this agent is configured here".
fn marker(agent: &str) -> &'static str {
    match agent {
        codex::ID => "config.toml",
        zcode::ID => "provider_config.json",
        _ => "mimocode.jsonc",
    }
}

fn has_marker(dir: &std::path::Path, agent: &str) -> bool {
    dir.join(marker(agent)).exists() || (agent == zcode::ID && dir.join("v2").join(marker(agent)).exists())
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
}

pub fn detect_all() -> Vec<Detect> {
    ALL.iter()
        .map(|a| {
            let inst = process::detect(a);
            let custom = store::get_str(&store::load(), a, "configDir").filter(|s| !s.trim().is_empty());
            let dir = dir_override(a).unwrap_or_else(|| default_dir(a));
            let found = has_marker(&dir, a);
            let name = match *a { codex::ID => "Codex", zcode::ID => "ZCode", _ => "MiMo Desktop" };
            let wsl_desktop = crate::env::is_wsl() && *a != codex::ID;
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
                note: wsl_desktop.then(|| format!("{name} 是 Windows 应用；在 WSL 里需要手动指定配置目录")),
            }
        })
        .collect()
}

pub fn set_dir(agent: &str, path: Option<&str>) -> Result<()> {
    if !ALL.contains(&agent) {
        return Err(anyhow!("未知 Agent {agent}"));
    }
    let mut s = store::load();
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let dir = crate::env::resolve_path(p);
            if !dir.is_dir() {
                return Err(anyhow!("找不到目录 {}", dir.display()));
            }
            if !has_marker(&dir, agent) {
                return Err(anyhow!("这个目录里没有 {}，不像是 {agent} 的配置目录", marker(agent)));
            }
            store::set_str(&mut s, agent, "configDir", p);
        }
        None => store::set_value(&mut s, agent, "configDir", serde_json::Value::Null),
    }
    store::save(&s)
}

pub fn state(agent: &str) -> Result<AgentState> {
    let inst = process::detect(agent);
    let mut st = match agent {
        codex::ID => codex::state(&inst),
        zcode::ID => zcode::state(&inst),
        mimo::ID => mimo::state(&inst),
        _ => return Err(anyhow!("未知 Agent {agent}")),
    };
    let custom = dir_override(agent).filter(|d| has_marker(d, agent));
    if custom.is_some() {
        st.installed = true;
    }
    if crate::env::is_wsl() {
        if agent == codex::ID || custom.is_some() {
            // Fast injection patches the desktop app; the CLI has nothing to patch.
            st.settings.retain(|s| s.key != "fast_inject");
            st.notes.insert(0, "WSL 里是 Codex CLI：改动写入后，新开的 codex 会话就会读取。".into());
        } else {
            st.installed = false;
            st.running = false;
            st.readonly = true;
            st.providers.clear();
            st.catalog = None;
            st.settings.clear();
            st.current.clear();
            st.notes = vec![format!("{} 是 Windows 应用，{} 里没有它的配置。切回「本机 · Windows」即可管理。", st.name, crate::env::label())];
            return Ok(st);
        }
        return Ok(st);
    }
    st.settings.push(bool_setting(
        AUTO_RESTART,
        "AgentPlus",
        &format!("应用后自动重启 {}", st.name),
        "写入配置后自动重启，让改动马上生效（没在运行时不会启动它）。Codex 开启了 Fast 注入时会一并注入。",
        auto_restart(agent),
    ));
    Ok(st)
}

/// (base_url, key, api) of an existing provider; the key stays in the backend.
pub fn provider_endpoint(agent: &str, provider: &str) -> Result<(String, Option<String>, String)> {
    match agent {
        codex::ID => codex::provider_endpoint(provider),
        zcode::ID => zcode::provider_endpoint(provider),
        mimo::ID => mimo::provider_endpoint(provider),
        _ => Err(anyhow!("未知 Agent {agent}")),
    }
}

/// Turns "copy provider X from agent A (or the library)" into a normal UpsertProvider.
fn resolve_import(agent: &str, from: &str, provider: &str, api: Option<&str>, name: Option<&str>) -> Result<Op> {
    let (src_name, base_url, key, src_api, models) = if from == crate::library::FROM {
        crate::library::endpoint(provider)?
    } else {
        let (base_url, key, api) = provider_endpoint(from, provider)?;
        let src = state(from)?;
        let p = src.providers.iter().find(|p| p.id == provider).ok_or_else(|| anyhow!("找不到供应商 {provider}"))?;
        (p.name.clone(), base_url, key, api, p.models.iter().filter(|m| m.visible).map(|m| m.id.clone()).collect())
    };
    let api = api.map(String::from).unwrap_or(src_api);
    if agent == codex::ID && api != "responses" {
        return Err(anyhow!("Codex 只支持 Responses 接口，这个供应商是 {api}"));
    }
    let models = if agent == codex::ID { vec![] } else { models };
    Ok(Op::UpsertProvider {
        provider: ProviderInput { id: None, name: name.map(String::from).unwrap_or(src_name), base_url, api, api_key: key, models },
    })
}

pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    // The auto-restart switch is handled here; the adapters never see it.
    let (own, rest): (Vec<&Op>, Vec<&Op>) = ops.iter().partition(|o| matches!(o, Op::SetSetting { key, .. } if key == AUTO_RESTART));
    let rest: Vec<Op> = rest
        .into_iter()
        .map(|o| match o {
            Op::ImportProvider { from_agent, provider, api, name } => resolve_import(agent, from_agent, provider, api.as_deref(), name.as_deref()),
            other => Ok(other.clone()),
        })
        .collect::<Result<_>>()?;
    let (mut diff, written, backup) = match agent {
        codex::ID => codex::plan(&rest, dry_run)?,
        zcode::ID => zcode::plan(&rest, dry_run)?,
        mimo::ID => mimo::plan(&rest, dry_run)?,
        _ => return Err(anyhow!("未知 Agent {agent}")),
    };
    for op in own {
        if let Op::SetSetting { value, .. } = op {
            let on = value.as_bool().unwrap_or(false);
            if auto_restart(agent) != on {
                diff.push("AgentPlus 设置", if on { "应用后自动重启 → 开" } else { "应用后自动重启 → 关" }, on);
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
