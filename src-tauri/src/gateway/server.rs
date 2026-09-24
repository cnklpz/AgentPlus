//! Local gateway: a small HTTP server on 127.0.0.1 that lets any client speak Chat
//! Completions, Responses or Anthropic Messages to any upstream, converting on the fly
//! (streaming included). Each *route* forwards to one provider from the library:
//!
//!   http://127.0.0.1:<port>/<route>/v1/chat/completions | /responses | /messages | /models
//!
//! Several forwards can be combined into one address, which then works like the unified
//! entry restricted to them:
//!
//!   http://127.0.0.1:<port>/<route>+<route>/v1/...
//!
//! Same-protocol requests are passed through byte for byte. Config lives in
//! `~/.agentplus/store.json` under "gateway". Each request reads the store once and works
//! from that snapshot: edits (routes, library address or key) apply from the next request,
//! while requests already under way finish with what they started with.
//!
//! Every request must carry an agent's gateway key (see `keys`) as `Authorization: Bearer`
//! or `x-api-key`. Web pages are kept out: a request from a non-local browser origin, or
//! with a Host that isn't this machine (DNS rebinding), is refused before anything else.

use super::breaker::{self, Outcome};
use super::convert::{self, DownstreamStream, Proto, UpstreamStream};
use super::keys;
use crate::{library, store};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 18650;
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Longest request line or header line, and most header lines.
const MAX_LINE: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;
/// Connections served at once; more get a 503 right away.
const MAX_CONNS: usize = 256;

// ---------------------------------------------------------------- config

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// URL segment: http://127.0.0.1:<port>/<id>/v1/...
    pub id: String,
    pub name: String,
    /// Library entry that holds the upstream address and key.
    pub library: String,
    /// Protocol the upstream speaks: "chat" | "responses" | "anthropic".
    pub upstream_api: String,
    /// Model rewrites, applied in order: exact name, or "*" for every request.
    #[serde(default)]
    pub model_map: Vec<(String, String)>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Share of the unified entry's traffic when several forwards serve the same model.
    #[serde(default = "hundred")]
    pub weight: u32,
    /// Agent providers (agent id, provider id) whose address was switched to this forward
    /// when it was added; deleting the forward can point them back at the upstream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<(String, String)>,
}

fn yes() -> bool {
    true
}

fn hundred() -> u32 {
    100
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
    #[serde(default)]
    pub routes: Vec<Route>,
    /// Pauses a forward after repeated upstream errors.
    #[serde(default)]
    pub breaker: breaker::Config,
    /// Ports used before the current one, newest first: agent addresses still at an old
    /// port are recognised as this gateway (and moved to the new one by the UI).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_ports: Vec<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: false, port: DEFAULT_PORT, routes: vec![], breaker: breaker::Config::default(), former_ports: vec![] }
    }
}

fn config_in(root: &Value) -> Config {
    decode(root).0
}

/// The gateway section field by field: a bad value falls back to its default on its own,
/// and a route that doesn't decode is skipped (and returned raw, so saving keeps it).
fn decode(root: &Value) -> (Config, Vec<Value>) {
    let Some(g) = root.get("gateway").filter(|g| g.is_object()) else { return (Config::default(), vec![]) };
    fn field<T: serde::de::DeserializeOwned>(g: &Value, k: &str) -> Option<T> {
        g.get(k).and_then(|v| serde_json::from_value(v.clone()).ok())
    }
    let (mut routes, mut bad) = (vec![], vec![]);
    for r in g.get("routes").and_then(|r| r.as_array()).into_iter().flatten() {
        match serde_json::from_value::<Route>(r.clone()) {
            Ok(x) => routes.push(x),
            Err(_) => bad.push(r.clone()),
        }
    }
    let d = Config::default();
    let c = Config {
        enabled: field(g, "enabled").unwrap_or(d.enabled),
        port: field(g, "port").unwrap_or(d.port),
        routes,
        breaker: field::<breaker::Config>(g, "breaker").unwrap_or_default().clamped(),
        former_ports: field(g, "formerPorts").unwrap_or_default(),
    };
    (c, bad)
}

pub fn load_config() -> Config {
    config_in(&store::load())
}

/// The config as stored, with the routes that didn't decode put back unchanged.
fn encode(c: &Config, bad: Vec<Value>) -> Result<Value> {
    let mut v = serde_json::to_value(c)?;
    if let Some(r) = v.get_mut("routes").and_then(|r| r.as_array_mut()) {
        r.extend(bad);
    }
    Ok(v)
}

/// Changes the gateway config under the store's write lock.
fn update_config<T>(f: impl FnOnce(&mut Config) -> Result<T>) -> Result<T> {
    store::update(|s| {
        if s.get("gateway").is_some_and(|g| !g.is_object() && !g.is_null()) {
            return Err(anyhow!(crate::i18n::l("store.json 里的 gateway 设置无法读取，没有保存", "The gateway section of store.json can't be read; nothing was saved")));
        }
        let (mut c, bad) = decode(s);
        let out = f(&mut c)?;
        s["gateway"] = encode(&c, bad)?;
        Ok(out)
    })
}

// ---------------------------------------------------------------- runtime state

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub at: String,
    pub route: String,
    pub method: String,
    pub path: String,
    pub inbound: String,
    pub upstream: String,
    pub model: String,
    pub status: u16,
    pub ms: u64,
    pub stream: bool,
    pub converted: bool,
    pub error: Option<String>,
    /// (input, output) tokens the upstream reported.
    pub usage: Option<(u64, u64)>,
    /// The upstream broke off mid-stream; counts toward the breaker (the error text is translated).
    #[serde(skip)]
    pub upstream_broken: bool,
    /// Agent whose gateway key the request carried (`keys::LEGACY` for the old shared key,
    /// "agentplus" for AgentPlus's own test); None when it was refused.
    pub agent: Option<String>,
}

/// One agent's share of a minute.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentUse {
    pub requests: u32,
    pub failures: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One minute of traffic, for the charts.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Minute {
    /// Unix seconds at the start of the minute.
    pub t: i64,
    pub requests: u32,
    pub failures: u32,
    pub ms_total: u64,
    pub ms_max: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Most requests in flight at once during the minute.
    pub peak_active: u32,
    /// Requests by calling agent.
    pub agents: BTreeMap<String, AgentUse>,
}

struct Runtime {
    port: u16,
    stop: Arc<AtomicBool>,
    /// Disconnects once the accept loop has exited (and closed the listener).
    done: mpsc::Receiver<()>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
/// Serializes start / stop / port changes (UI commands and the store watcher).
static CONTROL: Mutex<()> = Mutex::new(());
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static REQUESTS: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicU64 = AtomicU64::new(0);
static LOG: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static SERIES: Mutex<VecDeque<Minute>> = Mutex::new(VecDeque::new());
const SERIES_MINUTES: i64 = 60;

/// Runs `f` on the current minute's bucket, starting a new one when the minute turns.
fn with_minute(f: impl FnOnce(&mut Minute)) {
    let now = chrono::Local::now().timestamp();
    let t = now - now.rem_euclid(60);
    let mut q = SERIES.lock().unwrap();
    if q.back().map(|m| m.t) != Some(t) {
        q.push_back(Minute { t, ..Default::default() });
    }
    while q.front().is_some_and(|m| m.t <= t - SERIES_MINUTES * 60) {
        q.pop_front();
    }
    f(q.back_mut().unwrap());
}

fn push_log(e: LogEntry) {
    let mut l = LOG.lock().unwrap();
    if l.len() >= 200 {
        l.pop_back();
    }
    l.push_front(e);
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouteView {
    #[serde(flatten)]
    pub route: Route,
    /// Local base URL clients should use (ends in /v1).
    pub local_base: String,
    pub upstream_name: Option<String>,
    pub upstream_url: Option<String>,
    pub upstream_missing: bool,
    /// Models this forward is known to serve (library list, mapped names, cached upstream list).
    pub models: Vec<String>,
    /// Error breaker state; None while there is nothing to report.
    pub breaker: Option<breaker::View>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub error: Option<String>,
    pub requests: u64,
    pub failures: u64,
    pub active: u64,
    pub routes: Vec<RouteView>,
    pub log: Vec<LogEntry>,
    /// Per-minute traffic for the last hour (minutes without requests are left out).
    pub series: Vec<Minute>,
    /// Server clock, unix seconds, so the charts line up with `series`.
    pub now: i64,
    /// Unified entry that picks a forward by model: http://127.0.0.1:<port>/v1
    pub unified_base: String,
    pub breaker: breaker::Config,
    /// Earlier ports; agent addresses at these still belong to the gateway.
    pub former_ports: Vec<u16>,
    /// Agent → fingerprint of its gateway key, to spot entries still on an old key.
    pub key_fps: BTreeMap<String, String>,
    /// Fingerprint of `keys::PLACEHOLDER` (entries written before per-agent keys).
    pub legacy_fp: String,
}

pub fn running_port() -> Option<u16> {
    RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.port)
}

pub fn status() -> Status {
    let root = store::load();
    let c = config_in(&root);
    let running_port = running_port();
    let lib = library::list_in(&root);
    let port = running_port.unwrap_or(c.port);
    Status {
        enabled: c.enabled,
        running: running_port.is_some(),
        port,
        error: LAST_ERROR.lock().unwrap().clone(),
        requests: REQUESTS.load(Ordering::Relaxed),
        failures: FAILURES.load(Ordering::Relaxed),
        active: ACTIVE.load(Ordering::Relaxed),
        routes: c
            .routes
            .iter()
            .map(|r| {
                let e = lib.iter().find(|e| e.id == r.library);
                RouteView {
                    route: r.clone(),
                    local_base: format!("http://127.0.0.1:{port}/{}/v1", r.id),
                    upstream_name: e.map(|e| e.name.clone()),
                    upstream_url: e.map(|e| e.base_url.clone()),
                    upstream_missing: e.is_none(),
                    models: known_models(&root, r),
                    breaker: breaker::view(&c.breaker, &r.id),
                }
            })
            .collect(),
        log: LOG.lock().unwrap().iter().take(60).cloned().collect(),
        series: SERIES.lock().unwrap().iter().cloned().collect(),
        now: chrono::Local::now().timestamp(),
        unified_base: format!("http://127.0.0.1:{port}/v1"),
        breaker: c.breaker.clone(),
        former_ports: c.former_ports.iter().copied().filter(|&p| p != port).collect(),
        key_fps: keys::fingerprints_in(&root),
        legacy_fp: crate::model::key_fingerprint(keys::PLACEHOLDER),
    }
}

// ---------------------------------------------------------------- start / stop

pub fn set_enabled(enabled: bool, port: Option<u16>) -> Result<()> {
    let _g = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if port.is_some_and(|p| p < 1024) {
        return Err(anyhow!(tr!("端口需要在 1024–65535 之间", "The port must be between 1024 and 65535")));
    }
    let port = port.unwrap_or_else(|| load_config().port);
    if enabled {
        if running_port() != Some(port) {
            // Open the new port first: when it is taken, the gateway stays where it was
            // and the saved config is left alone.
            let l = bind(port)?;
            stop();
            // A fresh start forgets paused forwards.
            breaker::reset(None);
            run(l, port)?;
        }
    } else {
        stop();
        breaker::reset(None);
        *LAST_ERROR.lock().unwrap() = None;
    }
    update_config(|c| {
        move_port(c, port);
        c.enabled = enabled;
        Ok(())
    })
}

/// Sets the port, remembering the one it replaces.
fn move_port(c: &mut Config, port: u16) {
    if c.port != port {
        let old = c.port;
        remember_port(c, old);
        c.former_ports.retain(|&p| p != port);
        c.port = port;
    }
}

fn remember_port(c: &mut Config, old: u16) {
    c.former_ports.retain(|&p| p != old);
    c.former_ports.insert(0, old);
    c.former_ports.truncate(5);
}

/// Starts the gateway at launch when it was left on, then follows changes to the switch
/// or port made outside the app's own controls (store.json edited by hand, sync).
pub fn autostart() {
    reconcile();
    let _ = std::thread::Builder::new().name("agentplus-gateway-watch".into()).spawn(|| {
        let path = crate::util::agentplus_dir().join("store.json");
        let mtime = || std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut seen = mtime();
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let now = mtime();
            if now != seen {
                seen = now;
                reconcile();
            }
        }
    });
}

