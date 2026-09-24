//! UI injection for Codex.
//!
//! We start Codex with a local DevTools port and rewrite small expressions in the
//! UI bundle. Each patch is optional:
//! - Fast: Codex's UI hides the Fast option unless the auth method is "chatgpt":
//!   `isServiceTierAllowed = authMethod==="chatgpt" && !loading && requirements?.featureRequirements?.fast_mode!==false`
//!   We drop the auth check, keeping the admin switch (`fast_mode === false`) intact.
//! - Full model names: the model-name formatter takes `stripGptPrefix` and turns
//!   "GPT-6 Sol" into "6 Sol" unless a Statsig gate (only granted to ChatGPT accounts)
//!   is on. We make it always return the full name.
//! - Quota: signed in with ChatGPT, an atom disables the composer's send button once
//!   the account's `rate_limit.allowed` is false and a usage window is at 0%. With the
//!   official sign-in mix the requests go to the relay, so we make it always false.
//!
//! The patches live in different bundles (app-initial, app-primary); every bundle is
//! tried with every patch, and a patch is missing only when no bundle has it.
//!
//! Strategy 1: intercept the bundle responses on reload (Fetch domain).
//! Strategy 2: live-edit the already-loaded scripts (Debugger.setScriptSource).

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
const QUOTA_MARK: &str = "/*agentplus-quota*/";
const BUNDLE_HINTS: [&str; 2] = ["app-initial-", "app-primary-"];

/// Which UI patches to apply.
#[derive(Clone, Copy, Default)]
pub struct Patches {
    pub fast: bool,
    pub full_names: bool,
    pub quota: bool,
}

impl Patches {
    pub fn any(self) -> bool {
        self.fast || self.full_names || self.quota
    }

    fn flags(self) -> [bool; 3] {
        [self.fast, self.full_names, self.quota]
    }

    /// Display names, in the order `patch_source` applies them.
    fn names() -> [&'static str; 3] {
        ["Fast", crate::i18n::l("完整模型名", "Full model names"), crate::i18n::l("额度用完仍可发送", "Send after quota runs out")]
    }

