//! Provider network calls: latency and model listing.

use crate::util::clip;
use std::time::{Duration, Instant};

/// Anthropic API version AgentPlus speaks.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adds the provider's key: `x-api-key` for Anthropic's API (which always gets
/// `anthropic-version`, key or not, so keyless local proxies work), a bearer token otherwise.
pub fn with_key(req: reqwest::blocking::RequestBuilder, anthropic: bool, key: Option<&str>) -> reqwest::blocking::RequestBuilder {
    match (anthropic, key) {
        (true, Some(k)) => req.header("x-api-key", k).header("anthropic-version", ANTHROPIC_VERSION),
        (true, None) => req.header("anthropic-version", ANTHROPIC_VERSION),
        (false, Some(k)) => req.bearer_auth(k),
        (false, None) => req,
    }
}

fn with_api_key(req: reqwest::blocking::RequestBuilder, api: &str, key: Option<&str>) -> reqwest::blocking::RequestBuilder {
    if api == "gemini" {
        match key { Some(k) => req.header("x-goog-api-key", k), None => req }
    } else {
        with_key(req, api == "anthropic", key)
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    client_with(Duration::from_secs(12))
}

fn client_with(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(same_host_redirects())
        .build()
        .map_err(|e| e.to_string())
}

/// Follows redirects only within one origin (an added slash, a moved path). reqwest drops
/// `Authorization` when the host or port changes but keeps custom API-key headers, so a key must
/// never be replayed to wherever a provider points; anything else comes back as the 3xx.
fn same_host_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|a| {
        let origin = |u: &url::Url| (u.scheme().to_string(), u.host_str().map(String::from), u.port_or_known_default());
        let same = a.previous().first().map(|u| origin(u) == origin(a.url())).unwrap_or(false);
        if a.previous().len() > 5 {
            a.error(crate::i18n::l("Too many redirects", "重定向次数过多"))
        } else if same {
            a.follow()
        } else {
            a.stop()
        }
    })
}

/// Model lists run to a few MB (OpenRouter); anything far bigger is not a model list.
const MAX_MODELS_BODY: u64 = 32 << 20;
/// A one-word test reply is tiny; an error page can be a whole HTML document.
const MAX_TEST_BODY: u64 = 4 << 20;

/// For a redirect that wasn't followed (another host, port or scheme), what to tell the user.
fn moved_to(resp: &reqwest::blocking::Response) -> Option<String> {
    if !resp.status().is_redirection() {
        return None;
    }
    let to = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("?");
    Some(tr!("The URL redirects to {to} (HTTP {}); not followed to protect the API key. Use the new URL directly", "地址被重定向到 {to}（HTTP {}），为保护密钥没有跟随；请直接填写新地址", resp.status().as_u16()))
}

/// The body as text, read up to `cap` bytes.
fn body_text(resp: reqwest::blocking::Response, cap: u64) -> Result<String, String> {
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(resp, cap + 1), &mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > cap {
        return Err(tr!("Response too large (over {} MB)", "响应太大（超过 {} MB）", cap >> 20));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `GET url` without credentials; the body as text (up to `cap` bytes) on a 2xx.
pub fn get_text(url: &str, timeout: Duration, cap: u64) -> Result<String, String> {
    let resp = client_with(timeout)?.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    body_text(resp, cap)
}

fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

/// Native Gemini defaults to v1beta, retaining an explicit version and proxy prefix.
/// Model names are one URL segment after the optional `models/` resource prefix.
fn gemini_url(base_url: &str, model: Option<&str>) -> Result<url::Url, String> {
    let invalid = || crate::i18n::l("Invalid Gemini URL; use an HTTP or HTTPS URL", "Gemini 地址无效，请填写 HTTP 或 HTTPS 地址").to_string();
    let mut url = url::Url::parse(base_url.trim_end_matches('/')).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid());
    }
    let model = model.map(|m| m.strip_prefix("models/").unwrap_or(m));
    if model.is_some_and(|m| m.is_empty() || matches!(m, "." | "..") || m.contains('/')) {
        return Err(crate::i18n::l("Invalid Gemini model name", "Gemini 模型名无效").into());
    }
    let versioned = matches!(url.path().trim_end_matches('/').rsplit('/').next(), Some("v1" | "v1beta" | "v1alpha"));
    let mut path = url.path_segments_mut().map_err(|_| invalid())?;
    path.pop_if_empty();
    if !versioned { path.push("v1beta"); }
    path.push("models");
    if let Some(model) = model { path.push(&format!("{model}:generateContent")); }
    drop(path);
    Ok(url)
}

