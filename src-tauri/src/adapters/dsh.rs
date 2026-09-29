//! DeepSeek Harness (`dsh`, npm `@deepseek-ai/dsh`): everything lives in `$DSH_HOME`
//! (default `~/.dsh`). Each profile (`profiles/<name>/`: `web` for `dsh web`, `desktop` for
//! the desktop app) composes the bundles its `package.json` selects (`dsh.profile.bundles`),
//! then its `cordis.patch.yml`: a YAML list of loader entries that replace a row's whole
//! `config` (`- id: x`), switch it (`disabled`), or add rows (`- insert: [...]`). The home
//! level `$DSH_HOME/cordis.patch.yml` outranks every profile's.
//!
//! Providers are the `providers` of the `llm-pi-ai` row (pi-ai's shape, see pimodels
//! `Flavor::Dsh`), edited in the layer that holds that row (the home one wins), else in the
//! profile picked in AgentPlus. Keys are references (`apiKeyEnv`) into `.credentials.yaml`.
//! Plugins are the optional bundles the installation ships and the ones installed into the
//! profile; switching one edits `dsh.profile.bundles`, as dsh's own plugin page does.
//!
//! [`Patch`] edits a patch file entry by entry: only changed entries are re-emitted, every
//! other line (comments, `!!js` expressions) stays byte for byte.

use super::msg;
use super::pimodels::{Dirty, Flavor, Fmt};
use super::{Endpoint, Plan};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value as J};
use serde_yaml::{Mapping, Value as Y};
use std::path::{Path, PathBuf};

pub const ID: &str = "dsh";
pub const NAME: &str = "DeepSeek Harness";
/// Relative to `$DSH_HOME`: the patch layer of `dsh web`, the profile the CLI starts with.
pub const MARKER: &str = "profiles/web/cordis.patch.yml";
/// The desktop app's own profile.
pub const DESKTOP_MARKER: &str = "profiles/desktop/cordis.patch.yml";
pub const WSL_SCRIPT: &str = "dsh --version 2>/dev/null | head -n 1; true";
pub const WSL_MARKER: &str = ".dsh/profiles/web/cordis.patch.yml";
const PKG: &str = "@deepseek-ai/dsh";
/// The loader row of the multi-provider adapter, and its module.
const PI_AI_ROW: &str = "llm-pi-ai";
const PI_AI_MODULE: &str = "@deepseek-ai/dsh-llm-pi-ai";
/// The native DeepSeek adapter's row and its defaults (dsh-llm-deepseek-api-key).
const DEEPSEEK_ROW: &str = "llm-deepseek";
const DEEPSEEK_KEY_REF: &str = "DEEPSEEK_API_KEY";
const DEEPSEEK_URL: &str = "https://api.deepseek.com/anthropic";
/// The default model row and what the base bundle puts in it.
const DEFAULT_MODEL_ROW: &str = "agent-default-model";
const DEFAULT_MODEL: (&str, &str) = ("deepseek-official", "deepseek-flash");
/// Setting keys: the profile AgentPlus edits and whether the web UI server AgentPlus starts
/// outlives it (both stored in the AgentPlus store), and one switch per plugin.
const PROFILE_SETTING: &str = "profile";
const INDEPENDENT_SETTING: &str = "independent";
const PLUGIN_SETTING: &str = "plugin:";

/// `$DSH_HOME` (Windows side only), else `~/.dsh`.
pub fn default_dir() -> PathBuf {
    if let Some(d) = crate::env::agent_var("DSH_HOME").filter(|d| !d.trim().is_empty()) {
        return crate::env::resolve_path(&d);
    }
    home().join(".dsh")
}

/// The folder picked in AgentPlus, else the default.
pub(crate) fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

/// The home-level patch layer: it outranks every profile's.
pub(crate) fn home_patch_path() -> PathBuf {
    dir().join("cordis.patch.yml")
}

pub(crate) fn credentials_path() -> PathBuf {
    dir().join(".credentials.yaml")
}

fn env_path() -> PathBuf {
    dir().join(".env")
}

