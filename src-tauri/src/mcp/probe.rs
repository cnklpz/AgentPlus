//! Connection test: starts a server (stdio) or calls it (streamable HTTP / legacy SSE), does
//! the MCP handshake (`initialize`, `notifications/initialized`), lists its tools and hangs up.
//! It runs what the configuration says, with its real values, on this machine; variable
//! references (`${X}`) are filled in from this process's environment.

use super::decode::Raw;
use crate::i18n::l;
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What a server said about itself.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// `serverInfo` name and version.
    pub server: Option<String>,
    pub protocol: Option<String>,
    pub tools: Vec<Tool>,
    pub ms: u64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
}

const PROTOCOL: &str = "2025-06-18";
/// A stdio server may first be downloaded (`npx -y …`, `uvx …`).
const STDIO_TIMEOUT: Duration = Duration::from_secs(90);
const REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_TOOLS: usize = 500;

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": PROTOCOL, "capabilities": {},
        "clientInfo": { "name": "AgentPlus", "version": env!("CARGO_PKG_VERSION") },
    } })
}

fn initialized() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

fn list_tools() -> Value {
    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })
}

/// `${X}`, `${X:-default}`, `{env:X}`, `$X`, `%X%` filled in from this process's environment.
fn expand(s: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}|\{env:([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)|%([A-Za-z_][A-Za-z0-9_]*)%").unwrap());
    r.replace_all(s, |c: &regex::Captures| {
        let name = [1, 3, 4, 5].iter().find_map(|&i| c.get(i)).map(|m| m.as_str()).unwrap_or_default();
        std::env::var(name).ok().filter(|v| !v.is_empty()).or_else(|| c.get(2).map(|d| d.as_str().to_string())).unwrap_or_default()
    })
    .into_owned()
}

/// The tools of a `tools/list` result.
fn tools_of(result: &Value) -> Vec<Tool> {
    result["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_TOOLS)
        .filter_map(|t| {
            let name = t["name"].as_str()?.to_string();
            let description = crate::util::clip(t["description"].as_str().unwrap_or_default().trim(), 300);
            Some(Tool { name, description })
        })
        .collect()
}

/// A JSON-RPC response's result, or its error as ours.
fn result(v: Value) -> Result<Value> {
    if let Some(e) = v.get("error") {
        let msg = e["message"].as_str().unwrap_or("?");
        bail!("{}", tr!("The server answered with an error: {msg}", "服务器返回了错误：{msg}"));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn fill(p: &mut Probe, init: &Value, tools: &Value) {
    let info = &init["serverInfo"];
    p.server = info["name"].as_str().map(|n| match info["version"].as_str() {
        Some(v) => format!("{n} {v}"),
        None => n.to_string(),
    });
    p.protocol = init["protocolVersion"].as_str().map(String::from);
    p.tools = tools_of(tools);
}

pub fn probe(raw: &Raw) -> Result<Probe> {
    let t0 = Instant::now();
    let mut p = match raw.transport {
        "stdio" => stdio(raw)?,
        "http" => http(raw)?,
        "sse" => sse(raw)?,
        // OpenCode's remote: whichever the server speaks.
        "remote" => http(raw).or_else(|e| sse(raw).map_err(|_| e))?,
        _ => bail!("{}", l("Testing WebSocket servers isn't supported", "暂不支持测试 WebSocket 服务器")),
    };
    p.ms = t0.elapsed().as_millis() as u64;
    Ok(p)
}

// ---------------------------------------------------------------- stdio

/// The program to start: a bare name is looked up on PATH (on Windows also as .exe / .cmd /
/// .bat, which is how `npx` and friends are installed).
fn program(cmd: &str) -> Result<std::path::PathBuf> {
    let cmd = expand(cmd.trim());
    let p = std::path::PathBuf::from(&cmd);
    if p.components().count() > 1 || p.is_absolute() {
        return Ok(p);
    }
    let names: Vec<String> = if cfg!(windows) && p.extension().is_none() { ["exe", "cmd", "bat"].iter().map(|e| format!("{cmd}.{e}")).collect() } else { vec![cmd.clone()] };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    crate::process::on_path(&names).ok_or_else(|| anyhow!(tr!("Can't find the command \"{cmd}\" on PATH", "在 PATH 里找不到命令「{cmd}」")))
}

/// Lines a thread reads from `r`, for waiting on with a timeout.
fn lines(r: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The response to request `id`, skipping notifications, logs and requests from the server.
fn wait(rx: &Receiver<Value>, id: u64, until: Instant, stderr: &dyn Fn() -> String) -> Result<Value> {
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(v) if v["id"].as_u64() == Some(id) && v.get("method").is_none() => return result(v),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => bail!("{}{}", l("The server didn't answer in time", "服务器没有及时响应"), stderr()),
            Err(RecvTimeoutError::Disconnected) => bail!("{}{}", l("The server stopped before answering", "服务器没有响应就退出了"), stderr()),
        }
    }
}

fn stdio(raw: &Raw) -> Result<Probe> {
    if crate::env::is_wsl() {
        bail!("{}", l("Testing stdio servers inside WSL isn't supported yet", "暂不支持在 WSL 里测试 stdio 服务器"));
    }
    let exe = program(raw.command.as_deref().unwrap_or_default())?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(raw.args.iter().map(|a| expand(a)))
        .envs(raw.env.iter().map(|(k, v)| (k, expand(v))))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = raw.cwd.as_deref().map(expand).filter(|d| !d.trim().is_empty()) {
        cmd.current_dir(dir);
    }
    crate::process::no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| anyhow!(tr!("Couldn't start {}: {e}", "无法启动 {}：{e}", exe.display())))?;
    let out = lines(child.stdout.take().unwrap());
    // The last lines of stderr, for the error message.
    let err_tail: Arc<Mutex<Vec<String>>> = Arc::default();
    let tail = err_tail.clone();
    let err_lines = lines(child.stderr.take().unwrap());
    std::thread::spawn(move || {
        for l in err_lines {
            let mut t = crate::util::lock(&tail);
            t.push(l);
            if t.len() > 8 {
                t.remove(0);
            }
        }
    });
    let stderr = move || {
        let t = crate::util::lock(&err_tail);
        if t.is_empty() { String::new() } else { format!("\n{}", crate::util::clip(&t.join("\n"), 800)) }
    };
    // stdout lines that are JSON-RPC messages (servers sometimes log there too).
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for l in out {
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    let mut send = |v: &Value| -> Result<()> {
        writeln!(stdin, "{v}")?;
        stdin.flush()?;
        Ok(())
    };
    let run = (|| {
        let until = Instant::now() + STDIO_TIMEOUT;
        send(&initialize())?;
        let init = wait(&rx, 1, until, &stderr)?;
        send(&initialized())?;
        send(&list_tools())?;
        let tools = wait(&rx, 2, Instant::now() + REMOTE_TIMEOUT, &stderr)?;
        let mut p = Probe::default();
        fill(&mut p, &init, &tools);
        Ok(p)
    })();
    // The whole tree: `npx.cmd` / `uvx` start the server as a grandchild, which would keep
    // running (and keep the pipes above open) after only the launcher ends.
    crate::process::kill_tree(child.id());
    let _ = child.kill();
    let _ = child.wait();
    run
}

// ---------------------------------------------------------------- remote

fn client() -> Result<reqwest::blocking::Client> {
    crate::net::client_with(REMOTE_TIMEOUT).map_err(|e| anyhow!(e))
}

fn url_of(raw: &Raw) -> Result<String> {
    let u = expand(raw.url.as_deref().unwrap_or_default().trim());
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        bail!("{}", tr!("Not an http(s) address: {}", "不是 http(s) 地址：{}", super::mask::url(&u)));
    }
    Ok(u)
}

fn status_error(status: reqwest::StatusCode) -> anyhow::Error {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        anyhow!(tr!(
            "The server wants authorization ({status}). Check the headers; OAuth sign-in can't be tested here",
            "服务器要求授权（{status}）。请检查请求头；需要 OAuth 登录的服务器无法在这里测试"
        ))
    } else {
        anyhow!(tr!("The server answered {status}", "服务器返回 {status}"))
    }
}

/// The `data:` payloads of an SSE body, as they arrive.
fn sse_events(r: impl Read) -> impl Iterator<Item = (String, String)> {
    let mut lines = BufReader::new(r).lines().map_while(Result::ok);
    std::iter::from_fn(move || {
        let (mut event, mut data) = (String::from("message"), String::new());
        for line in lines.by_ref() {
            if line.is_empty() {
                if !data.is_empty() {
                    return Some((event, data));
                }
                continue;
            }
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim().to_string();
            } else if let Some(d) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(d.strip_prefix(' ').unwrap_or(d));
            }
        }
        (!data.is_empty()).then_some((event, data))
    })
}

