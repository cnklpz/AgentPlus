//! Which machine's configs AgentPlus edits: this Windows user, or a WSL distro.
//! The choice lives in `~/.agentplus/store.json` ("env") on the Windows side.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use crate::process::output_within;
use std::process::Command;
use std::time::Duration;
use std::sync::RwLock;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Windows,
    Wsl { distro: String, unix_home: String },
}

static CURRENT: RwLock<Option<Target>> = RwLock::new(None);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EnvInfo {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub current: bool,
}

/// A distro that is shut down takes a few seconds to boot on the first call.
const WSL_TIMEOUT: Duration = Duration::from_secs(30);

fn load() -> Target {
    let s = crate::store::load();
    let e = s.get("env");
    let id = e.and_then(|e| e.get("id")).and_then(|v| v.as_str()).unwrap_or("windows");
    let home = e.and_then(|e| e.get("home")).and_then(|v| v.as_str());
    match (id.strip_prefix("wsl:"), home) {
        (Some(d), Some(h)) if !d.is_empty() && h.starts_with('/') => Target::Wsl { distro: d.into(), unix_home: h.into() },
        _ => Target::Windows,
    }
}

pub fn current() -> Target {
    if let Some(t) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return t;
    }
    let t = load();
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(t.clone());
    t
}

pub fn is_wsl() -> bool {
    matches!(current(), Target::Wsl { .. })
}

pub fn id() -> String {
    match current() {
        Target::Windows => "windows".into(),
        Target::Wsl { distro, .. } => format!("wsl:{distro}"),
    }
}

pub fn label() -> String {
    match current() {
        Target::Windows => crate::i18n::l("本机 · Windows", "This PC · Windows").into(),
        Target::Wsl { distro, .. } => format!("WSL · {distro}"),
    }
}

/// Home folder of the target, as a path Windows can open.
pub fn home() -> PathBuf {
    match current() {
        Target::Windows => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        Target::Wsl { distro, unix_home } => {
            let mut p = PathBuf::from(format!(r"\\wsl.localhost\{distro}\"));
            for part in unix_home.split('/').filter(|s| !s.is_empty()) {
                p.push(part);
            }
            p
        }
    }
}

/// A path typed by the user: "~/x", a Linux path inside WSL ("/home/me/.codex"),
/// or a plain Windows / UNC path.
pub fn resolve_path(s: &str) -> PathBuf {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        return home().join(rest.replace('\\', "/"));
    }
    if let (Target::Wsl { distro, .. }, true) = (current(), s.starts_with('/')) {
        let mut p = PathBuf::from(format!(r"\\wsl.localhost\{distro}\"));
        for part in s.split('/').filter(|x| !x.is_empty()) {
            p.push(part);
        }
        return p;
    }
    PathBuf::from(s)
}

/// An environment variable of the agent's environment, for its config (a config folder, a
/// key variable). That is this process's environment on Windows; in WSL mode the agent runs
/// in the distro's shell, whose variables AgentPlus can't see, so it is None. Empty is None.
pub fn agent_var(name: &str) -> Option<String> {
    #[cfg(test)]
    if crate::util::test_home().is_some() {
        return test_var(name).filter(|v| !v.is_empty());
    }
    if is_wsl() {
        return None;
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Runs a command inside the current WSL distro; None on failure or outside WSL.
pub fn wsl_sh(script: &str) -> Option<String> {
    let Target::Wsl { distro, .. } = current() else { return None };
    let out = output_within(Command::new("wsl.exe").args(["-d", &distro, "-e", "sh", "-c", script]), WSL_TIMEOUT)?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Installed distros (Docker's internal ones are skipped).
fn distros() -> Vec<String> {
    let Some(out) = output_within(Command::new("wsl.exe").args(["-l", "-q"]), Duration::from_secs(10)) else { return vec![] };
    // wsl.exe prints UTF-16LE.
    let units: Vec<u16> = out.stdout.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|l| l.trim().trim_matches('\u{feff}').to_string())
        .filter(|l| !l.is_empty() && !l.starts_with("docker-desktop"))
        .collect()
}

pub fn list() -> Vec<EnvInfo> {
    let cur = id();
    let mut v = vec![EnvInfo {
        id: "windows".into(),
        label: crate::i18n::l("本机 · Windows", "This PC · Windows").into(),
        detail: dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default(),
        current: cur == "windows",
    }];
    for d in distros() {
        let id = format!("wsl:{d}");
        v.push(EnvInfo {
            current: cur == id,
            id,
            label: format!("WSL · {d}"),
            detail: crate::i18n::l("Codex CLI 的配置与会话；ZCode、MiMo Desktop 只在 Windows 上", "Codex CLI config and sessions; ZCode and MiMo Desktop are Windows-only").into(),
        });
    }
    v
}

/// Switches the target. For WSL, asks the distro for its $HOME (starts it if needed).
pub fn set(id: &str) -> Result<()> {
    let target = if id == "windows" {
        Target::Windows
    } else if let Some(d) = id.strip_prefix("wsl:") {
        if !distros().iter().any(|x| x == d) {
            return Err(anyhow!(tr!("没有找到 WSL 发行版 {d}", "WSL distro not found: {d}")));
        }
        let out = output_within(Command::new("wsl.exe").args(["-d", d, "-e", "sh", "-c", "printf %s \"$HOME\""]), WSL_TIMEOUT)
            .ok_or_else(|| anyhow!(tr!("启动 WSL 发行版 {d} 失败或超时", "WSL distro {d} failed to start or timed out")))?;
        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !h.starts_with('/') {
            return Err(anyhow!(tr!("读取 {d} 的 HOME 失败", "Failed to read HOME in {d}")));
        }
        Target::Wsl { distro: d.into(), unix_home: h }
    } else {
        return Err(anyhow!(tr!("未知环境 {id}", "Unknown environment {id}")));
    };
    // Runs off the main thread (set_env is async), so load and save under the store lock.
    crate::store::update(|s| {
        s["env"] = match &target {
            Target::Windows => json!({ "id": "windows" }),
            Target::Wsl { distro, unix_home } => json!({ "id": format!("wsl:{distro}"), "home": unix_home }),
        };
        Ok(())
    })?;
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(target);
    Ok(())
}

#[cfg(test)]
pub fn force(t: Target) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(t);
}

#[cfg(test)]
thread_local! {
    static TEST_VARS: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: the agent's environment variables on this thread while a `util::TestHome` is set
/// (the real process environment is never read then).
#[cfg(test)]
pub fn set_test_vars(vars: &[(&str, &str)]) {
    TEST_VARS.with(|v| *v.borrow_mut() = vars.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect());
}

#[cfg(test)]
pub fn test_var(name: &str) -> Option<String> {
    TEST_VARS.with(|v| v.borrow().iter().find(|(k, _)| k == name).map(|(_, x)| x.clone()))
}