/// Brings the running gateway in line with the saved switch and port.
fn reconcile() {
    let _g = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    let c = load_config();
    let running = running_port();
    if !c.enabled {
        if running.is_some() {
            stop();
        }
        return;
    }
    if running == Some(c.port) {
        return;
    }
    // A failed bind is reported through LAST_ERROR and tried again on the next change.
    let Ok(l) = bind(c.port) else { return };
    stop();
    if run(l, c.port).is_ok() {
        if let Some(old) = running {
            let _ = update_config(|c| {
                remember_port(c, old);
                Ok(())
            });
        }
    }
}

fn bind(port: u16) -> Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => {
            *LAST_ERROR.lock().unwrap() = None;
            Ok(l)
        }
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                tr!("端口 {port} 已被占用", "Port {port} is already in use")
            } else {
                tr!("监听 127.0.0.1:{port} 失败：{e}", "Could not listen on 127.0.0.1:{port}: {e}")
            };
            *LAST_ERROR.lock().unwrap() = Some(msg.clone());
            Err(anyhow!(msg))
        }
    }
}

fn run(listener: TcpListener, port: u16) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done) = mpsc::channel::<()>();
    let flag = stop.clone();
    std::thread::Builder::new().name("agentplus-gateway".into()).spawn(move || {
        let _done = done_tx;
        for conn in listener.incoming() {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            let mut s = match conn {
                Ok(s) => s,
                Err(_) => {
                    // Out of sockets or similar: don't spin.
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            let Some(slot) = ConnSlot::take() else {
                let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                let msg = crate::i18n::l("网关同时处理的连接太多，请稍后再试", "Too many connections to the gateway; try again shortly");
                write_json(&mut s, 503, &json!({ "error": { "message": msg, "type": "overloaded" } }));
                continue;
            };
            // On failure the closure (connection and slot) is dropped, which closes it.
            let _ = std::thread::Builder::new().name("agentplus-gateway-conn".into()).spawn(move || {
                let _slot = slot;
                handle(s)
            });
        }
    })?;
    *RUNTIME.lock().unwrap() = Some(Runtime { port, stop, done });
    Ok(())
}

static CONNS: AtomicUsize = AtomicUsize::new(0);

/// One of the `MAX_CONNS` connection slots, freed on drop.
struct ConnSlot;

impl ConnSlot {
    fn take() -> Option<ConnSlot> {
        if CONNS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            CONNS.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(ConnSlot)
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        CONNS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Stops accepting and waits (briefly) until the listener is closed, so the same port
/// can be opened again right away. Requests already being served run to completion.
fn stop() {
    let r = RUNTIME.lock().unwrap().take();
    if let Some(r) = r {
        r.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&([127, 0, 0, 1], r.port).into(), Duration::from_millis(300));
        let _ = r.done.recv_timeout(Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------- routes

pub fn save_route(mut r: Route, old_id: Option<String>) -> Result<()> {
    r.id = crate::model::slug(r.id.trim());
    if r.id.is_empty() || r.id == "v1" {
        return Err(anyhow!(crate::i18n::l("路由名只能用字母、数字和连字符", "Route names may only use letters, digits and hyphens")));
    }
    if Proto::from_api(&r.upstream_api).is_none() {
        return Err(anyhow!(tr!("未知协议 {}", "Unknown protocol {}", r.upstream_api)));
    }
    library::endpoint(&r.library)?;
    let old = old_id.as_deref().unwrap_or(&r.id).to_string();
    let new = r.id.clone();
    update_config(|c| {
        if c.routes.iter().any(|x| x.id == r.id && x.id != old) {
            return Err(anyhow!(tr!("路由 {} 已存在", "Route {} already exists", r.id)));
        }
        match c.routes.iter_mut().find(|x| x.id == old) {
            Some(x) => *x = r,
            None => c.routes.push(r),
        }
        Ok(())
    })?;
    if old != new {
        breaker::reset(Some(&old));
    }
    forget_models(&[&old, &new]);
    Ok(())
}

pub fn delete_route(id: &str) -> Result<()> {
    update_config(|c| {
        c.routes.retain(|r| r.id != id);
        Ok(())
    })?;
    breaker::reset(Some(id));
    forget_models(&[id]);
    Ok(())
}

pub fn set_breaker(b: breaker::Config) -> Result<()> {
    let b = b.clamped();
    let enabled = b.enabled;
    update_config(|c| {
        c.breaker = b;
        Ok(())
    })?;
    if !enabled {
        breaker::reset(None);
    }
    Ok(())
}

/// Lets a paused forward (or every one) take requests again right away.
pub fn reset_breaker(id: Option<&str>) {
    breaker::reset(id);
}

fn breaker_cfg(root: &Value) -> breaker::Config {
    #[cfg(test)]
    {
        let _ = root;
        breaker::Config::default()
    }
    #[cfg(not(test))]
    config_in(root).breaker
}

/// API key AgentPlus's own "测试" button sends: it goes through even while the forward is
/// paused, so a successful test closes the breaker. Random per launch and never written
/// anywhere, so nothing but AgentPlus itself knows it.
pub fn test_key() -> &'static str {
    static K: OnceLock<String> = OnceLock::new();
    K.get_or_init(|| {
        let mut b = [0u8; 20];
        if getrandom::getrandom(&mut b).is_err() {
            // No OS randomness: fall back to the clock and address-space layout.
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) ^ (&b as *const _ as usize as u128);
            b[..16].copy_from_slice(&n.to_le_bytes());
        }
        format!("agp-test-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
    })
}

fn is_test(req: &Request) -> bool {
    inbound_key(req) == Some(test_key())
}

// ---------------------------------------------------------------- HTTP plumbing

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A request the client got wrong, with the status to answer it with.
#[derive(Debug)]
struct BadRequest(u16, String);

impl std::fmt::Display for BadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

impl std::error::Error for BadRequest {}

fn bad(status: u16, msg: &str) -> anyhow::Error {
    anyhow::Error::new(BadRequest(status, msg.to_string()))
}

fn too_large() -> anyhow::Error {
    bad(413, crate::i18n::l("请求体太大", "Request body too large"))
}

/// Reads from the client, giving up once the whole request has taken longer than `until`.
struct Deadline<'a> {
    s: &'a TcpStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if Instant::now() > self.until {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, crate::i18n::l("读取请求超时", "Timed out reading the request")));
        }
        (&mut &*self.s).read(buf)
    }
}

/// One line of at most `MAX_LINE` bytes (431 when longer); empty at end of input.
fn read_line(r: &mut impl BufRead) -> Result<String> {
    let mut line = String::new();
    r.take(MAX_LINE as u64 + 1).read_line(&mut line)?;
    if line.len() > MAX_LINE {
        return Err(bad(431, crate::i18n::l("请求行或请求头太长", "Request line or header too long")));
    }
    Ok(line)
}

/// Exactly `n` more bytes into `body`, growing it as they arrive.
fn read_body(r: &mut impl Read, body: &mut Vec<u8>, n: usize) -> Result<()> {
    let want = body.len() + n;
    r.take(n as u64).read_to_end(body)?;
    if body.len() < want {
        return Err(anyhow!(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)));
    }
    Ok(())
}

fn read_request(stream: &TcpStream) -> Result<Request> {
    // Per read, and for the whole request.
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut r = BufReader::new(Deadline { s: stream, until: Instant::now() + Duration::from_secs(120) });
    let line = read_line(&mut r)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or_else(|| anyhow!(crate::i18n::l("空请求", "Empty request")))?.to_string();
    let path = parts.next().ok_or_else(|| anyhow!(crate::i18n::l("缺少路径", "Missing path")))?.to_string();
    let mut headers = vec![];
    loop {
        let h = read_line(&mut r)?;
        if h.is_empty() {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(bad(431, crate::i18n::l("请求头太多", "Too many request headers")));
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let find = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone());
    let mut body = vec![];
    if find("transfer-encoding").map(|v| v.to_ascii_lowercase().contains("chunked")).unwrap_or(false) {
        loop {
            let size = read_line(&mut r)?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).map_err(|_| anyhow!(crate::i18n::l("分块长度无效", "Invalid chunk size")))?;
            if n == 0 {
                let _ = read_line(&mut r);
                break;
            }
            if body.len().checked_add(n).is_none_or(|t| t > MAX_BODY) {
                return Err(too_large());
            }
            read_body(&mut r, &mut body, n)?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf)?;
        }
    } else if let Some(n) = find("content-length").and_then(|v| v.parse::<usize>().ok()) {
        if n > MAX_BODY {
            return Err(too_large());
        }
        read_body(&mut r, &mut body, n)?;
    }
    Ok(Request { method, path, headers, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

thread_local! {
    /// CORS headers for the response on this connection's thread: empty unless the request
    /// came from an allowed browser origin (see `origin_allowed`).
    static CORS: RefCell<String> = const { RefCell::new(String::new()) };
}

fn cors() -> String {
    CORS.with(|c| c.borrow().clone())
}

/// Lets the request's origin (already checked) read the response.
fn set_cors(req: &Request) {
    let h = match req.header("origin") {
        Some(o) => {
            let asked = req.header("access-control-request-headers").filter(|h| !h.chars().any(|c| c.is_control())).unwrap_or("*");
            format!("Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\nAccess-Control-Allow-Headers: {asked}\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Max-Age: 600\r\n")
        }
        None => String::new(),
    };
    CORS.with(|c| *c.borrow_mut() = h);
}

/// localhost, *.localhost, 127.x.x.x, ::1 (IPv6 with or without brackets).
fn is_loopback_host(h: &str) -> bool {
    let h = h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Browsers send Origin on cross-site requests. An ordinary web page must not reach the
/// gateway, but local pages and desktop apps' own pages may: AgentPlus itself (tauri://,
/// http(s)://tauri.localhost), Electron apps (app://, file://) and VS Code webviews. "null"
/// is what sandboxed iframes and data: pages send, so it is refused.
fn origin_allowed(origin: &str) -> bool {
    if origin.chars().any(|c| c.is_control()) {
        return false;
    }
    match url::Url::parse(origin) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u.host_str().is_some_and(is_loopback_host),
        Ok(u) => matches!(u.scheme(), "tauri" | "app" | "file" | "vscode-webview" | "vscode-file"),
        Err(_) => false,
    }
}

/// The Host header names this machine; anything else is DNS rebinding.
fn host_allowed(req: &Request) -> bool {
    let Some(h) = req.header("host") else { return true };
    let name = match h.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => h.split(':').next().unwrap_or(""),
    };
    is_loopback_host(name)
}

/// The gateway key a client sent: `x-api-key`, `api-key` or `Authorization: Bearer`.
fn inbound_key(req: &Request) -> Option<&str> {
    let bearer = || req.header("authorization").and_then(|a| a.get(..7).filter(|p| p.eq_ignore_ascii_case("bearer ")).map(|_| &a[7..]));
    req.header("x-api-key").or_else(|| req.header("api-key")).or_else(bearer).map(str::trim).filter(|k| !k.is_empty())
}

/// Agent the request comes from (per the store snapshot `root`), or why it is refused.
fn authenticate(root: &Value, req: &Request) -> std::result::Result<String, String> {
    let Some(key) = inbound_key(req) else {
        return Err(crate::i18n::l(
            "缺少网关密钥：在 AgentPlus 里把这个供应商重新写入 Agent，或在请求头带上 Authorization: Bearer <网关密钥>",
            "Missing gateway key: write this provider to the agent again from AgentPlus, or send Authorization: Bearer <gateway key>",
        )
        .into());
    };
    if key == test_key() {
        return Ok("agentplus".into());
    }
    keys::caller_in(root, key).ok_or_else(|| {
        crate::i18n::l(
            "网关密钥不对：在 AgentPlus 里把这个供应商重新写入 Agent（每个 Agent 有自己的网关密钥）",
            "Wrong gateway key: write this provider to the agent again from AgentPlus (each agent has its own gateway key)",
        )
        .into()
    })
}

fn write_full(s: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
        reason(status),
        body.len(),
        cors()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn write_json(s: &mut TcpStream, status: u16, v: &Value) {
    write_full(s, status, "application/json", v.to_string().as_bytes());
}

/// Chunked streaming response.
struct Chunked<'a>(&'a mut TcpStream);

impl Chunked<'_> {
    fn start(s: &mut TcpStream, status: u16, content_type: &str) -> std::io::Result<()> {
        let head = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n{}Connection: close\r\n\r\n",
            reason(status),
            cors()
        );
        s.write_all(head.as_bytes())?;
        s.flush()
    }
    fn send(&mut self, data: &[u8]) -> std::io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        write!(self.0, "{:x}\r\n", data.len())?;
        self.0.write_all(data)?;
        self.0.write_all(b"\r\n")?;
        self.0.flush()
    }
    fn end(&mut self) {
        let _ = self.0.write_all(b"0\r\n\r\n");
        let _ = self.0.flush();
    }
}

