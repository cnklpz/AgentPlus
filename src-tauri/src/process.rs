//! Install detection, running state and restart for each agent (Windows and macOS).

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

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
    /// A local server the CLI runs in the background (`dsh web`), started and restarted
    /// instead of a desktop app; set without `dir` / `exe`.
    pub server: Option<Server>,
}

/// A CLI that serves a web UI: it has no folder of its own (it runs under node), so its
/// processes are told apart by their command line.
#[derive(Clone, Debug)]
pub struct Server {
    /// Started as `program args…` in the home folder (the program's file name also picks
    /// which processes' command lines are read).
    pub program: PathBuf,
    pub args: Vec<String>,
    /// What a process's command line serves, if it is such a server; it is this one when
    /// that equals `name`.
    pub serves: fn(&[String]) -> Option<String>,
    pub name: String,
    /// Where its output goes. It is up once a URL shows up there.
    pub log: PathBuf,
    /// Keeps running after AgentPlus quits when AgentPlus started it (see [`set_independent`]).
    pub independent: bool,
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
    // An npm CLI is a `#!/usr/bin/env node` script: it needs the PATH that has node.
    if let Some(p) = LOGIN_PATH.get() {
        cmd.env("PATH", p);
    }
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
                // With what it started: a .cmd shim's node would keep running, and keep the pipe
                // (and the thread reading it) open.
                kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// `base` as the file name of a program on this system: `codex.exe` on Windows, `codex` elsewhere.
pub(crate) fn exe(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

/// A CLI file name written the Windows way (`x.exe`, `x.cmd`) as it is named here: the
/// same on Windows, without the extension elsewhere.
fn native_name(name: &str) -> &str {
    if cfg!(windows) {
        name
    } else {
        name.strip_suffix(".exe").or_else(|| name.strip_suffix(".cmd")).unwrap_or(name)
    }
}

/// First of `names` found in a PATH folder (see [`search_path`]). Names are written the
/// Windows way; elsewhere `x.exe` and `x.cmd` both mean `x`.
pub(crate) fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = search_path();
    std::env::split_paths(&path).find_map(|d| names.iter().map(|n| d.join(native_name(n))).find(|p| p.is_file()))
}

/// The login shell's PATH merged in front of this process's, once asked (macOS).
static LOGIN_PATH: OnceLock<OsString> = OnceLock::new();

/// The PATH to find and run CLIs with. On macOS an app started from the Dock or Finder
/// gets only the system folders, not what the login shell adds (Homebrew, npm, nvm,
/// ~/.local/bin), so the login shell is asked once; callers meanwhile wait for it.
pub(crate) fn search_path() -> OsString {
    let own = std::env::var_os("PATH").unwrap_or_default();
    if !cfg!(target_os = "macos") || cfg!(test) {
        return own;
    }
    LOGIN_PATH
        .get_or_init(|| {
            let login = login_shell_path().unwrap_or_default();
            let home = dirs::home_dir().unwrap_or_default();
            merge_paths(&[&login, &own], &common_bins(&home))
        })
        .clone()
}

/// Where installers and package managers put CLIs on macOS: kept in the search even when the
/// login shell couldn't be asked (it failed, took too long, or leaves them out).
fn common_bins(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![home.join(".local").join("bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), home.join("bin")];
    v.extend([".bun", ".volta", ".npm-global"].map(|d| home.join(d).join("bin")));
    v
}

/// `paths` in order, then `extra`, each folder once.
fn merge_paths(paths: &[&OsString], extra: &[PathBuf]) -> OsString {
    let mut dirs: Vec<PathBuf> = vec![];
    for d in paths.iter().flat_map(|p| std::env::split_paths(p)).chain(extra.iter().cloned()) {
        if !d.as_os_str().is_empty() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| paths.first().map(|p| (*p).clone()).unwrap_or_default())
}

/// PATH as an interactive login shell sets it; None when the shell fails or takes over 5 s.
fn login_shell_path() -> Option<OsString> {
    const MARK: &str = "__AGENTPLUS_PATH__";
    let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
    // printenv prints PATH colon-separated in any shell (fish keeps it as a list); the mark
    // skips whatever the shell's startup files print first.
    let out = output_within(Command::new(shell).args(["-ilc", &format!("echo {MARK}; /usr/bin/printenv PATH")]), Duration::from_secs(5))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.rsplit_once(MARK)?.1.lines().map(str::trim).find(|l| !l.is_empty()).map(OsString::from)
}

type CodexPkg = Option<(PathBuf, String, String, Option<PathBuf>)>;

/// How long a PowerShell answer about the Codex package is trusted when nothing says it changed.
const CODEX_PKG_TTL: Duration = Duration::from_secs(300);

/// The MSIX package folder (`…\WindowsApps\OpenAI.Codex_<version>_…`) an executable is in.
fn codex_package_dir(exe: &Path) -> Option<&Path> {
    exe.ancestors().find(|d| {
        d.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("openai.codex_"))
            && d.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("WindowsApps"))
    })
}

/// Whether a cached answer no longer describes the installed package: an update installs
/// Codex into a new versioned folder (and removes the old one once nothing runs from it),
/// so the cached folder is gone, or Codex is running from another package folder.
fn codex_package_stale(cached: &CodexPkg, age: Duration, sys: &System) -> bool {
    let Some((dir, ..)) = cached else { return age > Duration::from_secs(60) };
    if age > CODEX_PKG_TTL || !dir.is_dir() {
        return true;
    }
    let own = norm_dir(&dir.to_string_lossy());
    sys.processes().values().filter_map(|p| p.exe()).filter_map(codex_package_dir).any(|d| norm_dir(&d.to_string_lossy()) != own)
}

/// (install dir, version, package family name, main executable) of the Codex MSIX package.
/// Cached once PowerShell has answered, until the package changes (see
/// [`codex_package_stale`]); a probe that failed or timed out is tried again on the next
/// refresh instead of hiding Codex for the rest of the run.
fn codex_package(sys: &System) -> CodexPkg {
    static CACHE: std::sync::Mutex<Option<(Instant, CodexPkg)>> = std::sync::Mutex::new(None);
    /// One probe at a time: callers that arrive meanwhile use its answer.
    static PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let cached = || crate::util::lock(&CACHE).clone().filter(|(t, p)| !codex_package_stale(p, t.elapsed(), sys)).map(|(_, p)| p);
    if let Some(p) = cached() {
        return p;
    }
    let _probe = crate::util::lock(&PROBE);
    if let Some(p) = cached() {
        return p;
    }
    let t0 = Instant::now();
    let out = output_within(
        Command::new("powershell.exe").args([
            "-NoProfile",
            "-Command",
            "$p = Get-AppxPackage OpenAI.Codex | Select-Object -First 1; if ($p) { $p.InstallLocation; $p.Version; $p.PackageFamilyName; (($p | Get-AppxPackageManifest).Package.Applications.Application | Where-Object Id -eq 'App' | Select-Object -First 1).Executable }",
        ]),
        Duration::from_secs(20),
    )
    .filter(|o| o.status.success());
    let Some(out) = out else {
        crate::applog::warn("detect", format!("Codex package probe failed after {:.1}s", t0.elapsed().as_secs_f32()));
        return None;
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let pkg = (|| {
        let dir = PathBuf::from(lines.next()?);
        let (ver, pfn) = (lines.next()?.to_string(), lines.next()?.to_string());
        // The executable of the "App" entry, relative to the package (app/ChatGPT.exe in 26.9).
        let main = lines.next().map(|e| dir.join(e.replace('/', "\\")));
        Some((dir, ver, pfn, main))
    })();
    let found = pkg.as_ref().map(|(d, v, ..)| format!("{v} at {}", d.display())).unwrap_or_else(|| "not installed".into());
    crate::applog::info("detect", format!("Codex package: {found} ({:.1}s)", t0.elapsed().as_secs_f32()));
    *crate::util::lock(&CACHE) = Some((Instant::now(), pkg.clone()));
    pkg
}

/// What an uninstall entry in the registry says about an installed app.
// Only the Windows registry scan builds one.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct UninstallEntry {
    /// DisplayVersion
    pub version: Option<String>,
    /// DisplayIcon
    pub icon: Option<String>,
    /// UninstallString
    pub uninstall: Option<String>,
    /// InstallLocation
    pub location: Option<String>,
}

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
                out.push(UninstallEntry {
                    version: k.get_value("DisplayVersion").ok(),
                    icon: k.get_value("DisplayIcon").ok(),
                    uninstall: k.get_value("UninstallString").ok(),
                    location: k.get_value("InstallLocation").ok(),
                });
            }
        }
    }
    out
}
#[cfg(not(windows))]
fn uninstall_entries(_: &str) -> Vec<UninstallEntry> {
    vec![]
}

/// The first uninstall entry whose name starts with `prefix`, current user first.
pub(crate) fn uninstall_entry(prefix: &str) -> Option<UninstallEntry> {
    uninstall_entries(prefix).into_iter().next()
}

/// The executable in a registry `DisplayIcon` / `UninstallString` value (`"C:\a b\x.exe",0`).
/// None unless it is an absolute path: an MSI entry (`MsiExec.exe /X{GUID}`) names no folder.
pub(crate) fn unquote_exe(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    let s = if let Some(rest) = s.strip_prefix('"') { rest.split('"').next().unwrap_or(rest) } else { s.split(',').next().unwrap_or(s) };
    Some(PathBuf::from(s.trim())).filter(|p| p.is_absolute())
}

