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
//! - Usage banners: the composer's usage-banner slot shows the account's limit banners
//!   (server-sent `rate_limit_upsell` ones like "You're out of Codex and Work usage", and
//!   the app's own warnings). Those are about the ChatGPT quota, not the relay, so we
//!   make the slot always show what it would without them.
//!
//! The patches live in different bundles (app-initial, app-primary); every bundle is
//! tried with every patch, and a patch is missing only when no bundle has it.
//!
//! Strategy 1: intercept the bundle responses on reload (Fetch domain). The cache is off
//! for the reload: app-primary is otherwise often served from the memory cache, which
//! never reaches the interception.
//! Strategy 2: live-edit the already-loaded scripts (Debugger.setScriptSource).

use crate::i18n::join;
use crate::process::Progress;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use regex::Regex;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

pub const PORT: u16 = 39229;
const FAST_MARK: &str = "/*agentplus-fast*/";
const NAMES_MARK: &str = "/*agentplus-names*/";
const QUOTA_MARK: &str = "/*agentplus-quota*/";
const BANNER_MARK: &str = "/*agentplus-banner*/";
const BUNDLE_HINTS: [&str; 2] = ["app-initial-", "app-primary-"];

/// Which UI patches to apply.
#[derive(Clone, Copy, Default)]
pub struct Patches {
    pub fast: bool,
    pub full_names: bool,
    pub quota: bool,
    pub usage_banner: bool,
}

impl Patches {
    pub fn any(self) -> bool {
        self.flags().contains(&true)
    }

    fn flags(self) -> [bool; 4] {
        [self.fast, self.full_names, self.quota, self.usage_banner]
    }

    /// Display names, in the order `patch_source` applies them.
    fn names() -> [&'static str; 4] {
        [
            "Fast",
            crate::i18n::l("完整模型名", "Full model names"),
            crate::i18n::l("额度用完仍可发送", "Send after quota runs out"),
            crate::i18n::l("隐藏用量提示横幅", "Hide usage banners"),
        ]
    }

    fn wanted(self) -> Vec<&'static str> {
        self.flags().into_iter().zip(Self::names()).filter(|(on, _)| *on).map(|(_, n)| n).collect()
    }
}

/// A regex compiled once, on first use (patches run for every bundle of every window).
fn cached(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// The first match replaced, or None when there is none. Every replacement adds a marker,
/// so a match always changes the text.
fn replace_once(src: &str, re: &Regex, rep: impl regex::Replacer) -> Option<String> {
    match re.replacen(src, 1, rep) {
        Cow::Borrowed(_) => None,
        Cow::Owned(s) => Some(s),
    }
}

/// Rewrites the Fast gate. Minified names change per release, so match on shape.
fn patch_fast(src: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = cached(&RE, r"([\w$]+)=[\w$]+&&!([\w$]+)&&([\w$]+)!=null&&[\w$]+\?\.requirements\?\.featureRequirements\?\.fast_mode!==!1");
    replace_once(src, re, format!("${{1}}=!${{2}}&&${{3}}?.requirements?.featureRequirements?.fast_mode!==!1{FAST_MARK}"))
}

/// Makes the model-name formatter ignore `stripGptPrefix`:
///   return t?r.replace(/^GPT-/iu,``):r  →  return r
fn patch_names(src: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = cached(&RE, r"return [\w$]+\?[\w$]+\.replace\(/\^GPT-/iu,``\):([\w$]+)");
    replace_once(src, re, format!("return ${{1}}{NAMES_MARK}"))
}

/// Makes the "rate limit reached, block sending" atom return false up front:
///   ({get:e})=>{let t=e(Zx),n=e(iT).data;if(t.authMethod!==`chatgpt`||…||n.rate_limit?.allowed!==!1||…)return!1;…
///   → ({get:e})=>{return!1;let t=…
fn patch_quota(src: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = cached(
        &RE,
        r"\(\{get:([\w$]+)\}\)=>\{(let [\w$]+=[\w$]+\([\w$]+\),[\w$]+=[\w$]+\([\w$]+\)\.data;if\([\w$]+\.authMethod!==`chatgpt`[^;{}]{0,400}?\.rate_limit\?\.allowed!==!1)",
    );
    replace_once(src, re, format!("({{get:${{1}}}})=>{{return!1{QUOTA_MARK};${{2}}"))
}

/// Makes the composer's usage-banner slot return its fallback content right away:
///   N=F($v,_);if(!n)return u;let P=pYe({hasImageGenerationLimit:…
///   → N=F($v,_);if(!0)return u;let P=…
/// The early return already exists (for `canShowUsageBanners` off), so every hook before
/// it still runs as usual.
fn patch_banner(src: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = cached(&RE, r"if\(![\w$]+\)return ([\w$]+);(let [\w$]+=[\w$]+\(\{hasImageGenerationLimit:)");
    replace_once(src, re, format!("if(!0)return ${{1}}{BANNER_MARK};${{2}}"))
}

/// Applies the wanted patches that aren't in `src` yet. Returns the patched source
/// (`None` when nothing changed) and the patches whose code couldn't be found.
pub fn patch_source(src: &str, want: Patches) -> (Option<String>, Vec<&'static str>) {
    type Patch = (&'static str, fn(&str) -> Option<String>);
    let patches: [Patch; 4] = [(FAST_MARK, patch_fast), (NAMES_MARK, patch_names), (QUOTA_MARK, patch_quota), (BANNER_MARK, patch_banner)];
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
        join(missing)
    ))
}

