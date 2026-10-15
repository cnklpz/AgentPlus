//! Chinese UI for Antigravity (2.x).
//!
//! Antigravity has no translations of its own: its UI (served by the bundled language
//! server, not shipped as files) has English text in the code. So nothing is patched on
//! disk; instead `agzh.js` runs in each Antigravity window and swaps text that matches a
//! dictionary (`agzh_dict.json`, plus the user's own entries), and keeps doing so as the UI
//! re-renders. Conversations, code, editors and terminals are left alone.
//!
//! The script gets there through the DevTools port Antigravity always opens (it starts with
//! `--remote-debugging-port=0` unless given one, and writes the port to `DevToolsActivePort`
//! in its data folder): while the switch is on, a background thread checks every window
//! every couple of seconds and evaluates the script where it is missing (a new window, a
//! reload) or outdated (the dictionary changed). Switching it off undoes it in open windows.
//! The file outlives Antigravity, so before attaching, the port has to answer as Antigravity.

use crate::adapters::antigravity::ID;
use crate::cdp::Session;
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

const DICT: &str = include_str!("agzh_dict.json");
const SCRIPT: &str = include_str!("agzh.js");
/// Store keys (under the agent): the switch, and the user's entries ("English = 中文" lines).
const ON_KEY: &str = "zhUi";
const CUSTOM_KEY: &str = "zhCustom";
/// Relative times, only applied inside Antigravity's timestamps (see `agzh.js`): elsewhere
/// "2d" or "1m" can be a name or a count.
const TIMES: [(&str, &str); 8] = [
    ("{n}s", "{n} 秒"),
    ("{n}m", "{n} 分钟"),
    ("{n}h", "{n} 小时"),
    ("{n}d", "{n} 天"),
    ("{n}w", "{n} 周"),
    ("{n}mo", "{n} 个月"),
    ("{n}y", "{n} 年"),
    ("{x} ago", "{x}前"),
];
/// How often open windows are checked.
const TICK: Duration = Duration::from_millis(1500);

/// The bundled dictionary, English → Chinese.
pub fn builtin() -> &'static BTreeMap<String, String> {
    static B: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    B.get_or_init(|| serde_json::from_str(DICT).expect("agzh_dict.json"))
}

/// The user's entries: one `English = 中文` per line; an empty right side keeps that text in
/// English. Splits at the first `=`.
pub fn parse_custom(lines: &[String]) -> Result<Vec<(String, String)>> {
    let mut out = vec![];
    for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        let Some((en, zh)) = line.split_once('=') else {
            bail!("{}", tr!("Not \"English = Chinese\": {line}", "不是「英文 = 中文」的格式：{line}"));
        };
        let (en, zh) = (en.trim(), zh.trim());
        if en.is_empty() {
            bail!("{}", tr!("The English side is empty: {line}", "左边的英文是空的：{line}"));
        }
        out.push((en.to_string(), zh.to_string()));
    }
    Ok(out)
}

/// A dictionary key with `{name}` placeholders as a JavaScript regex source and the names in
/// order; None for a plain text. `{n}` and `{m}` match a number, any other name some text.
fn pattern(key: &str) -> Option<(String, Vec<String>)> {
    let esc = |s: &str| s.chars().fold(String::new(), |mut o, c| {
        if ".*+?^${}()|[]\\/".contains(c) {
            o.push('\\');
        }
        o.push(c);
        o
    });
    let mut src = String::new();
    let mut names = vec![];
    let mut rest = key;
    while let Some(open) = rest.find('{') {
        let Some(len) = rest[open + 1..].find('}') else { break };
        let name = &rest[open + 1..open + 1 + len];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
            src.push_str(&esc(&rest[..open + 1]));
            rest = &rest[open + 1..];
            continue;
        }
        src.push_str(&esc(&rest[..open]));
        src.push_str(if name == "n" || name == "m" { r"(\d+)" } else { "(.+?)" });
        names.push(name.to_string());
        rest = &rest[open + 2 + len..];
    }
    if names.is_empty() {
        return None;
    }
    src.push_str(&esc(rest));
    Some((src, names))
}

