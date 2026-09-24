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
/// First token that looks like a version ("2.1.226 (Claude Code)" → "2.1.226").
fn version_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && w.contains('.'))
        .map(String::from)
}

/// Inside WSL only CLIs exist; the desktop apps are Windows-only.
fn detect_wsl(agent: &str) -> Install {
    let mut inst = Install::default();
    let (script, marker) = match agent {
        "codex" => ("codex --version 2>/dev/null; pgrep -x codex >/dev/null && echo @running; true", ".codex"),
        "claude" => ("claude --version 2>/dev/null; pgrep -x claude >/dev/null && echo @running; true", ".claude/settings.json"),
        "opencode" => (
            "(command -v opencode >/dev/null && opencode --version || $HOME/.opencode/bin/opencode --version) 2>/dev/null; pgrep -x opencode >/dev/null && echo @running; true",
            ".config/opencode",
        ),
        _ => match crate::adapters::ext(agent) {
            Some(e) => (e.wsl_script, e.wsl_marker),
            None => return inst,
        },
    };
    let out = crate::env::wsl_sh(script).unwrap_or_default();
    inst.version = out.lines().find(|l| !l.starts_with('@')).and_then(version_in);
    inst.installed = inst.version.is_some() || crate::util::home().join(marker).exists();
    inst.running = out.lines().any(|l| l == "@running");
    inst
}

/// Version printed by a CLI (`<exe> --version`), cached per path.
fn cli_version(exe: &Path) -> Option<String> {
    static CACHE: std::sync::Mutex<Vec<(PathBuf, Option<String>)>> = std::sync::Mutex::new(Vec::new());
    if let Some((_, v)) = CACHE.lock().unwrap().iter().find(|(p, _)| p == exe) {
        return v.clone();
    }
    let v = no_window(Command::new(exe).arg("--version")).output().ok().and_then(|o| version_in(&String::from_utf8_lossy(&o.stdout)));
    CACHE.lock().unwrap().push((exe.to_path_buf(), v.clone()));
    v
}

/// Claude Code: npm global install or the native installer (~/.local/bin/claude.exe).
fn detect_claude(inst: &mut Install) {
    let npm = dirs::data_dir().map(|d| d.join("npm"));
    if let Some(pkg) = npm.as_ref().map(|d| d.join("node_modules").join("@anthropic-ai").join("claude-code").join("package.json")) {
        if let Ok(text) = std::fs::read_to_string(&pkg) {
            inst.installed = true;
            inst.version = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("version").and_then(|x| x.as_str()).map(String::from));
            return;
        }
    }
    let native = dirs::home_dir().map(|h| h.join(".local").join("bin").join("claude.exe"));
    if let Some(exe) = native.filter(|p| p.exists()) {
        inst.installed = true;
        inst.version = cli_version(&exe);
    }
    inst.running = any_process(|name, _| name.eq_ignore_ascii_case("claude.exe"));
}

/// OpenCode: the desktop app (registry) or the CLI.
fn detect_opencode(inst: &mut Install) {
    if let Some((_, ver, icon, uninst)) = uninstall_entry("OpenCode") {
        let exe = icon.map(|i| unquote_exe(&i)).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false)).or_else(|| {
            uninst.map(|u| unquote_exe(&u)).and_then(|p| p.parent().map(|d| d.join("OpenCode.exe"))).filter(|p| p.exists())
        });
        inst.installed = true;
        inst.version = ver;
        inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
        inst.exe = exe;
        return;
    }
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [home.join(".opencode").join("bin").join("opencode.exe"), dirs::data_dir().unwrap_or_default().join("npm").join("opencode.cmd")];
    if let Some(exe) = candidates.iter().find(|p| p.exists()) {
        inst.installed = true;
        if exe.extension().map(|e| e == "exe").unwrap_or(false) {
            inst.version = cli_version(exe);
        }
    }
    inst.running = any_process(|name, _| name.eq_ignore_ascii_case("opencode.exe"));
}