/// Events kept for `next_event`; others (Network.* while the cache is off) are dropped.
fn buffered(msg: &Value) -> bool {
    matches!(msg.get("method").and_then(|m| m.as_str()), Some("Fetch.requestPaused" | "Debugger.scriptParsed"))
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
                if buffered(&msg) {
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
                if buffered(&msg) {
                    self.events.push_back(msg);
                }
            }
        }
    }
}

fn page_targets(port: u16) -> Result<Vec<Value>> {
    let resp = reqwest::blocking::Client::new().get(format!("http://127.0.0.1:{port}/json/list")).timeout(Duration::from_secs(2)).send()?;
    let v: Value = serde_json::from_str(&resp.text()?)?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
        .filter(|t| t.get("url").and_then(|x| x.as_str()).map(|u| u.starts_with("app://")).unwrap_or(false))
        .collect())
}

/// The bundle a script URL belongs to. Only `.js` counts: each bundle has a same-named
/// `.css` that loads first and would otherwise mark the bundle as seen.
fn bundle_of(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let file = path.rsplit('/').next().unwrap_or_default();
    if !file.ends_with(".js") {
        return None;
    }
    BUNDLE_HINTS.into_iter().find(|h| file.starts_with(h))
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

/// The outcome in one window.
struct Patched {
    /// Patches not found in any bundle of the window.
    missing: Vec<&'static str>,
    /// Every bundle was there (overlay windows only load app-initial).
    complete: bool,
}

/// `Ok(None)` when no bundle request was seen. Once the first bundle is through, waits
/// up to `lazy` for the rest, calling `waiting` first.
fn via_fetch(s: &mut Session, want: Patches, lazy: Duration, waiting: &dyn Fn()) -> Result<Option<Patched>> {
    let patterns: Vec<Value> = BUNDLE_HINTS.iter().map(|h| json!({ "urlPattern": format!("*{h}*.js*"), "requestStage": "Response" })).collect();
    s.call("Fetch.enable", json!({ "patterns": patterns }))?;
    s.call("Network.enable", json!({}))?;
    s.call("Network.setCacheDisabled", json!({ "cacheDisabled": true }))?;
    s.call("Page.reload", json!({ "ignoreCache": true }))?;
    let mut deadline = Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<&str> = vec![];
    let mut missing: Option<Vec<&'static str>> = None;
    let result = (|| -> Result<()> {
        while seen.len() < BUNDLE_HINTS.len() {
            let Some(ev) = s.next_event("Fetch.requestPaused", deadline)? else { break };
            let p = &ev["params"];
            let Some(h) = bundle_of(p["request"]["url"].as_str().unwrap_or("")) else {
                s.call("Fetch.continueRequest", json!({ "requestId": p["requestId"] }))?;
                continue;
            };
            if !seen.contains(&h) {
                seen.push(h);
                // app-primary is imported lazily by app-initial once the app is up; right
                // after Codex starts that can take well over 10 s.
                if seen.len() == 1 {
                    deadline = Instant::now() + lazy;
                    waiting();
                }
            }
            let miss = fulfill(s, p, want)?;
            missing = Some(still_missing(missing.take(), miss));
        }
        Ok(())
    })();
    let _ = s.call("Fetch.disable", json!({}));
    let _ = s.call("Network.setCacheDisabled", json!({ "cacheDisabled": false }));
    let _ = s.call("Network.disable", json!({}));
    result?;
    Ok(missing.map(|missing| Patched { missing, complete: seen.len() == BUNDLE_HINTS.len() }))
}

/// `Ok(None)` when the window has no bundle loaded (overlay and detached windows don't);
/// otherwise what was patched.
fn via_live_edit(s: &mut Session, want: Patches) -> Result<Option<Patched>> {
    s.call("Debugger.enable", json!({}))?;
    // Already-loaded scripts are reported right away; stop once they've gone quiet.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut quiet = Instant::now() + Duration::from_millis(1500);
    let mut scripts: Vec<(&str, String)> = vec![];
    while scripts.len() < BUNDLE_HINTS.len() {
        let Some(ev) = s.next_event("Debugger.scriptParsed", deadline.min(quiet))? else { break };
        quiet = Instant::now() + Duration::from_millis(1500);
        if let (Some(h), Some(id)) = (bundle_of(ev["params"]["url"].as_str().unwrap_or("")), ev["params"]["scriptId"].as_str()) {
            if !scripts.iter().any(|(x, _)| *x == h) {
                scripts.push((h, id.to_string()));
            }
        }
    }
    if scripts.is_empty() {
        let _ = s.call("Debugger.disable", json!({}));
        return Ok(None);
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
    Ok(Some(Patched { missing: result?, complete: scripts.len() == BUNDLE_HINTS.len() }))
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
    // Main windows first: overlay windows never load app-primary, so once a main window
    // has had every bundle there's no point waiting long for it elsewhere.
    let mut pages = pages;
    pages.sort_by_key(|p| p["url"].as_str().unwrap_or("").contains('?'));
    let mut lazy = Duration::from_secs(40);
    let total = pages.len();
    let mut done = vec![];
    let mut missing: Option<Vec<&'static str>> = None;
    for (i, page) in pages.into_iter().enumerate() {
        // A window being patched is finished first: its requests are paused until then.
        crate::process::check_cancel()?;
        on(Progress::step("patch", "active", Some(tr!("窗口 {}/{}", "Window {}/{}", i + 1, total))));
        let ws = page["webSocketDebuggerUrl"].as_str().ok_or_else(|| anyhow!(crate::i18n::l("调试目标缺少 WebSocket 地址", "Debug target has no WebSocket URL")))?;
        let mut s = Session::open(ws)?;
        let waiting = || on(Progress::step("patch", "active", Some(tr!("窗口 {}/{}：等待其余界面脚本加载", "Window {}/{}: waiting for the rest of the UI scripts", i + 1, total))));
        let (how, r) = match via_fetch(&mut s, want, lazy, &waiting)? {
            Some(r) => (crate::i18n::l("响应拦截", "response interception"), r),
            None => match via_live_edit(&mut s, want)? {
                Some(r) => (crate::i18n::l("热替换", "live patch"), r),
                // No UI bundle in this window: nothing to patch.
                None => continue,
            },
        };
        done.push(how);
        if r.complete {
            lazy = Duration::from_secs(5);
        }
        // Found in any window counts: the overlay lacking the composer's code is fine.
        missing = Some(still_missing(missing.take(), r.missing));
    }
    let Some(missing) = missing else {
        return Err(anyhow!(crate::i18n::l("没有找到 Codex 界面脚本", "Codex UI script not found")));
    };
    let missing = check_missing(want, missing)?;
    let what: Vec<&str> = want.wanted().into_iter().filter(|w| !missing.contains(w)).collect();
    on(if missing.is_empty() {
        Progress::step("patch", "done", Some(join(&what)))
    } else {
        Progress::step("patch", "warn", Some(tr!("没找到：{}", "Not found: {}", join(&missing))))
    });
    let mut msg = tr!("界面已注入：{}（{}，{} 个窗口）", "UI patched: {} ({}; {} window(s))", join(&what), join(&done), done.len());
    if !missing.is_empty() {
        msg.push_str(&tr!("；没找到「{}」的注入位置，可能是 Codex 版本变了", "; couldn't find where to patch {}, the Codex version may have changed", join(&missing)));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Patches = Patches { fast: true, full_names: false, quota: false, usage_banner: false };
    const NAMES: Patches = Patches { fast: false, full_names: true, quota: false, usage_banner: false };
    const QUOTA: Patches = Patches { fast: false, full_names: false, quota: true, usage_banner: false };
    const BANNER: Patches = Patches { fast: false, full_names: false, quota: false, usage_banner: true };
    const BOTH: Patches = Patches { fast: true, full_names: true, quota: false, usage_banner: false };
    const ALL: Patches = Patches { fast: true, full_names: true, quota: true, usage_banner: false };
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
    fn hides_usage_banners() {
        // Codex 26.917 app-primary bundle.
        let slot = "N=F($v,_);if(!n)return u;let P=pYe({hasImageGenerationLimit:i!=null,showModelLimit:f,showUpsell:g,showWorkspaceUsageLimit:h}),I=null;";
        let (out, missing) = patch_source(slot, BANNER);
        let out = out.unwrap();
        assert!(missing.is_empty());
        assert_eq!(out, "N=F($v,_);if(!0)return u/*agentplus-banner*/;let P=pYe({hasImageGenerationLimit:i!=null,showModelLimit:f,showUpsell:g,showWorkspaceUsageLimit:h}),I=null;");
        assert_eq!(patch_source(&out, BANNER), (None, vec![]));
        // The helper that picks the banner kind has the same key but isn't touched.
        let helper = "function pYe({hasImageGenerationLimit:e,showModelLimit:t}){return t}";
        assert_eq!(patch_source(helper, BANNER).0, None);
    }

    #[test]
    fn bundle_is_js_only() {
        assert_eq!(bundle_of("app://-/assets/app-primary-a7ff54c980af.js"), Some("app-primary-"));
        assert_eq!(bundle_of("app://-/assets/app-initial-fc9a33fdda88.js?v=1"), Some("app-initial-"));
        // The same-named stylesheet loads first; it must not count as the bundle.
        assert_eq!(bundle_of("app://-/assets/app-primary-484df789f2f5.css"), None);
        assert_eq!(bundle_of("app://-/assets/app-primary-a7ff54c980af.js.map"), None);
        assert_eq!(bundle_of("app://-/assets/wrap-app-primary-x.js"), None);
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