/// Whether `p` is a program to run directly: an `.exe` (any case) on Windows, not a `.cmd`
/// shim; elsewhere a file with an execute bit.
#[cfg(windows)]
pub(crate) fn is_exe(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}
#[cfg(unix)]
pub(crate) fn is_exe(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A macOS app bundle, as its Info.plist describes it.
#[derive(Clone, Debug)]
pub(crate) struct Bundle {
    /// `…/X.app`
    pub path: PathBuf,
    /// CFBundleIdentifier
    pub id: Option<String>,
    /// Contents/MacOS/<CFBundleExecutable>
    pub exe: PathBuf,
    /// CFBundleShortVersionString, else CFBundleVersion
    pub version: Option<String>,
}

pub(crate) fn bundle_info(bundle: &Path) -> Option<Bundle> {
    let contents = bundle.join("Contents");
    let v = plist::Value::from_file(contents.join("Info.plist")).ok()?;
    let d = v.as_dictionary()?;
    let s = |k: &str| d.get(k).and_then(plist::Value::as_string).map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    Some(Bundle {
        path: bundle.to_path_buf(),
        id: s("CFBundleIdentifier"),
        exe: contents.join("MacOS").join(s("CFBundleExecutable")?),
        version: s("CFBundleShortVersionString").or_else(|| s("CFBundleVersion")),
    })
}

/// Folders apps are installed in on macOS.
fn app_folders() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/Applications")];
    v.extend(dirs::home_dir().map(|h| h.join("Applications")));
    v
}

/// Every app bundle directly in `folders` whose executable exists.
fn bundles_in(folders: &[PathBuf]) -> Vec<Bundle> {
    let mut out = vec![];
    for dir in folders {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
                out.extend(bundle_info(&p).filter(|b| b.exe.is_file()));
            }
        }
    }
    out
}

/// The installed macOS apps; one scan serves every agent's detection for a few seconds.
fn installed_bundles() -> Vec<Bundle> {
    static CACHE: std::sync::Mutex<Option<(Instant, Vec<Bundle>)>> = std::sync::Mutex::new(None);
    let mut c = crate::util::lock(&CACHE);
    if let Some((_, v)) = c.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(3)) {
        return v.clone();
    }
    let v = bundles_in(&app_folders());
    *c = Some((Instant::now(), v.clone()));
    v
}

/// Which of `bundles` are the app: those whose bundle id or file name (`Codex.app`) is in
/// `keys`. Ids come first, as a renamed bundle keeps its id (Codex became ChatGPT.app);
/// in key order, the newest version first.
fn pick_bundles(bundles: &[Bundle], keys: &[&str]) -> Vec<Bundle> {
    let rank = |b: &Bundle| {
        let name = b.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        keys.iter().position(|k| b.id.as_deref() == Some(*k) || name.eq_ignore_ascii_case(k))
    };
    let mut found: Vec<(usize, Bundle)> = bundles.iter().filter_map(|b| rank(b).map(|r| (r, b.clone()))).collect();
    found.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then_with(|| version_key(b.version.as_deref()).cmp(&version_key(a.version.as_deref()))));
    found.into_iter().map(|(_, b)| b).collect()
}

/// Installed copies of a macOS app, by bundle id (`com.openai.codex`) or bundle file name
/// (`ZCode.app`); see [`pick_bundles`]. Empty on other systems.
pub(crate) fn app_bundles(keys: &[&str]) -> Vec<DesktopCopy> {
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    pick_bundles(&installed_bundles(), keys).into_iter().map(|b| DesktopCopy { exe: b.exe, version: b.version, running: false }).collect()
}

/// The app bundle (`…/X.app`) an executable lives in, if any.
pub(crate) fn bundle_of(exe: &Path) -> Option<&Path> {
    exe.ancestors().skip(1).find(|d| d.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
}

/// The folder a desktop app's processes live under: its bundle on macOS (helpers sit in
/// Contents/Frameworks), else the executable's folder.
pub(crate) fn app_dir(exe: &Path) -> Option<PathBuf> {
    bundle_of(exe).or_else(|| exe.parent()).map(Path::to_path_buf)
}

/// `inst` found as the desktop copy `c`.
pub(crate) fn use_copy(inst: &mut Install, c: &DesktopCopy) {
    inst.installed = true;
    inst.version = c.version.clone();
    inst.dir = app_dir(&c.exe);
    inst.exe = Some(c.exe.clone());
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
    let shared = ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "HOME"];
    let mut broad: Vec<String> = shared.iter().filter_map(|v| std::env::var(v).ok()).map(|p| norm_dir(&p)).collect();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        broad.push(norm_dir(&format!("{l}\\Programs")));
    }
    for d in app_folders().iter().chain(&["/System/Applications".into(), "/usr".into(), "/usr/local".into(), "/opt/homebrew".into()]) {
        broad.push(norm_dir(&d.to_string_lossy()));
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

/// The running server `s`: processes of its program whose command line serves its name.
fn server_roots(sys: &System, s: &Server) -> Vec<Pid> {
    use sysinfo::{ProcessRefreshKind, UpdateKind};
    let Some(name) = s.program.file_name() else { return vec![] };
    let pids: Vec<Pid> = sys.processes().iter().filter(|(_, p)| p.name().eq_ignore_ascii_case(name)).map(|(pid, _)| *pid).collect();
    if pids.is_empty() {
        return vec![];
    }
    // Only these few: reading every process's command line is slow on Windows.
    let mut cmds = System::new();
    cmds.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, ProcessRefreshKind::new().with_cmd(UpdateKind::Always));
    pids.into_iter()
        .filter(|pid| {
            let argv: Vec<String> = cmds.process(*pid).map(|p| p.cmd().iter().map(|a| a.to_string_lossy().to_string()).collect()).unwrap_or_default();
            (s.serves)(&argv).as_deref() == Some(s.name.as_str())
        })
        .collect()
}

/// `roots` and every process they started (a started process is newer than its parent).
fn with_descendants(sys: &System, roots: &[Pid]) -> Vec<Pid> {
    if roots.is_empty() {
        return vec![];
    }
    let under = |pid: Pid| {
        let mut cur = sys.process(pid);
        for _ in 0..64 {
            let Some(p) = cur else { return false };
            if roots.contains(&p.pid()) {
                return true;
            }
            cur = p.parent().and_then(|pp| sys.process(pp)).filter(|par| par.start_time() <= p.start_time());
        }
        false
    };
    sys.processes().keys().copied().filter(|pid| under(*pid)).collect()
}

/// The running desktop app of `inst`, or its server (empty when it has neither).
fn app_of(sys: &System, inst: &Install) -> Vec<Pid> {
    if let Some(s) = &inst.server {
        return with_descendants(sys, &server_roots(sys, s));
    }
    inst.dir.as_deref().map(|d| app_processes(sys, d, inst.exe.as_deref())).unwrap_or_default()
}

/// Kills process `pid` and everything it started. Launchers (`npx.cmd`, `uvx`) run the real
/// program as a grandchild, which would outlive a plain `Child::kill`.
pub(crate) fn kill_tree(pid: u32) {
    let sys = processes();
    for p in with_descendants(&sys, &[Pid::from_u32(pid)]).iter().filter_map(|pid| sys.process(*pid)) {
        p.kill();
    }
}

/// Whether AgentPlus can tell `inst`'s app processes (a desktop app's folder, or a server).
fn has_app(inst: &Install) -> bool {
    inst.dir.is_some() || inst.server.is_some()
}

/// Sets `running` from the desktop app's (or server's) processes. An install without either
/// (a CLI) keeps what its detector found.
pub(crate) fn set_app_running(inst: &mut Install) {
    if has_app(inst) {
        inst.running = !app_of(&processes(), inst).is_empty();
    }
}

/// Executable names of the CLI that an agent with a desktop app also has.
fn cli_names(agent: &str) -> Vec<String> {
    use crate::adapters::{codex, opencode};
    let bases: &[&str] = match agent {
        codex::ID => &["codex"],
        opencode::ID => &["opencode", "opencode-cli"],
        _ => &[],
    };
    bases.iter().map(|b| exe(b)).collect()
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
    processes().processes().values().any(|p| {
        let name = p.name().to_string_lossy();
        let path = p.exe().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        pred(&name, &path)
    })
}

/// First token that looks like a version ("2.1.226 (Claude Code)" → "2.1.226").
pub(crate) fn version_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_start_matches('v').trim_end_matches(','))
        .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && w.contains('.'))
        .map(String::from)
}

/// Inside WSL only CLIs exist; the desktop apps (no `wsl_script`) are Windows-only.
fn detect_wsl(e: &crate::adapters::Ext) -> Install {
    let mut inst = Install::default();
    if e.wsl_script.is_empty() {
        return inst;
    }
    let out = crate::env::wsl_sh(e.wsl_script).unwrap_or_default();
    inst.version = out.lines().find(|l| !l.starts_with('@')).and_then(version_in);
    inst.installed = inst.version.is_some() || crate::util::home().join(e.wsl_marker).exists();
    inst.running = out.lines().any(|l| l == "@running");
    inst
}

/// Version printed by a CLI (`<exe> --version`), killed after 5 s. A .cmd / .bat is started
/// directly too: the standard library runs it through `cmd /d /c` with proper quoting (paths
/// with `&` or parentheses) and without AutoRun hooks.
///
/// Cached per path while the file stays the same (size and time): an update in place (Claude
/// Code's native installer) shows at once. No answer is kept only for a minute, since a first
/// run can be slowed past the limit by an antivirus scan.
pub(crate) fn cli_version(exe: &Path) -> Option<String> {
    type Stamp = Option<(u64, Option<std::time::SystemTime>)>;
    type Entry = (PathBuf, Stamp, Option<String>, Instant);
    static CACHE: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());
    let cache = || crate::util::lock(&CACHE);
    let stamp: Stamp = std::fs::metadata(exe).ok().map(|m| (m.len(), m.modified().ok()));
    let fresh = |s: &Stamp, v: &Option<String>, at: &Instant| *s == stamp && (v.is_some() || at.elapsed() < Duration::from_secs(60));
    if let Some((.., v, _)) = cache().iter().find(|(p, s, v, at)| p == exe && fresh(s, v, at)) {
        return v.clone();
    }
    // Lets `output_within` pass on the login PATH (macOS) that an npm script needs.
    search_path();
    let v = output_within(Command::new(exe).arg("--version"), Duration::from_secs(5)).and_then(|o| version_in(&String::from_utf8_lossy(&o.stdout)));
    let mut c = cache();
    c.retain(|(p, ..)| p != exe);
    c.push((exe.to_path_buf(), stamp, v.clone(), Instant::now()));
    v
}