/** Trae (international or CN build); detection only. */
pub fn detect_trae() -> Install {
    let mut inst = Install::default();
    if let Some((_, ver, icon, _)) = uninstall_entry("Trae (User)").or_else(|| uninstall_entry("TraeCode")).or_else(|| uninstall_entry("Trae")) {
        let exe = icon.map(|i| unquote_exe(&i));
        inst.installed = exe.as_ref().map(|e| e.exists()).unwrap_or(false);
        inst.version = ver;
        inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
    }
    if let Some(d) = &inst.dir {
        inst.running = !processes_in(d).is_empty();
    }
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
        "claude" => detect_claude(&mut inst),
        "opencode" => detect_opencode(&mut inst),
        "mimo" => {
            if let Some((_, ver, icon, _)) = uninstall_entry("Xiaomi MiMo") {
                let exe = icon.map(|i| unquote_exe(&i));
                inst.installed = exe.is_some();
                inst.version = ver;
                inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
                inst.exe = exe;
            }
        }
        other => {
            if let Some(e) = crate::adapters::ext(other) {
                return (e.detect)();
            }
        }
    }
    if let Some(d) = &inst.dir {
        inst.running = !processes_in(d).is_empty();
    }
    inst
}

/// Progress of a restart, sent to the UI while it runs.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Progress {
    /// The steps this run goes through, in order: "stop", "start", then "port", "patch" for UI injection.
    Plan { steps: Vec<&'static str> },
    /// `status`: "active" | "done" | "skip" | "warn".
    Step { step: &'static str, status: &'static str, detail: Option<String> },
}

impl Progress {
    pub fn step(step: &'static str, status: &'static str, detail: Option<String>) -> Self {
        Progress::Step { step, status, detail }
    }
}

/// Kills every process started from the agent's folder and waits for them to exit.
fn stop(dir: &Path, on: &dyn Fn(Progress)) -> Result<()> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    for pid in processes_in(dir) {
        if let Some(p) = sys.process(pid) {
            p.kill();
        }
    }
    let t0 = Instant::now();
    let mut left_seen = usize::MAX;
    loop {
        let left = processes_in(dir).len();
        if left == 0 {
            return Ok(());
        }
        if left != left_seen {
            left_seen = left;
            on(Progress::step("stop", "active", Some(tr!("等待 {left} 个进程退出", "Waiting for {left} process(es) to exit"))));
        }
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(anyhow!(crate::i18n::l("进程没有在 10 秒内退出", "The process did not exit within 10 seconds")));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
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
    Err(anyhow!(crate::i18n::l("仅支持 Windows", "Windows only")))
}

/// Restarts the agent, or starts it when it isn't running; returns whether it was running.
/// `args` are passed to the new process (e.g. a debug port).
pub fn restart(agent: &str, args: &str, on: &dyn Fn(Progress)) -> Result<bool> {
    if crate::env::is_wsl() {
        return Err(anyhow!(crate::i18n::l("WSL 里的 Codex 是命令行工具，不用重启：新开的 codex 会话会读取新配置", "Codex in WSL is a command-line tool and doesn't need a restart: new codex sessions read the new config")));
    }
    let inst = detect(agent);
    if !inst.installed {
        return Err(anyhow!(crate::i18n::l("没有检测到安装", "No installation detected")));
    }
    let running = inst.dir.as_deref().map(processes_in).unwrap_or_default().len();
    match &inst.dir {
        Some(d) if running > 0 => {
            on(Progress::step("stop", "active", Some(tr!("正在结束 {running} 个进程", "Ending {running} process(es)"))));
            stop(d, on)?;
            on(Progress::step("stop", "done", None));
            std::thread::sleep(Duration::from_millis(400));
        }
        _ => on(Progress::step("stop", "skip", Some(crate::i18n::l("没有在运行", "Not running").into()))),
    }
    on(Progress::step("start", "active", None));
    if let Some(aumid) = &inst.aumid {
        activate(aumid, args)?;
    } else if let Some(exe) = &inst.exe {
        let mut cmd = Command::new(exe);
        if !args.is_empty() {
            cmd.args(args.split_whitespace());
        }
        cmd.spawn().map_err(|e| anyhow!(tr!("启动失败：{e}", "Failed to start: {e}")))?;
    }
    // Wait until its process shows up, so "done" means it is actually up.
    if let Some(d) = &inst.dir {
        on(Progress::step("start", "active", Some(crate::i18n::l("等待进程出现", "Waiting for the process").into())));
        let t0 = Instant::now();
        while processes_in(d).is_empty() {
            if t0.elapsed() > Duration::from_secs(15) {
                on(Progress::step("start", "warn", Some(crate::i18n::l("15 秒内没有看到它的进程，可能还在启动", "No process seen within 15 seconds; it may still be starting").into())));
                return Ok(running > 0);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    on(Progress::step("start", "done", None));
    Ok(running > 0)
}

/// Whether the agent's app is running now (cheap; used to keep the Start/Restart button honest).
pub fn running(agent: &str) -> bool {
    detect(agent).running
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