/// Time to first response header of `GET <base>/models`. Any HTTP status counts
/// (an unauthenticated 401 still measures the round trip).
pub fn latency(base_url: &str) -> Result<u64, String> {
    let mut req = client()?.get(models_url(base_url)).timeout(Duration::from_secs(8));
    // The local gateway refuses requests without a key; its test key lets the probe through
    // to the upstream, so the time still covers the upstream. Only sent to the gateway's own
    // port on this machine, never to whatever else listens on localhost.
    let gateway = url::Url::parse(base_url).ok().is_some_and(|u| {
        matches!(u.host_str(), Some("127.0.0.1" | "localhost")) && u.port().is_some() && u.port() == crate::gateway::server::running_port()
    });
    if gateway {
        req = req.bearer_auth(crate::gateway::server::test_key());
    }
    let t0 = Instant::now();
    req.send()
        .map_err(|e| if e.is_timeout() { crate::i18n::l("Timed out", "超时").to_string() } else { crate::i18n::l("Connection failed", "连接失败").to_string() })?;
    Ok(t0.elapsed().as_millis() as u64)
}

/// Lists model ids from OpenAI, Anthropic or native Gemini endpoints. Gemini pages
/// share one time/body budget; page tokens can only change the query on this URL.
pub fn list_models(base_url: &str, key: Option<&str>, api: &str) -> Result<Vec<String>, String> {
    let base = if api == "gemini" { gemini_url(base_url, None)?.to_string() } else { models_url(base_url) };
    let client = client()?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut url = base.clone();
    let mut ids = Vec::new();
    let mut tokens = std::collections::BTreeSet::new();
    let mut bytes = 0;
    loop {
        let timeout = deadline.checked_duration_since(Instant::now()).ok_or_else(|| crate::i18n::l("Request timed out", "请求超时").to_string())?;
        let req = with_api_key(client.get(&url).timeout(timeout), api, key);
        let resp = req.send().map_err(|e| if e.is_timeout() { crate::i18n::l("Request timed out", "请求超时").to_string() } else { tr!("Connection failed: {e}", "连接失败：{e}") })?;
        let status = resp.status();
        if let Some(to) = moved_to(&resp) { return Err(to); }
        let text = body_text(resp, MAX_MODELS_BODY)?;
        bytes += text.len() as u64;
        if bytes > MAX_MODELS_BODY {
            return Err(tr!("Response too large (over {} MB)", "响应太大（超过 {} MB）", MAX_MODELS_BODY >> 20));
        }
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => tr!("Invalid API key or no permission (HTTP {})", "密钥无效或没有权限（HTTP {}）", status.as_str()),
                404 => crate::i18n::l("This URL has no /models endpoint (HTTP 404)", "这个地址没有 /models 接口（HTTP 404）").to_string(),
                _ => format!("HTTP {status}"),
            });
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| crate::i18n::l("Response is not JSON", "返回的不是 JSON").to_string())?;
        let list = v.get("data").or_else(|| v.get("models")).and_then(|d| d.as_array()).ok_or(crate::i18n::l("No model list in the response", "返回里没有模型列表"))?;
        ids.extend(list.iter().filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(|x| x.as_str())).map(|id| {
            if api == "gemini" { id.strip_prefix("models/").unwrap_or(id) } else { id }.to_string()
        }));
        let next = (api == "gemini").then(|| v.get("nextPageToken").and_then(|t| t.as_str()).filter(|t| !t.is_empty())).flatten();
        let Some(token) = next else { break };
        if !tokens.insert(token.to_string()) || tokens.len() >= 100 {
            return Err(crate::i18n::l("Invalid model list pagination; check the upstream API", "模型列表分页异常，请检查上游接口").into());
        }
        let mut next_url = url::Url::parse(&base).map_err(|e| e.to_string())?;
        let query: Vec<_> = next_url.query_pairs().filter(|(k, _)| k != "pageToken").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        next_url.set_query(None);
        next_url.query_pairs_mut().extend_pairs(query).append_pair("pageToken", token);
        url = next_url.to_string();
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    /// HTTP status, when the server answered.
    pub status: Option<u16>,
    /// Time until the full reply arrived.
    pub ms: u64,
    pub model: String,
    /// Endpoint that was called (no key).
    pub url: String,
    /// First part of the model's answer.
    pub reply: Option<String>,
    pub error: Option<String>,
    /// Tokens reported by the server (input, output).
    pub usage: Option<(u64, u64)>,
}

/// Request bodies for a test call, tried in order. The first ones turn thinking off (the
/// test only needs one word back, and thinking would eat the small token budget and
/// time); each field is one that some servers reject as unknown, so on HTTP 400/422
/// the next body is tried, ending with the plain request.
fn test_bodies(api: &str, model: &str, prompt: &str) -> Vec<serde_json::Value> {
    use serde_json::json;
    let with = |base: &serde_json::Value, extra: serde_json::Value| {
        let mut b = base.clone();
        if let (Some(b), Some(extra)) = (b.as_object_mut(), extra.as_object()) {
            b.extend(extra.clone());
        }
        b
    };
    match api {
        "anthropic" => {
            let plain = json!({ "model": model, "max_tokens": 32, "messages": [{ "role": "user", "content": prompt }] });
            vec![with(&plain, json!({ "thinking": { "type": "disabled" } })), plain]
        }
        "chat" => {
            let plain = json!({ "model": model, "max_tokens": 32, "stream": false, "messages": [{ "role": "user", "content": prompt }] });
            // `thinking`: DeepSeek, GLM, Doubao, MiMo, Kimi…; `enable_thinking`: Qwen, SiliconFlow.
            vec![with(&plain, json!({ "thinking": { "type": "disabled" }, "enable_thinking": false })), plain]
        }
        "gemini" => {
            // Models that cannot disable thinking need room for thoughts before the reply.
            let plain = json!({ "contents": [{ "role": "user", "parts": [{ "text": prompt }] }], "generationConfig": { "maxOutputTokens": 1024 } });
            vec![with(&plain, json!({ "generationConfig": { "maxOutputTokens": 64, "thinkingConfig": { "thinkingBudget": 0 } } })), plain]
        }
        _ => {
            let plain = json!({ "model": model, "input": prompt, "max_output_tokens": 64, "stream": false });
            // Newer OpenAI models take "none", older reasoning models only "minimal".
            vec![with(&plain, json!({ "reasoning": { "effort": "none" } })), with(&plain, json!({ "reasoning": { "effort": "minimal" } })), plain]
        }
    }
}

