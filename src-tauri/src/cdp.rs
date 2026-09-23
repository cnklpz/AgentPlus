//! Fast visibility injection for Codex.
//!
//! Codex's UI hides the Fast option unless the auth method is "chatgpt":
//!   isServiceTierAllowed = authMethod==="chatgpt" && !loading && requirements?.featureRequirements?.fast_mode!==false
//! We start Codex with a local DevTools port and rewrite that one expression in the
//! UI bundle, keeping the admin switch (`fast_mode === false`) intact.
//! Strategy 1: intercept the bundle response on reload (Fetch domain).
//! Strategy 2: live-edit the already-loaded script (Debugger.setScriptSource).

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

pub const PORT: u16 = 39229;
const MARK: &str = "/*agentplus-fast*/";
const BUNDLE_HINT: &str = "app-initial-";

/// Rewrites the Fast gate. Minified names change per release, so match on shape.
pub fn patch_source(src: &str) -> Option<String> {
    if src.contains(MARK) {
        return None;
    }
    let re = Regex::new(
        r"([\w$]+)=[\w$]+&&!([\w$]+)&&([\w$]+)!=null&&[\w$]+\?\.requirements\?\.featureRequirements\?\.fast_mode!==!1",
    )
    .unwrap();
    let out = re.replacen(src, 1, format!("${{1}}=!${{2}}&&${{3}}?.requirements?.featureRequirements?.fast_mode!==!1{MARK}"));
    if out == src { None } else { Some(out.into_owned()) }
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
                        return Err(anyhow!("{method} 失败：{err}"));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                if msg.get("method").is_some() {
                    self.events.push_back(msg);
                }
            }
        }
        Err(anyhow!("{method} 超时"))
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

fn via_fetch(s: &mut Session) -> Result<bool> {
    s.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": format!("*{BUNDLE_HINT}*"), "requestStage": "Response" }] }))?;
    s.call("Page.reload", json!({ "ignoreCache": true }))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = match s.next_event("Fetch.requestPaused", deadline)? {
        None => Ok(false),
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
            match patch_source(&text) {
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
                    Ok(true)
                }
                None => {
                    s.call("Fetch.continueRequest", json!({ "requestId": rid }))?;
                    Err(anyhow!("在 Codex 界面代码里没找到 Fast 判断，可能是版本变了"))
                }
            }
        }
    };
    let _ = s.call("Fetch.disable", json!({}));
    result
}

fn via_live_edit(s: &mut Session) -> Result<()> {
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
    let id = script_id.ok_or_else(|| anyhow!("没有找到 Codex 界面脚本"))?;
    let src = s.call("Debugger.getScriptSource", json!({ "scriptId": id }))?;
    let text = src["scriptSource"].as_str().unwrap_or_default();
    if text.contains(MARK) {
        let _ = s.call("Debugger.disable", json!({}));
        return Ok(());
    }
    let patched = patch_source(text).ok_or_else(|| anyhow!("在 Codex 界面代码里没找到 Fast 判断，可能是版本变了"))?;
    let r = s.call("Debugger.setScriptSource", json!({ "scriptId": id, "scriptSource": patched }))?;
    let _ = s.call("Debugger.disable", json!({}));
    if let Some(ex) = r.get("exceptionDetails") {
        return Err(anyhow!("热替换失败：{ex}"));
    }
    if let Some(st) = r.get("status").and_then(|x| x.as_str()) {
        if st != "Ok" {
            return Err(anyhow!("热替换失败：{st}"));
        }
    }
    Ok(())
}

/// Waits for Codex windows on the debug port and patches each one.
pub fn inject(port: u16) -> Result<String> {
    let t0 = Instant::now();
    let pages = loop {
        if let Ok(p) = page_targets(port) {
            if !p.is_empty() {
                break p;
            }
        }
        if t0.elapsed() > Duration::from_secs(60) {
            return Err(anyhow!("60 秒内没有连上 Codex 的调试端口 {port}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    // Let the first render settle before reloading.
    std::thread::sleep(Duration::from_secs(2));
    let mut done = vec![];
    for page in pages {
        let ws = page["webSocketDebuggerUrl"].as_str().ok_or_else(|| anyhow!("调试目标缺少 WebSocket 地址"))?;
        let mut s = Session::open(ws)?;
        let how = match via_fetch(&mut s) {
            Ok(true) => "响应拦截",
            Ok(false) => {
                via_live_edit(&mut s)?;
                "热替换"
            }
            Err(e) => return Err(e),
        };
        done.push(how);
    }
    Ok(format!("Fast 已注入（{}，{} 个窗口）", done.join("、"), done.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_gate_once() {
        let src = "let x=1;d=a&&!u&&c!=null&&c?.requirements?.featureRequirements?.fast_mode!==!1,f;";
        let out = patch_source(src).unwrap();
        assert!(out.contains("d=!u&&c?.requirements?.featureRequirements?.fast_mode!==!1/*agentplus-fast*/"));
        assert!(patch_source(&out).is_none());
    }
}
