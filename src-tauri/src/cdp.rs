//! UI injection for Codex.
//!
//! We start Codex with a local DevTools port and rewrite small expressions in the
//! UI bundle. Each patch is optional:
//! - Fast: Codex's UI hides the Fast option unless the auth method is "chatgpt":
//!     isServiceTierAllowed = authMethod==="chatgpt" && !loading && requirements?.featureRequirements?.fast_mode!==false
//!   We drop the auth check, keeping the admin switch (`fast_mode === false`) intact.
//! - Full model names: the model-name formatter takes `stripGptPrefix` and turns
//!   "GPT-6 Sol" into "6 Sol" unless a Statsig gate (only granted to ChatGPT accounts)
//!   is on. We make it always return the full name.
//! Strategy 1: intercept the bundle response on reload (Fetch domain).
//! Strategy 2: live-edit the already-loaded script (Debugger.setScriptSource).

use crate::process::Progress;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

pub const PORT: u16 = 39229;
const FAST_MARK: &str = "/*agentplus-fast*/";
const NAMES_MARK: &str = "/*agentplus-names*/";
const BUNDLE_HINT: &str = "app-initial-";

/// Which UI patches to apply.
#[derive(Clone, Copy, Default)]
pub struct Patches {
    pub fast: bool,
    pub full_names: bool,
}

impl Patches {
    pub fn any(self) -> bool {
        self.fast || self.full_names
    }

    /// Display names, in the order `patch_source` applies them.
    fn names() -> [&'static str; 2] {
        ["Fast", crate::i18n::l("完整模型名", "Full model names")]
    }

    fn wanted(self) -> Vec<&'static str> {
        [self.fast, self.full_names].into_iter().zip(Self::names()).filter(|(on, _)| *on).map(|(_, n)| n).collect()
    }
}

/// Rewrites the Fast gate. Minified names change per release, so match on shape.
fn patch_fast(src: &str) -> Option<String> {
    let re = Regex::new(
        r"([\w$]+)=[\w$]+&&!([\w$]+)&&([\w$]+)!=null&&[\w$]+\?\.requirements\?\.featureRequirements\?\.fast_mode!==!1",
    )
    .unwrap();
    let out = re.replacen(src, 1, format!("${{1}}=!${{2}}&&${{3}}?.requirements?.featureRequirements?.fast_mode!==!1{FAST_MARK}"));
    if out == src { None } else { Some(out.into_owned()) }
}

/// Makes the model-name formatter ignore `stripGptPrefix`:
///   return t?r.replace(/^GPT-/iu,``):r  →  return r
fn patch_names(src: &str) -> Option<String> {
    let re = Regex::new(r"return [\w$]+\?[\w$]+\.replace\(/\^GPT-/iu,``\):([\w$]+)").unwrap();
    let out = re.replacen(src, 1, format!("return ${{1}}{NAMES_MARK}"));
    if out == src { None } else { Some(out.into_owned()) }
}