/// Profiles that dsh has set up (a folder under `profiles/` with its manifest), sorted.
fn profiles() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir().join("profiles"))
        .map(|rd| rd.flatten().filter(|e| e.path().join("package.json").is_file()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// The profile AgentPlus edits: the one picked in its settings when it still exists, else the
/// desktop app's, else `web`, else the first one there is.
fn profile_in(root: &J, all: &[String]) -> String {
    let picked = store::get_str(root, ID, PROFILE_SETTING).filter(|p| all.contains(p));
    picked
        .or_else(|| ["desktop", "web"].iter().find(|p| all.iter().any(|a| a == *p)).map(|p| p.to_string()))
        .or_else(|| all.first().cloned())
        .unwrap_or_else(|| "web".into())
}

fn profile_dir(profile: &str) -> PathBuf {
    dir().join("profiles").join(profile)
}

/// The patch layer of a profile.
pub(crate) fn profile_patch_path(profile: &str) -> PathBuf {
    profile_dir(profile).join("cordis.patch.yml")
}

/// The patch layer of the profile AgentPlus edits.
pub(crate) fn patch_path() -> PathBuf {
    profile_patch_path(&profile_in(&store::load(), &profiles()))
}

// ---------------------------------------------------------------- patch files

/// A cordis patch file: a top-level YAML list of loader entries. Entries are edited one by
/// one; writing re-emits only the changed ones, so comments and `!!js` expressions elsewhere
/// survive. An entry that has comments, anchors or tags of its own can't be changed.
pub(crate) struct Patch {
    pub path: PathBuf,
    lines: Vec<String>,
    meta: TextMeta,
    items: Vec<Item>,
    /// The entries aren't all block-style `- ` items (a flow list): read-only.
    flow: bool,
    /// `!!js` environment references (`process.env.X`) don't block edits: the caller writes
    /// them back as `!!js` itself (see [`Patch::allow_env_js`]).
    env_js: bool,
}

struct Item {
    /// Lines [start, end) in the file; None for an added entry.
    span: Option<(usize, usize)>,
    value: Y,
    changed: bool,
    removed: bool,
}

/// A line with its `!!js` environment references (`!!js process.env.X`, or a template literal
/// of them) written as plain scalars, for the editability check.
fn env_js_plain(line: &str) -> String {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let r = R.get_or_init(|| {
        // A template literal whose only substitutions are `${process.env.X}`.
        let env = r"\$\{process\.env\.[A-Za-z_][A-Za-z0-9_]*\}";
        let tpl = format!(r#"`(?:[^`$'"\\]|\$[^{{`]|{env})*`"#);
        regex::Regex::new(&format!(r#"!!js\s+(process\.env\.[A-Za-z_][A-Za-z0-9_]*|'{tpl}'|"{tpl}")\s*$"#)).unwrap()
    });
    r.replace(line, "$1").into_owned()
}

/// The `id` of a loader entry.
pub(crate) fn row_id(v: &Y) -> Option<&str> {
    v.get("id").and_then(|x| x.as_str())
}

/// The rows an `- insert: [...]` entry adds.
pub(crate) fn inserted(v: &Y) -> Option<&Vec<Y>> {
    v.get("insert").and_then(|x| x.as_sequence())
}

impl Patch {
    /// Reads a patch file; a missing or comments-only one has no entries.
    pub fn load(path: &Path) -> Result<Patch> {
        let (text, meta) = read_text_or_new(path)?;
        let name = display_path(path);
        let doc: Y = if text.trim().is_empty() { Y::Null } else { serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("Failed to parse {name}: {e}", "{name} 解析失败：{e}")))? };
        let values = match doc {
            Y::Null => vec![],
            Y::Sequence(s) => s,
            _ => return Err(anyhow!(tr!("The top level of {name} is not a list", "{name} 顶层不是列表"))),
        };
        let lines: Vec<String> = if text.is_empty() { vec![] } else { text.split('\n').map(String::from).collect() };
        let starts: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| *l == "-" || l.starts_with("- ")).map(|(i, _)| i).collect();
        let content = |l: &&String| !l.trim().is_empty() && !l.trim_start().starts_with('#') && l.trim() != "---";
        let flow = if values.is_empty() { lines.iter().filter(content).any(|l| l.trim() != "[]") } else { starts.len() != values.len() };
        let items = values
            .into_iter()
            .enumerate()
            .map(|(k, value)| {
                let span = (!flow).then(|| {
                    let start = starts[k];
                    let mut end = starts.get(k + 1).copied().unwrap_or(lines.len());
                    // Trailing blank lines and column-0 comments belong to the gap after the entry.
                    while end > start + 1 && (lines[end - 1].trim().is_empty() || lines[end - 1].starts_with('#')) {
                        end -= 1;
                    }
                    (start, end)
                });
                Item { span, value, changed: false, removed: false }
            })
            .collect();
        Ok(Patch { path: path.to_path_buf(), lines, meta, items, flow, env_js: false })
    }

    pub fn file(&self) -> String {
        display_path(&self.path)
    }

    /// The entries still in the file, with their indexes.
    pub fn entries(&self) -> impl Iterator<Item = (usize, &Y)> {
        self.items.iter().enumerate().filter(|(_, i)| !i.removed).map(|(k, i)| (k, &i.value))
    }

    pub fn get(&self, i: usize) -> &Y {
        &self.items[i].value
    }

    /// The last entry overriding row `id` (the one that wins).
    pub fn find(&self, id: &str) -> Option<usize> {
        self.entries().filter(|(_, v)| inserted(v).is_none() && row_id(v) == Some(id)).map(|(k, _)| k).last()
    }

    /// Lets entries whose only `!!js` expressions read environment variables be rewritten. The
    /// caller promises to put those back as `!!js` scalars in every value it sets (serde_yaml
    /// reads them as plain strings); the MCP codec does, the provider code doesn't.
    pub fn allow_env_js(&mut self) {
        self.env_js = true;
    }

    /// The `insert` entry that adds row `id`, and the row's position in it.
    #[allow(dead_code)] // unused so far
    pub fn find_inserted(&self, id: &str) -> Option<(usize, usize)> {
        self.entries().find_map(|(k, v)| inserted(v)?.iter().position(|r| row_id(r) == Some(id)).map(|j| (k, j)))
    }

    /// Whether entry `i` can be rewritten (nothing in its lines would be lost).
    pub fn check_editable(&self, i: usize) -> Result<()> {
        if self.flow {
            return Err(anyhow!(tr!("{} is written as a flow list ([...]); not writing it", "{} 是流式列表（[...]），不写入", self.file())));
        }
        if let Some((s, e)) = self.items[i].span {
            let kept: Vec<String> = self.lines[s..e].iter().map(|l| if self.env_js { env_js_plain(l) } else { l.clone() }).collect();
            let lines: Vec<&str> = kept.iter().map(String::as_str).collect();
            if super::hermes::has_extras(&lines) {
                let id = row_id(&self.items[i].value).unwrap_or("?");
                return Err(anyhow!(tr!(
                    "The {id} entry in {} has comments, anchors or !!js expressions; not writing it so they aren't lost",
                    "{} 里的 {id} 条目含注释、锚点或 !!js 表达式，为避免丢失不写入",
                    self.file()
                )));
            }
        }
        Ok(())
    }

    pub fn set(&mut self, i: usize, v: Y) -> Result<()> {
        if self.items[i].value != v {
            self.check_editable(i)?;
            self.items[i].value = v;
            self.items[i].changed = true;
        }
        Ok(())
    }

    /// Adds an entry at the end; returns its index.
    pub fn push(&mut self, v: Y) -> Result<usize> {
        if self.flow {
            return Err(anyhow!(tr!("{} is written as a flow list ([...]); not writing it", "{} 是流式列表（[...]），不写入", self.file())));
        }
        self.items.push(Item { span: None, value: v, changed: true, removed: false });
        Ok(self.items.len() - 1)
    }

    pub fn remove(&mut self, i: usize) -> Result<()> {
        self.check_editable(i)?;
        self.items[i].removed = true;
        Ok(())
    }

    /// Switches row `id` on or off through `disabled` on its last override, adding an
    /// override when there is none (as dsh's plugin manager does: a lower layer may switch the
    /// row off, so an explicit `disabled: false` stays). True when it changed.
    #[allow(dead_code)] // for the MCP codec (mcp/)
    pub fn set_disabled(&mut self, id: &str, disabled: bool) -> Result<bool> {
        match self.find(id) {
            Some(i) => {
                let mut v = self.get(i).clone();
                if v.get("disabled").and_then(|d| d.as_bool()) == Some(disabled) {
                    return Ok(false);
                }
                let m = v.as_mapping_mut().ok_or_else(|| anyhow!(tr!("The {id} entry is not a mapping", "{id} 条目不是映射")))?;
                m.insert(Y::from("disabled"), Y::Bool(disabled));
                self.set(i, v)?;
            }
            None => {
                let mut m = Mapping::new();
                m.insert(Y::from("id"), Y::from(id));
                m.insert(Y::from("disabled"), Y::Bool(disabled));
                self.push(Y::Mapping(m))?;
            }
        }
        Ok(true)
    }

    pub fn changed(&self) -> bool {
        self.items.iter().any(|i| i.changed || i.removed)
    }

    /// The file with the edits applied, checked by parsing it back.
    pub fn render(&self) -> Result<String> {
        if self.flow {
            return Err(anyhow!(tr!("{} is written as a flow list ([...]); not writing it", "{} 是流式列表（[...]），不写入", self.file())));
        }
        let emit = |v: &Y| -> Result<Vec<String>> {
            let mut out = vec![];
            super::hermes::emit_seq(&mut out, 0, std::slice::from_ref(v))?;
            Ok(out)
        };
        let live: Vec<&Y> = self.entries().map(|(_, v)| v).collect();
        let mut out: Vec<String> = vec![];
        let starts: Vec<usize> = self.items.iter().filter_map(|i| i.span.map(|s| s.0)).collect();
        let first = starts.first().copied().unwrap_or(self.lines.len());
        // The header, less an empty `[]` once there is an entry.
        out.extend(self.lines[..first].iter().filter(|l| live.is_empty() || l.trim() != "[]").cloned());
        let mut next = starts.iter().skip(1).copied().chain(std::iter::once(self.lines.len()));
        for it in self.items.iter().filter(|i| i.span.is_some()) {
            let (s, e) = it.span.unwrap();
            let gap_end = next.next().unwrap_or(self.lines.len());
            if !it.removed {
                if it.changed {
                    out.extend(emit(&it.value)?);
                } else {
                    out.extend(self.lines[s..e].iter().cloned());
                }
            }
            out.extend(self.lines[e..gap_end].iter().cloned());
        }
        let added: Vec<&Item> = self.items.iter().filter(|i| i.span.is_none() && !i.removed).collect();
        if !added.is_empty() {
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            for it in added {
                out.extend(emit(&it.value)?);
            }
            out.push(String::new());
        }
        // dsh refuses an empty or comments-only patch file.
        if live.is_empty() && !out.iter().any(|l| l.trim() == "[]") {
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            out.push("[]".into());
            out.push(String::new());
        }
        let text = out.join("\n");
        let back: Y = serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("The generated YAML can't be parsed: {e}", "生成的 YAML 无法解析：{e}")))?;
        let expected = super::hermes::untag_js(&Y::Sequence(live.into_iter().cloned().collect()));
        if back != expected {
            return Err(anyhow!(tr!("The rewritten {} failed verification; not writing it", "改写后的 {} 校验失败，不写入", self.file())));
        }
        Ok(text)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        write_text_atomic(&self.path, &self.render()?, self.meta)
    }
}

// ---------------------------------------------------------------- credentials

/// `.credentials.yaml` as JSON (`{ version, refs: { NAME: key }, records: {...} }`); a missing
/// file is an empty version-1 document.
pub(crate) fn load_credentials(path: &Path) -> Result<(J, TextMeta)> {
    let (text, meta) = read_text_or_new(path)?;
    if text.trim().is_empty() {
        return Ok((json!({ "version": 1 }), meta));
    }
    let y: Y = serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("Failed to parse {}: {e}", "{} 解析失败：{e}", display_path(path))))?;
    let j = serde_json::to_value(&y)?;
    if !j.is_object() {
        return Err(anyhow!(tr!("The top level of {} is not a mapping", "{} 顶层不是映射", display_path(path))));
    }
    Ok((j, meta))
}