/// Upstream idle limit. The blocking client applies `timeout` to each wait on its own (the
/// response head, then every read of the body), not to the whole exchange, so a stream that
/// keeps sending is never cut off, while an upstream that goes silent is dropped. Generous,
/// because a non-streaming answer from a reasoning model can take minutes to start.
const UPSTREAM_IDLE: Duration = Duration::from_secs(600);

fn client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(UPSTREAM_IDLE)
            .pool_idle_timeout(Duration::from_secs(60))
            // A redirect would carry the upstream key to wherever it points.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client")
    })
}

// ---------------------------------------------------------------- request handling

/// "/<route>/v1/chat/completions" → (route, "/chat/completions"). "/v1" is optional.
fn split_path(path: &str) -> Option<(String, String)> {
    let path = path.split('?').next().unwrap_or(path);
    let mut it = path.trim_start_matches('/').splitn(2, '/');
    let route = it.next()?.to_string();
    let rest = format!("/{}", it.next().unwrap_or(""));
    let rest = rest.strip_prefix("/v1").map(|r| if r.is_empty() { "/".to_string() } else { r.to_string() }).unwrap_or(rest);
    (!route.is_empty()).then_some((route, rest.trim_end_matches('/').to_string()))
}

fn inbound_of(rest: &str) -> Option<Proto> {
    match rest {
        "/chat/completions" => Some(Proto::Chat),
        "/responses" => Some(Proto::Responses),
        "/messages" => Some(Proto::Anthropic),
        _ => None,
    }
}

fn api_name(p: Proto) -> &'static str {
    match p {
        Proto::Chat => "chat",
        Proto::Responses => "responses",
        Proto::Anthropic => "anthropic",
    }
}

/// Counts a request in flight until dropped (also when the handler panics).
struct Active;

