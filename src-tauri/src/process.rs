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

/// (install dir, version, package family name, main executable) of the Codex MSIX package.
/// Cached once PowerShell has answered; a probe that failed or timed out is tried again
/// on the next refresh instead of hiding Codex for the rest of the run.
fn codex_package() -> Option<(PathBuf, String, String, Option<PathBuf>)> {
    type Pkg = Option<(PathBuf, String, String, Option<PathBuf>)>;
    static CACHE: std::sync::Mutex<Option<Pkg>> = std::sync::Mutex::new(None);
    if let Some(p) = crate::util::lock(&CACHE).clone() {
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
    *crate::util::lock(&CACHE) = Some(pkg.clone());
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
fn use_copy(inst: &mut Install, c: &DesktopCopy) {
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

/// The running desktop app of `inst` (empty when it has no folder).
fn app_of(sys: &System, inst: &Install) -> Vec<Pid> {
    inst.dir.as_deref().map(|d| app_processes(sys, d, inst.exe.as_deref())).unwrap_or_default()
}

/// Sets `running` from the desktop app's processes. An install without a folder (a CLI)
/// keeps what its detector found.
pub(crate) fn set_app_running(inst: &mut Install) {
    if inst.dir.is_some() {
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

/// Version printed by a CLI (`<exe> --version`), cached per path, killed after 5 s. A .cmd /
/// .bat is started directly too: the standard library runs it through `cmd /d /c` with proper
/// quoting (paths with `&` or parentheses) and without AutoRun hooks.
pub(crate) fn cli_version(exe: &Path) -> Option<String> {
    static CACHE: std::sync::Mutex<Vec<(PathBuf, Option<String>)>> = std::sync::Mutex::new(Vec::new());
    let cache = || crate::util::lock(&CACHE);
    if let Some((_, v)) = cache().iter().find(|(p, _)| p == exe) {
        return v.clone();
    }
    // Lets `output_within` pass on the login PATH (macOS) that an npm script needs.
    search_path();
    let v = output_within(Command::new(exe).arg("--version"), Duration::from_secs(5)).and_then(|o| version_in(&String::from_utf8_lossy(&o.stdout)));
    cache().push((exe.to_path_buf(), v.clone()));
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
    } else if let Some((dir, ver, pfn, main)) = codex_package() {
        inst.installed = true;
        inst.version = Some(ver);
        inst.aumid = Some(format!("{pfn}!App"));
        inst.exe = main;
        inst.dir = Some(dir);
    }
    set_app_running(&mut inst);
    inst
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
/// Windows its uninstall entry's icon is the app. Without the app, the MiMo Code CLI, which
/// reads the same mimocode.jsonc (installer `~/.mimocode/bin/mimo`, npm, Homebrew).
pub(crate) fn detect_mimo() -> Install {
    let mut inst = Install::default();
    if let Some(c) = app_bundles(&["Xiaomi MiMo AI.app", "Xiaomi MiMo.app", "MiMo.app"]).first() {
        use_copy(&mut inst, c);
    } else if let Some(e) = uninstall_entry("Xiaomi MiMo") {
        let exe = e.icon.and_then(|i| unquote_exe(&i));
        inst.installed = exe.is_some();
        inst.version = e.version;
        inst.dir = exe.as_ref().and_then(|x| x.parent().map(Path::to_path_buf));
        inst.exe = exe;
    } else {
        let native = dirs::home_dir().map(|h| h.join(".mimocode").join("bin").join(exe("mimo"))).filter(|p| p.is_file());
        let mut cli = detect_cli(&["mimo.exe", "mimo.cmd"], "@mimo-ai/cli");
        if let (false, Some(p)) = (cli.installed, native) {
            cli.installed = true;
            cli.version = cli_version(&p);
        }
        return cli;
    }
    set_app_running(&mut inst);
    inst
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

/// Ends the desktop app's processes: asked to quit (SIGTERM) where the system has that
/// (macOS), killed otherwise (Windows) or when `force`.
fn end_app(inst: &Install, force: bool) {
    let sys = processes();
    for pid in app_of(&sys, inst) {
        if let Some(p) = sys.process(pid) {
            if force || p.kill_with(Signal::Term) != Some(true) {
                p.kill();
            }
        }
    }
}

/// Ends the desktop app's processes and waits for them to exit; whatever is left after 5 s
/// is killed.
fn stop(inst: &Install, on: &dyn Fn(Progress)) -> Result<()> {
    end_app(inst, false);
    let t0 = Instant::now();
    let mut left_seen = usize::MAX;
    let mut forced = false;
    loop {
        let left = app_of(&processes(), inst).len();
        if left == 0 {
            return Ok(());
        }
        if !forced && t0.elapsed() > Duration::from_secs(5) {
            forced = true;
            end_app(inst, true);
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
        start_exe(exe, args).map_err(|e| anyhow!(tr!("启动失败：{e}", "Failed to start: {e}")))?;
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

#[cfg(test)]
mod any_os_tests {
    use super::*;

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
