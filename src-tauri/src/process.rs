//! Install detection, running state and restart for each agent (Windows).

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Clone, Debug, Default)]
pub struct Install {
    pub installed: bool,
    pub version: Option<String>,
    /// Folder whose processes belong to the agent.
    pub dir: Option<PathBuf>,
    /// The app's main executable: started directly for non-packaged apps, and what tells the
    /// app's own processes apart from others in `dir` (see [`app_members`]).
    pub exe: Option<PathBuf>,
    /// AppUserModelID (packaged apps).
    pub aumid: Option<String>,
    pub running: bool,
    /// Every desktop copy found when there can be several; `exe` is the one in use.
    pub copies: Vec<DesktopCopy>,
}

/// One installed copy of a desktop app.
#[derive(Clone, Debug)]
pub struct DesktopCopy {
    pub exe: PathBuf,
    pub version: Option<String>,
    pub running: bool,
}

#[cfg(windows)]
pub(crate) fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
pub(crate) fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Runs `cmd` without a window and captures stdout; None if it can't start or is still
/// running after `limit` (it is killed then). Detection runs these on every refresh, so a
/// hung CLI or a WSL distro that won't start must not hang the caller.
pub(crate) fn output_within(cmd: &mut Command, limit: Duration) -> Option<std::process::Output> {
    use std::process::Stdio;
    let mut child = no_window(cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Read on a thread so a chatty child can't fill the pipe and stall.
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        let _ = tx.send(buf);
    });
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            // A grandchild that inherited stdout can keep the pipe open after the child exits:
            // wait for the output only as long as the limit allows.
            Ok(Some(status)) => {
                let stdout = rx.recv_timeout(limit.saturating_sub(t0.elapsed()).max(Duration::from_millis(200))).ok()?;
                return Some(std::process::Output { status, stdout, stderr: vec![] });
            }
            Ok(None) if t0.elapsed() < limit => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// First of `names` found in a PATH folder.
pub(crate) fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// (install dir, version, package family name, main executable) of the Codex MSIX package.
/// Cached once PowerShell has answered; a probe that failed or timed out is tried again
/// on the next refresh instead of hiding Codex for the rest of the run.
fn codex_package() -> Option<(PathBuf, String, String, Option<PathBuf>)> {
    type Pkg = Option<(PathBuf, String, String, Option<PathBuf>)>;
    static CACHE: std::sync::Mutex<Option<Pkg>> = std::sync::Mutex::new(None);
    if let Some(p) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return p;
    }
    let out = output_within(
        Command::new("powershell.exe").args([
            "-NoProfile",
            "-Command",
            "$p = Get-AppxPackage OpenAI.Codex | Select-Object -First 1; if ($p) { $p.InstallLocation; $p.Version; $p.PackageFamilyName; (($p | Get-AppxPackageManifest).Package.Applications.Application | Where-Object Id -eq 'App' | Select-Object -First 1).Executable }",
        ]),
        Duration::from_secs(20),
    )
    .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let pkg = (|| {
        let dir = PathBuf::from(lines.next()?);
        let (ver, pfn) = (lines.next()?.to_string(), lines.next()?.to_string());
        // The executable of the "App" entry, relative to the package (app/ChatGPT.exe in 26.9).
        let main = lines.next().map(|e| dir.join(e.replace('/', "\\")));
        Some((dir, ver, pfn, main))
    })();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(pkg.clone());
    pkg
}

/// (DisplayName, DisplayVersion, DisplayIcon, UninstallString) of an uninstall entry.
type UninstallEntry = (String, Option<String>, Option<String>, Option<String>);

/// Every uninstall entry whose name starts with `prefix`, current user first.
#[cfg(windows)]
fn uninstall_entries(prefix: &str) -> Vec<UninstallEntry> {
    use winreg::enums::*;
    use winreg::RegKey;
    let path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let mut out = vec![];
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(root) = RegKey::predef(hive).open_subkey(path) else { continue };
        for name in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&name) else { continue };
            let dn: String = k.get_value("DisplayName").unwrap_or_default();
            if dn.starts_with(prefix) {
                out.push((dn, k.get_value("DisplayVersion").ok(), k.get_value("DisplayIcon").ok(), k.get_value("UninstallString").ok()));
            }
        }
    }
    out
}
#[cfg(not(windows))]
fn uninstall_entries(_: &str) -> Vec<UninstallEntry> {
    vec![]
}