/// The `version` field of a package.json.
pub(crate) fn package_version(package_json: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(package_json).ok()?).ok()?;
    v.get("version")?.as_str().map(String::from)
}

/// package.json of npm package `pkg` under the npm prefix `root`
/// (`<root>\node_modules\<pkg>`, `pkg` may be scoped: "@scope/name").
pub(crate) fn npm_package_in(root: &Path, pkg: &str) -> PathBuf {
    pkg.split('/').fold(root.join("node_modules"), |d, part| d.join(part)).join("package.json")
}

/// Folders whose `node_modules` holds global npm packages: `%APPDATA%\npm` on Windows;
/// elsewhere `<prefix>/lib` of the prefixes npm commonly uses (a configured one, ~/.npm-global,
/// the one of the `node` on PATH, Homebrew, /usr/local).
fn npm_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        return dirs::data_dir().map(|d| d.join("npm")).into_iter().collect();
    }
    let mut prefixes: Vec<PathBuf> = vec![];
    prefixes.extend(std::env::var_os("NPM_CONFIG_PREFIX").map(PathBuf::from));
    prefixes.extend(dirs::home_dir().map(|h| h.join(".npm-global")));
    if let Some(node) = on_path(&["node"]) {
        // `<prefix>/bin/node`; a Homebrew node links into its Cellar, a version manager's
        // (nvm, fnm) is a real file in its own prefix: both count.
        prefixes.extend(node.parent().and_then(Path::parent).map(Path::to_path_buf));
        if let Ok(real) = std::fs::canonicalize(&node) {
            prefixes.extend(real.parent().and_then(Path::parent).map(Path::to_path_buf));
        }
    }
    prefixes.extend(["/opt/homebrew", "/usr/local"].map(PathBuf::from));
    let mut out: Vec<PathBuf> = vec![];
    for lib in prefixes.into_iter().map(|p| p.join("lib")) {
        if !out.contains(&lib) {
            out.push(lib);
        }
    }
    out
}

/// package.json of a global npm package (`%APPDATA%\npm\node_modules\<pkg>` on Windows);
/// None when it isn't installed.
pub(crate) fn npm_global_package(pkg: &str) -> Option<PathBuf> {
    npm_roots().iter().map(|r| npm_package_in(r, pkg)).find(|p| p.is_file())
}

/// Version of a global npm package; None when it isn't installed there.
pub(crate) fn npm_global_version(pkg: &str) -> Option<String> {
    package_version(&npm_global_package(pkg)?)
}

/// Version of a global npm package in the usual places, else next to `shim` (the CLI found on
/// PATH, for an npm with another prefix): `<shim dir>\node_modules` on Windows; elsewhere the
/// package the shim links into (`bin/x -> ../lib/node_modules/<pkg>/…`), or `<shim dir>/../lib`.
pub(crate) fn npm_version_near(pkg: &str, shim: Option<&Path>) -> Option<String> {
    npm_global_version(pkg).or_else(|| {
        let shim = shim?;
        let dir = shim.parent()?;
        if cfg!(windows) {
            return package_version(&npm_package_in(dir, pkg));
        }
        let named = |p: &Path| -> bool {
            let v: Option<serde_json::Value> = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok());
            v.as_ref().and_then(|v| v.get("name")).and_then(|n| n.as_str()) == Some(pkg)
        };
        std::fs::canonicalize(shim)
            .ok()
            .and_then(|real| real.ancestors().skip(1).take(6).map(|d| d.join("package.json")).find(|p| named(p)))
            .and_then(|p| package_version(&p))
            .or_else(|| package_version(&npm_package_in(&dir.parent()?.join("lib"), pkg)))
    })
}

/// A CLI on PATH (`names`, see [`on_path`]): its version from npm package `pkg`, else from
/// `--version`. `dir` and `exe` stay None: nothing to restart.
fn detect_cli(names: &[&str], pkg: &str) -> Install {
    let mut inst = Install::default();
    let shim = on_path(names);
    if let Some(v) = npm_version_near(pkg, shim.as_deref()) {
        inst.installed = true;
        inst.version = Some(v);
    } else if let Some(p) = shim {
        inst.installed = true;
        inst.version = cli_version(&p);
    }
    inst
}

/// The Codex desktop app on macOS. Since 2026-07 it ships as ChatGPT.app, keeping Codex's
/// bundle id; an older copy can still be Codex.app. Not by the name ChatGPT.app: that was
/// the chat-only app before it became "ChatGPT Classic" (com.openai.chat).
const CODEX_APP: &[&str] = &["com.openai.codex", "Codex.app"];

/// Codex desktop: the MSIX package on Windows, ChatGPT.app / Codex.app on macOS. Without
/// the app on macOS, the CLI: configured the same way, but there is nothing to restart.
pub(crate) fn detect_codex() -> Install {
    let mut inst = Install::default();
    if let Some(c) = app_bundles(CODEX_APP).first() {
        use_copy(&mut inst, c);
    } else if cfg!(target_os = "macos") {
        let mut cli = detect_cli(&["codex"], "@openai/codex");
        let name = exe("codex");
        cli.running = cli.installed && any_process(|n, _| n == name);
        return cli;
    } else {
        // One process scan both checks the cached package and finds the running app.
        let sys = processes();
        if let Some((dir, ver, pfn, main)) = codex_package(&sys) {
            inst.installed = true;
            inst.version = Some(ver);
            inst.aumid = Some(format!("{pfn}!App"));
            inst.exe = main;
            inst.dir = Some(dir);
            inst.running = !app_of(&sys, &inst).is_empty();
        }
        return inst;
    }
    set_app_running(&mut inst);
    inst
}

/// Codex command-line programs of the installed Codex, best first: the one the desktop app
/// ships (`app\resources\codex.exe` in the Windows package, `Contents/Resources/codex` in
/// the macOS bundle), the copies the Windows app unpacks under
/// `%LOCALAPPDATA%\OpenAI\Codex\bin` (newest first; some systems won't let other programs
/// run what is inside WindowsApps), or the CLI itself when only that is installed.
pub(crate) fn codex_clis() -> Vec<PathBuf> {
    let inst = detect_codex();
    let mut found = vec![];
    if let Some(dir) = &inst.dir {
        found.push(dir.join("app").join("resources").join("codex.exe"));
    }
    if cfg!(windows) {
        if let Some(bin) = dirs::data_local_dir().map(|d| d.join("OpenAI").join("Codex").join("bin")) {
            let mut copies: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&bin)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path().join("codex.exe"))
                .chain([bin.join("codex.exe")])
                .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
                .collect();
            copies.sort_by_key(|c| std::cmp::Reverse(c.0));
            found.extend(copies.into_iter().map(|(_, p)| p));
        }
    }
    if let Some(exe) = &inst.exe {
        if let Some(app) = exe.ancestors().find(|d| d.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))) {
            found.push(app.join("Contents").join("Resources").join("codex"));
        }
        if exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("codex")) {
            found.push(exe.clone());
        }
    }
    let mut out: Vec<PathBuf> = vec![];
    for p in found {
        if p.is_file() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Gives `cmd` the login shell's PATH, which an npm CLI (a `#!/usr/bin/env node` script) needs.
pub(crate) fn with_login_path(cmd: &mut Command) -> &mut Command {
    if let Some(p) = LOGIN_PATH.get() {
        cmd.env("PATH", p);
    }
    cmd
}

/// ZCode desktop: ZCode.app on macOS; on Windows its uninstall entry names the install folder.
pub(crate) fn detect_zcode() -> Install {
    let mut inst = Install::default();
    if let Some(c) = app_bundles(&["dev.zcode.app", "ZCode.app"]).first() {
        use_copy(&mut inst, c);
    } else if let Some(e) = uninstall_entry("ZCode") {
        let dir = e.uninstall.and_then(|u| unquote_exe(&u)).and_then(|p| p.parent().map(Path::to_path_buf));
        inst.installed = dir.is_some();
        inst.version = e.version;
        inst.exe = dir.as_ref().map(|d| d.join("ZCode.exe"));
        inst.dir = dir;
    }
    set_app_running(&mut inst);
    inst
}

/// MiMo Desktop: its app bundle on macOS (Electron productName "Xiaomi MiMo AI"); on
/// Windows its uninstall entry's icon is the app. Only the desktop app counts: a bare
/// `mimo` on PATH or a generic "MiMo.app" can be something else.
pub(crate) fn detect_mimo() -> Install {
    let mut inst = Install::default();
    if let Some(c) = app_bundles(&["Xiaomi MiMo AI.app", "Xiaomi MiMo.app"]).first() {
        use_copy(&mut inst, c);
    } else if let Some(e) = uninstall_entry("Xiaomi MiMo") {
        let exe = e.icon.and_then(|i| unquote_exe(&i));
        inst.installed = exe.is_some();
        inst.version = e.version;
        inst.dir = exe.as_ref().and_then(|x| x.parent().map(Path::to_path_buf));
        inst.exe = exe;
    }
    set_app_running(&mut inst);
    inst
}

/// The DeepSeek Harness desktop app (electron-builder, per-user NSIS on Windows): its bundle
/// on macOS; on Windows its uninstall entry ("DeepSeek Harness <version>"), else the
/// installer's default folder.
pub(crate) fn detect_dsh_desktop() -> Option<DesktopCopy> {
    const MAIN: &str = "DeepSeek Harness.exe";
    if let Some(c) = app_bundles(&["com.deepseek.harness", "DeepSeek Harness.app"]).into_iter().next() {
        return Some(c);
    }
    if let Some(c) = desktop_copies("DeepSeek Harness", MAIN).into_iter().next() {
        return Some(c);
    }
    let exe = dirs::data_local_dir()?.join("Programs").join("DeepSeek Harness").join(MAIN);
    (cfg!(windows) && exe.is_file()).then_some(DesktopCopy { exe, version: None, running: false })
}

/// Newest version folder of the Claude Code that Claude Desktop keeps for itself
/// (`<app data>/Claude/claude-code/<version>/`).
fn desktop_claude_code(app_data: &Path) -> Option<String> {
    std::fs::read_dir(app_data.join("Claude").join("claude-code"))
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(|c: char| c.is_ascii_digit()))
        .max_by_key(|n| version_key(Some(n)))
}