/// Streamable HTTP: every message is a POST; the answer is JSON or an SSE stream.
fn http(raw: &Raw) -> Result<Probe> {
    let url = url_of(raw)?;
    let c = client()?;
    let headers: Vec<(String, String)> = raw.headers.iter().map(|(k, v)| (k.clone(), expand(v))).collect();
    let mut session: Option<String> = None;
    let mut post = |msg: &Value, id: Option<u64>| -> Result<Value> {
        let mut req = c.post(&url).header("Content-Type", "application/json").header("Accept", "application/json, text/event-stream");
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        if let Some(s) = &session {
            req = req.header("Mcp-Session-Id", s).header("MCP-Protocol-Version", PROTOCOL);
        }
        let resp = req.body(msg.to_string()).send().map_err(|e| anyhow!(tr!("Couldn't reach the server: {e}", "连不上服务器：{e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            session = Some(s.to_string());
        }
        let Some(id) = id else { return Ok(Value::Null) };
        let sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/event-stream"));
        if sse {
            for (_, data) in sse_events(resp) {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    if v["id"].as_u64() == Some(id) {
                        return result(v);
                    }
                }
            }
            bail!("{}", l("The server closed the stream without answering", "服务器没有响应就关闭了连接"));
        }
        let not_mcp = || anyhow!(l("The server's answer isn't MCP (JSON-RPC)", "服务器的响应不是 MCP（JSON-RPC）"));
        let v: Value = serde_json::from_str(&resp.text().map_err(|_| not_mcp())?).map_err(|_| not_mcp())?;
        result(v)
    };
    let init = post(&initialize(), Some(1))?;
    post(&initialized(), None)?;
    let tools = post(&list_tools(), Some(2))?;
    let mut p = Probe::default();
    fill(&mut p, &init, &tools);
    Ok(p)
}

/// Where the server says to POST (legacy SSE `endpoint` event). Only on its own origin: the
/// POSTs carry the configured headers (API keys), which another host must not get (the MCP
/// SDKs refuse such an endpoint too).
fn endpoint_of(base: &url::Url, data: &str) -> Result<String> {
    let to = base.join(data)?;
    if !crate::net::same_origin(base, &to) {
        bail!("{}", tr!(
            "The server told AgentPlus to send its messages to another address ({}); not done, to protect the request headers",
            "服务器要求把消息发到另一个地址（{}），为保护请求头没有照做",
            super::mask::url(to.as_str())
        ));
    }
    Ok(to.to_string())
}

/// Legacy SSE: a GET stream first names the endpoint to POST to; answers come on the stream.
fn sse(raw: &Raw) -> Result<Probe> {
    let url = url_of(raw)?;
    let headers: Vec<(String, String)> = raw.headers.iter().map(|(k, v)| (k.clone(), expand(v))).collect();
    // The stream stays open: no overall timeout on it, the waits below have their own. Its
    // headers (API keys) must not follow a redirect to another host either.
    let stream_client = crate::net::stream_client(REMOTE_TIMEOUT).map_err(|e| anyhow!(e))?;
    let mut req = stream_client.get(&url).header("Accept", "text/event-stream");
    for (k, v) in &headers {
        req = req.header(k, v);
    }
    let resp = req.send().map_err(|e| anyhow!(tr!("Couldn't reach the server: {e}", "连不上服务器：{e}")))?;
    if !resp.status().is_success() {
        return Err(status_error(resp.status()));
    }
    let (tx, rx) = channel::<(String, String)>();
    std::thread::spawn(move || {
        for ev in sse_events(resp) {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });
    let until = Instant::now() + REMOTE_TIMEOUT;
    let next = |until: Instant| rx.recv_timeout(until.saturating_duration_since(Instant::now())).map_err(|_| anyhow!(l("The server didn't answer in time", "服务器没有及时响应")));
    let base = url::Url::parse(&url)?;
    let endpoint = loop {
        let (event, data) = next(until)?;
        if event == "endpoint" {
            break endpoint_of(&base, data.trim())?;
        }
    };
    let c = client()?;
    let post = |msg: &Value| -> Result<()> {
        let mut req = c.post(&endpoint).header("Content-Type", "application/json");
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        let resp = req.body(msg.to_string()).send()?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        Ok(())
    };
    let answer = |id: u64| -> Result<Value> {
        loop {
            let (_, data) = next(until)?;
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                if v["id"].as_u64() == Some(id) && v.get("method").is_none() {
                    return result(v);
                }
            }
        }
    };
    post(&initialize())?;
    let init = answer(1)?;
    post(&initialized())?;
    post(&list_tools())?;
    let tools = answer(2)?;
    let mut p = Probe::default();
    fill(&mut p, &init, &tools);
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn variables_come_from_the_environment() {
        std::env::set_var("AGENTPLUS_PROBE_T", "tok");
        assert_eq!(expand("Bearer ${AGENTPLUS_PROBE_T}"), "Bearer tok");
        assert_eq!(expand("{env:AGENTPLUS_PROBE_T}/$AGENTPLUS_PROBE_T/%AGENTPLUS_PROBE_T%"), "tok/tok/tok");
        assert_eq!(expand("${AGENTPLUS_PROBE_NONE:-dflt}"), "dflt");
        assert_eq!(expand("${AGENTPLUS_PROBE_NONE}"), "");
    }

    #[test]
    fn sse_bodies_split_into_events() {
        let body = "event: endpoint\ndata: /messages?s=1\n\n: comment\ndata: {\"id\":1}\n\n";
        let ev: Vec<(String, String)> = sse_events(body.as_bytes()).collect();
        assert_eq!(ev, [("endpoint".into(), "/messages?s=1".into()), ("message".into(), "{\"id\":1}".into())]);
    }

    /// A one-shot HTTP server: answers each request with the next of `replies`.
    fn serve(replies: Vec<(&'static str, String)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for (ctype, body) in replies {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf);
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nMcp-Session-Id: abc\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}/mcp")
    }

    #[test]
    fn streamable_http_handshake_and_tools() {
        let init = json!({ "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": PROTOCOL, "serverInfo": { "name": "demo", "version": "1.2" }, "capabilities": {} } });
        let tools = json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "search", "description": "Find things" }, { "name": "fetch" }] } });
        let url = serve(vec![("application/json", init.to_string()), ("application/json", String::new()), ("text/event-stream", format!("event: message\ndata: {tools}\n\n"))]);
        let raw = Raw { transport: "http", url: Some(url), ..Default::default() };
        let p = probe(&raw).unwrap();
        assert_eq!(p.server.as_deref(), Some("demo 1.2"));
        assert_eq!(p.tools, [Tool { name: "search".into(), description: "Find things".into() }, Tool { name: "fetch".into(), description: String::new() }]);
    }

    #[test]
    fn the_sse_endpoint_stays_on_the_servers_origin() {
        let base = url::Url::parse("https://mcp.example.com/sse").unwrap();
        assert_eq!(endpoint_of(&base, "/messages?s=1").unwrap(), "https://mcp.example.com/messages?s=1");
        assert_eq!(endpoint_of(&base, "https://mcp.example.com:443/m").unwrap(), "https://mcp.example.com/m");
        for other in ["https://evil.example/collect", "//evil.example/collect", "http://mcp.example.com/m", "https://mcp.example.com:8443/m"] {
            assert!(endpoint_of(&base, other).is_err(), "{other}");
        }
    }

    /// The SSE stream's headers (API keys) don't follow a redirect to another host.
    #[test]
    fn sse_headers_never_follow_a_redirect_elsewhere() {
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let other_url = format!("http://{}/sse", other.local_addr().unwrap());
        other.set_nonblocking(true).unwrap();
        let first = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sse", first.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = first.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {other_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
        });
        let raw = Raw { transport: "sse", url: Some(url), headers: vec![("X-API-Key".into(), "secret".into())], ..Default::default() };
        assert!(probe(&raw).is_err());
        std::thread::sleep(Duration::from_millis(200));
        assert!(other.accept().is_err(), "the redirect target was contacted");
    }

    #[test]
    fn errors_are_explained() {
        let url = serve(vec![("application/json", json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32600, "message": "bad" } }).to_string())]);
        let raw = Raw { transport: "http", url: Some(url), ..Default::default() };
        assert_eq!(probe(&raw).unwrap_err().to_string(), "服务器返回了错误：bad");
        let missing = Raw { transport: "stdio", command: Some("agentplus-no-such-command".into()), ..Default::default() };
        assert_eq!(probe(&missing).unwrap_err().to_string(), "在 PATH 里找不到命令「agentplus-no-such-command」");
        let ws = Raw { transport: "ws", url: Some("wss://h".into()), ..Default::default() };
        assert!(probe(&ws).is_err());
    }
}
