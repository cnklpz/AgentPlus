//! Install detection, running state and restart for each agent (Windows).

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use sysinfo::{ProcessesToUpdate, System};

#[derive(Clone, Debug, Default)]
pub struct Install {
    pub installed: bool,
    pub version: Option<String>,
    /// Folder whose processes belong to the agent.
    pub dir: Option<PathBuf>,
    /// Executable to start (non-packaged apps).
    pub exe: Option<PathBuf>,
    /// AppUserModelID (packaged apps).
    pub aumid: Option<String>,
    pub running: bool,
}

#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// (install dir, version, package family name) of the Codex MSIX package.
fn codex_package() -> Option<(PathBuf, String, String)> {
    static CACHE: OnceLock<Option<(PathBuf, String, String)>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = no_window(Command::new("powershell.exe").args([
                "-NoProfile",
                "-Command",
                "$p = Get-AppxPackage OpenAI.Codex | Select-Object -First 1; if ($p) { $p.InstallLocation; $p.Version; $p.PackageFamilyName }",
            ]))
            .output()
            .ok()?;
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            Some((PathBuf::from(lines.next()?), lines.next()?.to_string(), lines.next()?.to_string()))
        })
        .clone()
}

#[cfg(windows)]
fn uninstall_entry(prefix: &str) -> Option<(String, Option<String>, Option<String>, Option<String>)> {
    use winreg::enums::*;
    use winreg::RegKey;
    let path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(root) = RegKey::predef(hive).open_subkey(path) else { continue };
        for name in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&name) else { continue };
            let dn: String = k.get_value("DisplayName").unwrap_or_default();
            if dn.starts_with(prefix) {
                return Some((
                    dn,
                    k.get_value("DisplayVersion").ok(),
                    k.get_value("DisplayIcon").ok(),
                    k.get_value("UninstallString").ok(),
                ));
            }
        }
    }
    None
}
#[cfg(not(windows))]
fn uninstall_entry(_: &str) -> Option<(String, Option<String>, Option<String>, Option<String>)> {
    None
}

fn unquote_exe(s: &str) -> PathBuf {
    let s = s.trim();
    let s = if let Some(rest) = s.strip_prefix('"') { rest.split('"').next().unwrap_or(rest) } else { s.split(',').next().unwrap_or(s) };
    PathBuf::from(s)
}

fn processes_in(dir: &Path) -> Vec<sysinfo::Pid> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let d = dir.to_string_lossy().to_lowercase();
    sys.processes()
        .iter()
        .filter(|(_, p)| p.exe().map(|e| e.to_string_lossy().to_lowercase().starts_with(&d)).unwrap_or(false))
        .map(|(pid, _)| *pid)
        .collect()
}

/// True if any running process matches `pred(name, exe_path)`.
pub fn any_process(pred: impl Fn(&str, &str) -> bool) -> bool {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        let name = p.name().to_string_lossy();
        let path = p.exe().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        pred(&name, &path)
    })
}

/// Inside WSL only the Codex CLI exists; the desktop apps are Windows-only.
fn detect_wsl(agent: &str) -> Install {
    let mut inst = Install::default();
    if agent != "codex" {
        return inst;
    }
    let out = crate::env::wsl_sh("codex --version 2>/dev/null; pgrep -x codex >/dev/null && echo @running; true").unwrap_or_default();
    inst.version = out.lines().find(|l| !l.starts_with('@')).map(|l| l.trim_start_matches("codex-cli").trim().to_string()).filter(|v| !v.is_empty());
    inst.installed = inst.version.is_some() || crate::util::home().join(".codex").exists();
    inst.running = out.lines().any(|l| l == "@running");
    inst
}

pub fn detect(agent: &str) -> Install {
    if crate::env::is_wsl() {
        return detect_wsl(agent);
    }
    let mut inst = Install::default();
    match agent {
        "codex" => {
            if let Some((dir, ver, pfn)) = codex_package() {
                inst.installed = true;
                inst.version = Some(ver);
                inst.aumid = Some(format!("{pfn}!App"));
                inst.dir = Some(dir);
            }
        }
        "zcode" => {
            if let Some((_, ver, _, uninst)) = uninstall_entry("ZCode") {
                let dir = uninst.map(|u| unquote_exe(&u)).and_then(|p| p.parent().map(Path::to_path_buf));
                inst.installed = dir.is_some();
                inst.version = ver;
                inst.exe = dir.as_ref().map(|d| d.join("ZCode.exe"));
                inst.dir = dir;
            }
        }
        "mimo" => {
            if let Some((_, ver, icon, _)) = uninstall_entry("Xiaomi MiMo") {
                let exe = icon.map(|i| unquote_exe(&i));
                inst.installed = exe.is_some();
                inst.version = ver;
                inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
                inst.exe = exe;
            }
        }
        _ => {}
    }
    if let Some(d) = &inst.dir {
        inst.running = !processes_in(d).is_empty();
    }
    inst
}

/// Kills every process started from the agent's folder and waits for them to exit.
fn stop(dir: &Path) -> Result<()> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    for pid in processes_in(dir) {
        if let Some(p) = sys.process(pid) {
            p.kill();
        }
    }
    let t0 = Instant::now();
    while !processes_in(dir).is_empty() {
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(anyhow!("进程没有在 10 秒内退出"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

#[cfg(windows)]
fn activate(aumid: &str, args: &str) -> Result<u32> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NONE};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mgr: IApplicationActivationManager = CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
        Ok(mgr.ActivateApplication(&HSTRING::from(aumid), &HSTRING::from(args), AO_NONE)?)
    }
}
#[cfg(not(windows))]
fn activate(_: &str, _: &str) -> Result<u32> {
    Err(anyhow!("仅支持 Windows"))
}

/// Restarts the agent. `args` are passed to the new process (e.g. a debug port).
pub fn restart(agent: &str, args: &str) -> Result<()> {
    if crate::env::is_wsl() {
        return Err(anyhow!("WSL 里的 Codex 是命令行工具，不用重启：新开的 codex 会话会读取新配置"));
    }
    let inst = detect(agent);
    if !inst.installed {
        return Err(anyhow!("没有检测到安装"));
    }
    if let Some(d) = &inst.dir {
        stop(d)?;
    }
    std::thread::sleep(Duration::from_millis(400));
    if let Some(aumid) = &inst.aumid {
        activate(aumid, args)?;
    } else if let Some(exe) = &inst.exe {
        let mut cmd = Command::new(exe);
        if !args.is_empty() {
            cmd.args(args.split_whitespace());
        }
        cmd.spawn().map_err(|e| anyhow!("启动失败：{e}"))?;
    }
    Ok(())
}

pub fn open_dir(dir: &str) -> Result<()> {
    Command::new("explorer.exe").arg(dir).spawn()?;
    Ok(())
}

/// Opens Explorer with the file selected.
pub fn reveal(path: &str) -> Result<()> {
    Command::new("explorer.exe").arg(format!("/select,{path}")).spawn()?;
    Ok(())
}