/// The dictionary the script gets: the bundled one with the user's entries over it.
pub fn dictionary(custom: &[(String, String)]) -> Value {
    let mut all = builtin().clone();
    for (en, zh) in custom {
        if zh.is_empty() {
            all.remove(en);
        } else {
            all.insert(en.clone(), zh.clone());
        }
    }
    let mut exact = Map::new();
    let mut patterns = vec![];
    for (en, zh) in all {
        match pattern(&en) {
            Some((src, names)) => patterns.push(json!([src, names, zh])),
            None => {
                exact.insert(en, json!(zh));
            }
        }
    }
    let times: Vec<Value> = TIMES.iter().filter_map(|(en, zh)| pattern(en).map(|(src, names)| json!([src, names, zh]))).collect();
    json!({ "exact": exact, "patterns": patterns, "times": times })
}

/// (version, script) for these entries; the version changes with the dictionary.
pub fn script(custom: &[(String, String)]) -> (String, String) {
    let dict = dictionary(custom).to_string();
    let ver = crate::model::key_fingerprint(&dict);
    let js = SCRIPT.replacen("__VER__", &ver, 1).replacen("__DICT__", &dict, 1);
    (ver, js)
}

// The switch and the entries are kept in Windows' section of the store, whichever
// environment AgentPlus is looking at: Antigravity is a Windows app. The adapter reads and
// writes them through these too.

fn section(root: &Value) -> Option<&Value> {
    root.get(ID)
}

fn section_mut(root: &mut Value) -> &mut Map<String, Value> {
    if !root.is_object() {
        *root = json!({});
    }
    let a = root.as_object_mut().unwrap().entry(ID).or_insert_with(|| json!({}));
    if !a.is_object() {
        *a = json!({});
    }
    a.as_object_mut().unwrap()
}

/// The switch: None when it was never set.
fn flag(root: &Value) -> Option<bool> {
    section(root).and_then(|a| a.get(ON_KEY)).and_then(Value::as_bool)
}

pub fn is_on(root: &Value) -> bool {
    flag(root) == Some(true)
}

pub fn set_on(root: &mut Value, on: bool) {
    section_mut(root).insert(ON_KEY.into(), json!(on));
}

/// The user's entries as typed, one per line.
pub fn lines(root: &Value) -> Vec<String> {
    crate::util::str_list(section(root).and_then(|a| a.get(CUSTOM_KEY))).unwrap_or_default()
}

pub fn set_lines(root: &mut Value, lines: &[String]) {
    section_mut(root).insert(CUSTOM_KEY.into(), json!(lines));
}

/// What the thread puts in windows: (version, script), None when off.
static WANT: Mutex<Option<(String, String)>> = Mutex::new(None);
/// Set when the switch goes off: open windows are cleaned up once.
static CLEANUP: AtomicBool = AtomicBool::new(false);
/// Windows translated at the last check.
static WINDOWS: AtomicUsize = AtomicUsize::new(0);

/// Changes asked for (`sync`) and the last one a finished check started from: `settle` waits
/// for a check that saw the latest change.
struct Gen {
    asked: u64,
    done: u64,
}
static GEN: Mutex<Gen> = Mutex::new(Gen { asked: 0, done: 0 });
static GEN_CHANGED: Condvar = Condvar::new();

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads the switch and the entries again (at startup, and after they change) and wakes the
/// thread to apply them.
pub fn sync() {
    let root = crate::store::load();
    let (on, flag) = (is_on(&root), flag(&root));
    // Entries that don't parse are refused when saved; here they are skipped.
    let custom: Vec<(String, String)> = lines(&root).iter().filter_map(|l| parse_custom(std::slice::from_ref(l)).ok()).flatten().collect();
    {
        let mut want = lock(&WANT);
        // Off after being on (here, or in an earlier run that left windows translated).
        if !on && (want.is_some() || flag.is_some()) {
            CLEANUP.store(true, Ordering::SeqCst);
        }
        *want = on.then(|| script(&custom));
    }
    lock(&GEN).asked += 1;
    GEN_CHANGED.notify_all();
}

/// Waits, at most `max`, for a check that saw the latest change: a page shown right after
/// applying then has the new state, and a stuck Antigravity can't hold it up for long.
pub fn settle(max: Duration) {
    let g = lock(&GEN);
    let target = g.asked;
    let _ = GEN_CHANGED.wait_timeout_while(g, max, |g| g.done < target);
}

/// Antigravity windows translated right now (as of the last check).
pub fn windows() -> usize {
    WINDOWS.load(Ordering::SeqCst)
}