/// Claude Code: npm global install, the native installer (~/.local/bin/claude[.exe]) or a
/// `claude` on PATH (Homebrew); on macOS also Claude Desktop (Claude.app), which brings its
/// own Claude Code reading the same ~/.claude. A CLI: `dir` stays None.
pub(crate) fn detect_claude() -> Install {
    let mut inst = Install::default();
    let name = exe("claude");
    let native = dirs::home_dir().map(|h| h.join(".local").join("bin").join(&name)).filter(|p| p.exists());
    if let Some(pkg) = npm_global_package("@anthropic-ai/claude-code") {
        inst.installed = true;
        inst.version = package_version(&pkg);
    } else if let Some(exe) = native.or_else(|| on_path(&["claude.exe"])) {
        inst.installed = true;
        inst.version = cli_version(&exe);
    } else if !app_bundles(&["com.anthropic.claudefordesktop", "Claude.app"]).is_empty() {
        inst.installed = true;
        inst.version = dirs::config_dir().and_then(|d| desktop_claude_code(&d));
    }
    // An npm install runs under node, so this only sees the native build. Case matters off
    // Windows: Claude Desktop's own process is "Claude".
    inst.running = any_process(|n, _| if cfg!(windows) { n.eq_ignore_ascii_case(&name) } else { n == name });
    inst
}