fn uninstall_entry(prefix: &str) -> Option<UninstallEntry> {
    uninstall_entries(prefix).into_iter().next()
}

/// The executable in a registry `DisplayIcon` / `UninstallString` value (`"C:\a b\x.exe",0`).
/// None unless it is an absolute path: an MSI entry (`MsiExec.exe /X{GUID}`) names no folder.
pub(crate) fn unquote_exe(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    let s = if let Some(rest) = s.strip_prefix('"') { rest.split('"').next().unwrap_or(rest) } else { s.split(',').next().unwrap_or(s) };
    Some(PathBuf::from(s.trim())).filter(|p| p.is_absolute())
}

fn norm_dir(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// `dir` as a lowercase prefix ending in `\`, or None when it is too broad to stand for one
/// app: a drive root, or a shared folder such as Program Files or the user profile. Every
/// process under it is killed on restart, so this errs on the side of None.
fn scope(dir: &Path) -> Option<String> {
    use std::path::Component;
    let names = dir.components().filter(|c| matches!(c, Component::Normal(_))).count();
    if !dir.is_absolute() || names == 0 || dir.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let d = norm_dir(&dir.to_string_lossy());
    let shared = ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "USERPROFILE", "APPDATA", "LOCALAPPDATA"];
    let mut broad: Vec<String> = shared.iter().filter_map(|v| std::env::var(v).ok()).map(|p| norm_dir(&p)).collect();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        broad.push(norm_dir(&format!("{l}\\Programs")));
    }
    if broad.contains(&d) {
        return None;
    }
    Some(format!("{d}\\"))
}

/// Whether `exe` lives under `scope` (from [`scope`]).
fn in_scope(scope: &str, exe: &Path) -> bool {
    exe.to_string_lossy().replace('/', "\\").to_lowercase().starts_with(scope)
}

/// A process as [`app_members`] sees it.
#[derive(Clone, Copy)]
struct Proc {
    pid: Pid,
    parent: Option<Pid>,
    start: u64,
    /// Its executable is under the app's folder.
    in_dir: bool,
    /// It is the app's main executable.
    main: bool,
}

/// Which of `procs` are the desktop app: its main executable and whatever that started. A
/// process in the app's folder that something outside started is not the app: a terminal
/// running the CLI the app bundles, a browser's native-messaging host, a Windows service. One
/// whose parent is gone (or whose pid now belongs to a newer process) is the app's leftover.
fn app_members(procs: &[Proc]) -> Vec<Pid> {
    let by_pid: HashMap<Pid, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let belongs = |p: &Proc| {
        let mut cur = p;
        for _ in 0..64 {
            if cur.main {
                return true;
            }
            match cur.parent.and_then(|pp| by_pid.get(&pp)) {
                None => return true,
                Some(par) if par.start > cur.start => return true,
                Some(par) if !par.in_dir => return false,
                Some(par) => cur = par,
            }
        }
        true
    };
    procs.iter().filter(|p| p.in_dir && belongs(p)).map(|p| p.pid).collect()
}

/// The desktop app's processes under `dir`, told apart by `main` (see [`app_members`]).
/// Without `main` every process under `dir` counts.
fn app_processes(sys: &System, dir: &Path, main: Option<&Path>) -> Vec<Pid> {
    let Some(scope) = scope(dir) else { return vec![] };
    let main = main.map(|m| norm_dir(&m.to_string_lossy()));
    let procs: Vec<Proc> = sys
        .processes()
        .iter()
        .map(|(pid, p)| {
            let exe = p.exe();
            let in_dir = exe.map(|e| in_scope(&scope, e)).unwrap_or(false);
            let is_main = in_dir && main.as_ref().is_none_or(|m| exe.map(|e| norm_dir(&e.to_string_lossy()) == *m).unwrap_or(false));
            Proc { pid: *pid, parent: p.parent(), start: p.start_time(), in_dir, main: is_main }
        })
        .collect();
    app_members(&procs)
}