/// dsh refuses to load a credentials file others can read (POSIX), so it stays owner-only.
fn save_credentials(path: &Path, creds: &J, meta: TextMeta) -> Result<()> {
    let mut text = serde_yaml::to_string(creds)?;
    if meta.crlf {
        text = text.replace('\n', "\r\n");
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    write_private_atomic(path, text.as_bytes())
}

// ---------------------------------------------------------------- detection

/// The installed `@deepseek-ai/dsh` package folder.
fn install_dir() -> Option<PathBuf> {
    let shim = crate::process::on_path(&["dsh.cmd", "dsh.exe", "dsh"]);
    crate::process::npm_global_package(PKG)
        .or_else(|| shim.as_deref().and_then(Path::parent).map(|d| crate::process::npm_package_in(d, PKG)).filter(|p| p.is_file()))
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// The npm package (another CLI is also called `dsh`, so a bare `dsh` on PATH only counts
/// together with a dsh home), else the desktop app or its profile (it bundles its own
/// runtime). What Start / Restart act on follows the profile AgentPlus edits: the desktop
/// app for `desktop`, else the web UI server of that profile (see [`server`]).
pub fn detect() -> Install {
    let mut inst = Install::default();
    let shim = crate::process::on_path(&["dsh.cmd", "dsh.exe", "dsh"]);
    if let Some(v) = crate::process::npm_version_near(PKG, shim.as_deref()) {
        inst.installed = true;
        inst.version = Some(v);
    } else if (shim.is_some() && default_dir().join("profiles").is_dir()) || default_dir().join(DESKTOP_MARKER).is_file() {
        inst.installed = true;
    }
    let profile = profile_in(&store::load(), &profiles());
    if profile == DESKTOP_PROFILE {
        if let Some(c) = crate::process::detect_dsh_desktop() {
            crate::process::use_copy(&mut inst, &c);
        }
    } else {
        inst.server = server(&profile);
    }
    crate::process::set_app_running(&mut inst);
    inst
}

/// The profile only the desktop app runs (the CLI refuses it).
const DESKTOP_PROFILE: &str = "desktop";
/// The bundle that makes a profile serve the browser UI (`dsh web` and profiles made from it).
const WEB_APP_BUNDLE: &str = "@deepseek-ai/dsh-web-app";

/// `dsh --profile <profile>` as a background server, when the profile serves the web UI and
/// the npm package and node are there to run it. Started with node directly, so the process
/// AgentPlus tracks is the server itself, not a shim in front of it.
fn server(profile: &str) -> Option<crate::process::Server> {
    let manifest = read_pkg(&profile_dir(profile).join("package.json"))?;
    if !selected(&manifest).iter().any(|b| b == WEB_APP_BUNDLE) {
        return None;
    }
    let bin = install_dir()?.join("lib").join("bin.js");
    let node = crate::process::on_path(&["node.exe"])?;
    bin.is_file().then(|| crate::process::Server {
        program: node,
        args: vec![bin.to_string_lossy().to_string(), "--profile".into(), profile.into()],
        serves: served_profile,
        name: profile.into(),
        log: crate::applog::dir().join(format!("dsh-{profile}.log")),
        independent: independent(&store::load()),
    })
}

/// Whether the web UI server AgentPlus starts keeps running after AgentPlus quits.
fn independent(root: &J) -> bool {
    store::get_flag(root, ID, INDEPENDENT_SETTING)
}

fn independent_setting(on: bool) -> Setting {
    Setting {
        key: INDEPENDENT_SETTING.into(),
        group: "AgentPlus".into(),
        label: l("Start as a separate process", "使用独立进程启动").into(),
        desc: l(
            "On: the web UI server started from AgentPlus keeps running after AgentPlus quits; stop it next to \"Launched\" on the right. Off: it stops when AgentPlus quits.",
            "打开后，从 AgentPlus 启动的网页服务在 AgentPlus 退出后继续运行，可以在右侧「启动方式」旁停止它；关闭时它随 AgentPlus 一起退出。",
        )
        .into(),
        kind: "bool".into(),
        value: J::from(on),
        options: vec![],
        hints: vec![],
        excludes: vec![],
    }
}

/// The running web UI of the edited profile, and a note when the link can't sign the browser
/// in: when AgentPlus started the server, its log has the launch token. Otherwise the plain
/// address relies on the cookie an earlier visit left (dsh keeps it across restarts).
pub(crate) fn web_link() -> Result<(String, Option<String>)> {
    let inst = detect();
    let Some(s) = &inst.server else { return Err(anyhow!(l("This profile has no web UI", "这个 profile 没有网页界面"))) };
    let not_running = || anyhow!(tr!("{} isn't running; start it first", "{} 没在运行，先启动它", NAME));
    if !inst.running {
        return Err(not_running());
    }
    let (ours, argv) = crate::process::server_launch(ID, &inst).ok_or_else(not_running)?;
    if ours {
        let log = std::fs::read_to_string(&s.log).unwrap_or_default();
        // Only a link that still carries its token (the log may have been cleared since).
        if let Some(link) = crate::process::served_link(&log).filter(|l| l.contains(['?', '#'])) {
            return Ok((link.to_string(), None));
        }
    }
    let link = web_address(&argv, &s.name);
    let note = if ours {
        tr!(
            "Opened {link}. Its sign-in link isn't in the server's log: if the page says unauthorized, restart it here.",
            "已打开 {link}。服务日志里没有找到登录链接，页面提示未授权时在这里重启一次即可。"
        )
    } else {
        tr!(
            "Opened {link}. It wasn't started by AgentPlus: if the page says unauthorized, restart it here.",
            "已打开 {link}。它不是 AgentPlus 启动的，页面提示未授权时在这里重启一次即可。"
        )
    };
    Ok((link, Some(note)))
}

/// The plain address a server of `profile` listens on: `--host` / `--port` on its command
/// line, else the `webserver` row of the patch layers (home first), else dsh's defaults. A
/// server bound to every interface is reached on loopback.
fn web_address(argv: &[String], profile: &str) -> String {
    let flag = |name: &str| {
        let eq = format!("{name}=");
        argv.iter().enumerate().find_map(|(i, a)| if a == name { argv.get(i + 1).cloned() } else { a.strip_prefix(&eq).map(String::from) })
    };
    let row = [home_patch_path(), profile_patch_path(profile)].iter().find_map(|p| {
        let patch = Patch::load(p).ok()?;
        let i = patch.find("webserver")?;
        patch.get(i).get("config").cloned()
    });
    // `!!js` expressions (the bundle's own) load as their source text: anything that isn't a
    // host name or address reads as absent.
    let cfg = |k: &str| row.as_ref().and_then(|c| c.get(k)).cloned();
    let is_host = |h: &String| h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c));
    let host = flag("--host").or_else(|| cfg("host").and_then(|h| h.as_str().map(String::from))).filter(is_host).unwrap_or_else(|| "127.0.0.1".into());
    let port = flag("--port").and_then(|p| p.parse::<u16>().ok()).or_else(|| cfg("port").and_then(|p| p.as_u64()).and_then(|p| u16::try_from(p).ok())).filter(|p| *p != 0).unwrap_or(3080);
    let host = match host.as_str() {
        "" | "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "[::1]".to_string(),
        h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
        h => h.to_string(),
    };
    format!("http://{host}:{port}/")
}

/// The profile a `dsh` command line (argv of its node process) boots, wherever the package
/// is (npm global, npx cache): `dsh web`, `dsh --profile web`, `--profile=web`, after any
/// `--patch` overlays. None for other commands (config dumps, `dsh plugin`) and other programs.
fn served_profile(argv: &[String]) -> Option<String> {
    let bin = argv.iter().position(|a| a.replace('\\', "/").to_ascii_lowercase().ends_with("/@deepseek-ai/dsh/lib/bin.js"))?;
    let mut args = argv[bin + 1..].iter();
    let mut profile: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = Some(args.next()?.clone()),
            "--patch" | "--from-default-profile" => {
                args.next();
            }
            "--dump-config" | "--dump-default-config" | "--dump-config-schema" | "-h" | "--help" | "-V" | "--version" => return None,
            s if s.starts_with("--profile=") => profile = Some(s["--profile=".len()..].to_string()),
            s if s.starts_with("--patch=") || s.starts_with("--from-default-profile=") => {}
            // The first other word is the profile (`dsh web`), or starts the app's own arguments.
            s if !s.starts_with('-') && profile.is_none() => return (s != "plugin").then(|| s.to_string()),
            _ => break,
        }
    }
    profile
}

// ---------------------------------------------------------------- plugins

/// A bundle the person can switch: shipped with dsh for them to turn on, or installed into
/// the profile.
#[derive(Debug, PartialEq)]
struct Plugin {
    name: String,
    title: Option<String>,
    desc: Option<String>,
    enabled: bool,
}

fn read_pkg(p: &Path) -> Option<J> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn is_bundle(pkg: &J) -> bool {
    pkg.pointer("/dsh/bundle").is_some()
}

/// A bundle package's display title and description in the UI language (`locale/<lang>.json`
/// `meta`), else its package description.
fn plugin_meta(dir: &Path, pkg: &J) -> (Option<String>, Option<String>) {
    let lang = if crate::i18n::is_en() { "en" } else { "zh" };
    let meta = [lang, "en"].iter().find_map(|l| read_pkg(&dir.join("locale").join(format!("{l}.json")))).and_then(|v| v.get("meta").cloned());
    let get = |k: &str| meta.as_ref().and_then(|m| m.get(k)).and_then(|x| x.as_str()).map(String::from).filter(|s| !s.is_empty());
    (get("title"), get("description").or_else(|| pkg.get("description").and_then(|d| d.as_str()).map(String::from)))
}