/// Every installed copy of a desktop app registered as `prefix…` (an old and a new build can
/// sit side by side), each with an executable that exists; `main` names it when the entry's
/// icon doesn't.
fn desktop_copies(prefix: &str, main: &str) -> Vec<DesktopCopy> {
    let mut out: Vec<DesktopCopy> = vec![];
    for e in uninstall_entries(prefix) {
        let exe = e
            .icon
            .and_then(|i| unquote_exe(&i))
            .filter(|p| is_exe(p) && p.is_file())
            .or_else(|| e.uninstall.and_then(|u| unquote_exe(&u)).and_then(|p| p.parent().map(|d| d.join(main))).filter(|p| p.is_file()));
        let Some(exe) = exe else { continue };
        if !out.iter().any(|c| norm_dir(&c.exe.to_string_lossy()) == norm_dir(&exe.to_string_lossy())) {
            out.push(DesktopCopy { exe, version: e.version, running: false });
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
pub const DESKTOP_EXE_STORE_KEY: &str = "desktopExe";

/// OpenCode: the desktop app (registry on Windows, OpenCode.app on macOS) or the CLI.
pub(crate) fn detect_opencode() -> Install {
    let mut inst = Install::default();
    let mut copies = desktop_copies("OpenCode", "OpenCode.exe");
    copies.extend(app_bundles(&["ai.opencode.desktop", "OpenCode.app"]));
    if !copies.is_empty() {
        let sys = processes();
        for c in &mut copies {
            c.running = app_dir(&c.exe).is_some_and(|d| !app_processes(&sys, &d, Some(&c.exe)).is_empty());
        }
        let preferred = crate::store::get_str(&crate::store::load(), crate::adapters::opencode::ID, DESKTOP_EXE_STORE_KEY).filter(|s| !s.is_empty());
        let c = &copies[choose_copy(&copies, preferred.as_deref())];
        use_copy(&mut inst, c);
        // Counted just above, the same way `set_app_running` would.
        inst.running = c.running;
        inst.copies = copies;
        return inst;
    }
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [home.join(".opencode").join("bin").join(exe("opencode")), dirs::data_dir().unwrap_or_default().join("npm").join("opencode.cmd")];
    if let Some(p) = candidates.into_iter().find(|p| p.exists()).or_else(|| on_path(&["opencode.exe", "opencode.cmd"])) {
        inst.installed = true;
        if is_exe(&p) {
            inst.version = cli_version(&p);
        }
    }
    let name = exe("opencode");
    inst.running = any_process(|n, _| n.eq_ignore_ascii_case(&name));
    inst
}

/** Trae (international or CN build); detection only. */
pub fn detect_trae() -> Install {
    let mut inst = Install::default();
    if let Some(c) = app_bundles(&["com.trae.app", "cn.trae.app", "Trae.app", "Trae CN.app"]).first() {
        use_copy(&mut inst, c);
    } else if let Some(e) = uninstall_entry("Trae (User)").or_else(|| uninstall_entry("TraeCode")).or_else(|| uninstall_entry("Trae")) {
        let exe = e.icon.and_then(|i| unquote_exe(&i));
        inst.installed = exe.as_ref().map(|x| x.exists()).unwrap_or(false);
        inst.version = e.version;
        inst.dir = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
    }
    if let Some(d) = &inst.dir {
        inst.running = !app_processes(&processes(), d, None).is_empty();
    }
    inst
}

/// What detection finds for `agent` in the current environment (nothing for an unknown id).
pub fn detect(agent: &str) -> Install {
    let Some(e) = crate::adapters::ext(agent) else { return Install::default() };
    if crate::env::is_wsl() {
        detect_wsl(e)
    } else {
        (e.detect)()
    }
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
        return Err(anyhow!(crate::i18n::l("Cancelled", "已取消")));
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

/// While alive, macOS doesn't put AgentPlus into App Nap. A restart starts an app whose
/// window then covers AgentPlus's, and a napping app's sleeps and timers stretch to many
/// seconds: the waits for the process and the DevTools port would lag far behind. No-op
/// elsewhere.
pub struct Awake {
    #[cfg(target_os = "macos")]
    token: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>,
}

pub fn stay_awake(reason: &str) -> Awake {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let opts = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep | NSActivityOptions::LatencyCritical;
        Awake { token: NSProcessInfo::processInfo().beginActivityWithOptions_reason(opts, &NSString::from_str(reason)) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reason;
        Awake {}
    }
}

#[cfg(target_os = "macos")]
impl Drop for Awake {
    fn drop(&mut self) {
        // SAFETY: the token is what beginActivityWithOptions:reason: returned.
        unsafe { objc2_foundation::NSProcessInfo::processInfo().endActivity(&self.token) };
    }
}

/// Ends the desktop app's processes: asked to quit (SIGTERM) where the system has that
/// (macOS), killed otherwise (Windows) or when `force`.
fn end_app(inst: &Install, force: bool) {
    let sys = processes();
    let pids = app_of(&sys, inst);
    // Parents first: an Electron main process relaunches a child that dies before it (a
    // renderer, the GPU process), which only leaves more processes to wait for.
    let mut procs: Vec<_> = pids.iter().filter_map(|pid| sys.process(*pid)).collect();
    procs.sort_by_key(|p| p.parent().is_some_and(|pp| pids.contains(&pp)));
    for p in procs {
        if force || p.kill_with(Signal::Term) != Some(true) {
            p.kill();
        }
    }
}

/// How long the app gets to quit by itself before what is left is killed. Only macOS asks
/// it to quit; elsewhere it was killed right away, and this just catches stragglers.
const QUIT_GRACE: Duration = Duration::from_secs(if cfg!(target_os = "macos") { 5 } else { 1 });

/// Ends the desktop app's processes and waits for them to exit; whatever is left after
/// [`QUIT_GRACE`] is killed.
fn stop(inst: &Install, on: &dyn Fn(Progress)) -> Result<()> {
    end_app(inst, false);
    let t0 = Instant::now();
    let mut left_seen = usize::MAX;
    let mut forced = false;
    loop {
        let left = app_of(&processes(), inst).len();
        if left == 0 {
            crate::applog::info("restart", format!("app exited after {:.1}s{}", t0.elapsed().as_secs_f32(), if forced { " (killed stragglers)" } else { "" }));
            return Ok(());
        }
        if !forced && t0.elapsed() > QUIT_GRACE {
            forced = true;
            end_app(inst, true);
        }
        if left != left_seen {
            left_seen = left;
            on(Progress::step("stop", "active", Some(tr!("Waiting for {left} process(es) to exit", "等待 {left} 个进程退出"))));
        }
        if t0.elapsed() > Duration::from_secs(10) {
            return Err(anyhow!(crate::i18n::l("The process did not exit within 10 seconds", "进程没有在 10 秒内退出")));
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
    Err(anyhow!(crate::i18n::l("Windows only", "仅支持 Windows")))
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
        return Err(anyhow!(crate::i18n::l("Codex in WSL is a command-line tool and doesn't need a restart: new codex sessions read the new config", "WSL 里的 Codex 是命令行工具，不用重启：新开的 codex 会话会读取新配置")));
    }
    let t0 = Instant::now();
    let inst = detect(agent);
    if !inst.installed {
        return Err(anyhow!(crate::i18n::l("No installation detected", "没有检测到安装")));
    }
    let sys = processes();
    let app = app_of(&sys, &inst);
    let running = app.len();
    let cli = cli_sessions(&sys, agent, &app);
    drop(sys);
    crate::applog::info(
        "restart",
        format!(
            "{agent} {}: {running} app process(es), {cli} CLI session(s), dir={:?} exe={:?} aumid={:?} args={args:?} (detect {:.1}s)",
            inst.version.as_deref().unwrap_or("?"),
            inst.dir,
            inst.exe,
            inst.aumid,
            t0.elapsed().as_secs_f32()
        ),
    );
    let cli_note = (cli > 0).then(|| tr!("{cli} CLI session(s) in terminals aren't restarted; they read the new config once reopened", "终端里的 {cli} 个 CLI 会话不会重启，重新打开后才读到新配置"));
    if running > 0 {
        on(Progress::step("stop", "active", Some(tr!("Ending {running} process(es)", "正在结束 {running} 个进程"))));
        stop(&inst, on)?;
        on(match cli_note {
            Some(n) => Progress::step("stop", "warn", Some(n)),
            None => Progress::step("stop", "done", None),
        });
        pause(Duration::from_millis(400))?;
    } else {
        let idle = crate::i18n::l("Not running", "没有在运行");
        on(match cli_note {
            Some(n) => Progress::step("stop", "warn", Some(format!("{idle}{}{n}", crate::i18n::l("; ", "；")))),
            None => Progress::step("stop", "skip", Some(idle.into())),
        });
    }
    let done = Restarted { was_running: running > 0, cli_sessions: cli };
    check_cancel()?;
    on(Progress::step("start", "active", None));
    let t1 = Instant::now();
    if let Some(s) = &inst.server {
        start_server(s).map_err(|e| anyhow!(tr!("Failed to start: {e}", "启动失败：{e}")))?;
    } else if let Some(aumid) = &inst.aumid {
        activate(aumid, args)?;
    } else if let Some(exe) = &inst.exe {
        start_exe(exe, args).map_err(|e| anyhow!(tr!("Failed to start: {e}", "启动失败：{e}")))?;
    }
    crate::applog::info("restart", format!("{agent}: launch call returned after {:.1}s", t1.elapsed().as_secs_f32()));
    if let Some(s) = &inst.server {
        let url = wait_served(s, on)?;
        remember_launch(agent, &inst);
        on(match url {
            Some(url) => Progress::step("start", "done", Some(url)),
            None => Progress::step("start", "warn", Some(tr!("No address seen within {}s; it may still be starting", "{} 秒内没有看到服务地址，可能还在启动", SERVE_WAIT.as_secs()))),
        });
        return Ok(done);
    }
    // Wait until its process shows up, so "done" means it is actually up.
    if inst.dir.is_some() {
        on(Progress::step("start", "active", Some(crate::i18n::l("Waiting for the process", "等待进程出现").into())));
        let t0 = Instant::now();
        let mut scans = 0u32;
        while app_of(&processes(), &inst).is_empty() {
            scans += 1;
            if t0.elapsed() > Duration::from_secs(15) {
                crate::applog::warn("restart", format!("{agent}: no app process seen within 15s ({scans} scans); {}", near_misses(&inst)));
                on(Progress::step("start", "warn", Some(crate::i18n::l("No process seen within 15 seconds; it may still be starting", "15 秒内没有看到它的进程，可能还在启动").into())));
                return Ok(done);
            }
            pause(Duration::from_millis(300))?;
        }
        crate::applog::info("restart", format!("{agent}: app process seen after {:.1}s ({scans} scans)", t0.elapsed().as_secs_f32()));
        remember_launch(agent, &inst);
    }
    on(Progress::step("start", "done", None));
    Ok(done)
}

/// The app's main process: its main executable, not started by another of its processes
/// (the oldest when several). (pid, start time).
fn main_process(sys: &System, inst: &Install) -> Option<(Pid, u64)> {
    if let Some(s) = &inst.server {
        return server_roots(sys, s).into_iter().filter_map(|pid| sys.process(pid)).min_by_key(|p| p.start_time()).map(|p| (p.pid(), p.start_time()));
    }
    let main = norm_dir(&inst.exe.as_deref()?.to_string_lossy());
    let pids = app_of(sys, inst);
    pids.iter()
        .filter_map(|pid| sys.process(*pid))
        .filter(|p| p.exe().is_some_and(|e| norm_dir(&e.to_string_lossy()) == main))
        .filter(|p| !p.parent().is_some_and(|pp| pids.contains(&pp)))
        .min_by_key(|p| p.start_time())
        .map(|p| (p.pid(), p.start_time()))
}

/// Remembers the app's main process as started by AgentPlus (see [`launch`]).
fn remember_launch(agent: &str, inst: &Install) {
    let Some((pid, start)) = main_process(&processes(), inst) else { return };
    let r = crate::store::update(|root| {
        if !root.get("launched").is_some_and(|v| v.is_object()) {
            root["launched"] = serde_json::json!({});
        }
        root["launched"][agent] = serde_json::json!({ "pid": pid.as_u32(), "start": start });
        Ok(())
    });
    if let Err(e) = r {
        crate::applog::warn("restart", format!("{agent}: could not remember the launch: {e:#}"));
    }
}

/// How the running app of `agent` was started: by AgentPlus when its main process is the
/// one the last restart from AgentPlus started, or when it carries AgentPlus's DevTools
/// port (an app started by an earlier AgentPlus). None while it isn't running.
pub fn launch(agent: &str, inst: &Install) -> Option<crate::model::Launch> {
    if !inst.running {
        return None;
    }
    let (ours, argv) = main_launch(agent, inst)?;
    let flag = "--remote-debugging-port=";
    let debug_port = argv.iter().any(|a| a.starts_with(flag));
    let wants_ui = agent == crate::adapters::codex::ID && crate::adapters::codex::ui_patches().any();
    Some(crate::model::Launch { by_agentplus: ours || debug_port, debug_port, ui_inactive: wants_ui && !debug_port })
}

/// From one process scan: whether `inst`'s main process is the one AgentPlus last started for
/// `agent`, and its command line. None when it isn't running.
fn main_launch(agent: &str, inst: &Install) -> Option<(bool, Vec<String>)> {
    use sysinfo::{ProcessRefreshKind, UpdateKind};
    let mut sys = processes();
    let (pid, start) = main_process(&sys, inst)?;
    let rec = crate::store::load().get("launched").and_then(|l| l.get(agent)).cloned();
    let ours = rec.is_some_and(|r| r["pid"].as_u64() == Some(u64::from(pid.as_u32())) && r["start"].as_u64() == Some(start));
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::new().with_cmd(UpdateKind::Always));
    let argv = sys.process(pid).map(|p| p.cmd().iter().map(|a| a.to_string_lossy().to_string()).collect()).unwrap_or_default();
    Some((ours, argv))
}

/// `inst`'s running server: whether AgentPlus started it, and its command line (argv). None
/// when it has no server or it isn't running.
pub fn server_launch(agent: &str, inst: &Install) -> Option<(bool, Vec<String>)> {
    inst.server.as_ref()?;
    main_launch(agent, inst)
}

/// For the log when a started app wasn't found: processes named like its main executable and
/// where they run from (a second copy, or a macOS app run from a translocated path).
fn near_misses(inst: &Install) -> String {
    let Some(name) = inst.exe.as_deref().and_then(Path::file_name) else { return "no main executable known".into() };
    let sys = processes();
    let seen: Vec<String> = sys.processes().values().filter(|p| p.name().eq_ignore_ascii_case(name)).take(5).map(|p| format!("pid {} exe={:?}", p.pid(), p.exe())).collect();
    if seen.is_empty() {
        format!("no process named {name:?}")
    } else {
        format!("processes named {name:?}: {}", seen.join(", "))
    }
}

/// Whether the agent's app is running now and how it was started (cheap; keeps the
/// Start/Restart button and the launch hint honest).
pub fn running(agent: &str) -> (bool, Option<crate::model::Launch>) {
    let t0 = Instant::now();
    let inst = detect(agent);
    let launch = if has_app(&inst) { launch(agent, &inst) } else { None };
    if t0.elapsed() > Duration::from_secs(2) {
        crate::applog::warn("detect", format!("{agent}: running check took {:.1}s", t0.elapsed().as_secs_f32()));
    }
    (inst.running, launch)
}

/// Starts a desktop app. A macOS app bundle goes through LaunchServices (`open`), as from
/// the Dock; `-n` because the old instance was just ended and `open` would otherwise only
/// activate it and drop `args`.
fn start_exe(exe: &Path, args: &str) -> Result<()> {
    if let Some(bundle) = bundle_of(exe).filter(|_| cfg!(target_os = "macos")) {
        let mut cmd = Command::new("open");
        cmd.arg("-n").arg(bundle);
        if !args.is_empty() {
            cmd.arg("--args").args(args.split_whitespace());
        }
        let out = cmd.output()?;
        if !out.status.success() {
            return Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        return Ok(());
    }
    let mut cmd = Command::new(exe);
    if !args.is_empty() {
        cmd.args(args.split_whitespace());
    }
    cmd.spawn()?;
    Ok(())
}

/// Starts server `s` in the background, without a window, its output going to `s.log`. It
/// ends with AgentPlus unless it is independent (see [`set_independent`]).
fn start_server(s: &Server) -> Result<()> {
    if let Some(d) = s.log.parent() {
        std::fs::create_dir_all(d)?;
    }
    let out = std::fs::File::create(&s.log)?;
    let err = out.try_clone()?;
    // Lets `with_login_path` pass on the login PATH (macOS) that node's children need.
    search_path();
    let mut cmd = Command::new(&s.program);
    cmd.args(&s.args).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    with_login_path(&mut cmd);
    set_independent(s.independent);
    let mut child = spawn_server(&mut cmd)?;
    #[cfg(windows)]
    if let Err(e) = job::adopt(&child, !s.independent) {
        crate::applog::warn("restart", format!("{}: could not tie the server to AgentPlus: {e}", s.name));
    }
    let pid = child.id();
    own_servers().push(pid);
    std::thread::spawn(move || {
        let _ = child.wait();
        own_servers().retain(|p| *p != pid);
    });
    Ok(())
}

/// Spawns a server without a window. On Windows it leaves any job AgentPlus runs in (a
/// terminal's or an IDE's) when that job allows it, so only AgentPlus's own job (see [`job`])
/// decides whether it outlives AgentPlus. Elsewhere it gets its own process group, so a
/// Ctrl+C in the terminal that started AgentPlus doesn't reach it.
fn spawn_server(cmd: &mut Command) -> Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        match cmd.creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB).spawn() {
            Ok(c) => Ok(c),
            // Access denied: the job AgentPlus is in doesn't allow leaving it.
            Err(e) if e.raw_os_error() == Some(5) => Ok(cmd.creation_flags(CREATE_NO_WINDOW).spawn()?),
            Err(e) => Err(e.into()),
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        Ok(cmd.process_group(0).spawn()?)
    }
}

/// Whether the servers AgentPlus started keep running after it quits (the agent's setting,
/// applied at once to the ones already running).
static INDEPENDENT: AtomicBool = AtomicBool::new(false);

/// Servers this AgentPlus started that haven't exited yet (each leaves once reaped, so its pid
/// can't have been reused).
fn own_servers() -> std::sync::MutexGuard<'static, Vec<u32>> {
    static PIDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(vec![]);
    PIDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lets the servers AgentPlus started outlive it (`true`), or ties them to it.
pub fn set_independent(on: bool) {
    INDEPENDENT.store(on, Ordering::SeqCst);
    #[cfg(windows)]
    job::kill_on_close(!on);
}

/// Agents whose Start runs a background server (see [`Server`]).
const SERVER_AGENTS: &[&str] = &[crate::adapters::dsh::ID];

/// On quit: ends the servers AgentPlus started unless they are independent. On Windows the
/// ones this AgentPlus started are ended by their job (also when it crashes or is killed).
pub fn end_own_servers() {
    // No server started by this AgentPlus or an earlier one: nothing to end, and no detection
    // (PowerShell, `--version`) to wait for while quitting.
    let launched = crate::store::load().get("launched").cloned().unwrap_or_default();
    if own_servers().is_empty() && SERVER_AGENTS.iter().all(|a| launched.get(*a).is_none()) {
        return;
    }
    if !cfg!(windows) && !INDEPENDENT.load(Ordering::SeqCst) {
        let roots: Vec<Pid> = own_servers().iter().map(|p| Pid::from_u32(*p)).collect();
        if !roots.is_empty() {
            let sys = processes();
            for pid in with_descendants(&sys, &roots) {
                if let Some(p) = sys.process(pid) {
                    if p.kill_with(Signal::Term) != Some(true) {
                        p.kill();
                    }
                }
            }
        }
    }
    end_launched_servers();
    // A server that ends with AgentPlus won't be opened with its link again.
    for agent in SERVER_AGENTS {
        if let Some(s) = detect(agent).server.filter(|s| !s.independent) {
            forget_served_token(&s.log);
        }
    }
}

/// Servers an earlier AgentPlus started while they were independent: once the setting is off,
/// they end with this one too (no job of this AgentPlus holds them).
fn end_launched_servers() {
    let launched = crate::store::load().get("launched").cloned().unwrap_or_default();
    for agent in SERVER_AGENTS.iter().filter(|a| launched.get(**a).is_some()) {
        let inst = detect(agent);
        let Some(s) = inst.server.as_ref().filter(|s| !s.independent && inst.running) else { continue };
        if server_launch(agent, &inst).is_some_and(|(ours, _)| ours) {
            end_app(&inst, true);
            crate::applog::info("restart", format!("{agent}: server {} ended with AgentPlus", s.name));
        }
    }
}

/// The job object the servers AgentPlus starts run in: while it kills on close, Windows ends
/// them (and whatever they started) once AgentPlus's handle to it closes, however AgentPlus
/// exits. The handle is never closed by hand.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// The job's handle as an integer (a raw pointer can't sit in a static); None when it
    /// couldn't be created.
    static JOB: OnceLock<Option<isize>> = OnceLock::new();

    fn handle(h: isize) -> HANDLE {
        HANDLE(h as *mut std::ffi::c_void)
    }

    fn set(job: HANDLE, kill: bool) -> windows::core::Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        if kill {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }
        // SAFETY: `info` is the structure this information class expects, alive for the call.
        unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, std::ptr::from_ref(&info).cast(), std::mem::size_of_val(&info) as u32) }
    }

    /// Puts `child` in the job, which kills on close when `kill`.
    pub(super) fn adopt(child: &std::process::Child, kill: bool) -> windows::core::Result<()> {
        // SAFETY: no name, default security; the handle lives as long as AgentPlus.
        let job = JOB.get_or_init(|| unsafe { CreateJobObjectW(None, None) }.ok().map(|h| h.0 as isize)).map(handle);
        let job = job.ok_or_else(windows::core::Error::from_win32)?;
        set(job, kill)?;
        // SAFETY: the child's process handle is valid while `child` is borrowed.
        unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) }
    }

    /// Whether the job kills its processes on close, when there is one.
    pub(super) fn kill_on_close(kill: bool) {
        if let Some(Some(h)) = JOB.get() {
            if let Err(e) = set(handle(*h), kill) {
                crate::applog::warn("restart", format!("could not update the server job: {e}"));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::CloseHandle;

        /// Whether `child` exits within a few seconds.
        fn exits(child: &mut std::process::Child) -> bool {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(5) {
                if child.try_wait().unwrap().is_some() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        }

        /// A job that no longer kills on close (switched after the child joined, as the
        /// setting does) lets it outlive the handle; one that does ends it.
        #[test]
        fn closing_the_job_ends_its_processes_only_while_it_kills_on_close() {
            for keep in [true, false] {
                let mut child = Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).spawn().unwrap();
                // SAFETY: as in `adopt`; this job's handle is closed below.
                let job = unsafe { CreateJobObjectW(None, None) }.unwrap();
                set(job, true).unwrap();
                unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) }.unwrap();
                if keep {
                    set(job, false).unwrap();
                }
                unsafe { CloseHandle(job) }.unwrap();
                assert_eq!(exits(&mut child), !keep);
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Stops the server `agent` runs in the background (dsh web), whoever started it. The number
/// of processes it had; 0 when it wasn't running.
pub fn stop_server(agent: &str) -> Result<usize> {
    let inst = detect(agent);
    if inst.server.is_none() {
        return Err(anyhow!(crate::i18n::l("Only a web UI server running in the background can be stopped here", "这里只能停止在后台运行的网页服务")));
    }
    let n = app_of(&processes(), &inst).len();
    if n > 0 {
        stop(&inst, &|_| {})?;
        crate::applog::info("restart", format!("{agent}: server stopped ({n} process(es))"));
    }
    if let Some(s) = &inst.server {
        forget_served_token(&s.log);
    }
    Ok(n)
}

/// On start: server logs whose server isn't running any more lose their sign-in token (it
/// only opened that run of the server). Logs of other profiles are included.
pub fn forget_idle_tokens() {
    let running: Vec<PathBuf> = SERVER_AGENTS
        .iter()
        .map(|a| detect(a))
        .filter(|i| i.running)
        .filter_map(|i| i.server.map(|s| s.log))
        .collect();
    for log in server_logs() {
        if !running.iter().any(|r| r == &log) {
            forget_served_token(&log);
        }
    }
}

/// The logs servers write (`dsh-<profile>.log` in the log folder).
pub(crate) fn server_logs() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(crate::applog::dir()) else { return vec![] };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("dsh-") && n.ends_with(".log")))
        .collect()
}