fn processes() -> System {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys
}

/// The running desktop app of `inst` (empty when it has no folder).
fn app_of(sys: &System, inst: &Install) -> Vec<Pid> {
    inst.dir.as_deref().map(|d| app_processes(sys, d, inst.exe.as_deref())).unwrap_or_default()
}

/// Executable names of the CLI that an agent with a desktop app also has.
fn cli_names(agent: &str) -> &'static [&'static str] {
    match agent {
        "codex" => &["codex.exe"],
        "opencode" => &["opencode.exe", "opencode-cli.exe"],
        _ => &[],
    }
}

/// The agent's CLI running outside its desktop app (`app`): sessions in terminals, which a
/// restart of the app leaves alone. A CLI the app itself started (its local server) is the app's.
fn cli_sessions(sys: &System, agent: &str, app: &[Pid]) -> usize {
    let names = cli_names(agent);
    let under_app = |pid: Pid| {
        let mut cur = sys.process(pid);
        for _ in 0..64 {
            let Some(p) = cur else { return false };
            if app.contains(&p.pid()) {
                return true;
            }
            cur = p.parent().and_then(|pp| sys.process(pp)).filter(|par| par.start_time() <= p.start_time());
        }
        false
    };
    sys.processes()
        .iter()
        .filter(|(pid, p)| names.iter().any(|n| p.name().to_string_lossy().eq_ignore_ascii_case(n)) && !under_app(**pid))
        .count()
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
pub(crate) fn version_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_start_matches('v').trim_end_matches(','))
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

/// Version printed by a CLI (`<exe> --version`), cached per path, killed after 5 s. A .cmd /
/// .bat is started directly too: the standard library runs it through `cmd /d /c` with proper
/// quoting (paths with `&` or parentheses) and without AutoRun hooks.
pub(crate) fn cli_version(exe: &Path) -> Option<String> {
    static CACHE: std::sync::Mutex<Vec<(PathBuf, Option<String>)>> = std::sync::Mutex::new(Vec::new());
    let cache = || CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = cache().iter().find(|(p, _)| p == exe) {
        return v.clone();
    }
    let v = output_within(Command::new(exe).arg("--version"), Duration::from_secs(5)).and_then(|o| version_in(&String::from_utf8_lossy(&o.stdout)));
    cache().push((exe.to_path_buf(), v.clone()));
    v
}

/// The `version` field of a package.json.
pub(crate) fn package_version(package_json: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(package_json).ok()?).ok()?;
    v.get("version")?.as_str().map(String::from)
}

/// package.json of a global npm package (`%APPDATA%\npm\node_modules\<pkg>`, `pkg` may be
/// scoped: "@scope/name").
pub(crate) fn npm_global_package(pkg: &str) -> Option<PathBuf> {
    let dir = pkg.split('/').fold(dirs::data_dir()?.join("npm").join("node_modules"), |d, part| d.join(part));
    Some(dir.join("package.json"))
}

/// Version of a global npm package; None when it isn't installed there.
pub(crate) fn npm_global_version(pkg: &str) -> Option<String> {
    package_version(&npm_global_package(pkg)?)
}

/// Claude Code: npm global install or the native installer (~/.local/bin/claude.exe).
fn detect_claude(inst: &mut Install) {
    if let Some(pkg) = npm_global_package("@anthropic-ai/claude-code").filter(|p| p.is_file()) {
        inst.installed = true;
        inst.version = package_version(&pkg);
        return;
    }
    let native = dirs::home_dir().map(|h| h.join(".local").join("bin").join("claude.exe"));
    if let Some(exe) = native.filter(|p| p.exists()) {
        inst.installed = true;
        inst.version = cli_version(&exe);
    }
    inst.running = any_process(|name, _| name.eq_ignore_ascii_case("claude.exe"));
}