/// The bundles the installation ships switched off (dsh's `OPTIONAL_BUNDLES`: a bundle with an
/// icon and locale metadata), from wherever npm placed its dependencies.
fn optional_bundles(install: &Path) -> Vec<(String, PathBuf, J)> {
    let scopes = [install.join("node_modules").join("@deepseek-ai"), install.parent().map(Path::to_path_buf).unwrap_or_default()];
    let mut out: Vec<(String, PathBuf, J)> = vec![];
    for scope in scopes {
        let Ok(rd) = std::fs::read_dir(&scope) else { continue };
        for e in rd.flatten() {
            let d = e.path();
            let Some(pkg) = read_pkg(&d.join("package.json")) else { continue };
            let Some(name) = pkg.get("name").and_then(|n| n.as_str()).map(String::from) else { continue };
            if is_bundle(&pkg) && d.join("icon.svg").is_file() && d.join("locale").is_dir() && !out.iter().any(|o| o.0 == name) {
                out.push((name, d, pkg));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `dsh.profile.bundles` of a profile manifest.
fn selected(manifest: &J) -> Vec<String> {
    manifest.pointer("/dsh/profile/bundles").and_then(|b| b.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// The profile's switchable bundles: the optional ones the installation ships, then the
/// bundles installed into the profile.
fn plugins(profile: &str, install: Option<&Path>) -> Vec<Plugin> {
    let pdir = profile_dir(profile);
    let manifest = read_pkg(&pdir.join("package.json")).unwrap_or(J::Null);
    let on = selected(&manifest);
    let mut out: Vec<Plugin> = vec![];
    for (name, d, pkg) in install.map(optional_bundles).unwrap_or_default() {
        let (title, desc) = plugin_meta(&d, &pkg);
        out.push(Plugin { enabled: on.contains(&name), name, title, desc });
    }
    let deps = manifest.get("dependencies").and_then(|d| d.as_object()).map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    for name in deps {
        let d = name.split('/').fold(pdir.join("node_modules"), |d, p| d.join(p));
        let Some(pkg) = read_pkg(&d.join("package.json")) else { continue };
        if is_bundle(&pkg) && !out.iter().any(|p| p.name == name) {
            let (title, desc) = plugin_meta(&d, &pkg);
            out.push(Plugin { enabled: on.contains(&name), name, title, desc });
        }
    }
    out
}

// ---------------------------------------------------------------- providers

fn fmt(path: PathBuf) -> Fmt {
    Fmt { agent: ID, flavor: Flavor::Dsh, path, ptr: "/providers", auth: Some(credentials_path()), env_file: Some(env_path()) }
}

/// The patch file that holds the providers, with the index of its `llm-pi-ai` row: the home
/// layer when it overrides that row, else the profile's.
fn provider_layer(profile: &str) -> Result<(Patch, Option<usize>)> {
    let home = Patch::load(&home_patch_path())?;
    if let Some(i) = home.find(PI_AI_ROW) {
        return Ok((home, Some(i)));
    }
    let p = Patch::load(&profile_patch_path(profile))?;
    let i = p.find(PI_AI_ROW);
    Ok((p, i))
}

/// A row's `config` as JSON (`{}` when it has none).
fn row_config(patch: &Patch, i: Option<usize>) -> J {
    i.and_then(|i| patch.get(i).get("config")).map(|c| serde_json::to_value(c).unwrap_or(J::Null)).filter(|c| c.is_object()).unwrap_or_else(|| json!({}))
}

/// `config` of the winning override of row `id`: the home layer, else the profile's.
fn effective_config(profile: &str, id: &str) -> Option<J> {
    [home_patch_path(), profile_patch_path(profile)].iter().find_map(|p| {
        let patch = Patch::load(p).ok()?;
        let i = patch.find(id)?;
        patch.get(i).get("config").and_then(|c| serde_json::to_value(c).ok())
    })
}

fn default_model(profile: &str) -> String {
    let cfg = effective_config(profile, DEFAULT_MODEL_ROW);
    let get = |k: &str| cfg.as_ref().and_then(|c| c.get(k)).and_then(|x| x.as_str()).map(String::from);
    format!("{}/{}", get("provider").unwrap_or_else(|| DEFAULT_MODEL.0.into()), get("model").unwrap_or_else(|| DEFAULT_MODEL.1.into()))
}

/// Read-only cards for what dsh serves itself: its native DeepSeek route, and sign-ins
/// stored in `.credentials.yaml` (`records` "llm-pi-ai/<provider>").
fn builtin_cards(f: &Fmt, profile: &str, known: &[Provider]) -> Vec<Provider> {
    let cfg = effective_config(profile, DEEPSEEK_ROW).unwrap_or_else(|| json!({}));
    let get = |k: &str| cfg.get(k).and_then(|x| x.as_str()).map(String::from).filter(|s| !s.is_empty());
    let key_ref = get("apiKeyEnv").unwrap_or_else(|| DEEPSEEK_KEY_REF.into());
    let url = get("baseURL").unwrap_or_else(|| DEEPSEEK_URL.into());
    let mut out = vec![Provider {
        has_key: f.resolve(&json!(key_ref)).is_some(),
        ..Provider::builtin(
            "deepseek-official",
            "DeepSeek",
            host_of(&url),
            "anthropic",
            l("Built-in", "内置"),
            vec![
                Kv::mono(lbl::config_id(), DEEPSEEK_ROW),
                Kv::mono(lbl::base_url(), url.clone()),
                Kv::text(lbl::api_key(), f.key_ref_desc(&key_ref)),
                Kv::text(lbl::note(), l("dsh's own DeepSeek adapter. Set its key in dsh: Settings → Models.", "dsh 自带的 DeepSeek 适配器，密钥在 dsh 的「设置 → 模型」里填写。")),
            ],
        )
    }];
    let (creds, _) = f.load_auth().unwrap_or((J::Null, TextMeta::NEW));
    for (k, _) in creds.get("records").and_then(|r| r.as_object()).into_iter().flatten() {
        let Some(id) = k.strip_prefix("llm-pi-ai/") else { continue };
        if known.iter().chain(out.iter()).any(|p| p.id == id) {
            continue;
        }
        out.push(Provider::builtin(
            id,
            id,
            l("Account sign-in (dsh: Settings → Models)", "账号登录（dsh：设置 → 模型）"),
            "chat",
            l("Built-in", "内置"),
            vec![Kv::mono(lbl::credentials(), format!(".credentials.yaml · records.{k}"))],
        ));
    }
    out
}

fn profile_setting(all: &[String], current: &str) -> Setting {
    Setting {
        key: PROFILE_SETTING.into(),
        group: "AgentPlus".into(),
        label: l("Profile to manage", "管理哪个 profile").into(),
        desc: l(
            "Each dsh profile (web for dsh web, desktop for the desktop app) keeps its own providers and plugins.",
            "dsh 的每个 profile（dsh web 用 web，桌面版用 desktop）各有一套供应商和插件。",
        )
        .into(),
        kind: "select".into(),
        value: J::from(current),
        options: all.to_vec(),
        hints: all.iter().map(|p| display_path(&profile_dir(p))).collect(),
        excludes: vec![],
    }
}

/// The plugins of the profile AgentPlus edits, for the Plugins tab. Switching one is an
/// `Op::SetSetting` on `plugin_key(name)` (see `plan`).
pub(crate) fn plugin_list() -> Vec<PluginInfo> {
    let profile = profile_in(&store::load(), &profiles());
    let install = install_dir();
    plugins(&profile, install.as_deref())
        .into_iter()
        .map(|p| PluginInfo { name: p.title.clone().unwrap_or_else(|| p.name.clone()), description: p.desc, source: Some(tr!("profile {profile}", "profile {profile}")), enabled: p.enabled, id: p.name, ..Default::default() })
        .collect()
}

/// The setting key that switches plugin `name`.
pub(crate) fn plugin_key(name: &str) -> String {
    format!("{PLUGIN_SETTING}{name}")
}

pub fn state(inst: &Install) -> AgentState {
    let root = store::load();
    let all = profiles();
    let profile = profile_in(&root, &all);
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&profile_patch_path(&profile)), display_path(&credentials_path())]);
    if all.is_empty() {
        st.fail(l("No dsh profile yet. Run dsh once (for example dsh web) so it sets one up.", "还没有 dsh 的 profile。先运行一次 dsh（比如 dsh web），让它创建 profile。"));
        return st;
    }
    st.notes.push(tr!(
        "Editing the {profile} profile. dsh web picks up changes right away; other profiles on their next start.",
        "正在管理 {profile} profile。dsh web 会立刻读取改动，其他 profile 下次启动时生效。"
    ));
    let (patch, row) = match provider_layer(&profile) {
        Ok(x) => x,
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    if patch.path == home_patch_path() {
        st.files.insert(0, patch.file());
        st.notes.push(tr!("Providers are set in {}, which overrides every profile, so they're edited there.", "供应商写在 {} 里，它会覆盖所有 profile，所以改动也写在那里。", patch.file()));
    }
    if let Some(i) = row {
        if let Err(e) = patch.check_editable(i) {
            st.fail(e);
        }
        if patch.get(i).get("disabled").and_then(|d| d.as_bool()) == Some(true) {
            st.notes.push(l("The llm-pi-ai adapter is switched off (disabled: true), so these providers aren't used.", "llm-pi-ai 适配器被关掉了（disabled: true），这些供应商不会被使用。").into());
        }
    }
    let f = fmt(patch.path.clone());
    let cfg = row_config(&patch, row);
    st.providers = f.providers(&cfg, &root, None);
    let cards = builtin_cards(&f, &profile, &st.providers);
    st.providers.extend(cards);
    st.current = f.summary(&st.providers, default_model(&profile), Some(Kv::mono(l("Profile", "Profile"), profile.clone())));
    if all.len() > 1 {
        st.settings.push(profile_setting(&all, &profile));
    }
    if inst.server.is_some() {
        st.settings.push(independent_setting(independent(&root)));
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let profile = profile_in(&store::load(), &profiles());
    let (patch, row) = provider_layer(&profile)?;
    let cfg = row_config(&patch, row);
    fmt(patch.path.clone()).endpoint(id, &cfg, &store::load())
}

/// Switches plugin `name` in the profile manifest: enabling appends it (dsh's order: later
/// bundles win), disabling keeps it installed.
fn set_plugin(manifest: &mut J, name: &str, on: bool) -> Result<bool> {
    let bundles = obj_at(manifest, &["dsh", "profile"])?.entry("bundles").or_insert_with(|| json!([]));
    let list = bundles.as_array_mut().ok_or_else(|| anyhow!(l("dsh.profile.bundles is not a list", "dsh.profile.bundles 不是列表")))?;
    let had = list.iter().any(|x| x.as_str() == Some(name));
    match (on, had) {
        (true, false) => list.push(json!(name)),
        (false, true) => list.retain(|x| x.as_str() != Some(name)),
        _ => return Ok(false),
    }
    Ok(true)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let mut root = store::load();
    let all = profiles();
    if all.is_empty() {
        return Err(anyhow!(l("No dsh profile yet. Run dsh once (for example dsh web) so it sets one up.", "还没有 dsh 的 profile。先运行一次 dsh（比如 dsh web），让它创建 profile。")));
    }
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();

    // The profile first: every other op edits the profile it names.
    for op in ops {
        if let Op::SetSetting { key, value } = op {
            if key == PROFILE_SETTING {
                let v = value.as_str().unwrap_or("");
                if !all.iter().any(|p| p == v) {
                    return Err(anyhow!(tr!("No dsh profile named {v}", "没有名为 {v} 的 dsh profile")));
                }
                if profile_in(&root, &all) != v {
                    store::set_str(&mut root, ID, PROFILE_SETTING, v);
                    diff.push(l("AgentPlus settings", "AgentPlus 设置"), &tr!("DeepSeek Harness profile → {v}", "DeepSeek Harness profile → {v}"), true);
                    dirty.store = true;
                }
            }
        }
    }
    let profile = profile_in(&root, &all);
    let (mut patch, row) = provider_layer(&profile)?;
    let f = fmt(patch.path.clone());
    let mut cfg = row_config(&patch, row);
    let mut creds = f.load_auth();
    let manifest_path = profile_dir(&profile).join("package.json");
    let mut manifest: Option<(J, TextMeta)> = None;

    for op in ops {
        if f.apply(op, &mut cfg, &mut root, &mut creds, &mut diff, &mut dirty)? {
            continue;
        }
        match op {
            Op::SetSetting { key, .. } if key == PROFILE_SETTING => {}
            Op::SetSetting { key, value } if key == INDEPENDENT_SETTING => {
                let on = value.as_bool().unwrap_or(false);
                if independent(&root) != on {
                    store::set_flag(&mut root, ID, INDEPENDENT_SETTING, on);
                    let line = if on { l("Web UI server keeps running after AgentPlus quits", "网页服务在 AgentPlus 退出后继续运行") } else { l("Web UI server stops when AgentPlus quits", "网页服务随 AgentPlus 一起退出") };
                    diff.push(l("AgentPlus settings", "AgentPlus 设置"), line, on);
                    dirty.store = true;
                }
            }
            Op::SetSetting { key, value } if key.starts_with(PLUGIN_SETTING) => {
                let name = &key[PLUGIN_SETTING.len()..];
                if manifest.is_none() {
                    manifest = Some(read_json_object(&manifest_path)?);
                }
                let (m, _) = manifest.as_mut().unwrap();
                let on = value.as_bool().unwrap_or(false);
                if set_plugin(m, name, on)? {
                    let file = display_path(&manifest_path);
                    let line = if on { format!("dsh.profile.bundles + \"{name}\"") } else { tr!("dsh.profile.bundles - \"{name}\" (stays installed)", "dsh.profile.bundles - \"{name}\"（保留安装）") };
                    diff.push(&file, line, on);
                }
            }
            Op::SetCurrentProvider { .. } => {
                return Err(anyhow!(l(
                    "DeepSeek Harness can use several providers at once: manage them by enabling/disabling, and pick the model in dsh.",
                    "DeepSeek Harness 可以同时用多个供应商：按启用/停用管理，在 dsh 里选择模型"
                )))
            }
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
            Op::SetSetting { key, .. } => return Err(msg::unknown_setting(key)),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            // Provider / model ops (pimodels); MCP ops are planned in mcp::write.
            _ => unreachable!("handled by pimodels or mcp::write"),
        }
    }

    if dirty.cfg {
        let config = serde_yaml::to_value(&cfg)?;
        match row {
            Some(i) => {
                let mut v = patch.get(i).clone();
                v.as_mapping_mut().ok_or_else(|| anyhow!(tr!("The {PI_AI_ROW} entry is not a mapping", "{PI_AI_ROW} 条目不是映射")))?.insert(Y::from("config"), config);
                patch.set(i, v)?;
            }
            None => {
                let mut m = Mapping::new();
                m.insert(Y::from("id"), Y::from(PI_AI_ROW));
                m.insert(Y::from("name"), Y::from(PI_AI_MODULE));
                m.insert(Y::from("config"), config);
                patch.push(Y::Mapping(m))?;
            }
        }
        // Checked before anything is written.
        patch.render()?;
    }
    let plugins_dirty = manifest.is_some() && diff.groups.iter().any(|g| g.file == display_path(&manifest_path));

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (dirty.cfg || dirty.auth || plugins_dirty) {
        let mut targets = vec![];
        if dirty.auth {
            targets.push(credentials_path());
        }
        if dirty.cfg {
            targets.push(patch.path.clone());
        }
        if plugins_dirty {
            targets.push(manifest_path.clone());
        }
        backup_dir = Some(backup(ID, &targets)?);
        // The key before the route that references it, so dsh never sees a route without its key.
        if let (true, Some((c, meta))) = (dirty.auth, &creds) {
            save_credentials(&credentials_path(), c, *meta)?;
            written.push(credentials_path());
        }
        if dirty.cfg {
            patch.save()?;
            written.push(patch.path.clone());
        }
        if let (true, Some((m, meta))) = (plugins_dirty, &manifest) {
            write_json(&manifest_path, m, *meta)?;
            written.push(manifest_path.clone());
        }
    }
    if !dry_run && dirty.store {
        store::save(&root)?;
        // Also the server already running, when this AgentPlus started it.
        crate::process::set_independent(independent(&root));
    }
    Ok((diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelInput, ProviderInput};

    const PATCH: &str = "# Your patch layer for this dsh profile, applied after every bundle layer:
# a top-level YAML array of loader patch entries.
- id: ui-settings-general
  name: \"@deepseek-ai/dsh-client-ui-settings-general\"
  config:
    welcomeNoticeVersion: 2026-08-13.1

# keep me
- id: session-query-sqlite
  config:
    path: !!js dshHomePath('search.db')
    openAt: first-search
";

    const MANIFEST: &str = r#"{
  "name": "dsh-profile-web",
  "private": true,
  "dependencies": {},
  "dsh": {
    "profile": {
      "bundles": [
        "@deepseek-ai/dsh-base",
        "@deepseek-ai/dsh-web-app"
      ]
    }
  }
}
"#;

    fn setup(name: &str, patch: &str) -> TestHome {
        let home = TestHome::new(&format!("dsh-{name}"));
        let web = default_dir().join("profiles").join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("cordis.patch.yml"), patch).unwrap();
        std::fs::write(web.join("package.json"), MANIFEST).unwrap();
        std::fs::write(default_dir().join(".credentials.yaml"), "version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload:\n      version: 1\n      secret: s3cret\n").unwrap();
        home
    }

    fn patch_text() -> String {
        std::fs::read_to_string(profile_patch_path("web")).unwrap()
    }

    fn creds() -> J {
        load_credentials(&credentials_path()).unwrap().0
    }

    fn lines(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect::<Vec<_>>().join("\n")
    }

    fn prov(name: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput { id: None, name: name.into(), base_url: "https://relay.example.com/v1".into(), api: api.into(), api_key: key.map(String::from), models: models.iter().map(|s| s.to_string()).collect(), key_from_library: None, key_from_sync: None, official_auth: None },
        }
    }

    #[test]
    fn key_refs_follow_dsh_naming() {
        use super::super::pimodels::dsh_key_ref;
        assert_eq!(dsh_key_ref("minimax-cn"), "MINIMAX_CN_API_KEY");
        assert_eq!(dsh_key_ref("my-relay-2"), "MY_RELAY_2_API_KEY");
        assert_eq!(dsh_key_ref("a--b"), "A_B_API_KEY");
    }

    #[test]
    fn patch_edits_leave_other_entries_byte_for_byte() {
        let _h = setup("patch", PATCH);
        let path = profile_patch_path("web");
        let mut p = Patch::load(&path).unwrap();
        assert_eq!(p.entries().count(), 2);
        assert_eq!(p.find("session-query-sqlite"), Some(1));
        // The !!js entry can't be rewritten.
        assert!(p.check_editable(1).unwrap_err().to_string().contains("!!js"));
        let mut v = p.get(0).clone();
        v["config"]["welcomeNoticeVersion"] = Y::from("2026-09-01.1");
        p.set(0, v).unwrap();
        assert!(p.set_disabled("tool-plugin-manager", false).unwrap());
        p.save().unwrap();
        let text = patch_text();
        assert!(text.starts_with("# Your patch layer"), "{text}");
        assert!(text.contains("\n# keep me\n- id: session-query-sqlite\n  config:\n    path: !!js dshHomePath('search.db')\n    openAt: first-search\n"), "{text}");
        assert!(text.contains("welcomeNoticeVersion: 2026-09-01.1") || text.contains("welcomeNoticeVersion: '2026-09-01.1'"), "{text}");
        assert!(text.ends_with("- id: tool-plugin-manager\n  disabled: false\n"), "{text}");

        // Toggling edits that override in place.
        let mut p = Patch::load(&path).unwrap();
        assert!(p.set_disabled("tool-plugin-manager", true).unwrap());
        assert!(!p.set_disabled("tool-plugin-manager", true).unwrap());
        p.save().unwrap();
        assert!(patch_text().ends_with("- id: tool-plugin-manager\n  disabled: true\n"), "{}", patch_text());
        assert_eq!(Patch::load(&path).unwrap().entries().count(), 3);

        // Inserted rows are found inside their insert entry.
        std::fs::write(&path, "- insert:\n    - id: memory-a\n      name: '@deepseek-ai/dsh-mcp-client'\n    - id: memory-b\n      name: '@deepseek-ai/dsh-mcp-client'\n").unwrap();
        let p = Patch::load(&path).unwrap();
        assert_eq!(p.find_inserted("memory-b"), Some((0, 1)));
        assert_eq!(p.find("memory-b"), None);
    }

    #[test]
    fn empty_and_flow_patch_files() {
        let _h = setup("empty", "[]\n");
        let path = profile_patch_path("web");
        let mut p = Patch::load(&path).unwrap();
        assert_eq!(p.entries().count(), 0);
        p.set_disabled("x", true).unwrap();
        p.save().unwrap();
        assert_eq!(patch_text(), "- id: x\n  disabled: true\n");
        // Removing the last entry leaves `[]`, not an empty file dsh would refuse.
        let mut p = Patch::load(&path).unwrap();
        p.remove(0).unwrap();
        p.save().unwrap();
        assert_eq!(patch_text(), "[]\n");

        std::fs::write(&path, "[{ id: a, disabled: true }]\n").unwrap();
        let mut p = Patch::load(&path).unwrap();
        assert_eq!(p.find("a"), Some(0));
        assert!(p.set_disabled("a", false).is_err());
        assert!(Patch::load(&path).is_ok());
        std::fs::write(&path, "id: a\n").unwrap();
        assert!(Patch::load(&path).err().unwrap().to_string().contains("顶层不是列表"));
    }

    #[test]
    fn providers_are_written_into_the_profile_patch_with_keys_in_credentials() {
        let _h = setup("providers", PATCH);
        crate::modelinfo::test_cache_acme();
        let (d, w, b) = plan(&[prov("My Relay", "chat", Some("sk-relay-secret-1234"), &["acme-vision-9", "m2"])], false).unwrap();
        let text = lines(&d);
        assert!(text.contains("••••1234") && !text.contains("sk-relay-secret"), "{text}");
        assert_eq!(w, vec![credentials_path(), profile_patch_path("web")]);
        assert!(b.is_some());
        let patch = patch_text();
        assert!(patch.contains("# keep me\n- id: session-query-sqlite\n  config:\n    path: !!js dshHomePath('search.db')"), "{patch}");
        let p = Patch::load(&profile_patch_path("web")).unwrap();
        let row = p.get(p.find("llm-pi-ai").unwrap());
        assert_eq!(row["name"], Y::from("@deepseek-ai/dsh-llm-pi-ai"));
        let def = serde_json::to_value(&row["config"]["providers"]["my-relay"]).unwrap();
        assert_eq!(def["displayName"], "My Relay");
        assert_eq!(def["baseURL"], "https://relay.example.com/v1");
        assert_eq!(def["api"], "openai-completions");
        assert_eq!(def["apiKeyEnv"], "MY_RELAY_API_KEY");
        assert_eq!(def["models"][0], json!({ "id": "acme-vision-9", "contextWindow": 64000, "input": ["text", "image"], "maxTokens": 8192 }));
        assert_eq!(def["models"][1], json!({ "id": "m2" }));
        let c = creds();
        assert_eq!(c["refs"]["MY_RELAY_API_KEY"], "sk-relay-secret-1234");
        assert_eq!(c["records"]["client-connection/browser-session"]["payload"]["secret"], "s3cret");
        assert_eq!(c["version"], 1);

        // State: the provider, the built-in DeepSeek route, no key values.
        let st = state(&Install::default());
        assert!(!st.readonly, "{:?}", st.notes);
        let p = st.providers.iter().find(|p| p.id == "my-relay").unwrap();
        assert!(p.has_key && p.enabled && p.compatible);
        assert_eq!(p.name, "My Relay");
        assert_eq!(p.models.len(), 2);
        assert!(p.details.iter().any(|k| k.v == ".credentials.yaml · MY_RELAY_API_KEY"), "{:?}", p.details);
        let ds = st.providers.iter().find(|p| p.id == "deepseek-official").unwrap();
        assert!(ds.builtin && !ds.has_key);
        assert!(st.current.iter().any(|k| k.v == "deepseek-official/deepseek-flash"));
        assert!(!format!("{st:?}").contains("sk-relay-secret"));
        assert_eq!(provider_endpoint("my-relay").unwrap(), ("https://relay.example.com/v1".into(), Some("sk-relay-secret-1234".into()), "chat".into()));

        // The environment wins over the credentials file, as in dsh.
        crate::env::set_test_vars(&[("MY_RELAY_API_KEY", "sk-from-env-0000")]);
        assert_eq!(provider_endpoint("my-relay").unwrap().1.as_deref(), Some("sk-from-env-0000"));
        crate::env::set_test_vars(&[("MY_RELAY_API_KEY", "")]);

        // Edit: name, url, key (same reference), protocol.
        let edit = Op::UpsertProvider { provider: ProviderInput { id: Some("my-relay".into()), name: "Relay".into(), base_url: "https://b.example.com".into(), api: "anthropic".into(), api_key: Some("sk-rotated-5678".into()), models: vec![], key_from_library: None, key_from_sync: None, official_auth: None } };
        plan(&[edit], false).unwrap();
        let (e, key, api) = provider_endpoint("my-relay").unwrap();
        assert_eq!((e.as_str(), key.as_deref(), api.as_str()), ("https://b.example.com", Some("sk-rotated-5678"), "anthropic"));
        assert!(patch_text().contains("displayName: Relay"));

        // Models: hide / show / edit fields.
        plan(&[Op::SetModelVisible { provider: "my-relay".into(), model: "m2".into(), visible: false }], false).unwrap();
        assert!(!patch_text().contains("id: m2"));
        plan(&[Op::SetModelVisible { provider: "my-relay".into(), model: "m2".into(), visible: true }], false).unwrap();
        let mut extra = crate::mfields::Extra::new();
        extra.insert("/maxTokens".into(), json!(4096));
        plan(&[Op::UpsertModel { provider: "my-relay".into(), model: ModelInput { id: "m2".into(), name: Some("M Two".into()), context: Some(128000), extra } }], false).unwrap();
        let v: J = serde_json::to_value(&Patch::load(&profile_patch_path("web")).unwrap().get(2)["config"]["providers"]["my-relay"]["models"][1]).unwrap();
        assert_eq!(v, json!({ "id": "m2", "name": "M Two", "contextWindow": 128000, "maxTokens": 4096 }));

        // Gemini isn't a protocol dsh routes.
        assert!(plan(&[prov("G", "gemini", None, &[])], true).err().unwrap().to_string().contains("Gemini"));

        // Delete: the provider and the key named after it.
        let (d, ..) = plan(&[Op::DeleteProvider { provider: "my-relay".into() }], false).unwrap();
        assert!(lines(&d).contains("- refs.MY_RELAY_API_KEY"), "{}", lines(&d));
        assert!(creds()["refs"].get("MY_RELAY_API_KEY").is_none());
        assert!(!state(&Install::default()).providers.iter().any(|p| p.id == "my-relay"));
    }

    #[test]
    fn shared_key_refs_and_disabled_providers() {
        let _h = setup(
            "shared",
            "- id: llm-pi-ai\n  config:\n    providers:\n      openai:\n        apiKeyEnv: OPENAI_API_KEY\n      gw:\n        api: openai-responses\n        baseURL: https://gw.example.com/v1\n        apiKeyEnv: SHARED_KEY\n        models:\n          - id: g1\n",
        );
        std::fs::write(credentials_path(), "version: 1\nrefs:\n  SHARED_KEY: sk-shared-1111\n  OPENAI_API_KEY: sk-openai-2222\n").unwrap();
        let st = state(&Install::default());
        let o = st.providers.iter().find(|p| p.id == "openai").unwrap();
        assert!(o.has_key && o.base_url.is_none());
        assert_eq!(o.api, "responses");
        // Disabling parks the definition; a new key while disabled keeps its reference.
        plan(&[Op::SetProviderEnabled { provider: "gw".into(), enabled: false }], false).unwrap();
        assert!(!patch_text().contains("gw:"));
        let key = Op::UpsertProvider { provider: ProviderInput { id: Some("gw".into()), name: "gw".into(), base_url: "https://gw.example.com/v1".into(), api: "responses".into(), api_key: Some("sk-new-3333".into()), models: vec![], key_from_library: None, key_from_sync: None, official_auth: None } };
        plan(&[key], false).unwrap();
        assert_eq!(creds()["refs"]["SHARED_KEY"], "sk-new-3333");
        plan(&[Op::SetProviderEnabled { provider: "gw".into(), enabled: true }], false).unwrap();
        assert!(patch_text().contains("apiKeyEnv: SHARED_KEY"));
        // A reference another provider also uses isn't overwritten: this one gets its own.
        let use_shared = "- id: llm-pi-ai\n  config:\n    providers:\n      gw:\n        api: openai-responses\n        baseURL: https://gw.example.com/v1\n        apiKeyEnv: SHARED_KEY\n        models:\n          - id: g1\n      other:\n        api: openai-responses\n        baseURL: https://other.example.com/v1\n        apiKeyEnv: SHARED_KEY\n        models:\n          - id: o1\n";
        let before = patch_text();
        std::fs::write(profile_patch_path("web"), use_shared).unwrap();
        let other_key = Op::UpsertProvider { provider: ProviderInput { id: Some("other".into()), name: "other".into(), base_url: "https://other.example.com/v1".into(), api: "responses".into(), api_key: Some("sk-other-4444".into()), models: vec![], key_from_library: None, key_from_sync: None, official_auth: None } };
        plan(&[other_key], false).unwrap();
        assert_eq!(creds()["refs"]["SHARED_KEY"], "sk-new-3333", "gw keeps its key");
        assert_eq!(creds()["refs"]["OTHER_API_KEY"], "sk-other-4444");
        assert!(patch_text().contains("apiKeyEnv: OTHER_API_KEY"));
        std::fs::write(profile_patch_path("web"), before).unwrap();
        // A reference not named after the provider is kept on delete.
        let (d, ..) = plan(&[Op::DeleteProvider { provider: "gw".into() }], false).unwrap();
        assert!(lines(&d).contains("refs.SHARED_KEY 保留"), "{}", lines(&d));
        assert_eq!(creds()["refs"]["SHARED_KEY"], "sk-new-3333");
        // So is a built-in provider's.
        plan(&[Op::DeleteProvider { provider: "openai".into() }], false).unwrap();
        assert_eq!(creds()["refs"]["OPENAI_API_KEY"], "sk-openai-2222");
    }

    #[test]
    fn home_layer_wins_and_commented_rows_are_read_only() {
        let _h = setup("home", PATCH);
        std::fs::write(home_patch_path(), "- id: llm-pi-ai\n  config:\n    providers: {}\n").unwrap();
        plan(&[prov("Home", "chat", None, &["m"])], false).unwrap();
        assert!(std::fs::read_to_string(home_patch_path()).unwrap().contains("displayName: Home"));
        assert!(!patch_text().contains("llm-pi-ai"));
        let st = state(&Install::default());
        assert!(st.notes.iter().any(|n| n.contains("覆盖所有 profile")), "{:?}", st.notes);

        std::fs::write(home_patch_path(), "- id: llm-pi-ai\n  config:\n    # my relays\n    providers: {}\n").unwrap();
        let st = state(&Install::default());
        assert!(st.readonly);
        assert!(plan(&[prov("X", "chat", None, &[])], false).is_err());
    }

    #[test]
    fn profiles_and_plugins() {
        let _h = setup("plugins", PATCH);
        let desktop = default_dir().join("profiles").join("desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("package.json"), MANIFEST.replace("dsh-web-app", "dsh-desktop-app")).unwrap();
        std::fs::write(desktop.join("cordis.patch.yml"), "[]\n").unwrap();
        // The desktop profile is the default once it exists; the pick is remembered.
        assert_eq!(profile_in(&store::load(), &profiles()), "desktop");
        let (d, ..) = plan(&[Op::SetSetting { key: PROFILE_SETTING.into(), value: json!("web") }], false).unwrap();
        assert!(lines(&d).contains("profile → web"));
        assert_eq!(patch_path(), profile_patch_path("web"));
        assert!(plan(&[Op::SetSetting { key: PROFILE_SETTING.into(), value: json!("nope") }], true).is_err());

        // A shipped optional bundle and one installed into the profile.
        let install = _h.0.join("npm").join("node_modules").join("@deepseek-ai").join("dsh");
        let voice = install.join("node_modules").join("@deepseek-ai").join("dsh-experimental-voice-input-bundle");
        std::fs::create_dir_all(voice.join("locale")).unwrap();
        std::fs::write(voice.join("package.json"), r#"{ "name": "@deepseek-ai/dsh-experimental-voice-input-bundle", "description": "Voice", "dsh": { "bundle": { "patch": "./cordis.patch.yml" } } }"#).unwrap();
        std::fs::write(voice.join("icon.svg"), "<svg/>").unwrap();
        std::fs::write(voice.join("locale").join("zh.json"), r#"{ "meta": { "title": "语音输入", "description": "本机转写" } }"#).unwrap();
        let base = install.join("node_modules").join("@deepseek-ai").join("dsh-base");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("package.json"), r#"{ "name": "@deepseek-ai/dsh-base", "dsh": { "bundle": { "patch": "./cordis.patch.yml" } } }"#).unwrap();
        let web = profile_dir("web");
        let mut m: J = serde_json::from_str(MANIFEST).unwrap();
        m["dependencies"]["turtle-ui"] = json!("github:x/turtle-ui");
        m["dsh"]["profile"]["bundles"].as_array_mut().unwrap().push(json!("turtle-ui"));
        std::fs::write(web.join("package.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
        std::fs::create_dir_all(web.join("node_modules").join("turtle-ui")).unwrap();
        std::fs::write(web.join("node_modules").join("turtle-ui").join("package.json"), r#"{ "name": "turtle-ui", "description": "A turtle UI", "dsh": { "bundle": { "patch": "./p.yml" } } }"#).unwrap();

        let got = plugins("web", Some(&install));
        assert_eq!(
            got,
            vec![
                Plugin { name: "@deepseek-ai/dsh-experimental-voice-input-bundle".into(), title: Some("语音输入".into()), desc: Some("本机转写".into()), enabled: false },
                Plugin { name: "turtle-ui".into(), title: None, desc: Some("A turtle UI".into()), enabled: true },
            ]
        );
        let on = Op::SetSetting { key: "plugin:@deepseek-ai/dsh-experimental-voice-input-bundle".into(), value: json!(true) };
        let off = Op::SetSetting { key: "plugin:turtle-ui".into(), value: json!(false) };
        let (d, w, _) = plan(&[on, off], false).unwrap();
        assert!(lines(&d).contains("保留安装"), "{}", lines(&d));
        assert_eq!(w, vec![web.join("package.json")]);
        let m: J = serde_json::from_str(&std::fs::read_to_string(web.join("package.json")).unwrap()).unwrap();
        assert_eq!(m["dsh"]["profile"]["bundles"], json!(["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app", "@deepseek-ai/dsh-experimental-voice-input-bundle"]));
        assert_eq!(m["dependencies"]["turtle-ui"], "github:x/turtle-ui");
    }

    #[test]
    fn served_profile_reads_the_launcher_flags() {
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let npm = r"C:\Users\me\AppData\Roaming\npm\\node_modules\@deepseek-ai\dsh\lib\bin.js";
        assert_eq!(served_profile(&argv(&format!("node {npm} web"))).as_deref(), Some("web"));
        assert_eq!(served_profile(&argv(&format!("D:\\nodejs\\node.exe {npm} web --no-open --port 8080"))).as_deref(), Some("web"));
        assert_eq!(served_profile(&argv("node /home/me/.npm/_npx/1a/node_modules/@deepseek-ai/dsh/lib/bin.js --profile rescue")).as_deref(), Some("rescue"));
        assert_eq!(served_profile(&argv(&format!("node {npm} --patch x.yml --profile=mine"))).as_deref(), Some("mine"));
        assert_eq!(served_profile(&argv(&format!("node {npm} --patch x.yml web --resume abc"))).as_deref(), Some("web"));
        // Not a server: a config dump, plugin management, the bare launcher, another program.
        assert_eq!(served_profile(&argv(&format!("node {npm} --profile web --dump-config"))), None);
        assert_eq!(served_profile(&argv(&format!("node {npm} plugin --profile web add x"))), None);
        assert_eq!(served_profile(&argv(&format!("node {npm}"))), None);
        assert_eq!(served_profile(&argv(r"node D:\nodejs\node_modules\npm\bin\npx-cli.js @deepseek-ai/dsh web")), None);
        assert_eq!(served_profile(&argv("node /x/other-dsh/lib/bin.js web")), None);
    }

    #[test]
    fn web_address_follows_flags_then_patch_then_defaults() {
        let _h = setup("address", PATCH);
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(web_address(&argv("node bin.js web"), "web"), "http://127.0.0.1:3080/");
        assert_eq!(web_address(&argv("node bin.js web --port 8080"), "web"), "http://127.0.0.1:8080/");
        assert_eq!(web_address(&argv("node bin.js web --host=0.0.0.0 --port=9000"), "web"), "http://127.0.0.1:9000/");
        assert_eq!(web_address(&argv("node bin.js web --host ::1 --port 0"), "web"), "http://[::1]:3080/");
        // A plain override in the profile; the home layer outranks it.
        std::fs::write(profile_patch_path("web"), format!("{PATCH}\n- id: webserver\n  config:\n    host: 192.168.1.5\n    port: 4000\n")).unwrap();
        assert_eq!(web_address(&argv("node bin.js web"), "web"), "http://192.168.1.5:4000/");
        assert_eq!(web_address(&argv("node bin.js web --port 5000"), "web"), "http://192.168.1.5:5000/");
        std::fs::write(home_patch_path(), "- id: webserver\n  config:\n    host: !!js ctx.webStartup.host ?? 'x'\n    port: 4100\n").unwrap();
        assert_eq!(web_address(&argv("node bin.js web"), "web"), "http://127.0.0.1:4100/");
    }

    #[test]
    fn only_web_ui_profiles_are_started_as_servers() {
        let _h = setup("server", PATCH);
        let tui = profile_dir("tui");
        std::fs::create_dir_all(&tui).unwrap();
        std::fs::write(tui.join("package.json"), MANIFEST.replace("dsh-web-app", "dsh-tui-app")).unwrap();
        assert!(server("tui").is_none());
        assert!(server("nope").is_none());
        // The package and node are looked up on this machine: checked only where they are.
        if install_dir().is_some() && crate::process::on_path(&["node.exe"]).is_some() {
            let s = server("web").unwrap();
            assert_eq!(s.args[1..], ["--profile", "web"]);
            assert_eq!((s.serves)(&[s.program.to_string_lossy().to_string()].into_iter().chain(s.args.clone()).collect::<Vec<_>>()).as_deref(), Some("web"));
            assert!(s.log.ends_with("dsh-web.log"));
            assert!(!s.independent);
        }
    }

    #[test]
    fn separate_process_setting_follows_the_web_ui() {
        let _h = setup("independent", PATCH);
        let has = |inst: &Install| state(inst).settings.iter().find(|s| s.key == INDEPENDENT_SETTING).map(|s| s.value.clone());
        // Only a profile served as a web UI has a server to keep running.
        assert_eq!(has(&Install::default()), None);
        let inst = Install {
            server: Some(crate::process::Server { program: "node".into(), args: vec![], serves: |_| None, name: "web".into(), log: PathBuf::new(), independent: false }),
            ..Default::default()
        };
        assert_eq!(has(&inst), Some(json!(false)));

        let on = Op::SetSetting { key: INDEPENDENT_SETTING.into(), value: json!(true) };
        let (d, written, _) = plan(std::slice::from_ref(&on), false).unwrap();
        assert!(lines(&d).contains("网页服务在 AgentPlus 退出后继续运行"), "{}", lines(&d));
        // Only AgentPlus's own store changes.
        assert!(written.is_empty());
        assert!(independent(&store::load()));
        assert_eq!(has(&inst), Some(json!(true)));
        // Already on: nothing to write.
        assert!(plan(&[on], true).unwrap().0.groups.is_empty());
        let (d, ..) = plan(&[Op::SetSetting { key: INDEPENDENT_SETTING.into(), value: json!(false) }], false).unwrap();
        assert!(lines(&d).contains("网页服务随 AgentPlus 一起退出"));
        assert!(!independent(&store::load()));
    }

    #[test]
    fn no_profile_is_read_only() {
        let _h = TestHome::new("dsh-none");
        let st = state(&Install::default());
        assert!(st.readonly && st.notes[0].contains("dsh web"));
        assert!(plan(&[prov("X", "chat", None, &[])], true).is_err());
    }

    /// Read-only look at the real machine: `cargo test --lib dump_dsh -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_dsh() {
        let inst = detect();
        println!("installed={} version={:?} install_dir={:?} running={} exe={:?} server={:?}", inst.installed, inst.version, install_dir(), inst.running, inst.exe, inst.server.as_ref().map(|s| (&s.program, &s.args)));
        let link = web_link().map(|(l, note)| (l.split('?').next().unwrap_or_default().to_string(), note));
        println!("web_link={link:?}");
        let st = state(&inst);
        println!("dir={} readonly={} files={:?}", st.config_dir, st.readonly, st.files);
        for p in &st.providers {
            println!("  [{}] {} api={} host={} key={} builtin={} models={:?}", p.id, p.name, p.api, p.host, p.has_key, p.builtin, p.models.iter().map(|m| &m.id).collect::<Vec<_>>());
        }
        for k in &st.current {
            println!("  {} = {}", k.k, k.v);
        }
        for s in &st.settings {
            println!("  setting {} = {} ({})", s.key, s.value, s.label);
        }
        println!("notes={:?}", st.notes);
        let (d, written, backup) = plan(&[prov("Dump Probe", "chat", Some("sk-dump-probe-0000"), &["m"])], true).unwrap();
        for g in &d.groups {
            for l in &g.lines {
                println!("  diff {} | {}", g.file, l.text);
            }
        }
        assert!(written.is_empty() && backup.is_none());
    }
}
