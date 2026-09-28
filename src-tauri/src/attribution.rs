//! Model attribution test. The frontend (src/attribution) builds the challenges and scores
//! the answers; this module decides which saved endpoint a model row is tested against and
//! sends one challenge to it, reading back the whole final answer. The key stays here.
//!
//! Requests carry only the challenge as one user message: no system prompt, no history, no
//! sampling parameters, reasoning left at the provider's default (the fingerprint bank was
//! enrolled that way). The only additions are an output budget large enough for the long
//! answer and, for a Chat endpoint that rejects it, the budget under its other names.

use crate::adapters::{self, codex};
use crate::gateway::{self, convert, convert::Proto, server::Route};
use crate::i18n::l;
use crate::util::clip;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::time::{Duration, Instant};

/// The id the model table uses for Codex's shared catalog (`CATALOG` in draft.ts): not a
/// provider; the test goes to the provider Codex is on.
pub const CATALOG: &str = "*";
/// A 332-number answer is about a thousand tokens; the rest is room for models that reason first.
pub const MAX_OUTPUT_TOKENS: u32 = 16_384;
/// Wait for the response head (a non-streamed answer arrives whole), then for each read.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Answers are a few KB; streamed replies carrying reasoning are larger.
const MAX_BODY: u64 = 8 << 20;
/// Challenge prompts are about 300 characters.
const MAX_PROMPT: usize = 4000;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    /// Stable: "noEndpoint" | "signIn" | "protocol" | "gatewayOff" | "gatewayRoute" | "gatewayPool".
    pub code: &'static str,
    pub message: String,
}

/// How a request reaches the model through the local gateway.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayPath {
    /// The provider's address: "route" (one forward), "unified" or "combined" (several).
    pub entry: &'static str,
    pub route: String,
    pub route_name: String,
    pub upstream_name: String,
    pub upstream_url: String,
    pub upstream_api: String,
    /// What the forward's model map sends upstream instead of the requested model.
    pub mapped_model: Option<String>,
    /// A unified or combined entry, sent straight to its one forward for this model.
    pub pinned: bool,
    /// Forwards the entry could fail over to, left out of the pinned test.
    pub skipped: Vec<String>,
}

/// What a model row is tested against.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub agent: String,
    /// The provider tested: for Codex's shared catalog, the one Codex is on.
    pub provider: String,
    /// The config entry read instead, when it isn't `provider` (Codex's fixed-id mirror).
    pub entry: Option<String>,
    /// The model id sent: always the row's own id.
    pub model: String,
    pub api: String,
    /// Endpoint called (no key).
    pub url: String,
    pub has_key: bool,
    pub gateway: Option<GatewayPath>,
    /// Why nothing can be sent; the fields above are filled as far as they are known.
    pub blocked: Option<Blocked>,
    /// Endpoint, protocol, key and model in one hash: every sample of a run must hit the same.
    pub fp: String,
}

struct Resolved {
    target: Target,
    key: Option<String>,
}

fn blocked(code: &'static str, message: impl Into<String>) -> Option<Blocked> {
    Some(Blocked { code, message: message.into() })
}