/// Some relays answer with an SSE stream even for `"stream": false`. Folds the events into
/// the JSON the non-streaming call returns (reply text and usage), or the error the stream
/// reported. None when the body isn't an event stream.
fn from_sse(api: &str, text: &str) -> Option<Result<serde_json::Value, String>> {
    use serde_json::{json, Value};
    let events: Vec<Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .filter_map(|d| serde_json::from_str(d).ok())
        .collect();
    if events.is_empty() {
        return None;
    }
    let ty = |e: &Value| e.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    for e in &events {
        let t = ty(e);
        let msg = e.pointer("/error/message").or_else(|| e.pointer("/response/error/message")).or_else(|| if t == "error" { e.get("message") } else { None });
        if let Some(m) = msg {
            return Some(Err(m.as_str().map(String::from).unwrap_or_else(|| m.to_string())));
        }
        if matches!(t.as_str(), "error" | "response.failed") {
            return Some(Err(t));
        }
    }
    let text_of = |kind: &str, ptr: &str| events.iter().filter(|e| kind.is_empty() || ty(e) == kind).filter_map(|e| e.pointer(ptr).and_then(|t| t.as_str())).collect::<String>();
    Some(Ok(match api {
        "anthropic" => {
            let input = events.iter().find_map(|e| e.pointer("/message/usage/input_tokens").cloned());
            let output = events.iter().rev().find_map(|e| (ty(e) == "message_delta").then(|| e.pointer("/usage/output_tokens").cloned()).flatten());
            json!({ "content": [{ "type": "text", "text": text_of("content_block_delta", "/delta/text") }], "usage": { "input_tokens": input, "output_tokens": output } })
        }
        "chat" => {
            let usage = events.iter().rev().find_map(|e| e.get("usage").filter(|u| u.is_object()).cloned());
            json!({ "choices": [{ "message": { "content": text_of("", "/choices/0/delta/content") } }], "usage": usage })
        }
        _ => {
            // response.completed carries the whole response (output and usage); the deltas
            // cover relays whose final event leaves the output out.
            let done = events.iter().rev().find(|e| matches!(ty(e).as_str(), "response.completed" | "response.incomplete" | "response.done"));
            let mut v = done.and_then(|e| e.get("response")).filter(|r| r.is_object()).cloned().unwrap_or_else(|| json!({}));
            let deltas = text_of("response.output_text.delta", "/delta");
            if !deltas.is_empty() {
                v["output_text"] = Value::String(deltas);
            }
            v
        }
    }))
}

