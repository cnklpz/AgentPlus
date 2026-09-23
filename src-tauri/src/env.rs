//! Which machine's configs AgentPlus edits: this Windows user, or a WSL distro.
//! The choice lives in `~/.agentplus/store.json` ("env") on the Windows side.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
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

#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}
#[cfg(not(windows))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

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
    if let Some(t) = CURRENT.read().unwrap().clone() {
        return t;
    }
    let t = load();
    *CURRENT.write().unwrap() = Some(t.clone());
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
        Target::Windows => "本机 · Windows".into(),
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

/// Runs a command inside the current WSL distro; None on failure or outside WSL.
pub fn wsl_sh(script: &str) -> Option<String> {
    let Target::Wsl { distro, .. } = current() else { return None };
    let out = no_window(Command::new("wsl.exe").args(["-d", &distro, "-e", "sh", "-c", script])).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Installed distros (Docker's internal ones are skipped).
fn distros() -> Vec<String> {
    let Ok(out) = no_window(Command::new("wsl.exe").args(["-l", "-q"])).output() else { return vec![] };
    // wsl.exe prints UTF-16LE.
    let units: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
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
        label: "本机 · Windows".into(),
        detail: dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default(),
        current: cur == "windows",
    }];
    for d in distros() {
        let id = format!("wsl:{d}");
        v.push(EnvInfo {
            current: cur == id,
            id,
            label: format!("WSL · {d}"),
            detail: "Codex CLI 的配置与会话；ZCode、MiMo Desktop 只在 Windows 上".into(),
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
            return Err(anyhow!("没有找到 WSL 发行版 {d}"));
        }
        let out = no_window(Command::new("wsl.exe").args(["-d", d, "-e", "sh", "-c", "printf %s \"$HOME\""]))
            .output()
            .map_err(|e| anyhow!("启动 WSL 失败：{e}"))?;
        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !h.starts_with('/') {
            return Err(anyhow!("读取 {d} 的 HOME 失败"));
        }
        Target::Wsl { distro: d.into(), unix_home: h }
    } else {
        return Err(anyhow!("未知环境 {id}"));
    };
    let mut s = crate::store::load();
    s["env"] = match &target {
        Target::Windows => json!({ "id": "windows" }),
        Target::Wsl { distro, unix_home } => json!({ "id": format!("wsl:{distro}"), "home": unix_home }),
    };
    crate::store::save(&s)?;
    *CURRENT.write().unwrap() = Some(target);
    Ok(())
}

#[cfg(test)]
pub fn force(t: Target) {
    *CURRENT.write().unwrap() = Some(t);
}