fn fingerprint(url: &str, api: &str, key: Option<&str>, model: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (url, api, key, model).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Resolves the endpoint for `model` of `provider` (or of Codex's catalog) from the saved
/// config: never a pending draft.
fn resolve(agent: &str, provider: &str, model: &str) -> anyhow::Result<Resolved> {
    if model.trim().is_empty() {
        anyhow::bail!("{}", l("Model ID is required", "模型 ID 不能为空"));
    }
    let mut t = Target { agent: agent.into(), provider: provider.into(), model: model.into(), ..Default::default() };
    let catalog = provider == CATALOG;
    let entry = if catalog {
        if adapters::base_agent(agent) != codex::ID {
            t.blocked = blocked("noEndpoint", l("This model list belongs to no provider", "这个模型列表不属于任何供应商"));
            return Ok(Resolved { target: t, key: None });
        }
        match codex::active_provider() {
            Ok((entry, shown)) => {
                t.provider = shown;
                entry
            }
            Err(e) => {
                t.blocked = blocked("noEndpoint", e.to_string());
                return Ok(Resolved { target: t, key: None });
            }
        }
    } else {
        provider.to_string()
    };
    if entry != t.provider {
        t.entry = Some(entry.clone());
    }
    let (base, key, api) = match adapters::provider_endpoint(agent, &entry) {
        Ok(e) => e,
        Err(e) => {
            t.blocked = if catalog && entry == "openai" {
                blocked("signIn", l(
                    "Codex is on its built-in OpenAI provider, which uses the ChatGPT sign-in. Only providers with a base URL and API key can be tested.",
                    "Codex 正在使用内置的 OpenAI 供应商（ChatGPT 登录），只能测试有地址和密钥的供应商。",
                ))
            } else {
                blocked("noEndpoint", e.to_string())
            };
            return Ok(Resolved { target: t, key: None });
        }
    };
    t.api = api.clone();
    t.has_key = key.is_some();
    let Some(proto) = Proto::from_api(&api) else {
        t.url = base;
        t.blocked = blocked("protocol", tr!(
            "Attribution tests speak Chat Completions, Responses and Anthropic Messages; this provider uses {api}",
            "归因测试支持 Chat Completions、Responses 和 Anthropic Messages，这个供应商是 {api}"
        ));
        return Ok(Resolved { target: t, key });
    };
    t.url = format!("{}{}", base.trim_end_matches('/'), proto.path());
    let cfg = gateway::server::load_config();
    let mut ports = vec![cfg.port];
    ports.extend(&cfg.former_ports);
    let root = crate::store::load();
    let view = GatewayView {
        ports,
        running: gateway::server::running_port(),
        routes: &cfg.routes,
        serving: &gateway::server::serving_forwards,
        upstream: &|r: &Route| crate::library::endpoint_in(&root, &r.library).ok().map(|e| (e.name, e.base_url)),
    };
    match through_gateway(&t.url, model, &view) {
        None => {}
        Some(Ok((url, path))) => {
            t.url = url;
            t.gateway = Some(path);
        }
        Some(Err((path, why))) => {
            t.gateway = path;
            t.blocked = Some(why);
        }
    }
    t.fp = fingerprint(&t.url, &t.api, key.as_deref(), model);
    Ok(Resolved { target: t, key })
}

pub fn target(agent: &str, provider: &str, model: &str) -> anyhow::Result<Target> {
    Ok(resolve(agent, provider, model)?.target)
}

/// `server::serving_forwards`: (forwards that serve a model, forwards that may).
type Serving<'a> = &'a dyn Fn(Option<&[String]>, &str) -> (Vec<Route>, Vec<Route>);

/// What `through_gateway` needs to know about the local gateway.
struct GatewayView<'a> {
    /// Every port the gateway uses or used (its address may still be at an old one).
    ports: Vec<u16>,
    running: Option<u16>,
    routes: &'a [Route],
    serving: Serving<'a>,
    /// A forward's library entry: (name, base URL).
    upstream: &'a dyn Fn(&Route) -> Option<(String, String)>,
}

type GatewayOutcome = Result<(String, GatewayPath), (Option<GatewayPath>, Blocked)>;

/// For a URL on the local gateway: the URL to send to and the path through it, or why it
/// can't be tested. A unified or combined entry spreads a model over its forwards (and
/// fails over between them), which would mix answers from different upstreams in one
/// result: it is pinned to the one forward that serves the model, or refused.
fn through_gateway(url: &str, model: &str, gw: &GatewayView) -> Option<GatewayOutcome> {
    let mut u = url::Url::parse(url).ok()?;
    let local = matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let port = u.port_or_known_default()?;
    if !local || !gw.ports.contains(&port) {
        return None;
    }
    if gw.running != Some(port) {
        let why = match gw.running {
            None => l("The local gateway isn't running; start it first", "本地网关没有运行，请先启动"),
            Some(_) => l("This address is on a port the local gateway no longer uses", "这个地址所在的端口本地网关已不再使用"),
        };
        return Some(Err((None, Blocked { code: "gatewayOff", message: why.into() })));
    }
    let Some((entry_id, rest)) = gateway::server::split_path(u.path()) else {
        return Some(Err((None, Blocked { code: "gatewayRoute", message: l("Not a gateway address", "不是网关地址").into() })));
    };
    let describe = |r: &Route, entry: &'static str| -> Result<GatewayPath, Blocked> {
        let mut path = GatewayPath { entry, route: r.id.clone(), route_name: r.name.clone(), upstream_api: r.upstream_api.clone(), ..Default::default() };
        if !r.enabled {
            return Err(Blocked { code: "gatewayRoute", message: tr!("Forward \"{}\" is paused", "转发「{}」已暂停", r.name) });
        }
        let Some((name, base)) = (gw.upstream)(r) else {
            return Err(Blocked { code: "gatewayRoute", message: tr!("Forward \"{}\" has no upstream in the provider library", "转发「{}」在供应商库里没有上游", r.name) });
        };
        path.upstream_name = name;
        path.upstream_url = base;
        path.mapped_model = gateway::server::mapped_model(&r.model_map, model);
        Ok(path)
    };
    if entry_id != "v1" && !entry_id.contains('+') {
        let Some(r) = gw.routes.iter().find(|r| r.id == entry_id) else {
            return Some(Err((None, Blocked { code: "gatewayRoute", message: tr!("The local gateway has no forward \"{entry_id}\"", "本地网关没有转发「{entry_id}」") })));
        };
        return Some(describe(r, "route").map(|p| (url.to_string(), p)).map_err(|b| (None, b)));
    }
    let (entry, only): (&'static str, Option<Vec<String>>) = if entry_id == "v1" {
        ("unified", None)
    } else {
        ("combined", Some(entry_id.split('+').filter(|x| !x.is_empty()).map(String::from).collect()))
    };
    let (sure, maybe) = (gw.serving)(only.as_deref(), model);
    let ids = |rs: &[Route]| rs.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    let (pick, skipped) = match (sure.as_slice(), maybe.as_slice()) {
        ([one], rest) => (one, ids(rest)),
        ([], [one]) => (one, vec![]),
        ([], []) => {
            let why = tr!("No forward of this gateway address serves model \"{model}\"", "这个网关地址没有哪个转发提供模型「{model}」");
            return Some(Err((None, Blocked { code: "gatewayPool", message: why })));
        }
        _ => {
            let all = [ids(&sure), ids(&maybe)].concat();
            let why = tr!(
                "This gateway address spreads model \"{model}\" over several forwards ({}), so the answers could come from different upstreams. Point a provider at one forward's own address to test it.",
                "这个网关地址会把模型「{model}」分给多个转发（{}），样本可能来自不同上游。请让供应商指向单个转发的地址后再测。",
                crate::i18n::join(&all)
            );
            return Some(Err((None, Blocked { code: "gatewayPool", message: why })));
        }
    };
    match describe(pick, entry) {
        Ok(mut path) => {
            path.pinned = true;
            path.skipped = skipped;
            u.set_path(&format!("/{}/v1{rest}", pick.id));
            Some(Ok((u.to_string(), path)))
        }
        Err(b) => Some(Err((None, b))),
    }
}

// ---------------------------------------------------------------- one sample

/// How one challenge went. `kind` is stable for the UI; `error` is display text.
#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SampleResult {
    pub ok: bool,
    /// The whole final answer, reasoning left out; only when ok.
    pub text: Option<String>,
    /// "http" | "timeout" | "connect" | "network" | "redirect" | "tooLarge" | "invalidResponse" |
    /// "streamError" | "upstreamError" | "truncated" | "refused" | "empty" | "blocked" | "targetChanged"
    pub kind: Option<&'static str>,
    pub error: Option<String>,
    pub status: Option<u16>,
    /// HTTP requests sent for this sample (a Chat endpoint rejecting the budget field adds some).
    pub requests: u32,
    pub ms: u64,
    /// Tokens reported by the server (input, output).
    pub usage: Option<(u64, u64)>,
}

impl SampleResult {
    fn fail(mut self, kind: &'static str, error: String) -> Self {
        self.ok = false;
        self.kind = Some(kind);
        self.error = Some(error);
        self
    }
}

/// What a sample reports while it runs, for the live log.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Live {
    /// The `n`-th request of this sample goes out (more than one: a Chat budget fallback).
    Request { n: u32 },
    /// The response head arrived.
    Status { status: u16 },
    /// Answer text as it streams in (a non-streamed answer comes in one piece).
    Text { text: String },
    /// Reasoning the model streamed; shown, never scored.
    Reasoning { text: String },
}

/// Sends one challenge to the model row's endpoint, checking first that it is still the
/// endpoint the run started with (`fp` from `target`).
pub fn sample(agent: &str, provider: &str, model: &str, prompt: &str, fp: &str, on: &dyn Fn(Live)) -> anyhow::Result<SampleResult> {
    if prompt.trim().is_empty() || prompt.chars().count() > MAX_PROMPT {
        anyhow::bail!("{}", l("Invalid challenge", "挑战内容无效"));
    }
    let r = resolve(agent, provider, model)?;
    let t = &r.target;
    if let Some(b) = &t.blocked {
        return Ok(SampleResult::default().fail("blocked", b.message.clone()));
    }
    if t.fp != fp {
        let why = l("The provider's saved address, key or protocol changed during the test; start it again", "测试期间供应商已保存的地址、密钥或协议变了，请重新开始");
        return Ok(SampleResult::default().fail("targetChanged", why.into()));
    }
    let proto = Proto::from_api(&t.api).expect("resolve blocks other protocols");
    Ok(send(&t.url, proto, r.key.as_deref(), model, prompt, on))
}