/// Antigravity's DevTools port, from the file it writes when it starts.
fn port() -> Option<u16> {
    let file = dirs::config_dir()?.join("Antigravity").join("DevToolsActivePort");
    std::fs::read_to_string(file).ok()?.lines().next()?.trim().parse().ok()
}

/// Whether a browser's `/json/version` answer is Antigravity's (its user agent names it).
fn is_antigravity(version: &Value) -> bool {
    version["User-Agent"].as_str().is_some_and(|ua| ua.contains(" Antigravity/"))
}

/// Antigravity's windows: pages its language server serves (not the splash screen, not
/// DevTools, not the 1.x editor).
fn app_page(url: &str) -> bool {
    ["https://127.0.0.1:", "http://127.0.0.1:", "https://localhost:", "http://localhost:"].iter().any(|p| url.starts_with(p))
}

fn eval(s: &mut Session, expr: &str) -> Result<Value> {
    let r = s.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }))?;
    if let Some(e) = r.get("exceptionDetails") {
        bail!("{}", e["exception"]["description"].as_str().or(e["text"].as_str()).unwrap_or("exception"));
    }
    Ok(r["result"]["value"].clone())
}

/// Runs for the life of AgentPlus: a check every `TICK`, or right away after a change.
pub fn watch() {
    sync();
    let mut w = Watcher::default();
    loop {
        let asked = {
            let g = lock(&GEN);
            let (g, _) = GEN_CHANGED.wait_timeout_while(g, TICK, |g| g.done >= g.asked).unwrap_or_else(|e| e.into_inner());
            g.asked
        };
        w.tick();
        let mut g = lock(&GEN);
        g.done = g.done.max(asked);
        drop(g);
        GEN_CHANGED.notify_all();
    }
}

/// The thread's view of Antigravity between checks.
#[derive(Default)]
struct Watcher {
    /// The port the sessions are on.
    port: Option<u16>,
    /// The port last confirmed to be Antigravity's (asked again after a failed check).
    verified: Option<u16>,
    /// A port that answered as something else (logged once).
    rejected: Option<u16>,
    /// Attached windows, by target id.
    open: HashMap<String, Session>,
}

impl Watcher {
    fn reset(&mut self) {
        self.open.clear();
        self.port = None;
        self.verified = None;
        WINDOWS.store(0, Ordering::SeqCst);
    }