/// Applies the wanted patches that aren't in `src` yet. Returns the patched source
/// (`None` when nothing changed) and the patches whose code couldn't be found.
pub fn patch_source(src: &str, want: Patches) -> (Option<String>, Vec<&'static str>) {
    type Patch = (bool, &'static str, fn(&str) -> Option<String>);
    let patches: [Patch; 2] = [(want.fast, FAST_MARK, patch_fast), (want.full_names, NAMES_MARK, patch_names)];
    let mut out: Option<String> = None;
    let mut missing = vec![];
    for (i, (on, mark, f)) in patches.into_iter().enumerate() {
        let cur = out.as_deref().unwrap_or(src);
        if !on || cur.contains(mark) {
            continue;
        }
        match f(cur) {
            Some(s) => out = Some(s),
            None => missing.push(Patches::names()[i]),
        }
    }
    (out, missing)
}

fn not_found(missing: &[&str]) -> anyhow::Error {
    anyhow!(tr!(
        "在 Codex 界面代码里没找到「{}」的注入位置，可能是版本变了",
        "Couldn't find where to patch {} in the Codex UI code; the version may have changed",
        missing.join(crate::i18n::l("、", ", "))
    ))
}

struct Session {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
    events: VecDeque<Value>,
}

impl Session {
    fn open(url: &str) -> Result<Self> {
        let (ws, _) = tungstenite::connect(url)?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_millis(400)))?;
        }
        Ok(Session { ws, next: 1, events: VecDeque::new() })
    }

    /// Reads one message; `None` on read timeout.
    fn read(&mut self) -> Result<Option<Value>> {
        match self.ws.read() {
            Ok(Message::Text(t)) => Ok(Some(serde_json::from_str(&t)?)),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.ws.send(Message::Text(json!({ "id": id, "method": method, "params": params }).to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(msg) = self.read()? {
                if msg.get("id").and_then(|x| x.as_u64()) == Some(id) {
                    if let Some(err) = msg.get("error") {
                        return Err(anyhow!(tr!("{method} 失败：{err}", "{method} failed: {err}")));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                if msg.get("method").is_some() {
                    self.events.push_back(msg);
                }
            }
        }
        Err(anyhow!(tr!("{method} 超时", "{method} timed out")))
    }

    fn next_event(&mut self, method: &str, deadline: Instant) -> Result<Option<Value>> {
        loop {
            if let Some(pos) = self.events.iter().position(|e| e.get("method").and_then(|m| m.as_str()) == Some(method)) {
                return Ok(self.events.remove(pos));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            if let Some(msg) = self.read()? {
                if msg.get("method").is_some() {
                    self.events.push_back(msg);
                }
            }
        }
    }
}

fn page_targets(port: u16) -> Result<Vec<Value>> {
    let v: Value = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .timeout(Duration::from_secs(2))
        .send()?
        .json_value()?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
        .filter(|t| t.get("url").and_then(|x| x.as_str()).map(|u| u.starts_with("app://")).unwrap_or(false))
        .collect())
}

trait JsonValue {
    fn json_value(self) -> Result<Value>;
}
impl JsonValue for reqwest::blocking::Response {
    fn json_value(self) -> Result<Value> {
        Ok(serde_json::from_str(&self.text()?)?)
    }
}

/// `Ok(None)` when the bundle request wasn't seen; otherwise the patches not found.
fn via_fetch(s: &mut Session, want: Patches) -> Result<Option<Vec<&'static str>>> {
    s.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": format!("*{BUNDLE_HINT}*"), "requestStage": "Response" }] }))?;
    s.call("Page.reload", json!({ "ignoreCache": true }))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = match s.next_event("Fetch.requestPaused", deadline)? {
        None => Ok(None),
        Some(ev) => {
            let p = &ev["params"];
            let rid = p["requestId"].as_str().unwrap_or_default().to_string();
            let body = s.call("Fetch.getResponseBody", json!({ "requestId": rid }))?;
            let raw = body["body"].as_str().unwrap_or_default();
            let text = if body["base64Encoded"].as_bool().unwrap_or(false) {
                String::from_utf8(B64.decode(raw)?)?
            } else {
                raw.to_string()
            };
            match patch_source(&text, want) {
                (Some(patched), missing) => {
                    let headers: Vec<Value> = p["responseHeaders"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|h| !h["name"].as_str().unwrap_or("").eq_ignore_ascii_case("content-length"))
                        .collect();
                    s.call("Fetch.fulfillRequest", json!({
                        "requestId": rid,
                        "responseCode": p["responseStatusCode"].as_u64().unwrap_or(200),
                        "responseHeaders": headers,
                        "body": B64.encode(patched.as_bytes()),
                    }))?;
                    Ok(Some(missing))
                }
                (None, missing) => {
                    s.call("Fetch.continueRequest", json!({ "requestId": rid }))?;
                    if missing.is_empty() { Ok(Some(missing)) } else { Err(not_found(&missing)) }
                }
            }
        }
    };
    let _ = s.call("Fetch.disable", json!({}));
    result
}

/// Returns the patches not found.
fn via_live_edit(s: &mut Session, want: Patches) -> Result<Vec<&'static str>> {
    s.call("Debugger.enable", json!({}))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut script_id = None;
    while script_id.is_none() {
        match s.next_event("Debugger.scriptParsed", deadline)? {
            Some(ev) => {
                if ev["params"]["url"].as_str().unwrap_or("").contains(BUNDLE_HINT) {
                    script_id = ev["params"]["scriptId"].as_str().map(String::from);
                }
            }
            None => break,
        }
    }
    let id = script_id.ok_or_else(|| anyhow!(crate::i18n::l("没有找到 Codex 界面脚本", "Codex UI script not found")))?;
    let src = s.call("Debugger.getScriptSource", json!({ "scriptId": id }))?;
    let text = src["scriptSource"].as_str().unwrap_or_default();
    let (patched, missing) = match patch_source(text, want) {
        (Some(p), missing) => (p, missing),
        (None, missing) => {
            let _ = s.call("Debugger.disable", json!({}));
            return if missing.is_empty() { Ok(missing) } else { Err(not_found(&missing)) };
        }
    };
    let r = s.call("Debugger.setScriptSource", json!({ "scriptId": id, "scriptSource": patched }))?;
    let _ = s.call("Debugger.disable", json!({}));
    if let Some(ex) = r.get("exceptionDetails") {
        return Err(anyhow!(tr!("热替换失败：{ex}", "Live patch failed: {ex}")));
    }
    if let Some(st) = r.get("status").and_then(|x| x.as_str()) {
        if st != "Ok" {
            return Err(anyhow!(tr!("热替换失败：{st}", "Live patch failed: {st}")));
        }
    }
    Ok(missing)
}

/// Waits for Codex windows on the debug port and patches each one.
pub fn inject(port: u16, want: Patches, on: &dyn Fn(Progress)) -> Result<String> {
    on(Progress::step("port", "active", Some(port.to_string())));
    let t0 = Instant::now();
    let pages = loop {
        if let Ok(p) = page_targets(port) {
            if !p.is_empty() {
                break p;
            }
        }
        if t0.elapsed() > Duration::from_secs(60) {
            return Err(anyhow!(tr!("60 秒内没有连上 Codex 的调试端口 {port}", "Couldn't connect to Codex's debug port {port} within 60 seconds")));
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    on(Progress::step("port", "done", Some(tr!("{} 个窗口", "{} window(s)", pages.len()))));
    // Let the first render settle before reloading.
    on(Progress::step("patch", "active", Some(crate::i18n::l("等待界面加载", "Waiting for the UI to load").into())));
    std::thread::sleep(Duration::from_secs(2));
    let total = pages.len();
    let sep = crate::i18n::l("、", ", ");
    let mut done = vec![];
    let mut missing: Vec<&str> = vec![];
    for (i, page) in pages.into_iter().enumerate() {
        on(Progress::step("patch", "active", Some(tr!("窗口 {}/{}", "Window {}/{}", i + 1, total))));
        let ws = page["webSocketDebuggerUrl"].as_str().ok_or_else(|| anyhow!(crate::i18n::l("调试目标缺少 WebSocket 地址", "Debug target has no WebSocket URL")))?;
        let mut s = Session::open(ws)?;
        let (how, miss) = match via_fetch(&mut s, want)? {
            Some(m) => (crate::i18n::l("响应拦截", "response interception"), m),
            None => (crate::i18n::l("热替换", "live patch"), via_live_edit(&mut s, want)?),
        };
        done.push(how);
        for m in miss {
            if !missing.contains(&m) {
                missing.push(m);
            }
        }
    }
    let what: Vec<&str> = want.wanted().into_iter().filter(|w| !missing.contains(w)).collect();
    on(if missing.is_empty() {
        Progress::step("patch", "done", Some(what.join(sep)))
    } else {
        Progress::step("patch", "warn", Some(tr!("没找到：{}", "Not found: {}", missing.join(sep))))
    });
    let mut msg = tr!("界面已注入：{}（{}，{} 个窗口）", "UI patched: {} ({}; {} window(s))", what.join(sep), done.join(sep), done.len());
    if !missing.is_empty() {
        msg.push_str(&tr!("；没找到「{}」的注入位置，可能是 Codex 版本变了", "; couldn't find where to patch {}, the Codex version may have changed", missing.join(sep)));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Patches = Patches { fast: true, full_names: false };
    const NAMES: Patches = Patches { fast: false, full_names: true };
    const BOTH: Patches = Patches { fast: true, full_names: true };
    const GATE: &str = "let x=1;d=a&&!u&&c!=null&&c?.requirements?.featureRequirements?.fast_mode!==!1,f;";
    const STRIP: &str = "join(``);return t?r.replace(/^GPT-/iu,``):r}function Cpa(){";

    #[test]
    fn patches_gate_once() {
        let (out, missing) = patch_source(GATE, FAST);
        let out = out.unwrap();
        assert!(missing.is_empty());
        assert!(out.contains("d=!u&&c?.requirements?.featureRequirements?.fast_mode!==!1/*agentplus-fast*/"));
        assert_eq!(patch_source(&out, FAST), (None, vec![]));
    }

    #[test]
    fn keeps_gpt_prefix() {
        let (out, _) = patch_source(STRIP, NAMES);
        let out = out.unwrap();
        assert_eq!(out, "join(``);return r/*agentplus-names*/}function Cpa(){");
        assert_eq!(patch_source(&out, NAMES), (None, vec![]));
        // Not requested → untouched.
        assert_eq!(patch_source(STRIP, FAST), (None, vec!["Fast"]));
    }

    #[test]
    fn applies_what_it_finds() {
        let src = format!("{GATE}{STRIP}");
        let (out, missing) = patch_source(&src, BOTH);
        let out = out.unwrap();
        assert!(missing.is_empty());
        assert!(out.contains("/*agentplus-fast*/") && out.contains("/*agentplus-names*/"));
        let (out, missing) = patch_source(STRIP, BOTH);
        assert!(out.unwrap().contains("/*agentplus-names*/"));
        assert_eq!(missing, vec!["Fast"]);
    }
}