impl Active {
    fn start() -> (Active, u64) {
        (Active, ACTIVE.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

fn handle(mut s: TcpStream) {
    let t0 = Instant::now();
    CORS.with(|c| c.borrow_mut().clear());
    // A client that stops reading must not hold the thread forever.
    let _ = s.set_write_timeout(Some(Duration::from_secs(60)));
    let req = match read_request(&s) {
        Ok(r) => r,
        Err(e) => {
            let status = e.downcast_ref::<BadRequest>().map_or(400, |b| b.0);
            write_json(&mut s, status, &json!({ "error": { "message": format!("{e:#}"), "type": "invalid_request_error" } }));
            return;
        }
    };
    if !host_allowed(&req) {
        let msg = crate::i18n::l("Host 不是本机地址，请求被拒绝", "Refused: the Host header isn't this machine");
        write_json(&mut s, 403, &json!({ "error": { "message": msg, "type": "permission_error" } }));
        return;
    }
    if req.header("origin").is_some_and(|o| !origin_allowed(o)) {
        let msg = crate::i18n::l("网页不能访问本地网关", "Web pages can't use the local gateway");
        write_json(&mut s, 403, &json!({ "error": { "message": msg, "type": "permission_error" } }));
        return;
    }
    set_cors(&req);
    if req.method == "OPTIONS" {
        write_full(&mut s, 204, "text/plain", b"");
        return;
    }
    if req.path == "/" || req.path == "/health" {
        // Anyone local may ask whether it's up; the forward names only go to key holders.
        let root = store::load();
        let mut v = json!({ "ok": true, "service": "agentplus-gateway" });
        if authenticate(&root, &req).is_ok() {
            v["routes"] = json!(routes(&root).iter().filter(|r| r.enabled).map(|r| r.id.clone()).collect::<Vec<_>>());
        }
        write_json(&mut s, 200, &v);
        return;
    }
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    let (_active, active) = Active::start();
    with_minute(|m| m.peak_active = m.peak_active.max(active as u32));
    let mut log = LogEntry {
        at: chrono::Local::now().format("%H:%M:%S").to_string(),
        route: String::new(),
        method: req.method.clone(),
        path: req.path.split('?').next().unwrap_or("").to_string(),
        inbound: String::new(),
        upstream: String::new(),
        model: String::new(),
        status: 0,
        ms: 0,
        stream: false,
        converted: false,
        error: None,
        usage: None,
        upstream_broken: false,
        agent: None,
    };
    let status = match serve(&mut s, &req, &mut log) {
        Ok(st) => st,
        Err(e) => {
            let msg = format!("{e:#}");
            let inbound = Proto::from_api(&log.inbound).unwrap_or(Proto::Chat);
            write_json(&mut s, 502, &convert::error_body(inbound, 502, &msg));
            log.error = Some(match log.error.take() {
                Some(note) => tr!("{msg}（{note}）", "{msg} ({note})"),
                None => msg,
            });
            502
        }
    };
    log.status = status;
    log.ms = t0.elapsed().as_millis() as u64;
    if status >= 400 {
        FAILURES.fetch_add(1, Ordering::Relaxed);
    }
    with_minute(|m| {
        m.requests += 1;
        m.failures += (status >= 400) as u32;
        m.ms_total += log.ms;
        m.ms_max = m.ms_max.max(log.ms);
        let (i, o) = log.usage.unwrap_or((0, 0));
        m.input_tokens = m.input_tokens.saturating_add(i);
        m.output_tokens = m.output_tokens.saturating_add(o);
        if let Some(a) = &log.agent {
            let u = m.agents.entry(a.clone()).or_default();
            u.requests += 1;
            u.failures += (status >= 400) as u32;
            u.input_tokens = u.input_tokens.saturating_add(i);
            u.output_tokens = u.output_tokens.saturating_add(o);
        }
    });
    push_log(log);
    let _ = s.shutdown(Shutdown::Both);
}

/// Where a forward sends requests.
struct Target {
    route: Route,
    proto: Proto,
    base: String,
    key: Option<String>,
    /// Identifies the upstream (protocol, address, key) for the model-list cache.
    fp: u64,
}

fn fingerprint(proto: Proto, base: &str, key: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (api_name(proto), base, key).hash(&mut h);
    h.finish()
}

/// A forward's upstream, as configured in the store snapshot `root`.
fn target(root: &Value, route: &Route) -> Result<Target> {
    let proto = Proto::from_api(&route.upstream_api).ok_or_else(|| anyhow!(tr!("转发「{}」的协议无效", "Forward \"{}\" has an invalid protocol", route.id)))?;
    #[cfg(test)]
    if let Some(t) = TEST_ROUTES.lock().unwrap().iter().find(|t| t.0.id == route.id) {
        let base: String = t.1.trim_end_matches('/').into();
        let fp = fingerprint(proto, &base, t.2.as_deref());
        return Ok(Target { route: route.clone(), proto, base, key: t.2.clone(), fp });
    }
    let (_, base, key, _, _) = library::endpoint_in(root, &route.library)?;
    let base: String = base.trim_end_matches('/').into();
    let fp = fingerprint(proto, &base, key.as_deref());
    Ok(Target { route: route.clone(), proto, base, key, fp })
}

fn routes(root: &Value) -> Vec<Route> {
    #[cfg(test)]
    {
        let t = TEST_ROUTES.lock().unwrap();
        if !t.is_empty() {
            return t.iter().map(|x| x.0.clone()).collect();
        }
    }
    config_in(root).routes
}

// ---------------------------------------------------------------- models per forward

/// (route id, upstream fingerprint, fetched at, list). A changed address, key or protocol
/// changes the fingerprint, so a list from the old upstream is never used for the new one.
#[allow(clippy::type_complexity)]
static MODEL_CACHE: Mutex<Vec<(String, u64, Instant, Vec<String>)>> = Mutex::new(Vec::new());

/// Drops the cached upstream model lists of these forwards.
fn forget_models(ids: &[&str]) {
    MODEL_CACHE.lock().unwrap().retain(|(id, ..)| !ids.contains(&id.as_str()));
}

/// What a forward is known to serve, without any network call.
fn known_models(root: &Value, r: &Route) -> Vec<String> {
    let mut out: Vec<String> = library::list_in(root).into_iter().find(|e| e.id == r.library).map(|e| e.models).unwrap_or_default();
    for (from, _) in &r.model_map {
        if from != "*" && !out.contains(from) {
            out.push(from.clone());
        }
    }
    let fp = target(root, r).ok().map(|t| t.fp);
    if let Some((.., cached)) = MODEL_CACHE.lock().unwrap().iter().find(|(id, f, ..)| id == &r.id && Some(*f) == fp) {
        for m in cached {
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
    }
    out
}

/// The upstream's own /models list, cached for five minutes (one minute after a failure).
fn upstream_models(t: &Target) -> Vec<String> {
    {
        let cache = MODEL_CACHE.lock().unwrap();
        if let Some((.., at, list)) = cache.iter().find(|(id, fp, ..)| id == &t.route.id && *fp == t.fp) {
            let ttl = if list.is_empty() { 60 } else { 300 };
            if at.elapsed() < Duration::from_secs(ttl) {
                return list.clone();
            }
        }
    }
    let list: Vec<String> = auth(client().get(format!("{}/models", t.base)).timeout(Duration::from_secs(10)), t.proto, t.key.as_deref())
        .send()
        .ok()
        .filter(|r| r.status().is_success())
        .and_then(|r| r.text().ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| {
            v.get("data")
                .or_else(|| v.get("models"))
                .and_then(|d| d.as_array())
                .map(|a| a.iter().filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(|x| x.as_str()).map(String::from)).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut cache = MODEL_CACHE.lock().unwrap();
    cache.retain(|(id, ..)| id != &t.route.id);
    cache.push((t.route.id.clone(), t.fp, Instant::now(), list.clone()));
    list
}

fn all_models(root: &Value, t: &Target) -> Vec<String> {
    let mut out = known_models(root, &t.route);
    for m in upstream_models(t) {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

/// Weighted random order (without replacement) of candidate forwards.
fn weighted_order(mut items: Vec<Target>) -> Vec<Target> {
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut out = vec![];
    while !items.is_empty() {
        let total: u64 = items.iter().map(|t| t.route.weight.max(1) as u64).sum();
        let mut pick = next() % total;
        let mut i = 0;
        while pick >= items[i].route.weight.max(1) as u64 {
            pick -= items[i].route.weight.max(1) as u64;
            i += 1;
        }
        out.push(items.remove(i));
    }
    out
}

// ---------------------------------------------------------------- request handling

fn serve(s: &mut TcpStream, req: &Request, log: &mut LogEntry) -> Result<u16> {
    let Some((route_id, rest)) = split_path(&req.path) else {
        write_json(s, 404, &json!({ "error": { "message": crate::i18n::l("路径应为 /v1/...（统一入口）或 /<转发>/v1/...", "Path should be /v1/... (unified entry) or /<forward>/v1/...") } }));
        return Ok(404);
    };
    // One read of the store for the whole request, so it sees one consistent config even
    // while the UI (or anything else) is saving.
    let root = store::load();
    match authenticate(&root, req) {
        Ok(agent) => log.agent = Some(agent),
        Err(msg) => {
            let inbound = inbound_of(&rest).unwrap_or(Proto::Chat);
            write_json(s, 401, &convert::error_body(inbound, 401, &msg));
            log.error = Some(msg);
            return Ok(401);
        }
    }
    if route_id == "v1" {
        return serve_unified(s, req, &rest, log, None, &root);
    }
    if route_id.contains('+') {
        let ids = route_id.split('+').filter(|x| !x.is_empty()).map(String::from).collect();
        return serve_unified(s, req, &rest, log, Some(ids), &root);
    }
    log.route = route_id.clone();
    let Some(route) = routes(&root).into_iter().find(|r| r.id == route_id && r.enabled) else {
        write_json(s, 404, &json!({ "error": { "message": tr!("没有启用的转发「{route_id}」", "No enabled forward \"{route_id}\""), "type": "not_found" } }));
        return Ok(404);
    };
    let t = target(&root, &route)?;
    log.upstream = api_name(t.proto).into();

    if rest == "/models" && req.method == "GET" {
        return models(s, &t, req, log);
    }
    if rest == "/messages/count_tokens" && req.method == "POST" {
        return count_tokens(s, Some(&t), req, log);
    }
    let Some((inbound, body)) = parse_call(s, req, &rest, log)? else { return Ok(log.status) };
    let cfg = breaker_cfg(&root);
    let ticket = if is_test(req) {
        breaker::Ticket::TEST
    } else {
        match breaker::admit(&cfg, &route.id) {
            Ok(t) => t,
            Err(why) => {
                write_json(s, 503, &convert::error_body(inbound, 503, &why));
                log.error = Some(why);
                return Ok(503);
            }
        }
    };
    match tracked(s, req, &t, inbound, &body, log, false, &cfg, ticket)? {
        Attempt::Done(st) => Ok(st),
        Attempt::Retry(_) => unreachable!("retries are off for a single forward"),
    }
}

/// Unified entry: picks the forwards that serve the requested model, in weighted random
/// order, and moves on to the next one when a forward cannot be reached or fails (5xx / 429).
/// Forwards paused by the breaker are skipped. With `only`, just those forwards take part.
fn serve_unified(s: &mut TcpStream, req: &Request, rest: &str, log: &mut LogEntry, only: Option<Vec<String>>, root: &Value) -> Result<u16> {
    let entry = match &only {
        None => crate::i18n::l("统一入口", "Unified entry").to_string(),
        Some(ids) => tr!("组合 {}", "Combined {}", ids.join("+")),
    };
    log.route = entry.clone();
    let targets: Vec<Target> = routes(root)
        .iter()
        .filter(|r| r.enabled && only.as_ref().is_none_or(|ids| ids.contains(&r.id)))
        .filter_map(|r| target(root, r).ok())
        .collect();
    if targets.is_empty() {
        let msg = match &only {
            None => crate::i18n::l("本地网关还没有启用的转发", "The local gateway has no enabled forwards").to_string(),
            Some(ids) => tr!("转发 {} 都不存在或已暂停", "Forwards {} don't exist or are paused", ids.join(crate::i18n::l("、", ", "))),
        };
        write_json(s, 503, &json!({ "error": { "message": msg, "type": "no_upstream" } }));
        return Ok(503);
    }
    if rest == "/models" && req.method == "GET" {
        let want = if req.header("anthropic-version").is_some() || req.header("x-api-key").is_some() { Proto::Anthropic } else { Proto::Chat };
        log.inbound = api_name(want).into();
        let mut data: Vec<Value> = vec![];
        for t in &targets {
            for m in all_models(root, t) {
                if !data.iter().any(|d| d["id"] == m) {
                    data.push(json!({ "id": m, "object": "model", "owned_by": t.route.name }));
                }
            }
        }
        write_json(s, 200, &convert::models_body(want, &json!({ "object": "list", "data": data })));
        return Ok(200);
    }
    if rest == "/messages/count_tokens" && req.method == "POST" {
        return count_tokens(s, None, req, log);
    }
    let Some((inbound, body)) = parse_call(s, req, rest, log)? else { return Ok(log.status) };
    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or_default().to_string();

    // Forwards that list the model; unknown lists count as "maybe" and come after.
    let (mut sure, mut maybe): (Vec<Target>, Vec<Target>) = (vec![], vec![]);
    for t in targets {
        let list = all_models(root, &t);
        let wildcard = t.route.model_map.iter().any(|(f, _)| f == "*");
        if wildcard || list.iter().any(|m| m == &model) {
            sure.push(t);
        } else if list.is_empty() {
            maybe.push(t);
        }
    }
    let mut order = weighted_order(sure);
    order.extend(weighted_order(maybe));
    if order.is_empty() {
        write_json(s, 404, &convert::error_body(inbound, 404, &tr!("没有哪个转发提供模型「{model}」", "No forward serves model \"{model}\"")));
        return Ok(404);
    }
    let cfg = breaker_cfg(root);
    let mut paused = vec![];
    order.retain(|t| match breaker::is_open(&cfg, &t.route.id) {
        Some(why) => {
            paused.push(why);
            false
        }
        None => true,
    });
    if order.is_empty() {
        let msg = tr!("提供模型「{model}」的转发都因连续出错暂停了。{}", "Every forward serving model \"{model}\" is paused after repeated errors. {}", paused.join(" "));
        write_json(s, 503, &convert::error_body(inbound, 503, &msg));
        log.error = Some(msg);
        return Ok(503);
    }
    let n = order.len();
    let mut tried = vec![];
    for (i, t) in order.iter().enumerate() {
        log.route = format!("{entry} → {}", t.route.id);
        log.upstream = api_name(t.proto).into();
        // Another request may have paused it meanwhile, or be probing it.
        let Ok(ticket) = breaker::admit(&cfg, &t.route.id) else {
            tried.push(tr!("{} 已熔断", "{} paused (circuit breaker)", t.route.id));
            continue;
        };
        match tracked(s, req, t, inbound, &body, log, i + 1 < n, &cfg, ticket)? {
            Attempt::Done(st) => {
                if !tried.is_empty() {
                    log.error = Some(tr!("已切换：{}", "Failed over: {}", tried.join(crate::i18n::l("；", "; "))));
                }
                return Ok(st);
            }
            Attempt::Retry(why) => {
                let note = log.error.take().map(|n| tr!("（{n}）", " ({n})")).unwrap_or_default();
                tried.push(format!("{} {why}{note}", t.route.id));
            }
        }
    }
    // Nothing answered: every forward failed, or was paused while we were trying.
    let msg = tr!("所有转发都失败了：{}", "Every forward failed: {}", tried.join(crate::i18n::l("；", "; ")));
    write_json(s, 502, &convert::error_body(inbound, 502, &msg));
    log.error = Some(msg);
    Ok(502)
}

/// Inbound protocol + JSON body of a model call; writes the error itself when invalid.
fn parse_call(s: &mut TcpStream, req: &Request, rest: &str, log: &mut LogEntry) -> Result<Option<(Proto, Value)>> {
    let Some(inbound) = inbound_of(rest) else {
        write_json(s, 404, &json!({ "error": { "message": tr!("不支持的接口 {rest}，可用：/chat/completions、/responses、/messages、/models", "Unsupported endpoint {rest}; available: /chat/completions, /responses, /messages, /models") } }));
        log.status = 404;
        return Ok(None);
    };
    log.inbound = api_name(inbound).into();
    if req.method != "POST" {
        write_json(s, 405, &convert::error_body(inbound, 405, crate::i18n::l("只支持 POST", "Only POST is supported")));
        log.status = 405;
        return Ok(None);
    }
    // The client's mistake: a 400 that says why, not an upstream failure.
    let err = match serde_json::from_slice::<Value>(&req.body) {
        Ok(body) if body.is_object() => return Ok(Some((inbound, body))),
        Ok(_) => crate::i18n::l("请求体必须是 JSON 对象", "The request body must be a JSON object").to_string(),
        Err(e) => tr!("请求体不是 JSON：{e}", "Request body is not JSON: {e}"),
    };
    write_json(s, 400, &convert::error_body(inbound, 400, &err));
    log.status = 400;
    log.error = Some(err);
    Ok(None)
}

/// Claude Code asks for token counts; only Anthropic upstreams have that endpoint.
fn count_tokens(s: &mut TcpStream, t: Option<&Target>, req: &Request, log: &mut LogEntry) -> Result<u16> {
    log.inbound = "anthropic".into();
    if let Some(t) = t.filter(|t| t.proto == Proto::Anthropic) {
        let resp = auth(client().post(format!("{}/messages/count_tokens", t.base)).header("content-type", "application/json").body(req.body.clone()), t.proto, t.key.as_deref())
            .timeout(Duration::from_secs(30))
            .send()?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        write_full(s, status, "application/json", text.as_bytes());
        return Ok(status);
    }
    // Rough estimate (about 4 characters per token) so the client keeps working.
    write_json(s, 200, &json!({ "input_tokens": (req.body.len() / 4).max(1) }));
    Ok(200)
}

/// An error on the upstream's side (not the client's); it counts toward the breaker.
#[derive(Debug)]
struct UpstreamFault(String);

impl std::fmt::Display for UpstreamFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpstreamFault {}

fn fault(msg: String) -> anyhow::Error {
    anyhow::Error::new(UpstreamFault(msg))
}

/// `attempt`, reporting how it went to the forward's breaker. When this call pauses the
/// forward, the log entry says so.
#[allow(clippy::too_many_arguments)]
fn tracked(s: &mut TcpStream, req: &Request, t: &Target, inbound: Proto, body: &Value, log: &mut LogEntry, can_retry: bool, cfg: &breaker::Config, ticket: breaker::Ticket) -> Result<Attempt> {
    log.error = None;
    log.upstream_broken = false;
    let r = attempt(s, req, t, inbound, body, log, can_retry);
    let outcome = match &r {
        Ok(Attempt::Done(st)) if breaker::is_fault_status(*st) => Outcome::Fault(breaker::describe(*st, log.error.as_deref().unwrap_or(""))),
        Ok(Attempt::Done(_)) if log.upstream_broken => Outcome::Fault(log.error.clone().unwrap_or_default()),
        Ok(Attempt::Done(_)) => Outcome::Ok,
        Ok(Attempt::Retry(why)) => Outcome::Fault(why.clone()),
        Err(e) => match e.downcast_ref::<UpstreamFault>() {
            Some(f) => Outcome::Fault(f.0.clone()),
            None => Outcome::Neutral,
        },
    };
    if let Some(note) = breaker::record(cfg, &t.route.id, outcome, ticket) {
        log.error = Some(match log.error.take() {
            Some(e) => tr!("{e}（{note}）", "{e} ({note})"),
            None => note,
        });
    }
    r
}

enum Attempt {
    /// A response was written to the client.
    Done(u16),
    /// Nothing written yet; the caller may try another forward.
    Retry(String),
}

/// Sends one call through one forward. With `can_retry`, connection failures and
/// 5xx / 429 answers return `Retry` without touching the client connection.
fn attempt(s: &mut TcpStream, req: &Request, t: &Target, inbound: Proto, body: &Value, log: &mut LogEntry, can_retry: bool) -> Result<Attempt> {
    let upstream = t.proto;
    let mut body = body.clone();
    apply_model_map(&mut body, &t.route.model_map);
    let ctx = convert::req_ctx(inbound, &body);
    log.model = ctx.model.clone();
    log.stream = ctx.stream;
    log.converted = inbound != upstream;

    let upstream_body = if inbound == upstream {
        body
    } else {
        let chat = convert::request_to_chat(inbound, &body)?;
        convert::request_from_chat(upstream, &chat)?
    };
    let url = format!("{}{}", t.base, upstream.path());
    let mut up = client().post(&url).header("content-type", "application/json").body(upstream_body.to_string());
    up = auth(up, upstream, t.key.as_deref());
    if upstream == Proto::Anthropic {
        if let Some(b) = req.header("anthropic-beta").filter(|_| inbound == Proto::Anthropic) {
            up = up.header("anthropic-beta", b);
        }
    }
    if ctx.stream {
        up = up.header("accept", "text/event-stream");
    }
    let mut resp = match up.send() {
        Ok(r) => r,
        Err(e) if can_retry => return Ok(Attempt::Retry(tr!("连不上：{e}", "unreachable: {e}"))),
        Err(e) => return Err(fault(if e.is_connect() { tr!("连接上游失败：{e}", "Can't connect to upstream: {e}") } else { tr!("请求上游失败：{e}", "Upstream request failed: {e}") })),
    };
    let status = resp.status().as_u16();

    if !resp.status().is_success() {
        let text = resp.text().unwrap_or_default();
        if can_retry && (status >= 500 || status == 429) {
            return Ok(Attempt::Retry(breaker::describe(status, &text)));
        }
        log.error = Some(text.chars().take(300).collect());
        if inbound == upstream {
            write_full(s, status, "application/json", text.as_bytes());
        } else {
            write_json(s, status, &convert::error_body(inbound, status, &text));
        }
        return Ok(Attempt::Done(status));
    }

    let is_sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(|v| v.contains("event-stream")).unwrap_or(false);
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/json").to_string();
    if inbound == upstream && ctx.stream {
        // Passthrough, streamed as it arrives.
        let mut sniff = UsageSniff::new(true);
        let r = pipe(s, resp, status, &ct, &mut sniff, log);
        log.usage = sniff.finish();
        return r.map(Attempt::Done);
    }
    let read_err = |e: std::io::Error| fault(tr!("读取上游响应失败：{e}", "Failed to read the upstream response: {e}"));
    // Some upstreams stream SSE under another Content-Type, or even when asked not to
    // stream: look at the body too.
    let (is_sse, head) = if is_sse { (true, vec![]) } else { sniff_sse(&mut resp).map_err(read_err)? };
    let mut body = std::io::Cursor::new(head).chain(resp);
    if inbound == upstream && !is_sse {
        // Passthrough of a one-piece answer.
        let mut sniff = UsageSniff::new(false);
        let r = pipe(s, body, status, &ct, &mut sniff, log);
        log.usage = sniff.finish();
        return r.map(Attempt::Done);
    }
    if ctx.stream && is_sse {
        stream_convert(s, &mut body, inbound, upstream, ctx, log)?;
        return Ok(Attempt::Done(status));
    }
    // The client gets one piece: the upstream's JSON (also when it ignored stream=true), or
    // its stream collected when it streamed although asked not to.
    let mut raw = vec![];
    body.read_to_end(&mut raw).map_err(read_err)?;
    let text = String::from_utf8_lossy(&raw);
    let chat = if is_sse {
        let mut up = UpstreamStream::new(upstream);
        let mut chunks = up.feed(&text);
        chunks.extend(up.finish());
        chat_from_chunks(&chunks).map_err(|e| fault(tr!("上游的流式响应出错：{e}", "The upstream stream returned an error: {e}")))?
    } else {
        let v: Value = serde_json::from_str(&text).map_err(|_| fault(tr!("上游返回的不是 JSON：{}", "Upstream response is not JSON: {}", text.chars().take(200).collect::<String>())))?;
        convert::response_to_chat(upstream, &v).map_err(|e| fault(tr!("上游响应无法解析：{e:#}", "Can't parse the upstream response: {e:#}")))?
    };
    log.usage = convert::usage_tokens(&chat);
    if ctx.stream {
        // Client wanted a stream: replay the whole answer as one.
        Chunked::start(s, 200, "text/event-stream")?;
        let mut out = Chunked(s);
        let mut down = DownstreamStream::new(inbound, ctx);
        for c in chat_as_chunks(&chat) {
            for f in down.push(&c) {
                out.send(f.as_bytes())?;
            }
        }
        for f in down.finish() {
            out.send(f.as_bytes())?;
        }
        out.end();
    } else {
        write_json(s, 200, &convert::response_from_chat(inbound, &chat, &ctx)?);
    }
    Ok(Attempt::Done(200))
}

fn auth(req: reqwest::blocking::RequestBuilder, p: Proto, key: Option<&str>) -> reqwest::blocking::RequestBuilder {
    match (p, key) {
        (Proto::Anthropic, Some(k)) => req.header("x-api-key", k).header("anthropic-version", "2023-06-01"),
        (Proto::Anthropic, None) => req.header("anthropic-version", "2023-06-01"),
        (_, Some(k)) => req.bearer_auth(k),
        _ => req,
    }
}

fn apply_model_map(body: &mut Value, map: &[(String, String)]) {
    let Some(m) = body.get("model").and_then(|m| m.as_str()).map(String::from) else { return };
    for (from, to) in map {
        if !to.trim().is_empty() && (from == "*" || from == &m) {
            body["model"] = json!(to.trim());
            return;
        }
    }
}

fn models(s: &mut TcpStream, t: &Target, req: &Request, log: &mut LogEntry) -> Result<u16> {
    let want = if req.header("anthropic-version").is_some() || req.header("x-api-key").is_some() { Proto::Anthropic } else { Proto::Chat };
    log.inbound = api_name(want).into();
    let resp = auth(client().get(format!("{}/models", t.base)).timeout(Duration::from_secs(20)), t.proto, t.key.as_deref()).send()?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status >= 400 {
        write_json(s, status, &convert::error_body(want, status, &text));
        return Ok(status);
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(json!({ "data": [] }));
    write_json(s, 200, &convert::models_body(want, &v));
    Ok(200)
}

/// Picks the token usage out of a passthrough response without changing it: SSE is
/// scanned line by line, a plain body is parsed once it is complete.
struct UsageSniff {
    sse: bool,
    buf: Vec<u8>,
    usage: Option<(u64, u64)>,
}

impl UsageSniff {
    const MAX: usize = 8 * 1024 * 1024;

    fn new(sse: bool) -> Self {
        UsageSniff { sse, buf: vec![], usage: None }
    }

    fn feed(&mut self, data: &[u8]) {
        if !self.sse {
            if self.buf.len() + data.len() <= Self::MAX {
                self.buf.extend_from_slice(data);
            }
            return;
        }
        for &b in data {
            if b == b'\n' {
                self.line();
            } else if self.buf.len() < Self::MAX {
                self.buf.push(b);
            }
        }
    }

    fn line(&mut self) {
        let line = std::mem::take(&mut self.buf);
        let Some(rest) = line.strip_prefix(b"data:") else { return };
        if rest.windows(7).any(|w| w == b"\"usage\"") {
            if let Ok(v) = serde_json::from_slice::<Value>(rest.trim_ascii()) {
                self.take(&v);
            }
        }
    }

    /// Streams report usage in pieces (Anthropic: input at the start, output at the end).
    fn take(&mut self, v: &Value) {
        if let Some((i, o)) = convert::usage_tokens(v) {
            let (a, b) = self.usage.unwrap_or_default();
            self.usage = Some((a.max(i), b.max(o)));
        }
    }

    fn finish(mut self) -> Option<(u64, u64)> {
        if self.sse {
            self.line();
        } else if let Ok(v) = serde_json::from_slice::<Value>(&self.buf) {
            self.take(&v);
        }
        self.usage
    }
}

/// Reads the start of a body labelled as something other than SSE and tells whether it is
/// SSE after all. Returns what it read, to be replayed in front of the rest.
fn sniff_sse(r: &mut impl Read) -> std::io::Result<(bool, Vec<u8>)> {
    let mut head = vec![];
    let mut buf = [0u8; 1024];
    while head.len() < 4096 && head.iter().all(u8::is_ascii_whitespace) {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let t = head.trim_ascii_start();
    Ok((t.starts_with(b"data:") || t.starts_with(b"event:"), head))
}

fn pipe(s: &mut TcpStream, mut resp: impl Read, status: u16, ct: &str, sniff: &mut UsageSniff, log: &mut LogEntry) -> Result<u16> {
    Chunked::start(s, status, ct)?;
    let mut out = Chunked(s);
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                // No closing chunk: the client sees a cut-off body, not a complete answer.
                log.error = Some(tr!("上游中断：{e}", "Upstream interrupted: {e}"));
                log.upstream_broken = true;
                return Ok(status);
            }
        };
        sniff.feed(&buf[..n]);
        if out.send(&buf[..n]).is_err() {
            return Ok(status); // client went away
        }
    }
    out.end();
    Ok(status)
}

/// Decodes as much of `pending` as is valid UTF-8, turning invalid bytes into U+FFFD and
/// keeping only an incomplete character at the end for the next read.
fn take_utf8(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(t) => {
                text.push_str(t);
                pending.clear();
                return text;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                text.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or_default());
                match e.error_len() {
                    Some(n) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        pending.drain(..valid + n);
                    }
                    None => {
                        pending.drain(..valid);
                        return text;
                    }
                }
            }
        }
    }
}

fn stream_convert(s: &mut TcpStream, resp: &mut impl Read, inbound: Proto, upstream: Proto, ctx: convert::ReqCtx, log: &mut LogEntry) -> Result<()> {
    Chunked::start(s, 200, "text/event-stream")?;
    let mut out = Chunked(s);
    let mut up = UpstreamStream::new(upstream);
    let mut down = DownstreamStream::new(inbound, ctx);
    let mut buf = [0u8; 16 * 1024];
    let mut pending: Vec<u8> = vec![]; // bytes of a UTF-8 char split across reads
    let mut client_gone = false;
    loop {
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log.error = Some(tr!("上游中断：{e}", "Upstream interrupted: {e}"));
                log.upstream_broken = true;
                for f in down.error(502, &tr!("上游连接中断：{e}", "Upstream connection broken: {e}")) {
                    let _ = out.send(f.as_bytes());
                }
                out.end();
                return Ok(());
            }
        };
        pending.extend_from_slice(&buf[..n]);
        let text = take_utf8(&mut pending);
        for chunk in up.feed(&text) {
            if let Some(u) = convert::usage_tokens(&chunk) {
                log.usage = Some(u);
            }
            for f in down.push(&chunk) {
                if out.send(f.as_bytes()).is_err() {
                    client_gone = true;
                }
            }
        }
        if client_gone {
            return Ok(());
        }
    }
    // A character cut off at the very end.
    let mut tail = if pending.is_empty() { vec![] } else { up.feed(&String::from_utf8_lossy(&pending)) };
    tail.extend(up.finish());
    for chunk in tail {
        if let Some(u) = convert::usage_tokens(&chunk) {
            log.usage = Some(u);
        }
        for f in down.push(&chunk) {
            let _ = out.send(f.as_bytes());
        }
    }
    if up.ended_early() {
        // The client got an error event; the log and the breaker hear about it too.
        log.error = Some(crate::i18n::l("上游的流没有正常结束就断开了", "The upstream stream ended before it finished").into());
        log.upstream_broken = true;
    }
    for f in down.finish() {
        let _ = out.send(f.as_bytes());
    }
    out.end();
    Ok(())
}

/// A complete chat.completion as stream chunks (for clients that asked for a stream
/// when the upstream answered in one piece).
fn chat_as_chunks(chat: &Value) -> Vec<Value> {
    let id = chat.get("id").cloned().unwrap_or(json!("chatcmpl-agentplus"));
    let model = chat.get("model").cloned().unwrap_or(json!(""));
    let created = chat.get("created").cloned().unwrap_or(json!(chrono::Utc::now().timestamp()));
    let msg = chat.pointer("/choices/0/message").cloned().unwrap_or(json!({}));
    let finish = chat.pointer("/choices/0/finish_reason").cloned().unwrap_or(json!("stop"));
    let chunk = |delta: Value, finish: Value| json!({ "id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
    let mut out = vec![chunk(json!({ "role": "assistant" }), Value::Null)];
    let (text, reasoning) = convert::message_text(&msg);
    if !reasoning.is_empty() {
        out.push(chunk(json!({ "reasoning_content": reasoning }), Value::Null));
    }
    if !text.is_empty() {
        out.push(chunk(json!({ "content": text }), Value::Null));
    }
    if let Some(calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, c) in calls.iter().enumerate() {
            out.push(chunk(json!({ "tool_calls": [{ "index": i, "id": c.get("id"), "type": "function", "function": { "name": c.pointer("/function/name"), "arguments": c.pointer("/function/arguments") } }] }), Value::Null));
        }
    }
    out.push(chunk(json!({}), finish));
    if let Some(u) = chat.get("usage") {
        out.push(json!({ "id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [], "usage": u }));
    }
    out
}

/// The reverse of `chat_as_chunks`: stream chunks folded into one chat.completion, for
/// clients that asked for one piece when the upstream streamed anyway. An error chunk
/// (from the upstream, or a stream cut off) is returned as the error.
fn chat_from_chunks(chunks: &[Value]) -> std::result::Result<Value, String> {
    let (mut text, mut reasoning) = (String::new(), String::new());
    let mut tools: Vec<Value> = vec![];
    let mut finish = Value::Null;
    let mut usage = Value::Null;
    let str_at = |v: &Value, p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    for c in chunks {
        if let Some(e) = c.get("error").filter(|e| !e.is_null()) {
            return Err(str_at(e, "/message"));
        }
        if let Some(u) = c.get("usage").filter(|u| u.is_object()) {
            usage = u.clone();
        }
        let Some(ch) = c.pointer("/choices/0") else { continue };
        if let Some(f) = ch.get("finish_reason").filter(|f| !f.is_null()) {
            finish = f.clone();
        }
        let d = ch.get("delta").cloned().unwrap_or(Value::Null);
        text += &str_at(&d, "/content");
        reasoning += &str_at(&d, "/reasoning_content");
        for tc in d.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
            let i = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(tools.len() as u64) as usize;
            while tools.len() <= i {
                tools.push(json!({ "id": "", "type": "function", "function": { "name": "", "arguments": "" } }));
            }
            let t = &mut tools[i];
            for (key, ptr) in [("id", "/id"), ("name", "/function/name"), ("arguments", "/function/arguments")] {
                let add = str_at(tc, ptr);
                let slot = if key == "id" { &mut t["id"] } else { &mut t["function"][key] };
                let joined = format!("{}{add}", slot.as_str().unwrap_or(""));
                *slot = json!(joined);
            }
        }
    }
    let mut msg = json!({ "role": "assistant", "content": text });
    if !reasoning.is_empty() {
        msg["reasoning_content"] = json!(reasoning);
    }
    if !tools.is_empty() {
        msg["tool_calls"] = json!(tools);
    }
    let first = chunks.iter().find(|c| c.get("id").is_some());
    let field = |k: &str| first.and_then(|c| c.get(k)).cloned().unwrap_or(Value::Null);
    let finish = if finish.is_null() { json!(if tools.is_empty() { "stop" } else { "tool_calls" }) } else { finish };
    Ok(json!({
        "id": field("id"), "object": "chat.completion", "created": field("created"), "model": field("model"),
        "choices": [{ "index": 0, "message": msg, "finish_reason": finish }],
        "usage": usage,
    }))
}

/// Tests bypass the store: (route, upstream base, key).
#[cfg(test)]
static TEST_ROUTES: Mutex<Vec<(Route, String, Option<String>)>> = Mutex::new(Vec::new());
/// Gateway tests share TEST_ROUTES, so they run one at a time.
#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock upstream that only speaks Chat Completions (streaming), recording the request.
    fn chat_upstream(seen: Arc<Mutex<Option<Value>>>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let req = read_request(&s).unwrap();
            assert_eq!(req.path, "/v1/chat/completions");
            assert_eq!(req.header("authorization"), Some("Bearer up-key"));
            *seen.lock().unwrap() = serde_json::from_slice(&req.body).ok();
            let frames = [
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"glm-5","choices":[{"index":0,"delta":{"role":"assistant","content":"po"},"finish_reason":null}]}"#,
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"glm-5","choices":[{"index":0,"delta":{"content":"ng 你好"},"finish_reason":null}]}"#,
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"glm-5","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":"}}]},"finish_reason":null}]}"#,
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"glm-5","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#,
                r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"glm-5","choices":[],"usage":{"prompt_tokens":20,"completion_tokens":7,"total_tokens":27}}"#,
            ];
            let mut body = String::new();
            for f in frames {
                body.push_str(&format!("data: {f}

"));
            }
            body.push_str("data: [DONE]

");
            let head = format!("HTTP/1.1 200 OK
Content-Type: text/event-stream
Content-Length: {}
Connection: close

", body.len());
            s.write_all(head.as_bytes()).unwrap();
            // Dribble the body out in small pieces to exercise re-assembly (incl. split UTF-8).
            for c in body.as_bytes().chunks(5) {
                s.write_all(c).unwrap();
                s.flush().unwrap();
            }
        });
        format!("http://{addr}/v1")
    }

    /// Responses client (Codex style) → gateway → Chat-only upstream, streaming, with a tool call.
    #[test]
    fn end_to_end_responses_over_chat_upstream() {
        let seen = Arc::new(Mutex::new(None));
        let up = chat_upstream(seen.clone());
        let route = Route { id: "relay".into(), name: "relay".into(), library: "x".into(), upstream_api: "chat".into(), model_map: vec![("gpt-5.5".into(), "glm-5".into())], enabled: true, weight: 100, replaced: vec![] };
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *TEST_ROUTES.lock().unwrap() = vec![(route, up, Some("up-key".into()))];

        let gw = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = gw.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = gw.accept().unwrap();
            handle(s);
        });
        let body = json!({
            "model": "gpt-5.5", "stream": true, "instructions": "You are Codex.",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }],
            "tools": [{ "type": "function", "name": "shell", "description": "run", "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } } }],
        });
        let resp = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/relay/v1/responses"))
            .bearer_auth(keys::PLACEHOLDER)
            .body(body.to_string())
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let text = resp.text().unwrap();

        let sent = seen.lock().unwrap().clone().unwrap();
        assert_eq!(sent["model"], "glm-5", "model map applied");
        assert_eq!(sent["stream"], true);
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(sent["tools"][0]["function"]["name"], "shell");

        let events: Vec<Value> = text
            .split("

")
            .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")))
            .filter_map(|d| serde_json::from_str(d).ok())
            .collect();
        let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
        assert_eq!(types.first(), Some(&"response.created"));
        assert!(types.contains(&"response.output_text.delta"));
        assert!(types.contains(&"response.function_call_arguments.delta"));
        let done = events.iter().find(|e| e["type"] == "response.completed").expect("response.completed");
        let out = done["response"]["output"].as_array().unwrap();
        let msg = out.iter().find(|i| i["type"] == "message").unwrap();
        assert_eq!(msg["content"][0]["text"], "pong 你好");
        let call = out.iter().find(|i| i["type"] == "function_call").unwrap();
        assert_eq!(call["name"], "shell");
        assert_eq!(call["call_id"], "call_1");
        assert_eq!(call["arguments"], r#"{"cmd":"ls"}"#);
        assert_eq!(done["response"]["usage"]["output_tokens"], 7);
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// Mock upstream that answers every request with a fixed status and body (repeatable).
    fn fixed_upstream(status: u16, body: &'static str, hits: Arc<AtomicU64>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut s) = c else { continue };
                // Count model calls only (the gateway also asks for /models).
                if read_request(&s).map(|r| r.method == "POST").unwrap_or(false) {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                let head = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(body.as_bytes());
            }
        });
        format!("http://{addr}/v1")
    }

    fn gateway_once() -> u16 {
        gateway_n(4)
    }

    /// Gateway that serves `n` connections.
    fn gateway_n(n: usize) -> u16 {
        let gw = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = gw.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in gw.incoming().take(n).flatten() {
                handle(s);
            }
        });
        port
    }

    /// Unified entry: model routing, failover from a 503 forward, and the merged model list.
    #[test]
    fn unified_entry_routes_and_fails_over() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (bad_hits, good_hits, other_hits) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let bad = fixed_upstream(503, r#"{"error":{"message":"overloaded"}}"#, bad_hits.clone());
        let good = fixed_upstream(200, r#"{"id":"c","object":"chat.completion","model":"glm-5","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#, good_hits.clone());
        let other = fixed_upstream(200, r#"{"data":[{"id":"kimi-k3"}]}"#, other_hits.clone());
        let route = |id: &str, weight: u32, map: Vec<(String, String)>| Route { id: id.into(), name: id.into(), library: id.into(), upstream_api: "chat".into(), model_map: map, enabled: true, weight, replaced: vec![] };
        // Both "bad" and "good" serve glm-5 (via their model maps); "other" only kimi-k3.
        *TEST_ROUTES.lock().unwrap() = vec![
            (route("bad", 1000, vec![("glm-5".into(), "glm-5".into())]), bad, None),
            (route("good", 1, vec![("glm-5".into(), "glm-5".into())]), good, None),
            (route("other", 100, vec![("kimi-k3".into(), "kimi-k3".into())]), other, None),
        ];
        MODEL_CACHE.lock().unwrap().clear();
        breaker::reset(Some("bad"));
        let port = gateway_once();
        let client = reqwest::blocking::Client::new();

        let r = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth(test_key())
            .body(r#"{"model":"glm-5","messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        let v: Value = serde_json::from_str(&r.text().unwrap()).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "pong");
        assert!(good_hits.load(Ordering::SeqCst) == 1, "served by the healthy forward");
        assert_eq!(other_hits.load(Ordering::SeqCst), 0, "a forward without the model is never called");

        // A model nobody serves: 404 in the client's protocol, no upstream hit.
        let before = good_hits.load(Ordering::SeqCst);
        let r = client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("x-api-key", test_key())
            .body(r#"{"model":"nope","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .unwrap();
        // "nope" is unknown to every forward whose list is known; forwards with empty lists would be tried.
        assert!(r.status().as_u16() == 404 || good_hits.load(Ordering::SeqCst) > before);

        // Merged model list.
        let r = client.get(format!("http://127.0.0.1:{port}/v1/models")).bearer_auth(test_key()).send().unwrap();
        let v: Value = serde_json::from_str(&r.text().unwrap()).unwrap();
        let ids: Vec<&str> = v["data"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
        assert!(ids.contains(&"glm-5") && ids.contains(&"kimi-k3"), "{ids:?}");
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// A forward whose upstream keeps rejecting the key is paused after three failures; the
    /// client then gets a 503 naming the error, without the upstream being called again.
    #[test]
    fn breaker_pauses_failing_forward() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hits = Arc::new(AtomicU64::new(0));
        let up = fixed_upstream(401, r#"{"error":{"message":"invalid api key"}}"#, hits.clone());
        let route = Route { id: "flaky".into(), name: "flaky".into(), library: "x".into(), upstream_api: "chat".into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route, up, None)];
        breaker::reset(Some("flaky"));
        let port = gateway_n(6);
        let client = reqwest::blocking::Client::new();
        let call = |key: &str| {
            let r = client
                .post(format!("http://127.0.0.1:{port}/flaky/v1/chat/completions"))
                .bearer_auth(key)
                .body(r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#)
                .send()
                .unwrap();
            (r.status().as_u16(), r.text().unwrap())
        };
        for _ in 0..3 {
            assert_eq!(call(keys::PLACEHOLDER).0, 401);
        }
        let (st, text) = call(keys::PLACEHOLDER);
        assert_eq!(st, 503);
        assert!(text.contains("熔断") && text.contains("HTTP 401 密钥无效或未授权：invalid api key"), "{text}");
        assert_eq!(hits.load(Ordering::SeqCst), 3, "paused forward is not called");
        assert!(LOG.lock().unwrap().iter().any(|l| l.error.as_deref().is_some_and(|e| e.contains("连续 3 次出错，转发暂停 60 秒"))));
        // AgentPlus's own test still goes through while paused.
        assert_eq!(call(test_key()).0, 401);
        assert_eq!(hits.load(Ordering::SeqCst), 4);
        breaker::reset(Some("flaky"));
        assert_eq!(call(keys::PLACEHOLDER).0, 401, "reset lets requests through again");
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// "/a+b/v1" only uses forwards a and b.
    #[test]
    fn combined_forwards_address() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (a_hits, b_hits) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let ok = r#"{"id":"c","object":"chat.completion","model":"glm-5","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}]}"#;
        let a = fixed_upstream(200, ok, a_hits.clone());
        let b = fixed_upstream(200, ok, b_hits.clone());
        let route = |id: &str| Route { id: id.into(), name: id.into(), library: id.into(), upstream_api: "chat".into(), model_map: vec![("glm-5".into(), "glm-5".into())], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route("pa"), a, None), (route("pb"), b, None)];
        MODEL_CACHE.lock().unwrap().clear();
        let port = gateway_n(4);
        let client = reqwest::blocking::Client::new();
        for _ in 0..3 {
            let r = client.post(format!("http://127.0.0.1:{port}/pb+nope/v1/chat/completions")).bearer_auth(test_key()).body(r#"{"model":"glm-5","messages":[]}"#).send().unwrap();
            assert_eq!(r.status().as_u16(), 200);
        }
        assert_eq!((a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)), (0, 3));
        let r = client.post(format!("http://127.0.0.1:{port}/x+y/v1/chat/completions")).bearer_auth(test_key()).body(r#"{"model":"glm-5","messages":[]}"#).send().unwrap();
        assert_eq!(r.status().as_u16(), 503);
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// A forward pointed at another upstream (library address or key edited) stops using
    /// the model list cached from the old one.
    #[test]
    fn model_cache_follows_upstream_changes() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hits = Arc::new(AtomicU64::new(0));
        let old = fixed_upstream(200, r#"{"data":[{"id":"old-model"}]}"#, hits.clone());
        let new = fixed_upstream(200, r#"{"data":[{"id":"new-model"}]}"#, hits.clone());
        let route = Route { id: "moved".into(), name: "moved".into(), library: "x".into(), upstream_api: "chat".into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        let root = json!({});
        MODEL_CACHE.lock().unwrap().clear();
        *TEST_ROUTES.lock().unwrap() = vec![(route.clone(), old, None)];
        let t = target(&root, &route).unwrap();
        assert_eq!(all_models(&root, &t), vec!["old-model"]);
        assert_eq!(known_models(&root, &route), vec!["old-model"], "cached list shows while the upstream is the same");

        *TEST_ROUTES.lock().unwrap() = vec![(route.clone(), new, None)];
        assert!(known_models(&root, &route).is_empty(), "old upstream's list is not shown for the new one");
        let t = target(&root, &route).unwrap();
        assert_eq!(all_models(&root, &t), vec!["new-model"]);

        // Saving or deleting a forward drops its cache outright.
        forget_models(&["moved"]);
        assert!(MODEL_CACHE.lock().unwrap().iter().all(|(id, ..)| id != "moved"));
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// Stopping waits for the listener to close, so the same port opens again at once;
    /// a port that is taken is reported without disturbing the running gateway.
    #[test]
    fn restart_reuses_port_and_busy_port_keeps_old() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _c = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        for _ in 0..5 {
            let l = bind(port).expect("port reopens right after stop");
            run(l, port).unwrap();
            assert_eq!(running_port(), Some(port));
            stop();
            assert_eq!(running_port(), None);
        }

        run(bind(port).unwrap(), port).unwrap();
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = taken.local_addr().unwrap().port();
        let e = bind(busy).unwrap_err().to_string();
        assert!(e.contains(&busy.to_string()), "{e}");
        assert_eq!(running_port(), Some(port), "a failed bind leaves the gateway where it was");
        let r = reqwest::blocking::get(format!("http://127.0.0.1:{port}/health")).unwrap();
        assert_eq!(r.status().as_u16(), 200);
        stop();
        *LAST_ERROR.lock().unwrap() = None;
    }

    #[test]
    fn remembers_former_ports() {
        let mut c = Config { port: 18650, ..Default::default() };
        for p in [18651, 18652, 18652, 18650] {
            move_port(&mut c, p);
        }
        assert_eq!(c.port, 18650);
        assert_eq!(c.former_ports, vec![18652, 18651], "newest first, current port left out");
    }

    /// No key / a wrong key: 401 before any upstream call. A web page's origin or a foreign
    /// Host: 403. An agent's own key is accepted and the request counts for that agent.
    #[test]
    fn inbound_auth_and_origin() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hits = Arc::new(AtomicU64::new(0));
        let ok = r#"{"id":"c","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        let up = fixed_upstream(200, ok, hits.clone());
        let route = Route { id: "authr".into(), name: "authr".into(), library: "x".into(), upstream_api: "chat".into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route, up, None)];
        *keys::TEST_KEYS.lock().unwrap() = vec![("codex".into(), "agp-codex-test".into())];
        let port = gateway_n(6);
        let client = reqwest::blocking::Client::new();
        let url = format!("http://127.0.0.1:{port}/authr/v1/chat/completions");
        let send = |rb: reqwest::blocking::RequestBuilder| {
            let r = rb.body(r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#).send().unwrap();
            let cors = r.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap().to_string());
            (r.status().as_u16(), cors)
        };

        assert_eq!(send(client.post(&url)).0, 401, "no key");
        assert_eq!(send(client.post(&url).bearer_auth("sk-guess")).0, 401, "unknown key");
        assert_eq!(send(client.post(&url).bearer_auth("agp-codex-test").header("origin", "https://evil.example")).0, 403, "web page");
        assert_eq!(send(client.post(&url).bearer_auth("agp-codex-test").header("host", "evil.example")).0, 403, "DNS rebinding");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "refused requests never reach the upstream");

        assert_eq!(send(client.post(&url).header("x-api-key", "agp-codex-test")), (200, None), "agent key; no CORS header without an origin");
        let (st, cors) = send(client.post(&url).bearer_auth("agp-codex-test").header("origin", "app://zcode"));
        assert_eq!((st, cors.as_deref()), (200, Some("app://zcode")), "a desktop app's own page");
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // Counted after the response went out: give the handler a moment to finish.
        let codex = || SERIES.lock().unwrap().iter().filter_map(|m| m.agents.get("codex").cloned()).fold(AgentUse::default(), |a, u| AgentUse {
            requests: a.requests + u.requests,
            failures: a.failures + u.failures,
            input_tokens: a.input_tokens + u.input_tokens,
            output_tokens: a.output_tokens + u.output_tokens,
        });
        for _ in 0..50 {
            if codex().requests >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let u = codex();
        assert!(u.requests >= 2 && u.input_tokens >= 10 && u.output_tokens >= 4, "{u:?}");
        assert!(LOG.lock().unwrap().iter().take(6).any(|l| l.status == 401 && l.agent.is_none()));
        keys::TEST_KEYS.lock().unwrap().clear();
        TEST_ROUTES.lock().unwrap().clear();
    }

    #[test]
    fn checks_origins_and_hosts() {
        for o in ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost", "app://zcode", "file://", "vscode-webview://abc", "http://localhost:1420", "http://127.0.0.1:5173", "http://[::1]:3000"] {
            assert!(origin_allowed(o), "{o}");
        }
        // "null" is a sandboxed iframe or data: page; other schemes aren't apps we know.
        for o in ["null", "data:text/html,x", "chrome-extension://abc", "https://evil.example", "http://192.168.1.2", "http://localhost.evil.example", "not a url"] {
            assert!(!origin_allowed(o), "{o}");
        }
        assert!(is_loopback_host("127.0.0.1") && is_loopback_host("[::1]") && is_loopback_host("LOCALHOST"));
        assert!(!is_loopback_host("0.0.0.0") && !is_loopback_host("example.com"));
    }

    #[test]
    fn splits_paths() {
        assert_eq!(split_path("/relay/v1/chat/completions"), Some(("relay".into(), "/chat/completions".into())));
        assert_eq!(split_path("/relay/responses?x=1"), Some(("relay".into(), "/responses".into())));
        assert_eq!(split_path("/relay/v1/messages/"), Some(("relay".into(), "/messages".into())));
        assert_eq!(split_path("/relay/v1"), Some(("relay".into(), "".into())));
        assert_eq!(inbound_of("/messages"), Some(Proto::Anthropic));
    }

    #[test]
    fn sniffs_usage() {
        // Anthropic stream: input in message_start, output in message_delta, split across reads.
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":5,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\n\n";
        let mut s = UsageSniff::new(true);
        for part in sse.as_bytes().chunks(7) {
            s.feed(part);
        }
        assert_eq!(s.finish(), Some((15, 42)));

        let mut s = UsageSniff::new(true);
        s.feed(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}}");
        assert_eq!(s.finish(), Some((3, 4)));

        let mut s = UsageSniff::new(false);
        s.feed(b"{\n  \"choices\": [],\n  \"usage\": {\"prompt_tokens\": 8, \"completion_tokens\": 2}\n}");
        assert_eq!(s.finish(), Some((8, 2)));
    }

    #[test]
    fn maps_models() {
        let mut b = json!({ "model": "gpt-5.5" });
        apply_model_map(&mut b, &[("other".into(), "x".into()), ("gpt-5.5".into(), "glm-5".into())]);
        assert_eq!(b["model"], "glm-5");
        apply_model_map(&mut b, &[("*".into(), "kimi".into())]);
        assert_eq!(b["model"], "kimi");
    }

    /// The test key is random per launch, not a constant any web page could know.
    #[test]
    fn test_key_is_random() {
        let k = test_key();
        assert!(k.starts_with("agp-test-") && k.len() == 49, "{k}");
        assert_eq!(k, test_key(), "stable within one launch");
        assert_ne!(k, "agentplus-gateway-test");
    }

    #[test]
    fn invalid_utf8_does_not_stall() {
        // A bad byte becomes U+FFFD and is consumed; an incomplete tail waits for more.
        let mut p = b"ab\xffcd\xe4\xbd".to_vec();
        assert_eq!(take_utf8(&mut p), "ab\u{fffd}cd");
        assert_eq!(p, b"\xe4\xbd");
        p.push(0xa0);
        assert_eq!(take_utf8(&mut p), "你");
        assert!(p.is_empty());
        let mut p = b"\xc3\x28\xff\xff".to_vec();
        assert_eq!(take_utf8(&mut p), "\u{fffd}(\u{fffd}\u{fffd}");
        assert!(p.is_empty());
    }

    /// Raw request to the gateway; returns the status line's code.
    fn raw_status(port: u16, req: &[u8]) -> u16 {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _ = c.write_all(req);
        let _ = c.shutdown(Shutdown::Write);
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    #[test]
    fn request_limits() {
        let port = gateway_n(5);
        let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(MAX_LINE + 10));
        assert_eq!(raw_status(port, long.as_bytes()), 431, "request line too long");
        let many: String = (0..=MAX_HEADERS).map(|i| format!("x-h{i}: 1\r\n")).collect();
        assert_eq!(raw_status(port, format!("GET / HTTP/1.1\r\n{many}\r\n").as_bytes()), 431, "too many headers");
        // Refused before any body arrives (nothing is allocated up front).
        assert_eq!(raw_status(port, format!("POST /x/v1/responses HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1).as_bytes()), 413);
        // A chunk size that would overflow the running total.
        assert_eq!(raw_status(port, b"POST /x/v1/responses HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\nffffffffffffffff\r\n"), 413);
        // A body shorter than announced is an error, not a hang.
        assert_eq!(raw_status(port, b"POST /x/v1/responses HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc"), 400);
    }

    #[test]
    fn connection_slots_are_capped() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = CONNS.load(Ordering::SeqCst);
        let slots: Vec<ConnSlot> = std::iter::from_fn(ConnSlot::take).take(MAX_CONNS + 5).collect();
        assert_eq!(slots.len(), MAX_CONNS - before);
        assert!(ConnSlot::take().is_none());
        drop(slots);
        assert_eq!(CONNS.load(Ordering::SeqCst), before);
        assert!(ConnSlot::take().is_some());
    }

    #[test]
    fn active_count_survives_panic() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = ACTIVE.load(Ordering::Relaxed);
        let r = std::thread::spawn(|| {
            let (_a, n) = Active::start();
            assert!(n >= 1);
            panic!("handler blew up");
        })
        .join();
        assert!(r.is_err());
        assert_eq!(ACTIVE.load(Ordering::Relaxed), before);
    }

    /// One bad route (or field) doesn't wipe the rest, and saving keeps the bad route as is.
    #[test]
    fn bad_config_field_keeps_the_rest() {
        let root = json!({ "gateway": {
            "enabled": true, "port": "not a port",
            "routes": [
                { "id": "ok", "name": "ok", "library": "x", "upstreamApi": "chat" },
                { "id": "broken", "name": 5 },
            ],
            "breaker": { "enabled": true, "threshold": 3, "cooldownSecs": 18446744073709551615u64 },
        }});
        let (c, bad) = decode(&root);
        assert!(c.enabled);
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.routes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["ok"]);
        assert_eq!(c.breaker.cooldown_secs, 3600, "clamped on load");
        assert_eq!(bad, vec![json!({ "id": "broken", "name": 5 })]);
        let saved = encode(&c, bad).unwrap();
        assert_eq!(saved["routes"].as_array().unwrap().len(), 2);
        assert_eq!(saved["routes"][1]["id"], "broken");
        assert_eq!(saved["enabled"], true);
        assert!(!config_in(&json!({ "gateway": 5 })).enabled);
    }

    /// Mock upstream that answers once with a raw response (head and body as given).
    fn raw_upstream(resp: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in l.incoming().take(1) {
                let Ok(mut s) = c else { continue };
                let _ = read_request(&s);
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        format!("http://{addr}/v1")
    }

    fn one_route(id: &str, api: &str, up: String) {
        let route = Route { id: id.into(), name: id.into(), library: "x".into(), upstream_api: api.into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route, up, None)];
        breaker::reset(Some(id));
    }

    /// An upstream that breaks off mid-body: the client must not get a clean end of stream.
    #[test]
    fn broken_passthrough_is_not_a_clean_end() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        one_route("cut", "chat", raw_upstream("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\nConnection: close\r\n\r\ndata: {\"choices\":[]}\n\n"));
        let port = gateway_n(1);
        let r = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/cut/v1/chat/completions"))
            .bearer_auth(test_key())
            .body(r#"{"model":"m","stream":true,"messages":[]}"#)
            .send()
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        assert!(r.text().is_err(), "the body is cut off, not terminated");
        for _ in 0..50 {
            if LOG.lock().unwrap().iter().any(|l| l.route == "cut") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(LOG.lock().unwrap().iter().any(|l| l.route == "cut" && l.upstream_broken && l.error.as_deref().is_some_and(|e| e.contains("上游中断"))));
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// An Anthropic stream that ends without message_stop becomes an error for the client.
    #[test]
    fn truncated_converted_stream_reports_error() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"c\",\"usage\":{\"input_tokens\":1}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hal\"}}\n\n";
        let resp: &'static str = Box::leak(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_boxed_str());
        one_route("trunc", "anthropic", raw_upstream(resp));
        let port = gateway_n(1);
        let text = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/trunc/v1/chat/completions"))
            .bearer_auth(test_key())
            .body(r#"{"model":"m","stream":true,"messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .unwrap()
            .text()
            .unwrap();
        assert!(text.contains("\"hal\""), "{text}");
        assert!(text.contains("上游的流没有正常结束就断开了"), "{text}");
        assert!(!text.contains("\"finish_reason\":\"stop\""), "{text}");
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// SSE served as application/json is still recognised and converted.
    #[test]
    fn sniffs_sse_without_content_type() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let body = "\ndata: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi there\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let resp: &'static str = Box::leak(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_boxed_str());
        one_route("sniff", "chat", raw_upstream(resp));
        let port = gateway_n(1);
        let text = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/sniff/v1/messages"))
            .header("x-api-key", test_key())
            .body(r#"{"model":"m","stream":true,"max_tokens":9,"messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .unwrap()
            .text()
            .unwrap();
        assert!(text.contains("text_delta") && text.contains("hi there") && text.contains("message_stop"), "{text}");

        let mut r: &[u8] = b"  \n\nevent: x\n";
        let (sse, head) = sniff_sse(&mut r).unwrap();
        assert!(sse && head == b"  \n\nevent: x\n");
        let mut r: &[u8] = b"{\"data\":1}";
        assert!(!sniff_sse(&mut r).unwrap().0);
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// An upstream that streams although the client asked for one piece (stream=false): the
    /// stream is collected into the JSON answer, converted or passed through.
    #[test]
    fn stream_to_a_non_stream_request_is_collected() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let body = concat!(
            "event: response.created
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-x\",\"output\":[]}}

",
            "event: response.output_item.added
data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[]}}

",
            "event: response.output_text.delta
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"po\"}

",
            "event: response.output_text.delta
data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"ng\"}

",
            "event: response.completed
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-x\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":12,\"output_tokens\":2,\"total_tokens\":14}}}

",
        );
        // Labelled application/json, like some relays do.
        let resp = |ct: &str| -> &'static str { Box::leak(format!("HTTP/1.1 200 OK
Content-Type: {ct}
Content-Length: {}
Connection: close

{body}", body.len()).into_boxed_str()) };
        let call = |path: &str, req: &str| {
            let port = gateway_n(1);
            let r = reqwest::blocking::Client::new().post(format!("http://127.0.0.1:{port}/{path}")).bearer_auth(test_key()).body(req.to_string()).send().unwrap();
            { let st = r.status().as_u16(); let text = r.text().unwrap(); (st, serde_json::from_str::<Value>(&text).unwrap_or_else(|_| panic!("{text}"))) }
        };

        one_route("sse1", "responses", raw_upstream(resp("application/json")));
        let (st, v) = call("sse1/v1/chat/completions", r#"{"model":"m","stream":false,"messages":[{"role":"user","content":"hi"}]}"#);
        assert_eq!(st, 200, "{v}");
        assert_eq!(v["object"], "chat.completion", "{v}");
        assert_eq!(v["choices"][0]["message"]["content"], "pong", "{v}");
        assert_eq!(convert::usage_tokens(&v), Some((12, 2)), "{v}");

        one_route("sse2", "responses", raw_upstream(resp("text/event-stream")));
        let (st, v) = call("sse2/v1/responses", r#"{"model":"m","stream":false,"input":"hi"}"#);
        assert_eq!(st, 200, "{v}");
        let text: String = v["output"].as_array().unwrap().iter().flat_map(|i| i["content"].as_array().cloned().unwrap_or_default()).filter_map(|c| c["text"].as_str().map(String::from)).collect();
        assert_eq!(text, "pong", "{v}");

        // An error in the stream is an error for the client, not an empty answer.
        let failed = "event: response.failed
data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"model overloaded\"}}}

";
        let resp: &'static str = Box::leak(format!("HTTP/1.1 200 OK
Content-Type: text/event-stream
Content-Length: {}
Connection: close

{failed}", failed.len()).into_boxed_str());
        one_route("sse3", "responses", raw_upstream(resp));
        let (st, v) = call("sse3/v1/chat/completions", r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#);
        assert!(st >= 500 && v.to_string().contains("model overloaded"), "{st} {v}");
        TEST_ROUTES.lock().unwrap().clear();
    }

    #[test]
    fn chunks_fold_into_one_answer() {
        let c = |delta: Value, finish: Value| json!({ "id": "c1", "created": 5, "model": "m", "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
        let chunks = vec![
            c(json!({ "role": "assistant" }), Value::Null),
            c(json!({ "reasoning_content": "think" }), Value::Null),
            c(json!({ "content": "a" }), Value::Null),
            c(json!({ "tool_calls": [{ "index": 0, "id": "t1", "type": "function", "function": { "name": "run", "arguments": "{\"x\"" } }] }), Value::Null),
            c(json!({ "tool_calls": [{ "index": 0, "function": { "arguments": ":1}" } }] }), Value::Null),
            c(json!({}), json!("tool_calls")),
            json!({ "id": "c1", "choices": [], "usage": { "prompt_tokens": 3, "completion_tokens": 4 } }),
        ];
        let v = chat_from_chunks(&chunks).unwrap();
        // Round trip with chat_as_chunks.
        assert_eq!(chat_from_chunks(&chat_as_chunks(&v)).unwrap()["choices"], v["choices"]);
        let m = &v["choices"][0]["message"];
        assert_eq!((m["content"].as_str(), m["reasoning_content"].as_str()), (Some("a"), Some("think")));
        assert_eq!(m["tool_calls"][0]["id"], "t1");
        assert_eq!(m["tool_calls"][0]["function"]["arguments"], "{\"x\":1}");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!((v["id"].as_str(), v["model"].as_str()), (Some("c1"), Some("m")));
        assert_eq!(convert::usage_tokens(&v), Some((3, 4)));
        let err = chat_from_chunks(&[json!({ "choices": [], "error": { "message": "boom" } })]).unwrap_err();
        assert_eq!(err, "boom");
    }

    /// A redirect from the upstream is passed back, never followed with the upstream key.
    #[test]
    fn upstream_redirects_are_not_followed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hits = Arc::new(AtomicU64::new(0));
        let elsewhere = fixed_upstream(200, "{}", hits.clone());
        let resp: &'static str = Box::leak(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {elsewhere}/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_boxed_str());
        let route = Route { id: "redir".into(), name: "redir".into(), library: "x".into(), upstream_api: "chat".into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route, raw_upstream(resp), Some("up-secret".into()))];
        let port = gateway_n(1);
        let r = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/redir/v1/chat/completions"))
            .bearer_auth(test_key())
            .body(r#"{"model":"m","messages":[]}"#)
            .send()
            .unwrap();
        assert_eq!(r.status().as_u16(), 307);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// Bad JSON from the client: 400, no upstream call, nothing counted against the forward.
    #[test]
    fn client_mistakes_are_400() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hits = Arc::new(AtomicU64::new(0));
        let up = fixed_upstream(200, "{}", hits.clone());
        let route = Route { id: "c400".into(), name: "c400".into(), library: "x".into(), upstream_api: "anthropic".into(), model_map: vec![], enabled: true, weight: 100, replaced: vec![] };
        *TEST_ROUTES.lock().unwrap() = vec![(route, up, None)];
        breaker::reset(Some("c400"));
        let port = gateway_n(2);
        let client = reqwest::blocking::Client::new();
        for body in ["{not json", "[1,2]"] {
            let r = client.post(format!("http://127.0.0.1:{port}/c400/v1/responses")).bearer_auth(test_key()).body(body).send().unwrap();
            assert_eq!(r.status().as_u16(), 400, "{body}");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(breaker::view(&breaker::Config::default(), "c400").is_none());
        TEST_ROUTES.lock().unwrap().clear();
    }

    /// /health says the gateway is up to anyone, but lists forwards only to key holders.
    #[test]
    fn health_lists_routes_only_with_key() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        one_route("hidden", "chat", "http://127.0.0.1:9/v1".into());
        let port = gateway_n(2);
        let client = reqwest::blocking::Client::new();
        let v: Value = client.get(format!("http://127.0.0.1:{port}/health")).send().unwrap().json_body();
        assert_eq!(v["ok"], true);
        assert!(v.get("routes").is_none(), "{v}");
        let v: Value = client.get(format!("http://127.0.0.1:{port}/health")).bearer_auth(test_key()).send().unwrap().json_body();
        assert_eq!(v["routes"], json!(["hidden"]));
        TEST_ROUTES.lock().unwrap().clear();
    }

    trait JsonBody {
        fn json_body(self) -> Value;
    }

    impl JsonBody for reqwest::blocking::Response {
        fn json_body(self) -> Value {
            serde_json::from_str(&self.text().unwrap()).unwrap()
        }
    }

    #[test]
    fn replays_array_content_and_reasoning() {
        let chat = json!({ "id": "c", "model": "m", "choices": [{ "message": { "role": "assistant",
            "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }], "reasoning": "hmm" }, "finish_reason": "stop" }] });
        let chunks = chat_as_chunks(&chat);
        let text: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        let reasoning: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["reasoning_content"].as_str()).collect();
        assert_eq!((text.as_str(), reasoning.as_str()), ("a\nb", "hmm"));
    }
}