/// Request bodies, tried in order: only a Chat endpoint that rejects the budget field (HTTP
/// 400 / 422) gets the next one, so a sample costs at most three requests. Answers are
/// streamed so the log can show them as they come; the final text is read from the whole
/// stream afterwards, the same way as from a non-streamed answer.
fn request_bodies(proto: Proto, model: &str, prompt: &str) -> Vec<Value> {
    let messages = json!([{ "role": "user", "content": prompt }]);
    match proto {
        Proto::Anthropic => vec![json!({ "model": model, "max_tokens": MAX_OUTPUT_TOKENS, "messages": messages, "stream": true })],
        // `max_tokens` is the widely accepted name; OpenAI's reasoning models want `max_completion_tokens`.
        Proto::Chat => vec![
            json!({ "model": model, "messages": messages, "stream": true, "max_tokens": MAX_OUTPUT_TOKENS }),
            json!({ "model": model, "messages": messages, "stream": true, "max_completion_tokens": MAX_OUTPUT_TOKENS }),
            json!({ "model": model, "messages": messages, "stream": true }),
        ],
        Proto::Responses => vec![json!({ "model": model, "input": prompt, "stream": true, "max_output_tokens": MAX_OUTPUT_TOKENS })],
    }
}

/// Masks the key wherever an upstream message repeats it.
fn redact(text: &str, key: Option<&str>) -> String {
    match key.map(str::trim).filter(|k| k.len() >= 4) {
        Some(k) => text.replace(k, &crate::model::mask_key(k)),
        None => text.to_string(),
    }
}

/// Passes streamed text and reasoning to the log, a few times a second at most.
struct LiveOut<'a> {
    on: &'a dyn Fn(Live),
    text: String,
    reasoning: String,
    last: Instant,
}

impl<'a> LiveOut<'a> {
    const EVERY: Duration = Duration::from_millis(80);

    fn new(on: &'a dyn Fn(Live)) -> Self {
        LiveOut { on, text: String::new(), reasoning: String::new(), last: Instant::now() }
    }

    fn push(&mut self, chunks: &[Value]) {
        for c in chunks {
            let d = &c["choices"][0]["delta"];
            if let Some(r) = d.get("reasoning_content").or_else(|| d.get("reasoning")).and_then(Value::as_str) {
                self.reasoning.push_str(r);
            }
            if let Some(t) = d.get("content").and_then(Value::as_str) {
                self.text.push_str(t);
            }
        }
        if self.last.elapsed() >= Self::EVERY {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.reasoning.is_empty() {
            (self.on)(Live::Reasoning { text: std::mem::take(&mut self.reasoning) });
        }
        if !self.text.is_empty() {
            (self.on)(Live::Text { text: std::mem::take(&mut self.text) });
        }
        self.last = Instant::now();
    }
}

/// Whether a body starts like an event stream.
fn looks_sse(head: &str) -> bool {
    let t = head.trim_start();
    t.starts_with("data:") || t.starts_with("event:") || t.starts_with(':')
}

/// Reads the body (up to `MAX_BODY`), feeding an event stream to the log as it arrives.
/// Returns the whole body and whether it was an event stream.
fn read_body(mut resp: reqwest::blocking::Response, proto: Proto, timeout: Duration, live: &mut LiveOut) -> Result<(String, bool), Rejected> {
    let declared = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("event-stream"));
    let mut sse = declared.then_some(true);
    let mut raw: Vec<u8> = Vec::new();
    let mut fed = 0;
    let mut stream = convert::UpstreamStream::new(proto);
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                live.flush();
                let timed_out = e.kind() == std::io::ErrorKind::TimedOut || e.to_string().contains("timed out");
                return Err(if timed_out {
                    ("timeout", tr!("Timed out reading the answer ({} s)", "读取回答超时（{} 秒）", timeout.as_secs()))
                } else {
                    ("network", tr!("Reading the answer failed: {e}", "读取回答失败：{e}"))
                });
            }
        };
        raw.extend_from_slice(&buf[..n]);
        if raw.len() as u64 > MAX_BODY {
            live.flush();
            return Err(("tooLarge", tr!("Response too large (over {} MB)", "响应太大（超过 {} MB）", MAX_BODY >> 20)));
        }
        if sse.is_none() {
            let head = String::from_utf8_lossy(&raw[..raw.len().min(64)]).into_owned();
            if !head.trim().is_empty() {
                sse = Some(looks_sse(&head));
            }
        }
        if sse == Some(true) {
            // Whole UTF-8 characters only; a split one waits for the next read.
            let valid = match std::str::from_utf8(&raw[fed..]) {
                Ok(s) => s.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid > 0 {
                let chunks = stream.feed(std::str::from_utf8(&raw[fed..fed + valid]).expect("checked above"));
                live.push(&chunks);
                fed += valid;
            }
        }
    }
    let sse = sse == Some(true);
    if sse {
        live.push(&stream.finish());
    }
    live.flush();
    Ok((String::from_utf8_lossy(&raw).into_owned(), sse))
}

pub fn send(url: &str, proto: Proto, key: Option<&str>, model: &str, prompt: &str, on: &dyn Fn(Live)) -> SampleResult {
    send_within(url, proto, key, model, prompt, TIMEOUT, on)
}

fn send_within(url: &str, proto: Proto, key: Option<&str>, model: &str, prompt: &str, timeout: Duration, on: &dyn Fn(Live)) -> SampleResult {
    let mut r = SampleResult::default();
    let client = match crate::net::client_with(timeout) {
        Ok(c) => c,
        Err(e) => return r.fail("network", e),
    };
    // OpenCode's gateway wants a conversation id; each sample is a conversation of its own.
    let session = gateway::session::wants_session(url).then(|| format!("agp-attr-{:x}", chrono::Utc::now().timestamp_micros()));
    let mut bodies = request_bodies(proto, model, prompt).into_iter().peekable();
    let t0 = Instant::now();
    let (status, text, sse) = loop {
        let body = bodies.next().expect("request_bodies is never empty");
        let mut req = crate::net::with_api_key(client.post(url).header("content-type", "application/json").body(body.to_string()), proto.api(), key);
        if let Some(s) = &session {
            req = req.header(gateway::session::HEADER, s).header("user-agent", concat!("AgentPlus/", env!("CARGO_PKG_VERSION")));
        }
        r.requests += 1;
        on(Live::Request { n: r.requests });
        let resp = match req.send() {
            Ok(x) => x,
            Err(e) => {
                r.ms = t0.elapsed().as_millis() as u64;
                return if e.is_timeout() {
                    r.fail("timeout", tr!("Request timed out ({} s)", "请求超时（{} 秒）", timeout.as_secs()))
                } else if e.is_connect() {
                    r.fail("connect", l("Connection failed: URL unreachable", "连接失败：地址不可达").into())
                } else {
                    r.fail("network", redact(&tr!("Request failed: {e}", "请求失败：{e}"), key))
                };
            }
        };
        let status = resp.status();
        r.status = Some(status.as_u16());
        on(Live::Status { status: status.as_u16() });
        if matches!(status.as_u16(), 400 | 422) && bodies.peek().is_some() {
            continue;
        }
        if let Some(to) = crate::net::moved_to(&resp) {
            r.ms = t0.elapsed().as_millis() as u64;
            return r.fail("redirect", to);
        }
        // An error body isn't an answer: it goes to the error message, not the log.
        let quiet = |_: Live| {};
        let read = if status.is_success() { read_body(resp, proto, timeout, &mut LiveOut::new(on)) } else { read_body(resp, proto, timeout, &mut LiveOut::new(&quiet)) };
        r.ms = t0.elapsed().as_millis() as u64;
        match read {
            Ok((text, sse)) => break (status, text, sse),
            Err((kind, msg)) => return r.fail(kind, msg),
        }
    };
    if !status.is_success() {
        return r.fail("http", redact(&crate::net::http_error(status, &text), key));
    }
    let chat = match to_chat(proto, &text) {
        Ok(c) => c,
        Err((kind, msg)) => return r.fail(kind, redact(&msg, key)),
    };
    // A non-streamed answer shows up in the log in one piece.
    if !sse {
        let (answer, reasoning) = convert::message_text(&chat["choices"][0]["message"]);
        if !reasoning.is_empty() {
            on(Live::Reasoning { text: reasoning });
        }
        if !answer.is_empty() {
            on(Live::Text { text: answer });
        }
    }
    match judge(&chat) {
        Ok((answer, usage)) => {
            r.ok = true;
            r.text = Some(answer);
            r.usage = usage;
            r
        }
        Err((kind, msg)) => r.fail(kind, redact(&msg, key)),
    }
}