/// How long a started server gets to print its address.
const SERVE_WAIT: Duration = Duration::from_secs(30);

/// Waits for server `s` to print its address, and returns it; None when it is still starting
/// after [`SERVE_WAIT`]. Err when it exits first, with what it printed about why.
fn wait_served(s: &Server, on: &dyn Fn(Progress)) -> Result<Option<String>> {
    on(Progress::step("start", "active", Some(crate::i18n::l("Waiting for the server", "等待服务启动").into())));
    let read = || std::fs::read(&s.log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let t0 = Instant::now();
    let mut seen = false;
    loop {
        if let Some(url) = served_url(&read()) {
            crate::applog::info("restart", format!("{}: serving after {:.1}s", s.name, t0.elapsed().as_secs_f32()));
            return Ok(Some(url));
        }
        let up = !server_roots(&processes(), s).is_empty();
        seen |= up;
        // Not seen at all: give the scan a moment to catch up with the new process.
        if !up && (seen || t0.elapsed() > Duration::from_secs(5)) {
            return Err(anyhow!(tr!("It exited while starting: {}", "启动过程中退出了：{}", failure_summary(&read()))));
        }
        if t0.elapsed() > SERVE_WAIT {
            return Ok(None);
        }
        pause(Duration::from_millis(300))?;
    }
}

/// The local sign-in link in the latest `dsh web:` announcement. A second, parenthesized
/// LAN link and unrelated URLs in diagnostics must not become the browser's destination.
pub(crate) fn served_link(output: &str) -> Option<&str> {
    output.lines().rev().find_map(|line| {
        let link = line.trim().strip_prefix("dsh web:")?.split_whitespace().next()?;
        let url = url::Url::parse(link).ok()?;
        (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(link)
    })
}

/// Takes the sign-in token out of a server's log (the query and fragment of its `dsh web:`
/// links), once the link isn't needed to open the running server; the rest stays for
/// diagnosing a failed start. Written in place, so the file keeps its permissions.
pub(crate) fn forget_served_token(log: &Path) {
    use std::io::Write;
    static LINK: OnceLock<regex::Regex> = OnceLock::new();
    let Ok(bytes) = std::fs::read(log) else { return };
    let text = String::from_utf8_lossy(&bytes);
    let re = LINK.get_or_init(|| regex::Regex::new(r"(https?://[^\s?#()]*)[?#][^\s()]*").unwrap());
    let out: String = text
        .split_inclusive('\n')
        .map(|l| if l.trim_start().starts_with("dsh web:") { re.replace_all(l, "$1").into_owned() } else { l.to_string() })
        .collect();
    if out != text {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).truncate(true).open(log) {
            let _ = f.write_all(out.as_bytes());
        }
    }
}

/// [`served_link`] without its query: the access token stays out of the UI and the log.
fn served_url(output: &str) -> Option<String> {
    let url = served_link(output)?;
    Some(url.split(['?', '#']).next().unwrap_or(url).to_string())
}

/// What a server that failed to start printed about why: its first line, the error lines and
/// its last line (where dsh names its diagnostics file).
fn failure_summary(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut out: Vec<&str> = vec![];
    for (i, l) in lines.iter().enumerate() {
        if (i == 0 || i + 1 == lines.len() || l.starts_with("Error")) && !out.contains(l) {
            out.push(l);
        }
    }
    if out.is_empty() {
        return crate::i18n::l("it printed nothing", "没有任何输出").into();
    }
    out.join("; ")
}

/// Starts `cmd` and reaps it in the background (a finished child would linger as a zombie on
/// Unix until AgentPlus exits).
fn spawn_detached(cmd: &mut Command) -> Result<()> {
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Opens a folder in the file manager (Explorer, Finder).
pub fn open_dir(dir: &str) -> Result<()> {
    let opener = if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    spawn_detached(Command::new(opener).arg(dir))
}

/// Opens a web page in the default browser. On Windows not through explorer.exe, which
/// opens a File Explorer window for some addresses (a local `http://127.0.0.1:3080/`).
pub fn open_url(url: &str) -> Result<()> {
    if cfg!(windows) {
        spawn_detached(no_window(Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url])))
    } else {
        open_dir(url)
    }
}