/// Sends one tiny real request with the provider's key and model, so the address,
/// key, protocol and model are all checked (the request uses a few tokens).
pub fn test_call(base_url: &str, key: Option<&str>, api: &str, model: &str) -> TestResult {
    let base = base_url.trim_end_matches('/');
    let url = match api {
        "anthropic" => Ok(format!("{base}/messages")),
        "chat" => Ok(format!("{base}/chat/completions")),
        "gemini" => gemini_url(base, Some(model)).map(|u| u.to_string()),
        _ => Ok(format!("{base}/responses")),
    };
    let mut r = TestResult { ok: false, status: None, ms: 0, model: model.into(), url: base.into(), reply: None, error: None, usage: None };
    let url = match url { Ok(url) => url, Err(e) => { r.error = Some(e); return r; } };
    r.url = url.clone();
    let client = match client_with(Duration::from_secs(45)) {
        Ok(c) => c,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let mut bodies = test_bodies(api, model, "Reply with exactly one word: pong").into_iter().peekable();
    // OpenCode Go refuses requests without a conversation id; each test is its own conversation.
    let session = crate::gateway::session::wants_session(base).then(|| format!("agp-test-{:x}", chrono::Utc::now().timestamp_micros()));
    let (status, text) = loop {
        let body = bodies.next().expect("test_bodies is never empty");
        let mut req = with_api_key(client.post(&url).header("content-type", "application/json").body(body.to_string()), api, key);
        if let Some(s) = &session {
            req = req.header(crate::gateway::session::HEADER, s).header("user-agent", concat!("AgentPlus/", env!("CARGO_PKG_VERSION")));
        }
        let t0 = Instant::now();
        let resp = match req.send() {
            Ok(x) => x,
            Err(e) => {
                r.ms = t0.elapsed().as_millis() as u64;
                r.error = Some(if e.is_timeout() { crate::i18n::l("Request timed out (45 s)", "请求超时（45 秒）").into() } else if e.is_connect() { crate::i18n::l("Connection failed: URL unreachable", "连接失败：地址不可达").into() } else { tr!("Request failed: {e}", "请求失败：{e}") });
                return r;
            }
        };
        let status = resp.status();
        if matches!(status.as_u16(), 400 | 422) && bodies.peek().is_some() {
            continue;
        }
        if let Some(to) = moved_to(&resp) {
            r.ms = t0.elapsed().as_millis() as u64;
            r.status = Some(status.as_u16());
            r.error = Some(to);
            return r;
        }
        let text = match body_text(resp, MAX_TEST_BODY) {
            Ok(t) => t,
            Err(e) => {
                r.ms = t0.elapsed().as_millis() as u64;
                r.status = Some(status.as_u16());
                r.error = Some(e);
                return r;
            }
        };
        r.ms = t0.elapsed().as_millis() as u64;
        break (status, text);
    };
    r.status = Some(status.as_u16());
    let v: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        let msg = v.as_ref().and_then(crate::gateway::convert::error_message).unwrap_or_else(|| clip(text.trim(), 160));
        let hint = match status.as_u16() {
            401 | 403 => crate::i18n::l("Invalid API key or no permission", "密钥无效或没有权限"),
            404 => crate::i18n::l("Wrong base URL or API type, or no such model", "地址或接口类型不对，或者没有这个模型"),
            429 => crate::i18n::l("Too many requests or quota exhausted", "请求太频繁或额度用完"),
            400 | 422 => crate::i18n::l("Request rejected; the model name may be wrong or the API type mismatched", "请求被拒绝，可能是模型名不对或接口类型不匹配"),
            s if s >= 500 => crate::i18n::l("Server error", "服务端出错"),
            _ => crate::i18n::l("Request failed", "请求失败"),
        };
        r.error = Some(if msg.is_empty() { tr!("{hint} (HTTP {status})", "{hint}（HTTP {status}）") } else { tr!("{hint} (HTTP {}): {}", "{hint}（HTTP {}）：{}", status.as_u16(), clip(msg.trim(), 200)) });
        return r;
    }
    let v = match v.map(Ok).or_else(|| from_sse(api, &text)) {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            r.error = Some(tr!("The stream returned an error: {}", "流式响应中返回了错误：{}", clip(e.trim(), 200)));
            return r;
        }
        None => {
            r.error = Some(tr!("Response is not JSON: {}", "返回的不是 JSON：{}", clip(text.trim(), 120)));
            return r;
        }
    };
    let reply = match api {
        "anthropic" => v.get("content").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<String>()),
        "chat" => v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).map(String::from),
        "gemini" => v.pointer("/candidates/0/content/parts").and_then(|p| p.as_array()).map(|parts| {
            parts.iter().filter(|p| p.get("thought").and_then(|t| t.as_bool()) != Some(true))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<String>()
        }),
        _ => v.get("output_text").and_then(|t| t.as_str()).map(String::from).or_else(|| {
            v.get("output").and_then(|o| o.as_array()).map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.get("content").and_then(|c| c.as_array()))
                    .flatten()
                    .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                    .collect::<String>()
            })
        }),
    };
    r.reply = reply.filter(|s| !s.trim().is_empty()).map(|s| clip(s.trim(), 80));
    let u = v.get(if api == "gemini" { "usageMetadata" } else { "usage" });
    let num = |k: &[&str]| k.iter().find_map(|k| u.and_then(|u| u.get(*k)).and_then(|x| x.as_u64()));
    let (input, output): (&[&str], &[&str]) = if api == "gemini" { (&["promptTokenCount"], &["candidatesTokenCount"]) }
        else { (&["input_tokens", "prompt_tokens"], &["output_tokens", "completion_tokens"]) };
    if let (Some(i), Some(o)) = (num(input), num(output)) {
        r.usage = Some((i, o));
    }
    if api == "gemini" {
        let block = v.pointer("/promptFeedback/blockReason").and_then(|b| b.as_str()).filter(|b| !b.is_empty() && *b != "BLOCK_REASON_UNSPECIFIED");
        let finish = v.pointer("/candidates/0/finishReason").and_then(|f| f.as_str());
        let rejected = finish.filter(|f| !matches!(*f, "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED" | ""));
        if let Some(reason) = block.or(rejected) {
            r.error = Some(tr!("Gemini did not complete generation: {reason}", "Gemini 未完成生成：{reason}"));
            return r;
        }
        if v.get("error").is_some() || r.reply.is_none() {
            let reason = crate::gateway::convert::error_message(&v).or_else(|| finish.map(String::from)).unwrap_or_else(|| crate::i18n::l("No text in the response", "响应中没有文本").into());
            r.error = Some(tr!("Gemini test failed: {}", "Gemini 测试失败：{}", clip(&reason, 200)));
            return r;
        }
    }
    r.ok = true;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One-shot local HTTP server that answers with `status` and `body`.
    fn serve(status: u16, body: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            let resp = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            s.write_all(resp.as_bytes()).unwrap();
        });
        format!("http://{addr}/v1")
    }

    #[test]
    fn parses_replies_and_errors() {
        let r = test_call(&serve(200, r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"pong"}]}],"usage":{"input_tokens":14,"output_tokens":2}}"#), Some("k"), "responses", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.usage == Some((14, 2)) && r.url.ends_with("/v1/responses"));
        let r = test_call(&serve(200, r#"{"choices":[{"message":{"content":"pong"}}],"usage":{"prompt_tokens":9,"completion_tokens":1}}"#), None, "chat", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.url.ends_with("/chat/completions"));
        let r = test_call(&serve(200, r#"{"content":[{"type":"text","text":"pong"}]}"#), Some("k"), "anthropic", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.url.ends_with("/messages"));
        let r = test_call(&serve(401, r#"{"error":{"message":"invalid api key"}}"#), Some("k"), "chat", "m");
        assert!(!r.ok && r.status == Some(401) && r.error.as_deref().unwrap().contains("密钥无效") && r.error.as_deref().unwrap().contains("invalid api key"));
        let r = test_call("http://127.0.0.1:9", None, "chat", "m");
        assert!(!r.ok && r.status.is_none());
    }

    /// Local server answering `n` requests with `reply(request)`; each request's head is sent on the channel.
    fn serve_with(n: usize, reply: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (u16, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for s in l.incoming().take(n).flatten() {
                let mut s = s;
                let head = read_request(&mut s);
                let _ = s.write_all(&reply(&head));
                let _ = tx.send(head);
            }
        });
        (port, rx)
    }

    /// Reads one request, headers and body (by content-length), as text.
    fn read_request(s: &mut std::net::TcpStream) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let len = s.read(&mut buf).unwrap_or(0);
            got.extend_from_slice(&buf[..len]);
            let text = String::from_utf8_lossy(&got).to_string();
            let Some(end) = text.find("\r\n\r\n") else {
                if len == 0 { return text; }
                continue;
            };
            let want = text[..end]
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            if len == 0 || got.len() >= end + 4 + want {
                return text;
            }
        }
    }

    /// The error message the provider test shows, picked like the gateway picks it.
    #[test]
    fn test_call_error_messages() {
        let error_of = |status: &'static str, body: &'static str| {
            let (port, _seen) = serve_with(3, move |_| http(status, "", body));
            test_call(&format!("http://127.0.0.1:{port}/v1"), Some("k"), "chat", "m").error.unwrap_or_default()
        };
        assert_eq!(error_of("400 Bad Request", r#"{"message":"","detail":"bad model"}"#), "请求被拒绝，可能是模型名不对或接口类型不匹配（HTTP 400）：bad model");
        assert_eq!(error_of("429 Too Many", r#"{"error":"quota exceeded"}"#), "请求太频繁或额度用完（HTTP 429）：quota exceeded");
        assert_eq!(error_of("500 Oops", r#"{"error":{"message":"boom"}}"#), "服务端出错（HTTP 500）：boom");
        assert_eq!(error_of("503 Busy", r#"{"message":42}"#), "服务端出错（HTTP 503）：42");
        // No message field: the (shortened) body itself.
        assert_eq!(error_of("503 Busy", r#"{"error":{"code":1}}"#), r#"服务端出错（HTTP 503）：{"error":{"code":1}}"#);
        assert_eq!(error_of("502 Bad", "  "), "服务端出错（HTTP 502 Bad Gateway）");
    }

    #[test]
    fn test_call_turns_thinking_off_and_falls_back() {
        // Server that rejects the no-thinking fields: the plain request is sent next.
        let (port, seen) = serve_with(3, |req| {
            if req.contains("\"thinking\"") || req.contains("\"reasoning\"") {
                http("400 Bad Request", "", r#"{"error":{"message":"unknown field"}}"#)
            } else {
                http("200 OK", "", r#"{"choices":[{"message":{"content":"pong"}}]}"#)
            }
        });
        let r = test_call(&format!("http://127.0.0.1:{port}/v1"), Some("k"), "chat", "m");
        assert!(r.ok && r.status == Some(200) && r.reply.as_deref() == Some("pong"), "{r:?}");
        let first = seen.recv().unwrap();
        assert!(first.contains(r#""thinking":{"type":"disabled"}"#) && first.contains(r#""enable_thinking":false"#), "{first}");
        assert!(!seen.recv().unwrap().contains("thinking"));

        // Responses: "none", then "minimal", then plain; the last error is the one reported.
        let (port, seen) = serve_with(3, |_| http("400 Bad Request", "", r#"{"error":{"message":"no such model"}}"#));
        let r = test_call(&format!("http://127.0.0.1:{port}/v1"), None, "responses", "m");
        assert!(!r.ok && r.status == Some(400) && r.error.as_deref().unwrap().contains("no such model"), "{r:?}");
        assert!(seen.recv().unwrap().contains(r#""effort":"none""#));
        assert!(seen.recv().unwrap().contains(r#""effort":"minimal""#));
        assert!(!seen.recv().unwrap().contains("reasoning"));

        // Other errors are not retried.
        let (port, seen) = serve_with(2, |_| http("401 Unauthorized", "", "{}"));
        let r = test_call(&format!("http://127.0.0.1:{port}/v1"), Some("k"), "anthropic", "m");
        assert!(!r.ok && r.status == Some(401));
        assert!(seen.recv().unwrap().contains(r#""thinking":{"type":"disabled"}"#));
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    }

    fn http(status: &str, extra: &str, body: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).into_bytes()
    }

    #[test]
    fn list_models_shapes() {
        let ok = |body: &'static str| list_models(&serve(200, body), Some("k"), "chat");
        assert_eq!(ok(r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"},{"object":"x"}]}"#).unwrap(), ["a", "b"]);
        assert_eq!(ok(r#"{"models":[{"name":"x"}]}"#).unwrap(), ["x"]);
        assert_eq!(ok(r#"{"data":[]}"#).unwrap(), Vec::<String>::new());
        assert!(ok(r#"{"data":{}}"#).unwrap_err().contains("没有模型列表"));
        assert!(ok("<html>").unwrap_err().contains("不是 JSON"));
        assert!(list_models(&serve(404, "{}"), None, "chat").unwrap_err().contains("404"));
        assert!(list_models(&serve(403, "{}"), None, "chat").unwrap_err().contains("密钥无效"));
        assert!(list_models(&serve(500, "{}"), None, "chat").unwrap_err().contains("500"));
    }

    #[test]
    fn gemini_models_use_native_auth_and_pagination() {
        let (port, seen) = serve_with(2, |head| {
            let body = if head.starts_with("GET /relay/v1beta/models?pageToken=next%2Bpage%26x") {
                r#"{"models":[{"name":"models/gemini-b"},{"name":"models/gemini-a"}]}"#
            } else {
                r#"{"models":[{"name":"models/gemini-a"}],"nextPageToken":"next+page&x"}"#
            };
            http("200 OK", "", body)
        });
        assert_eq!(list_models(&format!("http://127.0.0.1:{port}/relay"), Some("gem-key"), "gemini").unwrap(), ["gemini-a", "gemini-b"]);
        for path in ["/relay/v1beta/models", "/relay/v1beta/models?pageToken=next%2Bpage%26x"] {
            let req = seen.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(req.starts_with(&format!("GET {path} HTTP/1.1")), "{req}");
            assert!(req.contains("x-goog-api-key: gem-key") && !req.contains("authorization:") && !req.contains("x-api-key:"), "{req}");
        }
    }

    #[test]
    fn gemini_test_uses_generate_content_and_reads_native_reply() {
        let (port, seen) = serve_with(1, |_| http("200 OK", "", r#"{"candidates":[{"content":{"parts":[{"text":"private thought","thought":true},{"text":"po"},{"text":"ng"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":2}}"#));
        let r = test_call(&format!("http://127.0.0.1:{port}/v1beta/"), Some("gem-key"), "gemini", "models/gemini-test");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.usage == Some((9, 2)), "{r:?}");
        assert!(r.url.ends_with("/v1beta/models/gemini-test:generateContent"), "{}", r.url);
        let req = seen.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(req.starts_with("POST /v1beta/models/gemini-test:generateContent HTTP/1.1"), "{req}");
        assert!(req.contains("x-goog-api-key: gem-key") && !req.contains("authorization:"), "{req}");
        let body: serde_json::Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "Reply with exactly one word: pong");
        assert!(body["generationConfig"]["maxOutputTokens"].as_u64().unwrap() > 0);
        assert!(body.get("input").is_none() && body.get("messages").is_none() && body.get("model").is_none());
    }

    #[test]
    fn gemini_urls_keep_versions_prefixes_and_model_boundaries() {
        for (base, path) in [
            ("https://example.com", "/v1beta/models"),
            ("https://example.com/", "/v1beta/models"),
            ("https://example.com/v1/", "/v1/models"),
            ("https://example.com/proxy/v1beta", "/proxy/v1beta/models"),
            ("https://example.com/proxy/v1alpha", "/proxy/v1alpha/models"),
        ] {
            assert_eq!(gemini_url(base, None).unwrap().path(), path);
            for model in ["gemini-test", "models/gemini-test"] {
                let url = gemini_url(base, Some(model)).unwrap();
                assert_eq!(url.path(), format!("{path}/gemini-test:generateContent"));
            }
        }
        let url = gemini_url("https://example.com/proxy", Some("m?x#y")).unwrap();
        assert_eq!(url.path(), "/proxy/v1beta/models/m%3Fx%23y:generateContent");
        assert!(url.query().is_none() && url.fragment().is_none());
        for model in ["", "models/", "..", "../other", "models/a/b"] {
            let r = test_call("https://example.com", Some("k"), "gemini", model);
            assert!(!r.ok && r.status.is_none() && r.error.unwrap().contains("模型名"));
        }
        assert!(gemini_url("file:///tmp/config", None).is_err());
        assert!(gemini_url("not a URL", None).is_err());
        let (port, seen) = serve_with(1, |_| http("200 OK", "", r#"{"models":[]}"#));
        assert!(list_models(&format!("http://127.0.0.1:{port}/v1"), None, "gemini").unwrap().is_empty());
        let req = seen.recv().unwrap();
        assert!(req.starts_with("GET /v1/models HTTP/1.1") && !req.contains("x-goog-api-key") && !req.contains("authorization:"), "{req}");
    }

    #[test]
    fn gemini_pagination_refuses_loops_and_later_page_failures() {
        let (port, seen) = serve_with(2, |_| http("200 OK", "", r#"{"models":[],"nextPageToken":"repeat"}"#));
        let err = list_models(&format!("http://127.0.0.1:{port}?pageSize=1&pageToken=initial"), None, "gemini").unwrap_err();
        assert!(err.contains("分页"), "{err}");
        seen.recv().unwrap();
        assert!(seen.recv().unwrap().starts_with("GET /v1beta/models?pageSize=1&pageToken=repeat HTTP/1.1"));
        let (port, _) = serve_with(2, |req| {
            if req.contains("pageToken=") { http("403 Forbidden", "", r#"{"error":{"message":"no permission"}}"#) }
            else { http("200 OK", "", r#"{"models":[{"name":"models/partial"}],"nextPageToken":"next"}"#) }
        });
        let err = list_models(&format!("http://127.0.0.1:{port}"), Some("k"), "gemini").unwrap_err();
        assert!(err.contains("403"), "partial catalogs must not be reported as complete");
    }

    #[test]
    fn gemini_test_falls_back_and_reports_blocked_or_empty_generation() {
        let (port, seen) = serve_with(2, |req| {
            if req.contains("thinkingConfig") { http("400 Bad Request", "", r#"{"error":{"message":"thinking cannot be disabled"}}"#) }
            else { http("200 OK", "", r#"{"candidates":[{"content":{"parts":[{"text":"pong"}]}}]}"#) }
        });
        let r = test_call(&format!("http://127.0.0.1:{port}"), Some("k"), "gemini", "gemini-test");
        assert!(r.ok && r.reply.as_deref() == Some("pong"), "{r:?}");
        assert!(seen.recv().unwrap().starts_with("POST /v1beta/models/gemini-test:generateContent HTTP/1.1"));
        let second = seen.recv().unwrap();
        assert!(!second.contains("thinkingConfig") && second.contains("\"maxOutputTokens\":1024"), "{second}");
        for (body, message) in [
            (r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#, "SAFETY"),
            (r#"{"candidates":[{"finishReason":"RECITATION","content":{"parts":[{"text":"partial"}]}}]}"#, "RECITATION"),
            (r#"{"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"thought","thought":true}]}}]}"#, "MAX_TOKENS"),
            (r#"{"error":{"message":"quota exhausted"}}"#, "quota exhausted"),
            (r#"{"candidates":[]}"#, "没有文本"),
        ] {
            let r = test_call(&serve(200, body), Some("k"), "gemini", "m");
            assert!(!r.ok && r.error.as_deref().unwrap().contains(message), "{r:?}");
        }
        let r = test_call(&serve(403, r#"{"error":{"message":"API key not valid"}}"#), Some("k"), "gemini", "m");
        assert!(!r.ok && r.status == Some(403) && r.error.unwrap().contains("API key not valid"));
    }

    #[test]
    fn gemini_keys_never_follow_cross_origin_redirects() {
        for listing in [true, false] {
            let (other, seen) = serve_with(1, |_| http("200 OK", "", "{}"));
            let (port, first) = serve_with(1, move |_| http("307 Temporary Redirect", &format!("location: http://127.0.0.1:{other}/v1beta/models\r\n"), ""));
            let base = format!("http://127.0.0.1:{port}");
            let err = if listing { list_models(&base, Some("gem-secret"), "gemini").unwrap_err() }
                else { test_call(&base, Some("gem-secret"), "gemini", "m").error.unwrap() };
            assert!(err.contains("307"), "{err}");
            assert!(first.recv().unwrap().contains("x-goog-api-key: gem-secret"));
            assert!(seen.recv_timeout(Duration::from_millis(100)).is_err(), "the redirect target must not receive the key");
        }
    }

    #[test]
    fn keys_never_follow_a_redirect_to_another_host() {
        let (other, seen) = serve_with(1, |_| http("200 OK", "", r#"{"data":[{"id":"stolen"}]}"#));
        let (port, first) = serve_with(1, move |_| http("307 Temporary Redirect", &format!("location: http://localhost:{other}/v1/models\r\n"), ""));
        let err = list_models(&format!("http://127.0.0.1:{port}/v1"), Some("sk-secret"), "anthropic").unwrap_err();
        assert!(err.contains("307") && err.contains(&format!("localhost:{other}")), "{err}");
        assert!(first.recv().unwrap().contains("x-api-key: sk-secret"));
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "the other host was contacted");
    }

    #[test]
    fn redirects_within_one_origin_are_followed() {
        let (port, seen) = serve_with(2, |head| {
            if head.starts_with("GET /v1/models") {
                http("301 Moved Permanently", "location: /v2/models\r\n", "")
            } else {
                http("200 OK", "", r#"{"data":[{"id":"m"}]}"#)
            }
        });
        assert_eq!(list_models(&format!("http://127.0.0.1:{port}/v1"), Some("k"), "anthropic").unwrap(), ["m"]);
        seen.recv().unwrap();
        let second = seen.recv().unwrap();
        assert!(second.starts_with("GET /v2/models") && second.contains("x-api-key: k"), "{second}");
    }

    #[test]
    fn redirects_to_another_port_are_not_followed() {
        let (other, seen) = serve_with(1, |_| http("200 OK", "", r#"{"data":[]}"#));
        let (port, _) = serve_with(1, move |_| http("308 Permanent Redirect", &format!("location: http://127.0.0.1:{other}/v1/models\r\n"), ""));
        let r = test_call(&format!("http://127.0.0.1:{port}/v1"), Some("k"), "anthropic", "m");
        assert!(!r.ok && r.status == Some(308) && r.error.as_deref().unwrap().contains("重定向"), "{:?}", r.error);
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn oversized_bodies_are_refused() {
        let big = "a".repeat((MAX_TEST_BODY + 10) as usize);
        let (port, _) = serve_with(1, move |_| http("200 OK", "", &big));
        let r = test_call(&format!("http://127.0.0.1:{port}/v1"), None, "chat", "m");
        assert!(!r.ok && r.error.as_deref().unwrap().contains("响应太大"), "{:?}", r.error);
    }

    #[test]
    fn stream_replies_to_a_non_stream_test_are_read() {
        let sse = |body: &str| {
            let body = body.to_string();
            let (port, _) = serve_with(1, move |_| http("200 OK", "content-type: text/event-stream\r\n", &body));
            format!("http://127.0.0.1:{port}/v1")
        };
        // Responses: text from the deltas, usage from response.completed (CRLF lines).
        let base = sse(concat!(
            "event: response.created\r\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"output\":[]}}\r\n\r\n",
            "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"po\"}\r\n\r\n",
            "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ng\"}\r\n\r\n",
            "event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"output\":[],\"usage\":{\"input_tokens\":12,\"output_tokens\":2}}}\r\n\r\n",
        ));
        let r = test_call(&base, Some("k"), "responses", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.usage == Some((12, 2)), "{r:?}");
        // Chat: deltas plus the usage chunk, then [DONE].
        let base = sse(concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ));
        let r = test_call(&base, None, "chat", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.usage == Some((9, 1)), "{r:?}");
        // Anthropic: input from message_start, output from message_delta.
        let base = sse(concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3}}\n\n",
        ));
        let r = test_call(&base, Some("k"), "anthropic", "m");
        assert!(r.ok && r.reply.as_deref() == Some("pong") && r.usage == Some((7, 3)), "{r:?}");
        // An error inside the stream fails the test with its message.
        let base = sse("event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"model overloaded\"}}}\n\n");
        let r = test_call(&base, Some("k"), "responses", "m");
        assert!(!r.ok && r.error.as_deref().unwrap().contains("model overloaded"), "{r:?}");
        // Plain text is still reported as not JSON.
        let r = test_call(&sse("hello"), None, "chat", "m");
        assert!(!r.ok && r.error.as_deref().unwrap().contains("不是 JSON"), "{r:?}");
    }

    /// A keyless Anthropic-style endpoint (a local proxy) still gets `anthropic-version`,
    /// for the model list and the test call alike, as it does through the gateway.
    #[test]
    fn keyless_anthropic_gets_the_version_header() {
        let (port, seen) = serve_with(2, |head| {
            if head.starts_with("GET") {
                http("200 OK", "", r#"{"data":[{"id":"m"}]}"#)
            } else {
                http("200 OK", "", r#"{"content":[{"type":"text","text":"pong"}]}"#)
            }
        });
        let base = format!("http://127.0.0.1:{port}/v1");
        assert_eq!(list_models(&base, None, "anthropic").unwrap(), ["m"]);
        assert!(test_call(&base, None, "anthropic", "m").ok);
        for _ in 0..2 {
            let head = seen.recv().unwrap().to_ascii_lowercase();
            assert!(head.contains(&format!("anthropic-version: {ANTHROPIC_VERSION}")) && !head.contains("x-api-key"), "{head}");
        }
        // Other protocols get neither.
        let (port, seen) = serve_with(1, |_| http("200 OK", "", r#"{"data":[]}"#));
        list_models(&format!("http://127.0.0.1:{port}/v1"), None, "chat").unwrap();
        let head = seen.recv().unwrap().to_ascii_lowercase();
        assert!(!head.contains("anthropic-version") && !head.contains("authorization"), "{head}");
    }
}