/// Why an answer doesn't count: (stable kind, display text).
type Rejected = (&'static str, String);

/// A `<think>…</think>` block some relays put before the answer (the model's reasoning inline).
fn strip_think(text: &str) -> &str {
    match text.trim_start().strip_prefix("<think>") {
        Some(rest) => rest.find("</think>").map_or("", |i| &rest[i + "</think>".len()..]),
        None => text,
    }
}

/// An error an upstream reported inside a successful HTTP answer.
fn reported_error(proto: Proto, v: &Value) -> Option<String> {
    let has_answer = v.get("choices").is_some() || v.get("content").is_some() || v.get("output").is_some() || v.get("output_text").is_some();
    if v.get("error").is_some_and(|e| !e.is_null()) && !has_answer {
        return Some(convert::error_message(v).unwrap_or_else(|| clip(&v.to_string(), 200)));
    }
    if proto == Proto::Responses {
        let status = v.get("status").and_then(Value::as_str).unwrap_or_default();
        if matches!(status, "failed" | "cancelled") {
            return Some(v.pointer("/error/message").and_then(Value::as_str).map(String::from).unwrap_or_else(|| status.to_string()));
        }
    }
    None
}

/// A JSON or SSE body as one chat completion.
fn to_chat(proto: Proto, text: &str) -> Result<Value, Rejected> {
    match serde_json::from_str::<Value>(text) {
        Ok(v) if v.is_object() => {
            if let Some(msg) = reported_error(proto, &v) {
                return Err(("upstreamError", tr!("The upstream reported an error: {}", "上游返回了错误：{}", clip(msg.trim(), 300))));
            }
            convert::response_to_chat(proto, &v).map_err(|e| ("invalidResponse", tr!("Can't read the answer: {e:#}", "无法解析回答：{e:#}")))
        }
        Ok(_) => Err(("invalidResponse", l("The answer is not a JSON object", "回答不是 JSON 对象").into())),
        Err(_) if text.lines().any(|l| l.trim_start().starts_with("data:")) => {
            convert::collect_stream(proto, text).map_err(|e| ("streamError", tr!("The stream ended with an error: {}", "流式响应出错：{}", clip(e.trim(), 300))))
        }
        Err(_) => Err(("invalidResponse", tr!("Response is not JSON: {}", "返回的不是 JSON：{}", clip(text.trim(), 120)))),
    }
}

/// The final answer of a chat completion (reasoning, thinking blocks and tool calls left
/// out) and the reported usage; or why it doesn't count.
fn judge(chat: &Value) -> Result<(String, Option<(u64, u64)>), Rejected> {
    let choice = &chat["choices"][0];
    let (answer, _reasoning) = convert::message_text(&choice["message"]);
    let usage = convert::usage_tokens(chat);
    match choice["finish_reason"].as_str().unwrap_or_default() {
        "length" => return Err(("truncated", l("The answer was cut off at the output limit and doesn't count", "回答在输出上限处被截断，不计入").into())),
        "content_filter" => return Err(("refused", l("The model refused or its answer was filtered", "模型拒答或回答被过滤").into())),
        _ => {}
    }
    let refusal = choice["message"]["refusal"].as_str().filter(|s| !s.trim().is_empty());
    let answer = strip_think(&answer).trim();
    if answer.is_empty() {
        return Err(match refusal {
            Some(_) => ("refused", l("The model refused or its answer was filtered", "模型拒答或回答被过滤").into()),
            None => ("empty", l("The model returned no answer text", "模型没有返回回答文本").into()),
        });
    }
    Ok((answer.to_string(), usage))
}

/// `to_chat` then `judge`.
#[cfg(test)]
fn final_text(proto: Proto, text: &str) -> Result<(String, Option<(u64, u64)>), Rejected> {
    judge(&to_chat(proto, text)?)
}

// ---------------------------------------------------------------- history

/// Results kept, newest first.
const HISTORY_MAX: usize = 200;
/// An entry is a summary (answers aren't kept); anything far bigger isn't one.
const HISTORY_ENTRY_MAX: usize = 64 * 1024;
static HISTORY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn history_path() -> std::path::PathBuf {
    crate::util::agentplus_dir().join("attribution-history.json")
}

/// Past results, newest first. The frontend makes the entries (it has the scores); a file
/// that can't be read counts as no history.
pub fn history() -> Vec<Value> {
    std::fs::read_to_string(history_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
        .map(|v| v.into_iter().filter(Value::is_object).collect())
        .unwrap_or_default()
}

fn save_history(list: &[Value]) -> anyhow::Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::util::write_private_atomic(&path, serde_json::to_string(list)?.as_bytes())
}

/// Adds a result, stamped with an id and the time; returns the whole list.
pub fn history_add(mut entry: Value) -> anyhow::Result<Vec<Value>> {
    let invalid = || anyhow::anyhow!(l("Invalid test result", "测试结果无效"));
    let o = entry.as_object_mut().ok_or_else(invalid)?;
    let now = chrono::Utc::now();
    o.insert("id".into(), json!(format!("{:x}", now.timestamp_micros())));
    o.insert("at".into(), json!(now.timestamp_millis()));
    if entry.to_string().len() > HISTORY_ENTRY_MAX {
        return Err(invalid());
    }
    let _g = crate::util::lock(&HISTORY_LOCK);
    let mut list = history();
    list.insert(0, entry);
    list.truncate(HISTORY_MAX);
    save_history(&list)?;
    Ok(list)
}

/// Removes these results, or every one with None; returns what is left.
pub fn history_delete(ids: Option<Vec<String>>) -> anyhow::Result<Vec<Value>> {
    let _g = crate::util::lock(&HISTORY_LOCK);
    let mut list = history();
    match ids {
        Some(ids) => list.retain(|e| !e.get("id").and_then(Value::as_str).is_some_and(|id| ids.iter().any(|x| x == id))),
        None => list.clear(),
    }
    save_history(&list)?;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc::{channel, Receiver};

    const NUMBERS: &str = "12, 45, 300, 7, 88";

    /// Local server answering `n` requests with `reply(request)`; each request is sent on the channel.
    fn serve(n: usize, reply: impl Fn(&str, usize) -> String + Send + 'static) -> (String, Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().take(n).enumerate() {
                let Ok(mut s) = s else { continue };
                let req = read_request(&mut s);
                let _ = s.write_all(reply(&req, i).as_bytes());
                let _ = tx.send(req);
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), rx)
    }

    fn read_request(s: &mut std::net::TcpStream) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 16384];
        loop {
            let n = s.read(&mut buf).unwrap_or(0);
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let want = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                if n == 0 || got.len() >= end + 4 + want {
                    return text;
                }
            } else if n == 0 {
                return text;
            }
        }
    }

    fn http(status: &str, extra: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
    }

    fn body_of(req: &str) -> Value {
        serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[test]
    fn requests_carry_only_the_challenge_and_a_large_budget() {
        let (base, seen) = serve(3, |_, _| http("200 OK", "", r#"{"content":[{"type":"text","text":"1, 2"}],"stop_reason":"end_turn"}"#));
        assert!(send(&format!("{base}/messages"), Proto::Anthropic, Some("sk-ant-secret"), "claude-opus-4-7", "挑战", &|_| {}).ok);
        let req = seen.recv().unwrap();
        assert!(req.starts_with("POST /v1/messages HTTP/1.1"), "{req}");
        assert!(req.contains("x-api-key: sk-ant-secret") && req.contains("anthropic-version: 2023-06-01"), "{req}");
        assert_eq!(body_of(&req), json!({ "model": "claude-opus-4-7", "max_tokens": MAX_OUTPUT_TOKENS, "messages": [{ "role": "user", "content": "挑战" }], "stream": true }));

        let (base, seen) = serve(1, |_, _| http("200 OK", "", r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"1, 2"}]}],"status":"completed"}"#));
        assert!(send(&format!("{base}/responses"), Proto::Responses, Some("sk-x"), "gpt-5.5", "挑战", &|_| {}).ok);
        let req = seen.recv().unwrap();
        assert!(req.contains("authorization: Bearer sk-x"), "{req}");
        let body = body_of(&req);
        assert_eq!(body, json!({ "model": "gpt-5.5", "input": "挑战", "stream": true, "max_output_tokens": MAX_OUTPUT_TOKENS }));
        for field in ["instructions", "temperature", "top_p", "reasoning", "tools", "system"] {
            assert!(body.get(field).is_none(), "{field}");
        }
    }

    #[test]
    fn full_answers_are_read_from_json_and_streams() {
        let long: String = (1..=332).map(|i| (i % 355 + 1).to_string()).collect::<Vec<_>>().join(", ");
        let cases: Vec<(Proto, String)> = vec![
            (Proto::Chat, json!({ "choices": [{ "message": { "content": long, "reasoning_content": "99 98 97" }, "finish_reason": "stop" }], "usage": { "prompt_tokens": 90, "completion_tokens": 900 } }).to_string()),
            (Proto::Anthropic, json!({ "content": [{ "type": "thinking", "thinking": "99 98 97" }, { "type": "text", "text": long }], "stop_reason": "end_turn" }).to_string()),
            (Proto::Responses, json!({ "status": "completed", "output": [{ "type": "reasoning", "summary": [{ "type": "summary_text", "text": "99 98" }] }, { "type": "message", "content": [{ "type": "output_text", "text": long }] }] }).to_string()),
            (Proto::Chat, json!({ "choices": [{ "message": { "content": format!("<think>1 2 3</think>\n{long}") }, "finish_reason": "stop" }] }).to_string()),
        ];
        for (proto, body) in cases {
            let (answer, _) = final_text(proto, &body).unwrap();
            assert_eq!(answer, long, "{proto:?}");
        }
        // Streamed although asked not to: text deltas only, never reasoning deltas.
        let chat_sse = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({ "id": "c", "choices": [{ "index": 0, "delta": { "reasoning_content": "77 " } }] }),
            json!({ "id": "c", "choices": [{ "index": 0, "delta": { "content": "5, 6, " } }] }),
            json!({ "id": "c", "choices": [{ "index": 0, "delta": { "content": "7" }, "finish_reason": "stop" }] }),
        );
        assert_eq!(final_text(Proto::Chat, &chat_sse).unwrap().0, "5, 6, 7");
        let anthropic_sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"42 43\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"8, 9\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        assert_eq!(final_text(Proto::Anthropic, anthropic_sse).unwrap().0, "8, 9");
    }

    #[test]
    fn truncated_refused_empty_and_broken_answers_do_not_count() {
        let kind = |proto: Proto, body: &str| final_text(proto, body).unwrap_err().0;
        assert_eq!(kind(Proto::Chat, r#"{"choices":[{"message":{"content":"1, 2, 3"},"finish_reason":"length"}]}"#), "truncated");
        assert_eq!(kind(Proto::Anthropic, r#"{"content":[{"type":"text","text":"1, 2"}],"stop_reason":"max_tokens"}"#), "truncated");
        assert_eq!(kind(Proto::Responses, r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","content":[{"type":"output_text","text":"1, 2"}]}]}"#), "truncated");
        assert_eq!(kind(Proto::Anthropic, r#"{"content":[{"type":"text","text":"No."}],"stop_reason":"refusal"}"#), "refused");
        assert_eq!(kind(Proto::Chat, r#"{"choices":[{"message":{"content":null,"refusal":"I can't"},"finish_reason":"stop"}]}"#), "refused");
        assert_eq!(kind(Proto::Chat, r#"{"choices":[{"message":{"content":"  ","reasoning_content":"1 2 3"},"finish_reason":"stop"}]}"#), "empty");
        assert_eq!(kind(Proto::Chat, r#"{"choices":[{"message":{"content":"<think>1 2 3"},"finish_reason":"stop"}]}"#), "empty");
        assert_eq!(kind(Proto::Chat, r#"{"error":{"message":"quota exhausted"}}"#), "upstreamError");
        assert_eq!(kind(Proto::Responses, r#"{"status":"failed","error":{"message":"overloaded"},"output":[]}"#), "upstreamError");
        assert_eq!(kind(Proto::Chat, "<html>busy</html>"), "invalidResponse");
        assert_eq!(kind(Proto::Chat, "[1,2]"), "invalidResponse");
        // A stream without its closing event was cut off.
        assert_eq!(kind(Proto::Anthropic, "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"1, 2\"}}\n\n"), "streamError");
    }

    #[test]
    fn http_errors_timeouts_and_connection_failures_are_reported_without_the_key() {
        let (base, _) = serve(1, |_, _| http("401 Unauthorized", "", r#"{"error":{"message":"invalid key sk-live-abcdef123456"}}"#));
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, Some("sk-live-abcdef123456"), "gpt-5.5", "x", &|_| {});
        assert_eq!((r.ok, r.kind, r.status, r.requests), (false, Some("http"), Some(401), 1));
        let e = r.error.unwrap();
        assert!(e.contains("密钥无效") && e.contains("••••3456") && !e.contains("abcdef123456"), "{e}");

        let (base, _) = serve(1, |_, _| http("429 Too Many", "", r#"{"error":{"message":"slow down"}}"#));
        let r = send(&format!("{base}/messages"), Proto::Anthropic, None, "m", "x", &|_| {});
        assert_eq!((r.kind, r.requests), (Some("http"), 1), "no retry for rate limits");

        let r = send("http://127.0.0.1:9/v1/messages", Proto::Anthropic, None, "m", "x", &|_| {});
        assert_eq!(r.kind, Some("connect"));

        // Head arrives, then the body stalls past the read timeout: covered by the client's
        // timeout; here a server that closes early gives a truncated JSON body instead.
        let (base, _) = serve(1, |_, _| "HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"choices\":".into());
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, None, "m", "x", &|_| {});
        assert!(!r.ok && matches!(r.kind, Some("network") | Some("invalidResponse")), "{r:?}");

        let (base, _) = serve(1, |_, _| http("200 OK", "", r#"{"choices":[{"message":{"content":""},"finish_reason":"stop"}]}"#));
        assert_eq!(send(&format!("{base}/chat/completions"), Proto::Chat, None, "m", "x", &|_| {}).kind, Some("empty"));
    }

    #[test]
    fn a_stalled_answer_times_out() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_request(&mut s);
            std::thread::sleep(Duration::from_secs(3));
        });
        let r = send_within(&format!("http://127.0.0.1:{port}/v1/messages"), Proto::Anthropic, None, "m", "x", Duration::from_millis(400), &|_| {});
        assert_eq!((r.ok, r.kind, r.requests), (false, Some("timeout"), 1), "{r:?}");
        assert!(r.error.unwrap().contains("超时"));
    }

    #[test]
    fn chat_budget_fallback_is_bounded_and_counted() {
        // Rejects `max_tokens`, accepts `max_completion_tokens`.
        let (base, seen) = serve(2, |req, _| {
            if req.contains("\"max_tokens\"") {
                http("400 Bad Request", "", r#"{"error":{"message":"Unsupported parameter: 'max_tokens'"}}"#)
            } else {
                http("200 OK", "", r#"{"choices":[{"message":{"content":"1, 2"},"finish_reason":"stop"}]}"#)
            }
        });
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, None, "gpt-5.5", "x", &|_| {});
        assert!(r.ok && r.requests == 2, "{r:?}");
        assert!(seen.recv().unwrap().contains("\"max_tokens\":16384"));
        assert!(seen.recv().unwrap().contains("\"max_completion_tokens\":16384"));

        // Always 400: three requests, then the last error.
        let (base, seen) = serve(4, |_, _| http("400 Bad Request", "", r#"{"error":{"message":"no such model"}}"#));
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, None, "m", "x", &|_| {});
        assert_eq!((r.kind, r.requests), (Some("http"), 3));
        assert!(r.error.unwrap().contains("no such model"));
        let last = [seen.recv().unwrap(), seen.recv().unwrap(), seen.recv().unwrap()][2].clone();
        assert!(!last.contains("max_tokens") && !last.contains("max_completion_tokens"), "{last}");
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err());

        // Anthropic and Responses have one body: no second request.
        let (base, seen) = serve(2, |_, _| http("400 Bad Request", "", "{}"));
        assert_eq!(send(&format!("{base}/messages"), Proto::Anthropic, None, "m", "x", &|_| {}).requests, 1);
        seen.recv().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn streamed_replies_and_redirects() {
        let (base, _) = serve(1, |_, _| {
            let sse = format!("data: {}\n\ndata: [DONE]\n\n", json!({ "id": "c", "choices": [{ "index": 0, "delta": { "content": "3, 4" }, "finish_reason": "stop" }] }));
            http("200 OK", "content-type: text/event-stream\r\n", &sse)
        });
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, None, "m", "x", &|_| {});
        assert_eq!(r.text.as_deref(), Some("3, 4"), "{r:?}");

        let (other, seen) = serve(1, |_, _| http("200 OK", "", "{}"));
        let (base, _) = serve(1, move |_, _| http("307 Temporary Redirect", &format!("location: {other}/messages\r\n"), ""));
        let r = send(&format!("{base}/messages"), Proto::Anthropic, Some("sk-secret-1234"), "m", "x", &|_| {});
        assert_eq!(r.kind, Some("redirect"));
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "the key must not follow a redirect");
    }

    // ------------------------------------------------------------ targets

    fn route(id: &str, enabled: bool, map: &[(&str, &str)]) -> Route {
        serde_json::from_value(json!({ "id": id, "name": format!("Forward {id}"), "library": format!("lib-{id}"), "upstreamApi": "chat", "enabled": enabled, "modelMap": map })).unwrap()
    }

    fn view<'a>(routes: &'a [Route], serving: Serving<'a>) -> GatewayView<'a> {
        GatewayView {
            ports: vec![18650, 18000],
            running: Some(18650),
            routes,
            serving,
            upstream: &|r: &Route| (r.id != "orphan").then(|| (format!("Upstream {}", r.id), format!("https://{}.example.com/v1", r.id))),
        }
    }

    #[test]
    fn gateway_addresses_resolve_to_one_forward() {
        let routes = [route("relay", true, &[("gpt-5.5", "gpt-5.5-2026")]), route("paused", false, &[]), route("orphan", true, &[])];
        let none = |_: Option<&[String]>, _: &str| (vec![], vec![]);
        let gw = view(&routes, &none);
        // Not the gateway: other hosts and ports.
        assert!(through_gateway("https://api.example.com/v1/chat/completions", "m", &gw).is_none());
        assert!(through_gateway("http://127.0.0.1:9999/relay/v1/chat/completions", "m", &gw).is_none());
        // One forward: path shown, the model map's rewrite too.
        let (url, path) = through_gateway("http://127.0.0.1:18650/relay/v1/chat/completions", "gpt-5.5", &gw).unwrap().unwrap();
        assert_eq!(url, "http://127.0.0.1:18650/relay/v1/chat/completions");
        assert_eq!((path.entry, path.route.as_str(), path.upstream_name.as_str(), path.upstream_url.as_str()), ("route", "relay", "Upstream relay", "https://relay.example.com/v1"));
        assert_eq!(path.mapped_model.as_deref(), Some("gpt-5.5-2026"));
        assert!(!path.pinned);
        let code = |u: &str| through_gateway(u, "m", &gw).unwrap().unwrap_err().1.code;
        assert_eq!(code("http://127.0.0.1:18650/paused/v1/chat/completions"), "gatewayRoute");
        assert_eq!(code("http://127.0.0.1:18650/orphan/v1/chat/completions"), "gatewayRoute");
        assert_eq!(code("http://127.0.0.1:18650/missing/v1/chat/completions"), "gatewayRoute");
        // An old port, or the gateway stopped.
        assert_eq!(code("http://localhost:18000/relay/v1/chat/completions"), "gatewayOff");
        let stopped = GatewayView { running: None, ..view(&routes, &none) };
        assert_eq!(through_gateway("http://127.0.0.1:18650/relay/v1/messages", "m", &stopped).unwrap().unwrap_err().1.code, "gatewayOff");
    }

    #[test]
    fn pooled_gateway_addresses_are_pinned_or_refused() {
        let routes = [route("a", true, &[]), route("b", true, &[]), route("c", true, &[])];
        let asked = std::sync::Mutex::new(vec![]);
        let serving = |only: Option<&[String]>, model: &str| {
            asked.lock().unwrap().push(only.map(|o| o.to_vec()));
            match model {
                "one" => (vec![route("a", true, &[])], vec![route("c", true, &[])]),
                "maybe" => (vec![], vec![route("b", true, &[])]),
                "two" => (vec![route("a", true, &[]), route("b", true, &[])], vec![]),
                _ => (vec![], vec![]),
            }
        };
        let gw = view(&routes, &serving);
        let (url, path) = through_gateway("http://127.0.0.1:18650/v1/messages", "one", &gw).unwrap().unwrap();
        assert_eq!(url, "http://127.0.0.1:18650/a/v1/messages");
        assert_eq!((path.entry, path.pinned, path.skipped.clone()), ("unified", true, vec!["c".to_string()]));
        let (url, path) = through_gateway("http://127.0.0.1:18650/a+b/v1/chat/completions", "maybe", &gw).unwrap().unwrap();
        assert_eq!(url, "http://127.0.0.1:18650/b/v1/chat/completions");
        assert_eq!((path.entry, path.route.as_str()), ("combined", "b"));
        assert_eq!(asked.lock().unwrap()[1], Some(vec!["a".to_string(), "b".to_string()]));
        let err = through_gateway("http://127.0.0.1:18650/v1/responses", "two", &gw).unwrap().unwrap_err().1;
        assert_eq!(err.code, "gatewayPool");
        assert!(err.message.contains('a') && err.message.contains('b'));
        assert_eq!(through_gateway("http://127.0.0.1:18650/v1/responses", "none", &gw).unwrap().unwrap_err().1.code, "gatewayPool");
    }

    #[test]
    fn targets_come_from_saved_config_and_samples_check_them() {
        let _h = crate::util::TestHome::new("attribution-target");
        let lib = json!([
            { "id": "relay", "name": "Relay", "baseUrl": "https://relay.example.com/v1/", "apiKey": "sk-lib-1234", "api": "chat" },
            { "id": "gem", "name": "Gem", "baseUrl": "https://gem.example.com", "api": "gemini" },
        ]);
        crate::store::save(&json!({ "library": lib })).unwrap();
        let from = crate::library::FROM;
        let t = target(from, "relay", "gpt-5.5").unwrap();
        assert_eq!((t.provider.as_str(), t.model.as_str(), t.api.as_str(), t.url.as_str()), ("relay", "gpt-5.5", "chat", "https://relay.example.com/v1/chat/completions"));
        assert!(t.has_key && t.blocked.is_none() && t.gateway.is_none() && t.entry.is_none());
        // The key is never part of what the frontend sees.
        let shown = serde_json::to_string(&t).unwrap();
        assert!(!shown.contains("sk-lib-1234"), "{shown}");
        assert_eq!(target(from, "gem", "m").unwrap().blocked.unwrap().code, "protocol");
        assert_eq!(target(from, "nope", "m").unwrap().blocked.unwrap().code, "noEndpoint");
        assert_eq!(target("claude", CATALOG, "m").unwrap().blocked.unwrap().code, "noEndpoint");
        assert!(target(from, "relay", " ").is_err());
        // Another model, or a changed key: another fingerprint.
        assert_ne!(target(from, "relay", "gpt-5.4").unwrap().fp, t.fp);
        let r = sample(from, "relay", "gpt-5.5", "挑战", "stale", &|_| {}).unwrap();
        assert_eq!(r.kind, Some("targetChanged"));
        assert_eq!(r.requests, 0);
        assert_eq!(sample(from, "gem", "m", "挑战", "", &|_| {}).unwrap().kind, Some("blocked"));
        assert!(sample(from, "relay", "gpt-5.5", "", &t.fp, &|_| {}).is_err());
        assert!(sample(from, "relay", "gpt-5.5", &"x".repeat(MAX_PROMPT + 1), &t.fp, &|_| {}).is_err());
    }

    #[test]
    fn codex_catalog_tests_the_provider_codex_is_on() {
        let h = crate::util::TestHome::new("attribution-codex");
        let dir = h.0.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |cfg: &str| std::fs::write(dir.join("config.toml"), cfg).unwrap();
        std::fs::write(dir.join(".env"), "RELAY_KEY=sk-relay-4321
").unwrap();
        let tables = "[model_providers.relay]
name = \"Relay\"
base_url = \"https://relay.example.com/v1\"
env_key = \"RELAY_KEY\"
wire_api = \"responses\"

[model_providers.agentplus]
name = \"Relay\"
base_url = \"https://mirror.example.com/v1\"
env_key = \"RELAY_KEY\"
wire_api = \"responses\"
";
        // Fixed-id mode: Codex reads the mirror; AgentPlus shows the provider it was copied from.
        write(&format!("model_provider = \"agentplus\"
{tables}"));
        let mut store = json!({});
        crate::store::set_str(&mut store, codex::ID, "fixedSource", "relay");
        crate::store::save(&store).unwrap();
        let t = target(codex::ID, CATALOG, "gpt-5.5").unwrap();
        assert_eq!((t.provider.as_str(), t.entry.as_deref(), t.url.as_str()), ("relay", Some("agentplus"), "https://mirror.example.com/v1/responses"));
        assert!(t.has_key && t.blocked.is_none());
        // A provider picked in its own list is that provider, whatever Codex is on.
        let own = target(codex::ID, "relay", "gpt-5.5").unwrap();
        assert_eq!((own.provider.as_str(), own.entry, own.url.as_str()), ("relay", None, "https://relay.example.com/v1/responses"));
        write(&format!("model_provider = \"relay\"
{tables}"));
        let t = target(codex::ID, CATALOG, "gpt-5.5").unwrap();
        assert_eq!((t.provider.as_str(), t.entry), ("relay", None));
        // The ChatGPT sign-in has no address or key to test with.
        write(tables);
        let t = target(codex::ID, CATALOG, "gpt-5.5").unwrap();
        assert_eq!((t.provider.as_str(), t.blocked.unwrap().code), ("openai", "signIn"));
    }

    #[test]
    fn samples_reach_the_saved_endpoint() {
        let _h = crate::util::TestHome::new("attribution-sample");
        let (base, seen) = serve(1, |_, _| http("200 OK", "", &json!({ "choices": [{ "message": { "content": NUMBERS }, "finish_reason": "stop" }] }).to_string()));
        crate::store::save(&json!({ "library": [{ "id": "local", "name": "Local", "baseUrl": base, "apiKey": "sk-local-5678", "api": "chat" }] })).unwrap();
        let from = crate::library::FROM;
        let t = target(from, "local", "claude-opus-4-7").unwrap();
        let r = sample(from, "local", "claude-opus-4-7", "挑战", &t.fp, &|_| {}).unwrap();
        assert!(r.ok && r.text.as_deref() == Some(NUMBERS) && r.requests == 1, "{r:?}");
        let req = seen.recv().unwrap();
        assert!(req.contains("authorization: Bearer sk-local-5678"));
        assert_eq!(body_of(&req)["model"], "claude-opus-4-7");
    }

    /// Collects what `send` reports to the live log.
    fn recorder() -> (std::sync::Arc<std::sync::Mutex<Vec<Live>>>, impl Fn(Live)) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let out = seen.clone();
        (seen, move |e| out.lock().unwrap().push(e))
    }

    /// (reasoning, text) the log received, joined.
    fn logged(events: &[Live]) -> (String, String) {
        let (mut r, mut t) = (String::new(), String::new());
        for e in events {
            match e {
                Live::Reasoning { text } => r.push_str(text),
                Live::Text { text } => t.push_str(text),
                _ => {}
            }
        }
        (r, t)
    }

    #[test]
    fn streamed_answers_reach_the_log_as_they_arrive() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            read_request(&mut s);
            let frames = [
                json!({ "id": "c", "choices": [{ "index": 0, "delta": { "reasoning_content": "想一想 " } }] }),
                json!({ "id": "c", "choices": [{ "index": 0, "delta": { "content": "12, 45, " } }] }),
                json!({ "id": "c", "choices": [{ "index": 0, "delta": { "content": "300，7" }, "finish_reason": "stop" }] }),
            ];
            let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).chain(["data: [DONE]\n\n".to_string()]).collect();
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n").unwrap();
            // In small pieces, splitting lines and multi-byte characters.
            for piece in body.as_bytes().chunks(7) {
                s.write_all(piece).unwrap();
                s.flush().unwrap();
                std::thread::sleep(Duration::from_millis(3));
            }
        });
        let (seen, on) = recorder();
        let r = send(&format!("http://127.0.0.1:{port}/v1/chat/completions"), Proto::Chat, None, "m", "x", &on);
        assert_eq!(r.text.as_deref(), Some("12, 45, 300，7"), "{r:?}");
        let events = seen.lock().unwrap().clone();
        assert_eq!(&events[..2], &[Live::Request { n: 1 }, Live::Status { status: 200 }]);
        assert_eq!(logged(&events), ("想一想 ".to_string(), "12, 45, 300，7".to_string()));
        assert!(events.len() > 3, "more than one piece: {events:?}");
    }

    #[test]
    fn non_streamed_answers_and_errors_in_the_log() {
        // A server that answers in one piece: the answer is logged whole, reasoning apart.
        let (base, _) = serve(1, |_, _| http("200 OK", "", &json!({ "content": [{ "type": "thinking", "thinking": "hm" }, { "type": "text", "text": NUMBERS }], "stop_reason": "end_turn" }).to_string()));
        let (seen, on) = recorder();
        assert!(send(&format!("{base}/messages"), Proto::Anthropic, None, "m", "x", &on).ok);
        assert_eq!(logged(&seen.lock().unwrap()), ("hm".to_string(), NUMBERS.to_string()));
        // An error body stays out of the log; each fallback request is logged.
        let (base, _) = serve(3, |_, _| http("400 Bad Request", "", r#"{"error":{"message":"nope"}}"#));
        let (seen, on) = recorder();
        let r = send(&format!("{base}/chat/completions"), Proto::Chat, None, "m", "x", &on);
        assert_eq!(r.kind, Some("http"));
        let events = seen.lock().unwrap().clone();
        let want: Vec<Live> = (1..=3).flat_map(|n| [Live::Request { n }, Live::Status { status: 400 }]).collect();
        assert_eq!(events, want);
    }

    #[test]
    fn history_keeps_results_newest_first() {
        let _h = crate::util::TestHome::new("attribution-history");
        assert!(history().is_empty());
        let a = history_add(json!({ "model": "a" })).unwrap();
        assert_eq!(a.len(), 1);
        assert!(a[0]["id"].is_string() && a[0]["at"].is_i64());
        std::thread::sleep(Duration::from_millis(2));
        let b = history_add(json!({ "model": "b", "id": "forged" })).unwrap();
        assert_eq!(b.iter().map(|e| e["model"].as_str().unwrap()).collect::<Vec<_>>(), ["b", "a"]);
        assert_ne!(b[0]["id"], "forged");
        assert_eq!(history(), b);
        let left = history_delete(Some(vec![b[0]["id"].as_str().unwrap().to_string()])).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0]["model"], "a");
        assert!(history_add(json!([1])).is_err());
        assert!(history_add(json!({ "x": "y".repeat(HISTORY_ENTRY_MAX) })).is_err());
        for i in 0..HISTORY_MAX + 5 {
            history_add(json!({ "i": i })).unwrap();
        }
        let all = history();
        assert_eq!(all.len(), HISTORY_MAX);
        assert_eq!(all[0]["i"], HISTORY_MAX + 4);
        assert!(history_delete(None).unwrap().is_empty() && history().is_empty());
        // A damaged file reads as no history.
        std::fs::write(history_path(), "{not json").unwrap();
        assert!(history().is_empty());
    }

    #[test]
    fn think_blocks_and_redaction() {
        assert_eq!(strip_think("  <think>a 1 2</think>\n3, 4"), "\n3, 4");
        assert_eq!(strip_think("3, 4 <think>x</think>"), "3, 4 <think>x</think>");
        assert_eq!(strip_think("<think>never closed 1 2 3"), "");
        assert_eq!(redact("bad key sk-abcdefgh9876 here", Some("sk-abcdefgh9876")), "bad key ••••9876 here");
        assert_eq!(redact("nothing", None), "nothing");
    }
}