    fn wanted(self) -> Vec<&'static str> {
        self.flags().into_iter().zip(Self::names()).filter(|(on, _)| *on).map(|(_, n)| n).collect()
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

/// Makes the "rate limit reached, block sending" atom return false up front:
///   ({get:e})=>{let t=e(Zx),n=e(iT).data;if(t.authMethod!==`chatgpt`||…||n.rate_limit?.allowed!==!1||…)return!1;…
///   → ({get:e})=>{return!1;let t=…
fn patch_quota(src: &str) -> Option<String> {
    let re = Regex::new(
        r"\(\{get:([\w$]+)\}\)=>\{(let [\w$]+=[\w$]+\([\w$]+\),[\w$]+=[\w$]+\([\w$]+\)\.data;if\([\w$]+\.authMethod!==`chatgpt`[^;{}]{0,400}?\.rate_limit\?\.allowed!==!1)",
    )
    .unwrap();
    let out = re.replacen(src, 1, format!("({{get:${{1}}}})=>{{return!1{QUOTA_MARK};${{2}}"));
    if out == src { None } else { Some(out.into_owned()) }
}

/// Applies the wanted patches that aren't in `src` yet. Returns the patched source
/// (`None` when nothing changed) and the patches whose code couldn't be found.
pub fn patch_source(src: &str, want: Patches) -> (Option<String>, Vec<&'static str>) {
    type Patch = (&'static str, fn(&str) -> Option<String>);
    let patches: [Patch; 3] = [(FAST_MARK, patch_fast), (NAMES_MARK, patch_names), (QUOTA_MARK, patch_quota)];
    let mut out: Option<String> = None;
    let mut missing = vec![];
    for (i, ((mark, f), on)) in patches.into_iter().zip(want.flags()).enumerate() {
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

/// Patches missing from every bundle seen so far (`acc` is `None` before the first one).
fn still_missing(acc: Option<Vec<&'static str>>, miss: Vec<&'static str>) -> Vec<&'static str> {
    match acc {
        None => miss,
        Some(a) => a.into_iter().filter(|m| miss.contains(m)).collect(),
    }
}

/// Every wanted patch missing means nothing was injected: that's an error.
fn check_missing(want: Patches, missing: Vec<&'static str>) -> Result<Vec<&'static str>> {
    if !missing.is_empty() && missing.len() == want.wanted().len() {
        Err(not_found(&missing))
    } else {
        Ok(missing)
    }
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

fn bundle_of(url: &str) -> Option<&'static str> {
    BUNDLE_HINTS.into_iter().find(|h| url.contains(h))
}

/// Patches one paused bundle response and lets it through. Returns the patches not found in it.
fn fulfill(s: &mut Session, p: &Value, want: Patches) -> Result<Vec<&'static str>> {
    let rid = p["requestId"].as_str().unwrap_or_default().to_string();
    let body = s.call("Fetch.getResponseBody", json!({ "requestId": rid }))?;
    let raw = body["body"].as_str().unwrap_or_default();
    let text = if body["base64Encoded"].as_bool().unwrap_or(false) {
        String::from_utf8(B64.decode(raw)?)?
    } else {
        raw.to_string()
    };
    let (patched, missing) = patch_source(&text, want);
    match patched {
        Some(patched) => {
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
        }
        None => {
            s.call("Fetch.continueRequest", json!({ "requestId": rid }))?;
        }
    }
    Ok(missing)
}

/// `Ok(None)` when no bundle request was seen; otherwise the patches not found in any bundle.
fn via_fetch(s: &mut Session, want: Patches) -> Result<Option<Vec<&'static str>>> {
    let patterns: Vec<Value> = BUNDLE_HINTS.iter().map(|h| json!({ "urlPattern": format!("*{h}*"), "requestStage": "Response" })).collect();
    s.call("Fetch.enable", json!({ "patterns": patterns }))?;
    s.call("Page.reload", json!({ "ignoreCache": true }))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<&str> = vec![];
    let mut missing: Option<Vec<&'static str>> = None;
    let result = (|| -> Result<()> {
        while seen.len() < BUNDLE_HINTS.len() {
            let Some(ev) = s.next_event("Fetch.requestPaused", deadline)? else { break };
            let p = &ev["params"];
            if let Some(h) = bundle_of(p["request"]["url"].as_str().unwrap_or("")) {
                if !seen.contains(&h) {
                    seen.push(h);
                }
            }
            let miss = fulfill(s, p, want)?;
            missing = Some(still_missing(missing.take(), miss));
        }
        Ok(())
    })();
    let _ = s.call("Fetch.disable", json!({}));
    result?;
    missing.map(|m| check_missing(want, m)).transpose()
}

/// Returns the patches not found in any loaded bundle.
fn via_live_edit(s: &mut Session, want: Patches) -> Result<Vec<&'static str>> {
    s.call("Debugger.enable", json!({}))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut scripts: Vec<(&str, String)> = vec![];
    while scripts.len() < BUNDLE_HINTS.len() {
        let Some(ev) = s.next_event("Debugger.scriptParsed", deadline)? else { break };
        if let (Some(h), Some(id)) = (bundle_of(ev["params"]["url"].as_str().unwrap_or("")), ev["params"]["scriptId"].as_str()) {
            if !scripts.iter().any(|(x, _)| *x == h) {
                scripts.push((h, id.to_string()));
            }
        }
    }
    if scripts.is_empty() {
        let _ = s.call("Debugger.disable", json!({}));
        return Err(anyhow!(crate::i18n::l("没有找到 Codex 界面脚本", "Codex UI script not found")));
    }
    let result = (|| -> Result<Vec<&'static str>> {
        let mut missing: Option<Vec<&'static str>> = None;
        for (_, id) in &scripts {
            let src = s.call("Debugger.getScriptSource", json!({ "scriptId": id }))?;
            let (patched, miss) = patch_source(src["scriptSource"].as_str().unwrap_or_default(), want);
            missing = Some(still_missing(missing.take(), miss));
            let Some(patched) = patched else { continue };
            let r = s.call("Debugger.setScriptSource", json!({ "scriptId": id, "scriptSource": patched }))?;
            if let Some(ex) = r.get("exceptionDetails") {
                return Err(anyhow!(tr!("热替换失败：{ex}", "Live patch failed: {ex}")));
            }
            if let Some(st) = r.get("status").and_then(|x| x.as_str()) {
                if st != "Ok" {
                    return Err(anyhow!(tr!("热替换失败：{st}", "Live patch failed: {st}")));
                }
            }
        }
        Ok(missing.unwrap_or_default())
    })();
    let _ = s.call("Debugger.disable", json!({}));
    check_missing(want, result?)
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
        crate::process::pause(Duration::from_millis(500))?;
    };
    on(Progress::step("port", "done", Some(tr!("{} 个窗口", "{} window(s)", pages.len()))));
    // Let the first render settle before reloading.
    on(Progress::step("patch", "active", Some(crate::i18n::l("等待界面加载", "Waiting for the UI to load").into())));
    crate::process::pause(Duration::from_secs(2))?;
    let total = pages.len();
    let sep = crate::i18n::l("、", ", ");
    let mut done = vec![];
    let mut missing: Vec<&str> = vec![];
    for (i, page) in pages.into_iter().enumerate() {
        // A window being patched is finished first: its requests are paused until then.
        crate::process::check_cancel()?;
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

    const FAST: Patches = Patches { fast: true, full_names: false, quota: false };
    const NAMES: Patches = Patches { fast: false, full_names: true, quota: false };
    const QUOTA: Patches = Patches { fast: false, full_names: false, quota: true };
    const BOTH: Patches = Patches { fast: true, full_names: true, quota: false };
    const ALL: Patches = Patches { fast: true, full_names: true, quota: true };
    const GATE: &str = "let x=1;d=a&&!u&&c!=null&&c?.requirements?.featureRequirements?.fast_mode!==!1,f;";
    const STRIP: &str = "join(``);return t?r.replace(/^GPT-/iu,``):r}function Cpa(){";
    // Codex 26.917 app-primary bundle.
    const BLOCK: &str = "Cu(),LK=Hs(Ir,({get:e})=>{let t=e(Zx),n=e(iT).data;if(t.authMethod!==`chatgpt`||t.authLoading||t.accountLoading||t.authenticatedAccountId==null||t.accountId!==t.authenticatedAccountId||n?.account_id!==t.authenticatedAccountId||n.user_id!==t.userId||n.plan_type!==t.plan||n.rate_limit?.allowed!==!1||cOe(n)||F_({rateLimitStatus:n,isWorkspaceAccount:t.accountStructure===`workspace`})||e(hS,`local`))return!1;let r=gpe(n)";

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

    #[test]
    fn unblocks_send_on_rate_limit() {
        let (out, missing) = patch_source(BLOCK, QUOTA);
        let out = out.unwrap();
        assert!(missing.is_empty());
        assert!(out.starts_with("Cu(),LK=Hs(Ir,({get:e})=>{return!1/*agentplus-quota*/;let t=e(Zx),n=e(iT).data;if(t.authMethod!==`chatgpt`"));
        assert_eq!(out.len(), BLOCK.len() + "return!1/*agentplus-quota*/;".len());
        assert_eq!(patch_source(&out, QUOTA), (None, vec![]));
        // A different atom without the rate-limit check is left alone.
        let other = "x=Hs(Ir,({get:e})=>{let t=e(Zx),n=e(iT).data;if(t.authMethod!==`chatgpt`||t.authLoading)return!1;";
        assert_eq!(patch_source(other, QUOTA), (None, vec![crate::i18n::l("额度用完仍可发送", "Send after quota runs out")]));
    }

    #[test]
    fn missing_only_when_no_bundle_has_it() {
        // app-initial has Fast and names, app-primary has the quota atom.
        let (_, a) = patch_source(&format!("{GATE}{STRIP}"), ALL);
        let (_, b) = patch_source(BLOCK, ALL);
        let m = still_missing(Some(still_missing(None, a)), b);
        assert!(m.is_empty());
        assert!(check_missing(ALL, m).unwrap().is_empty());
        let (_, a) = patch_source(STRIP, ALL);
        let m = still_missing(None, a);
        assert_eq!(m.len(), 2);
        assert_eq!(check_missing(ALL, m).unwrap().len(), 2);
        let (_, a) = patch_source("nothing here", QUOTA);
        assert!(check_missing(QUOTA, still_missing(None, a)).is_err());
    }
}