/// Every installed copy of a desktop app registered as `prefix…` (an old and a new build can
/// sit side by side), each with an executable that exists; `main` names it when the entry's
/// icon doesn't.
fn desktop_copies(prefix: &str, main: &str) -> Vec<DesktopCopy> {
    let mut out: Vec<DesktopCopy> = vec![];
    for (_, version, icon, uninst) in uninstall_entries(prefix) {
        let exe = icon
            .and_then(|i| unquote_exe(&i))
            .filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false))
            .filter(|p| p.is_file())
            .or_else(|| uninst.and_then(|u| unquote_exe(&u)).and_then(|p| p.parent().map(|d| d.join(main))).filter(|p| p.is_file()));
        let Some(exe) = exe else { continue };
        if !out.iter().any(|c| norm_dir(&c.exe.to_string_lossy()) == norm_dir(&exe.to_string_lossy())) {
            out.push(DesktopCopy { exe, version, running: false });
        }
    }
    out
}

/// Numeric parts of a version, for ordering ("1.18.32" > "1.14.25"; missing = oldest).
fn version_key(v: Option<&str>) -> Vec<u64> {
    v.unwrap_or("").split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().unwrap_or(0)).collect()
}

/// Which copy to use: the one picked in settings, else the one running, else the newest.
fn choose_copy(copies: &[DesktopCopy], preferred: Option<&str>) -> usize {
    if let Some(i) = preferred.and_then(|p| copies.iter().position(|c| norm_dir(&c.exe.to_string_lossy()) == norm_dir(p))) {
        return i;
    }
    if let Some(i) = copies.iter().position(|c| c.running) {
        return i;
    }
    (0..copies.len()).max_by(|&a, &b| version_key(copies[a].version.as_deref()).cmp(&version_key(copies[b].version.as_deref())).then(b.cmp(&a))).unwrap_or(0)
}

/// Store key of the desktop copy picked by hand (absent or empty = automatic).
pub const DESKTOP_EXE: &str = "desktopExe";