    /// One check of every Antigravity window: translate it, or undo that after the switch
    /// went off.
    fn tick(&mut self) {
        let want = lock(&WANT).clone();
        let cleanup = want.is_none() && CLEANUP.swap(false, Ordering::SeqCst);
        if want.is_none() && !cleanup {
            return self.reset();
        }
        let Some(port) = port() else { return self.reset() };
        if self.verified != Some(port) {
            match crate::cdp::browser(port) {
                Ok(v) if is_antigravity(&v) => {}
                Ok(v) => {
                    if self.rejected != Some(port) {
                        self.rejected = Some(port);
                        crate::applog::warn("agzh", format!("port {port} from DevToolsActivePort isn't Antigravity's ({}); left alone", v["User-Agent"].as_str().unwrap_or("?")));
                    }
                    return self.reset();
                }
                // Not answering (Antigravity closed or busy): nothing to do this time.
                Err(_) => return,
            }
            if self.port != Some(port) {
                self.open.clear();
            }
            self.port = Some(port);
            self.verified = Some(port);
        }
        let pages: Vec<Value> = match crate::cdp::targets(port) {
            Ok(t) => t.into_iter().filter(|t| app_page(t["url"].as_str().unwrap_or(""))).collect(),
            // Busy, or closed: the windows stay as they are, and the port is confirmed again
            // next time (another program may take it once Antigravity is gone).
            Err(e) => {
                self.verified = None;
                crate::applog::info("agzh", format!("no window list from port {port}: {e:#}"));
                return;
            }
        };
        self.open.retain(|id, _| pages.iter().any(|p| p["id"].as_str() == Some(id)));
        let mut done = 0;
        for p in &pages {
            let (Some(id), Some(ws)) = (p["id"].as_str(), p["webSocketDebuggerUrl"].as_str()) else { continue };
            let url = p["url"].as_str().unwrap_or_default();
            if !self.open.contains_key(id) {
                match Session::open(ws, port) {
                    Ok(s) => {
                        self.open.insert(id.to_string(), s);
                    }
                    Err(e) => {
                        crate::applog::warn("agzh", format!("couldn't attach to an Antigravity window: {e:#}"));
                        continue;
                    }
                }
            }
            let s = self.open.get_mut(id).unwrap();
            let r = match &want {
                None => eval(s, "window.__agpZh ? (window.__agpZh.off(), 'off') : 'none'").map(|_| ()),
                Some((ver, js)) => eval(s, "window.__agpZh && window.__agpZh.ver").and_then(|v| {
                    if v.as_str() == Some(ver.as_str()) {
                        return Ok(());
                    }
                    eval(s, js)?;
                    crate::applog::info("agzh", format!("translated {url}"));
                    Ok(())
                }),
            };
            match r {
                Ok(()) => done += 1,
                Err(e) => {
                    crate::applog::warn("agzh", format!("{url}: {e:#}"));
                    self.open.remove(id);
                }
            }
        }
        if want.is_none() {
            return self.reset();
        }
        WINDOWS.store(done, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_dictionary_loads_and_translates() {
        let b = builtin();
        assert!(b.len() > 1500);
        for (en, zh) in b {
            assert!(!en.trim().is_empty() && en.trim() == en, "{en:?}");
            assert!(!zh.trim().is_empty() && en != zh, "{en:?}");
        }
        assert_eq!(b["Conversation History"], "对话历史");
    }

    #[test]
    fn custom_lines_parse_or_say_which_one_is_wrong() {
        let lines = vec!["  Turbo = 极速 ".into(), "".into(), "Review =".into(), "a = b = c".into()];
        assert_eq!(parse_custom(&lines).unwrap(), [("Turbo".into(), "极速".into()), ("Review".into(), String::new()), ("a".into(), "b = c".into())]);
        assert_eq!(parse_custom(&["no equals".into()]).unwrap_err().to_string(), "不是「英文 = 中文」的格式：no equals");
        assert_eq!(parse_custom(&[" = 中".into()]).unwrap_err().to_string(), "左边的英文是空的：= 中");
    }

    #[test]
    fn placeholders_become_captures() {
        assert_eq!(pattern("Plain text"), None);
        assert_eq!(pattern("{n}d"), Some((r"(\d+)d".into(), vec!["n".into()])));
        assert_eq!(pattern("Open (beta) {x}?"), Some((r"Open \(beta\) (.+?)\?".into(), vec!["x".into()])));
        assert_eq!(pattern("showing {n} of {m}"), Some((r"showing (\d+) of (\d+)".into(), vec!["n".into(), "m".into()])));
        // Braces that aren't a placeholder are text.
        assert_eq!(pattern("{} {1} {x}"), Some((r"\{\} \{1\} (.+?)".into(), vec!["x".into()])));
        assert_eq!(pattern("{x"), None);
    }

    #[test]
    fn custom_entries_override_add_and_remove() {
        let custom = vec![("Settings".to_string(), "偏好设置".to_string()), ("Review".to_string(), String::new()), ("{n} tasks".to_string(), "{n} 个任务".to_string())];
        let d = dictionary(&custom);
        assert_eq!(d["exact"]["Settings"], "偏好设置");
        assert!(d["exact"].get("Review").is_none());
        assert!(d["patterns"].as_array().unwrap().iter().any(|p| p[0] == r"(\d+) tasks" && p[2] == "{n} 个任务"));
        assert!(d["exact"].get("{n} tasks").is_none());
    }

    #[test]
    fn the_script_carries_its_version_and_dictionary() {
        let (v1, js) = script(&[]);
        assert!(!js.contains("__VER__") && !js.contains("__DICT__"));
        assert!(js.contains(&format!("const VER = \"{v1}\"")));
        assert!(js.contains("\"对话历史\""));
        let (v2, _) = script(&[("Settings".into(), "偏好设置".into())]);
        assert_ne!(v1, v2);
        assert_eq!(script(&[]).0, v1);
    }

    #[test]
    fn only_language_server_pages_are_windows() {
        assert!(app_page("https://127.0.0.1:10489/c/abc"));
        assert!(!app_page("data:text/html,splash"));
        assert!(!app_page("vscode-file://vscode-app/workbench.html"));
        assert!(!app_page("devtools://devtools/bundled/inspector.html"));
    }

    #[test]
    fn relative_times_are_kept_apart() {
        let d = dictionary(&[]);
        let times = d["times"].as_array().unwrap();
        assert_eq!(times.len(), TIMES.len());
        assert!(times.iter().any(|t| t[0] == r"(\d+)d" && t[2] == "{n} 天"));
        // Not in the general lists: "2d" or "1m" elsewhere is a name or a count.
        assert!(!d["patterns"].as_array().unwrap().iter().any(|p| p[0] == r"(\d+)d" || p[0] == r"(\d+)m"));
        assert!(!builtin().contains_key("{n}d") && !builtin().contains_key("{x} ago"));
    }

    #[test]
    fn every_pattern_compiles() {
        let d = dictionary(&[(r"Odd [x] (y) {n}.*+? $ ^ | \ / {x}".into(), "怪 {n} {x}".into())]);
        for p in d["patterns"].as_array().unwrap().iter().chain(d["times"].as_array().unwrap()) {
            regex::Regex::new(&format!("^{}$", p[0].as_str().unwrap())).unwrap_or_else(|e| panic!("{p}: {e}"));
        }
        let odd = d["patterns"].as_array().unwrap().iter().find(|p| p[2] == "怪 {n} {x}").unwrap();
        let re = regex::Regex::new(&format!("^{}$", odd[0].as_str().unwrap())).unwrap();
        assert!(re.is_match(r"Odd [x] (y) 12.*+? $ ^ | \ / tail"));
    }

    #[test]
    fn only_antigravity_answers_count() {
        let ua = |s: &str| json!({ "Browser": "Chrome/152", "User-Agent": s });
        assert!(is_antigravity(&ua("Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Antigravity/2.23.1 Chrome/152.0 Electron/44.3.0 Safari/537.36")));
        assert!(!is_antigravity(&ua("Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36")));
        assert!(!is_antigravity(&ua("Mozilla/5.0 Code/1.99 Chrome/130 Electron/34")));
        assert!(!is_antigravity(&json!({})));
    }

    #[test]
    fn the_switch_lives_in_the_windows_section() {
        let mut root = json!({ "antigravity@wsl-x": { "zhUi": true } });
        assert!(!is_on(&root) && flag(&root).is_none());
        set_on(&mut root, true);
        set_lines(&mut root, &["Turbo = 极速".into()]);
        assert_eq!(root["antigravity"], json!({ "zhUi": true, "zhCustom": ["Turbo = 极速"] }));
        assert!(is_on(&root));
        assert_eq!(lines(&root), ["Turbo = 极速"]);
        // A broken section is replaced, not a panic.
        let mut bad = json!({ "antigravity": 3 });
        set_on(&mut bad, false);
        assert_eq!(flag(&bad), Some(false));
    }

    #[test]
    fn settle_waits_no_longer_than_asked() {
        // No thread runs in tests: a pending change is waited for only up to the limit.
        lock(&GEN).asked += 1;
        let t = std::time::Instant::now();
        settle(Duration::from_millis(200));
        assert!(t.elapsed() >= Duration::from_millis(150) && t.elapsed() < Duration::from_secs(2));
    }

    /// Against the Antigravity running on this machine: switch on with a custom entry, check
    /// the window is translated, switch off, check it is back to English.
    /// `cargo test --lib agzh::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live() {
        let _h = crate::util::TestHome::new("agzh-live");
        let port = port().expect("Antigravity's DevToolsActivePort");
        let mut w = Watcher::default();
        let text = |s: &mut Session| eval(s, "document.body.innerText").unwrap().as_str().unwrap_or_default().to_string();
        let mut root = json!({});
        set_on(&mut root, true);
        set_lines(&mut root, &["Settings = 偏好设置".into()]);
        crate::store::save(&root).unwrap();
        sync();
        w.tick();
        assert!(windows() >= 1, "no window on port {port}");
        let zh = text(w.open.values_mut().next().unwrap());
        println!("{}", zh.chars().take(300).collect::<String>());
        assert!(zh.contains("新建对话") && zh.contains("偏好设置"), "{zh}");
        // Checked again: already current, nothing evaluated.
        w.tick();
        set_on(&mut root, false);
        crate::store::save(&root).unwrap();
        sync();
        w.tick();
        assert_eq!(windows(), 0);
        let ws = crate::cdp::targets(port).unwrap().into_iter().find(|t| app_page(t["url"].as_str().unwrap_or(""))).unwrap();
        let en = text(&mut Session::open(ws["webSocketDebuggerUrl"].as_str().unwrap(), port).unwrap());
        assert!(en.contains("New Conversation") && !en.contains("新建对话"), "{en}");
    }
}