/// Opens the file manager with the file selected (Linux: its folder).
pub fn reveal(path: &str) -> Result<()> {
    if cfg!(windows) {
        spawn_detached(Command::new("explorer.exe").arg(format!("/select,{path}")))
    } else if cfg!(target_os = "macos") {
        spawn_detached(Command::new("open").arg("-R").arg(path))
    } else {
        open_dir(&Path::new(path).parent().unwrap_or(Path::new(path)).to_string_lossy())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn codex_package_dir_is_the_versioned_folder() {
        let exe = Path::new(r"C:\Program Files\WindowsApps\OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe");
        assert_eq!(codex_package_dir(exe), Some(Path::new(r"C:\Program Files\WindowsApps\OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0")));
        // The CLI Codex keeps in the profile, or a folder merely named like the package, isn't it.
        assert_eq!(codex_package_dir(Path::new(r"C:\Users\me\AppData\Local\OpenAI\Codex\bin\x\codex.exe")), None);
        assert_eq!(codex_package_dir(Path::new(r"D:\tmp\OpenAI.Codex_1\app\ChatGPT.exe")), None);
    }

    #[test]
    fn cached_codex_package_goes_stale() {
        let sys = System::new();
        let gone: CodexPkg = Some((PathBuf::from(r"C:\Program Files\WindowsApps\OpenAI.Codex_0.0.0.0_x64__none"), "0".into(), "f".into(), None));
        assert!(codex_package_stale(&gone, Duration::ZERO, &sys));
        let here: CodexPkg = Some((std::env::temp_dir(), "1".into(), "f".into(), None));
        assert!(!codex_package_stale(&here, Duration::ZERO, &sys));
        assert!(codex_package_stale(&here, CODEX_PKG_TTL + Duration::from_secs(1), &sys));
        // "Not installed" is asked again after a minute.
        assert!(!codex_package_stale(&None, Duration::from_secs(5), &sys));
        assert!(codex_package_stale(&None, Duration::from_secs(61), &sys));
    }

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
        // Updated in place: the new version, not the cached one.
        std::fs::write(&script, "@echo tool v10.0.0, build 22\r\n").unwrap();
        assert_eq!(cli_version(&script).as_deref(), Some("10.0.0"));
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

#[cfg(test)]
mod any_os_tests {
    use super::*;

    /// A launcher's grandchild (`npx.cmd` → node) goes with it.
    #[test]
    fn kill_tree_ends_the_grandchildren() {
        let mut launcher = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/c", "ping -n 30 127.0.0.1 >NUL"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30; true"]);
            c
        };
        let mut child = launcher.stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = Pid::from_u32(child.id());
        // Wait for the grandchild to show up.
        let t0 = Instant::now();
        let grandchild = loop {
            let sys = processes();
            if let Some(g) = sys.processes().values().find(|p| p.parent() == Some(pid)).map(|p| p.pid()) {
                break g;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "no grandchild");
            std::thread::sleep(Duration::from_millis(100));
        };
        kill_tree(child.id());
        let _ = child.wait();
        let t0 = Instant::now();
        while processes().process(grandchild).is_some() {
            assert!(t0.elapsed() < Duration::from_secs(5), "the grandchild kept running");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn a_cli_keeps_the_running_state_its_detector_found() {
        // Claude Code and the OpenCode CLI have no folder: nothing may overwrite what they found.
        let mut cli = Install { installed: true, running: true, ..Default::default() };
        set_app_running(&mut cli);
        assert!(cli.running);
        let mut idle = Install { installed: true, ..Default::default() };
        set_app_running(&mut idle);
        assert!(!idle.running);
    }

    #[test]
    fn desktop_only_agents_are_absent_in_wsl() {
        // No script to run, and an empty marker would name the home folder itself.
        for a in [crate::adapters::zcode::ID, crate::adapters::mimo::ID] {
            let inst = detect_wsl(crate::adapters::ext(a).unwrap());
            assert!(!inst.installed && !inst.running && inst.version.is_none(), "{a}");
        }
    }

    #[test]
    fn served_url_drops_the_token() {
        assert_eq!(served_url("dsh web: http://127.0.0.1:3080/?token=abc\n").as_deref(), Some("http://127.0.0.1:3080/"));
        assert_eq!(served_url("dsh web: https://localhost:8443#x").as_deref(), Some("https://localhost:8443"));
        assert_eq!(served_url("dsh: startup failed\n"), None);
        assert_eq!(served_url(""), None);
        // The whole link (it signs the browser in), the last one printed.
        assert_eq!(served_link("dsh web: http://127.0.0.1:3080/?token=abc\n"), Some("http://127.0.0.1:3080/?token=abc"));
        assert_eq!(served_link("see https://docs.example\ndsh web: http://127.0.0.1:3081/?token=x"), Some("http://127.0.0.1:3081/?token=x"));
    }

    #[test]
    fn forgetting_the_token_keeps_the_rest_of_the_log() {
        let h = crate::util::TestHome::new("forget-token");
        let log = h.0.join("dsh-web.log");
        let text = "starting\ndsh web: http://127.0.0.1:3080/?token=abc (LAN: http://192.168.1.5:3080/?token=abc#x)\nsee https://docs.example/?q=keep\n";
        std::fs::write(&log, text).unwrap();
        forget_served_token(&log);
        let out = std::fs::read_to_string(&log).unwrap();
        assert_eq!(out, "starting\ndsh web: http://127.0.0.1:3080/ (LAN: http://192.168.1.5:3080/)\nsee https://docs.example/?q=keep\n");
        assert_eq!(served_link(&out), Some("http://127.0.0.1:3080/"), "the plain address is still there");
        forget_served_token(&h.0.join("missing.log"));
    }

    #[test]
    fn served_link_uses_the_local_announcement_not_the_lan_link() {
        let output = "dsh web: http://127.0.0.1:3080/?token=abc (LAN: http://192.168.1.5:3080/?token=abc)\n\
                      dsh web: opening the default browser; pass --no-open to disable\n\
                      See https://docs.example/help for details\n";
        let link = served_link(output).unwrap();
        assert_eq!(link, "http://127.0.0.1:3080/?token=abc");
        let parsed = url::Url::parse(link).unwrap();
        assert_eq!(parsed.query_pairs().find(|(k, _)| k == "token").unwrap().1, "abc");
        assert_eq!(served_url(output).as_deref(), Some("http://127.0.0.1:3080/"));
        // An appended restart announcement supersedes the older one.
        let output = format!("{output}dsh web: https://localhost:8443/?token=new\r\n");
        assert_eq!(served_link(&output), Some("https://localhost:8443/?token=new"));
    }

    #[test]
    fn served_link_ignores_unrelated_or_invalid_urls() {
        for output in [
            "Startup failed; see https://docs.example/help",
            "dsh web: opening the default browser",
            "dsh web: not-a-url (LAN: http://192.168.1.5:3080/?token=x)",
            "dsh web: file:///tmp/index.html",
        ] {
            assert_eq!(served_link(output), None, "{output}");
        }
    }

    #[test]
    fn failure_summary_keeps_the_first_error_and_last_lines() {
        let out = "dsh: startup failed: 2 required plugins did not activate\n\nFailed plugins (1):\n  webserver (required)\n    Error: listen EADDRINUSE: address already in use 127.0.0.1:3080\n        at Server.setupListenHandle\n\nFull diagnostics: C:\\x.log\n";
        assert_eq!(
            failure_summary(out),
            "dsh: startup failed: 2 required plugins did not activate; Error: listen EADDRINUSE: address already in use 127.0.0.1:3080; Full diagnostics: C:\\x.log"
        );
        assert_eq!(failure_summary("only line\n"), "only line");
        assert_eq!(failure_summary("\n \n"), "没有任何输出");
    }

    #[test]
    fn a_server_install_counts_as_an_app() {
        let s = Server { program: "node-that-does-not-run".into(), args: vec![], serves: |_| None, name: "web".into(), log: PathBuf::new(), independent: false };
        let mut inst = Install { installed: true, running: true, server: Some(s), ..Default::default() };
        assert!(has_app(&inst));
        // No such process: not running, whatever the detector said.
        set_app_running(&mut inst);
        assert!(!inst.running);
    }

    #[test]
    fn unknown_agents_are_not_detected() {
        let inst = detect("nope");
        assert!(!inst.installed && inst.dir.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn is_exe_ignores_case() {
        assert!(is_exe(Path::new("a/b/OpenCode.EXE")));
        assert!(is_exe(Path::new("droid.exe")));
        assert!(!is_exe(Path::new("opencode.cmd")));
        assert!(!is_exe(Path::new("exe")));
    }

    /// A bundle at `dir` with an Info.plist; `exe` also creates the executable when `real`.
    fn app(dir: &Path, id: Option<&str>, exe: Option<&str>, version: Option<&str>, real: bool) {
        let mut body = String::new();
        if let Some(i) = id {
            body += &format!("<key>CFBundleIdentifier</key><string>{i}</string>");
        }
        if let Some(e) = exe {
            body += &format!("<key>CFBundleExecutable</key><string>{e}</string>");
        }
        if let Some(v) = version {
            body += &format!("<key>CFBundleShortVersionString</key><string>{v}</string>");
        }
        let macos = dir.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(
            dir.join("Contents").join("Info.plist"),
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>{body}<key>CFBundleVersion</key><string>9</string></dict></plist>"),
        )
        .unwrap();
        if let (true, Some(e)) = (real, exe) {
            std::fs::write(macos.join(e), b"").unwrap();
        }
    }

    #[test]
    fn reads_app_bundles() {
        let h = crate::util::TestHome::new("bundles");
        let (apps, user_apps) = (h.0.join("Applications"), h.0.join("home-Applications"));
        // Codex ships as ChatGPT.app now; an old Codex.app can sit next to it.
        let chatgpt = apps.join("ChatGPT.app");
        app(&chatgpt, Some("com.openai.codex"), Some("ChatGPT"), Some("26.920.1"), true);
        app(&apps.join("Codex.app"), Some("com.openai.codex"), Some("Codex"), Some("26.700.0"), true);
        // The chat-only app, renamed or (on an old Mac) still under the name ChatGPT.app.
        app(&apps.join("ChatGPT Classic.app"), Some("com.openai.chat"), Some("ChatGPT"), Some("1.2025"), true);
        app(&user_apps.join("ChatGPT.app"), Some("com.openai.chat"), Some("ChatGPT"), Some("1.2024"), true);
        // Found by name only (no id); no executable on disk; no executable named.
        app(&apps.join("ZCode.app"), None, Some("ZCode"), None, true);
        app(&apps.join("Ghost.app"), Some("x.ghost"), Some("Ghost"), Some("1"), false);
        app(&apps.join("Broken.app"), Some("x.broken"), None, Some("1.0"), true);
        std::fs::write(apps.join("notes.txt"), b"").unwrap();

        let b = bundle_info(&chatgpt).unwrap();
        assert_eq!(b.id.as_deref(), Some("com.openai.codex"));
        assert_eq!(b.exe, chatgpt.join("Contents").join("MacOS").join("ChatGPT"));
        assert_eq!(b.version.as_deref(), Some("26.920.1"));
        // CFBundleVersion stands in for a missing short version.
        assert_eq!(bundle_info(&apps.join("ZCode.app")).unwrap().version.as_deref(), Some("9"));
        assert!(bundle_info(&apps.join("Broken.app")).is_none());
        assert!(bundle_info(&apps.join("Missing.app")).is_none());

        let all = bundles_in(&[apps.clone(), user_apps, h.0.join("nowhere")]);
        let mut names: Vec<String> = all.iter().map(|b| b.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, ["ChatGPT Classic.app", "ChatGPT.app", "ChatGPT.app", "Codex.app", "ZCode.app"]);

        let codex = pick_bundles(&all, CODEX_APP);
        assert_eq!(codex.iter().map(|b| b.version.as_deref().unwrap()).collect::<Vec<_>>(), ["26.920.1", "26.700.0"], "newest first, never the chat app");
        assert_eq!(pick_bundles(&all, &["dev.zcode.app", "zcode.app"]).len(), 1, "names match without case");
        assert!(pick_bundles(&all, &["com.anthropic.claudefordesktop", "Claude.app"]).is_empty());

        let mut inst = Install::default();
        use_copy(&mut inst, &DesktopCopy { exe: codex[0].exe.clone(), version: codex[0].version.clone(), running: false });
        assert!(inst.installed);
        // The bundle, where the helper processes live too.
        assert_eq!(inst.dir.as_deref(), Some(chatgpt.as_path()));
        assert_eq!(bundle_of(&codex[0].exe), Some(chatgpt.as_path()));
    }

    #[test]
    fn finds_claude_desktops_own_claude_code() {
        let h = crate::util::TestHome::new("claude-desktop");
        assert_eq!(desktop_claude_code(&h.0), None);
        let cc = h.0.join("Claude").join("claude-code");
        for v in ["2.1.99", "2.1.246", "2.1.3"] {
            std::fs::create_dir_all(cc.join(v)).unwrap();
        }
        std::fs::create_dir_all(cc.join("tmp")).unwrap();
        std::fs::write(cc.join("9.9.9"), b"").unwrap();
        assert_eq!(desktop_claude_code(&h.0).as_deref(), Some("2.1.246"));
    }

    #[test]
    fn merged_path_keeps_order_and_adds_common_bins() {
        let a = std::env::join_paths(["/opt/homebrew/bin", "/usr/bin"]).unwrap();
        let b = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let merged = merge_paths(&[&a, &b, &OsString::new()], &[PathBuf::from("/Users/me/.local/bin"), PathBuf::from("/bin")]);
        let got: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        let want: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/bin", "/bin", "/Users/me/.local/bin"].map(PathBuf::from).to_vec();
        assert_eq!(got, want);
        assert!(common_bins(Path::new("/Users/me")).contains(&PathBuf::from("/Users/me/.local/bin")));
    }

    #[test]
    fn app_dir_is_the_bundle_or_the_folder() {
        let bundled = Path::new("/Applications/OpenCode.app/Contents/MacOS/OpenCode");
        assert_eq!(app_dir(bundled), Some(PathBuf::from("/Applications/OpenCode.app")));
        assert_eq!(app_dir(Path::new("/opt/x/bin/tool")), Some(PathBuf::from("/opt/x/bin")));
        assert_eq!(bundle_of(Path::new("/opt/x/bin/tool")), None);
    }

    #[test]
    fn cli_names_follow_the_system() {
        if cfg!(windows) {
            assert_eq!(exe("codex"), "codex.exe");
            assert_eq!(native_name("kimi.cmd"), "kimi.cmd");
        } else {
            assert_eq!(exe("codex"), "codex");
            assert_eq!(native_name("kimi.exe"), "kimi");
            assert_eq!(native_name("kimi.cmd"), "kimi");
            assert_eq!(native_name("kimi"), "kimi");
        }
    }

    #[test]
    fn broad_folders_are_no_scope() {
        assert!(scope(Path::new("/Applications")).is_none());
        assert!(scope(Path::new("/usr/local")).is_none());
        assert!(scope(Path::new("/")).is_none());
        #[cfg(unix)]
        assert_eq!(scope(Path::new("/Applications/Codex.app")).as_deref(), Some("\\applications\\codex.app\\"));
    }

    /// Unix npm layout: `<prefix>/bin/x` links to `<prefix>/lib/node_modules/<pkg>/…`.
    #[cfg(unix)]
    #[test]
    fn npm_version_follows_the_shim_link() {
        let h = crate::util::TestHome::new("npm-shim");
        let pkg_dir = h.0.join("lib").join("node_modules").join("@scope").join("tool");
        std::fs::create_dir_all(pkg_dir.join("bin")).unwrap();
        std::fs::write(pkg_dir.join("package.json"), r#"{"name":"@scope/tool","version":"1.2.3"}"#).unwrap();
        std::fs::write(pkg_dir.join("bin").join("cli.js"), "").unwrap();
        std::fs::create_dir_all(h.0.join("bin")).unwrap();
        let shim = h.0.join("bin").join("tool");
        std::os::unix::fs::symlink(pkg_dir.join("bin").join("cli.js"), &shim).unwrap();
        assert_eq!(npm_version_near("@scope/tool", Some(&shim)).as_deref(), Some("1.2.3"));
        assert_eq!(npm_version_near("@scope/other", Some(&shim)), None);
    }

    #[cfg(unix)]
    #[test]
    fn is_exe_needs_the_execute_bit() {
        use std::os::unix::fs::PermissionsExt;
        let h = crate::util::TestHome::new("exec-bit");
        let f = h.0.join("tool");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        assert!(!is_exe(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_exe(&f));
        assert!(!is_exe(&h.0));
    }
}