/// OpenCode: the desktop app (registry) or the CLI.
fn detect_opencode(inst: &mut Install) {
    let copies = desktop_copies("OpenCode", "OpenCode.exe");
    if !copies.is_empty() {
        let sys = processes();
        let mut copies = copies;
        for c in &mut copies {
            c.running = c.exe.parent().is_some_and(|d| !app_processes(&sys, d, Some(&c.exe)).is_empty());
        }
        let preferred = crate::store::get_str(&crate::store::load(), "opencode", DESKTOP_EXE).filter(|s| !s.is_empty());
        let c = &copies[choose_copy(&copies, preferred.as_deref())];
        inst.installed = true;
        inst.version = c.version.clone();
        inst.dir = c.exe.parent().map(Path::to_path_buf);
        inst.exe = Some(c.exe.clone());
        inst.copies = copies;
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
        let exe = icon.and_then(|i| unquote_exe(&i));
        inst.installed = exe.as_ref().map(|e| e.exists()).unwrap_or(false);
        inst.version = ver;
        inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
    }
    if let Some(d) = &inst.dir {
        inst.running = !app_processes(&processes(), d, None).is_empty();
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
            if let Some((dir, ver, pfn, main)) = codex_package() {
                inst.installed = true;
                inst.version = Some(ver);
                inst.aumid = Some(format!("{pfn}!App"));
                inst.exe = main;
                inst.dir = Some(dir);
            }
        }
        "zcode" => {
            if let Some((_, ver, _, uninst)) = uninstall_entry("ZCode") {
                let dir = uninst.and_then(|u| unquote_exe(&u)).and_then(|p| p.parent().map(Path::to_path_buf));
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
                let exe = icon.and_then(|i| unquote_exe(&i));
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
    inst.running = !app_of(&processes(), &inst).is_empty();
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

/// Set by the UI's "Cancel" while a restart runs; every wait of the run checks it.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Called when a restart begins, so a cancel from an earlier run doesn't carry over.
pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

/// Asks the running restart to stop at its next wait. An app already started keeps running.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Err once the restart has been cancelled.
pub fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::SeqCst) {
        return Err(anyhow!(crate::i18n::l("已取消", "Cancelled")));
    }
    Ok(())
}

/// Sleeps for `d`, returning early with Err when the restart is cancelled.
pub fn pause(d: Duration) -> Result<()> {
    let end = Instant::now() + d;
    loop {
        check_cancel()?;
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// Kills the desktop app's processes and waits for them to exit.
fn stop(inst: &Install, on: &dyn Fn(Progress)) -> Result<()> {
    let sys = processes();
    for pid in app_of(&sys, inst) {
        if let Some(p) = sys.process(pid) {
            p.kill();
        }
    }
    let t0 = Instant::now();
    let mut left_seen = usize::MAX;
    loop {
        let left = app_of(&processes(), inst).len();
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
        pause(Duration::from_millis(200))?;
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

/// What a restart found.
pub struct Restarted {
    pub was_running: bool,
    /// The agent's CLI running in terminals: not restarted, it reads the new config once reopened.
    pub cli_sessions: usize,
}

/// Restarts the agent's desktop app, or starts it when it isn't running.
/// `args` are passed to the new process (e.g. a debug port).
pub fn restart(agent: &str, args: &str, on: &dyn Fn(Progress)) -> Result<Restarted> {
    if crate::env::is_wsl() {
        return Err(anyhow!(crate::i18n::l("WSL 里的 Codex 是命令行工具，不用重启：新开的 codex 会话会读取新配置", "Codex in WSL is a command-line tool and doesn't need a restart: new codex sessions read the new config")));
    }
    let inst = detect(agent);
    if !inst.installed {
        return Err(anyhow!(crate::i18n::l("没有检测到安装", "No installation detected")));
    }
    let sys = processes();
    let app = app_of(&sys, &inst);
    let running = app.len();
    let cli = cli_sessions(&sys, agent, &app);
    drop(sys);
    let cli_note = (cli > 0).then(|| tr!("终端里的 {cli} 个 CLI 会话不会重启，重新打开后才读到新配置", "{cli} CLI session(s) in terminals aren't restarted; they read the new config once reopened"));
    if running > 0 {
        on(Progress::step("stop", "active", Some(tr!("正在结束 {running} 个进程", "Ending {running} process(es)"))));
        stop(&inst, on)?;
        on(match cli_note {
            Some(n) => Progress::step("stop", "warn", Some(n)),
            None => Progress::step("stop", "done", None),
        });
        pause(Duration::from_millis(400))?;
    } else {
        let idle = crate::i18n::l("没有在运行", "Not running");
        on(match cli_note {
            Some(n) => Progress::step("stop", "warn", Some(format!("{idle}{}{n}", crate::i18n::l("；", "; ")))),
            None => Progress::step("stop", "skip", Some(idle.into())),
        });
    }
    let done = Restarted { was_running: running > 0, cli_sessions: cli };
    check_cancel()?;
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
    if inst.dir.is_some() {
        on(Progress::step("start", "active", Some(crate::i18n::l("等待进程出现", "Waiting for the process").into())));
        let t0 = Instant::now();
        while app_of(&processes(), &inst).is_empty() {
            if t0.elapsed() > Duration::from_secs(15) {
                on(Progress::step("start", "warn", Some(crate::i18n::l("15 秒内没有看到它的进程，可能还在启动", "No process seen within 15 seconds; it may still be starting").into())));
                return Ok(done);
            }
            pause(Duration::from_millis(300))?;
        }
    }
    on(Progress::step("start", "done", None));
    Ok(done)
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn unquote_exe_needs_an_absolute_path() {
        assert_eq!(unquote_exe(r#""C:\a b\x.exe",0"#), Some(PathBuf::from(r"C:\a b\x.exe")));
        assert_eq!(unquote_exe(r"C:\Apps\ZCode\ZCode.exe,0"), Some(PathBuf::from(r"C:\Apps\ZCode\ZCode.exe")));
        assert_eq!(unquote_exe(r#"  "D:\z\u.exe" /S  "#), Some(PathBuf::from(r"D:\z\u.exe")));
        for bad in ["MsiExec.exe /X{0D5C1A2B-0000}", "", "  ", "\"\"", "x.exe", r"..\x.exe", "\"unterminated"] {
            assert_eq!(unquote_exe(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn scope_rejects_broad_folders() {
        assert_eq!(scope(Path::new(r"C:\Apps\ZCode")).as_deref(), Some(r"c:\apps\zcode\"));
        assert_eq!(scope(Path::new(r"D:\ZCode\")).as_deref(), Some(r"d:\zcode\"));
        assert_eq!(scope(Path::new("C:/Apps/ZCode")).as_deref(), Some(r"c:\apps\zcode\"));
        for bad in ["", r"C:\", "C:", r"\?\C:\", "relative", r"C:\Apps\..\Windows"] {
            assert_eq!(scope(Path::new(bad)), None, "{bad:?}");
        }
        for var in ["ProgramFiles", "USERPROFILE", "LOCALAPPDATA", "SystemRoot"] {
            if let Ok(p) = std::env::var(var) {
                assert_eq!(scope(Path::new(&p)), None, "{var}");
                assert_eq!(scope(Path::new(&format!("{p}\\"))), None, "{var} with a trailing separator");
                assert!(scope(&Path::new(&p).join("SomeApp")).is_some(), "{var} + SomeApp");
            }
        }
        if let Ok(l) = std::env::var("LOCALAPPDATA") {
            assert_eq!(scope(&Path::new(&l).join("Programs")), None);
        }
    }

    #[test]
    fn in_scope_needs_a_separator_after_the_folder() {
        let s = scope(Path::new(r"C:\Apps\ZCode")).unwrap();
        assert!(in_scope(&s, Path::new(r"C:\Apps\ZCode\ZCode.exe")));
        assert!(in_scope(&s, Path::new(r"c:\apps\zcode\resources\helper.exe")));
        assert!(!in_scope(&s, Path::new(r"C:\Apps\ZCode Beta\ZCode.exe")));
        assert!(!in_scope(&s, Path::new(r"C:\Apps\ZCode.exe")));
        assert!(!in_scope(&s, Path::new(r"C:\Windows\explorer.exe")));
    }

    #[test]
    fn output_within_times_out() {
        let out = output_within(Command::new("cmd").args(["/c", "echo hello"]), Duration::from_secs(10)).unwrap();
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("hello"));
        let t0 = Instant::now();
        assert!(output_within(Command::new("ping").args(["-n", "30", "127.0.0.1"]), Duration::from_millis(300)).is_none());
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(output_within(&mut Command::new(r"C:\no\such\program.exe"), Duration::from_secs(1)).is_none());
    }

    #[test]
    fn cli_version_runs_cmd_scripts_with_awkward_paths() {
        let d = std::env::temp_dir().join(format!("agentplus-cli a&b (x86)-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let script = d.join("tool.cmd");
        std::fs::write(&script, "@echo tool v9.8.7, build 1\r\n").unwrap();
        assert_eq!(cli_version(&script).as_deref(), Some("9.8.7"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn proc(pid: usize, parent: Option<usize>, start: u64, in_dir: bool, main: bool) -> Proc {
        Proc { pid: Pid::from(pid), parent: parent.map(Pid::from), start, in_dir, main }
    }

    fn members(procs: &[Proc]) -> Vec<usize> {
        let mut v: Vec<usize> = app_members(procs).into_iter().map(|p| p.as_u32() as usize).collect();
        v.sort();
        v
    }

    #[test]
    fn app_members_follow_the_main_executable() {
        let procs = [
            proc(1, None, 0, false, false),        // explorer / sihost
            proc(10, Some(1), 5, true, true),      // the app
            proc(11, Some(10), 6, true, false),    // its renderer
            proc(12, Some(11), 7, true, false),    // its bundled CLI as a local server
            proc(20, Some(1), 8, false, false),    // a terminal
            proc(21, Some(20), 9, true, false),    // the bundled CLI started from that terminal
            proc(22, Some(21), 9, true, false),    // and what it runs
            proc(30, Some(2), 1, true, false),     // its parent is gone: a leftover of the app
            proc(40, Some(20), 10, false, false),  // an unrelated program
        ];
        assert_eq!(members(&procs), [10, 11, 12, 30]);
    }

    #[test]
    fn app_members_keep_orphans_and_reused_pids() {
        // The main process exited and its pid went to a newer, unrelated process.
        let procs = [proc(1, None, 0, false, false), proc(10, Some(1), 50, false, false), proc(11, Some(10), 6, true, false)];
        assert_eq!(members(&procs), [11]);
        // A service started by services.exe (older, outside the folder) is not the app.
        let procs = [proc(3, None, 0, false, false), proc(31, Some(3), 4, true, false)];
        assert!(members(&procs).is_empty());
    }

    #[test]
    fn app_members_without_a_main_take_the_whole_folder() {
        // `app_processes` marks every process in the folder as main when it doesn't know one.
        let procs = [proc(1, None, 0, false, false), proc(20, Some(1), 3, false, false), proc(21, Some(20), 4, true, true)];
        assert_eq!(members(&procs), [21]);
    }

    /// Read-only: which processes count as each desktop app on this machine.
    /// `cargo test --lib process::tests::dump_app_processes -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_app_processes() {
        let sys = processes();
        for a in ["codex", "opencode", "zcode", "mimo", "codebuddy"] {
            let inst = detect(a);
            let app = app_of(&sys, &inst);
            let folder = inst.dir.as_deref().map(|d| app_processes(&sys, d, None).len()).unwrap_or(0);
            println!("{a:<10} running={} main={:?} app={} in_folder={} cli_sessions={}", inst.running, inst.exe, app.len(), folder, cli_sessions(&sys, a, &app));
            for c in &inst.copies {
                println!("           copy {:?} {:?} running={}", c.version, c.exe, c.running);
            }
        }
    }

    fn copy(exe: &str, version: Option<&str>, running: bool) -> DesktopCopy {
        DesktopCopy { exe: PathBuf::from(exe), version: version.map(String::from), running }
    }

    #[test]
    fn choose_copy_prefers_pick_then_running_then_newest() {
        let old = copy(r"D:\OpenCode\OpenCode.exe", Some("1.14.25"), false);
        let new = copy(r"C:\Apps\OpenCode\OpenCode.exe", Some("1.18.32"), false);
        let odd = copy(r"E:\oc\OpenCode.exe", None, false);
        // Newest when none runs, whatever the registry order; no version counts as oldest.
        assert_eq!(choose_copy(&[old.clone(), new.clone(), odd.clone()], None), 1);
        assert_eq!(choose_copy(&[new.clone(), old.clone()], None), 0);
        assert_eq!(choose_copy(std::slice::from_ref(&odd), None), 0);
        // Numbers compare as numbers: 1.9 < 1.10.
        let v19 = copy(r"C:\a\OpenCode.exe", Some("1.9.0"), false);
        let v110 = copy(r"C:\b\OpenCode.exe", Some("1.10.0"), false);
        assert_eq!(choose_copy(&[v110.clone(), v19.clone()], None), 0);
        // Same version: the first one listed.
        assert_eq!(choose_copy(&[v19.clone(), v19.clone()], None), 0);
        // The one running wins over a newer one.
        let old_running = DesktopCopy { running: true, ..old.clone() };
        assert_eq!(choose_copy(&[new.clone(), old_running.clone()], None), 1);
        // A pick wins over both, matched however the path is spelled; a stale pick is ignored.
        assert_eq!(choose_copy(&[new.clone(), old_running.clone()], Some("c:/apps/opencode/OPENCODE.exe")), 0);
        assert_eq!(choose_copy(&[new.clone(), old_running], Some(r"Z:\gone\OpenCode.exe")), 1);
        assert_eq!(choose_copy(&[old, new], Some("")), 1);
    }

    #[test]
    fn cancel_cuts_a_pause_short() {
        reset_cancel();
        assert!(pause(Duration::from_millis(50)).is_ok());
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            cancel();
        });
        let t0 = Instant::now();
        assert!(pause(Duration::from_secs(10)).is_err());
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        t.join().unwrap();
        assert!(check_cancel().is_err() && pause(Duration::ZERO).is_err());
        // The next run starts clean.
        reset_cancel();
        assert!(check_cancel().is_ok());
    }

    #[test]
    fn version_in_finds_the_first_version() {
        assert_eq!(version_in("2.1.226 (Claude Code)").as_deref(), Some("2.1.226"));
        assert_eq!(version_in("codex-cli v0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(version_in("no version here 42"), None);
        assert_eq!(version_in(""), None);
    }
}
