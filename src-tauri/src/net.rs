//! Provider network calls: latency and model listing.

use std::time::{Duration, Instant};

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| e.to_string())
}

fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

/// Time to first response header of `GET <base>/models`. Any HTTP status counts
/// (an unauthenticated 401 still measures the round trip).
pub fn latency(base_url: &str) -> Result<u64, String> {
    let mut req = client()?.get(models_url(base_url)).timeout(Duration::from_secs(8));
    // The local gateway refuses requests without a key; its test key lets the probe through
    // to the upstream, so the time still covers the upstream. Only sent to this machine.
    let local = url::Url::parse(base_url).ok().and_then(|u| u.host_str().map(|h| h == "127.0.0.1" || h == "localhost"));
    if local == Some(true) {
        req = req.bearer_auth(crate::gateway::server::TEST_KEY);
    }
    let t0 = Instant::now();
    req.send()
        .map_err(|e| if e.is_timeout() { crate::i18n::l("超时", "Timed out").to_string() } else { crate::i18n::l("连接失败", "Connection failed").to_string() })?;
    Ok(t0.elapsed().as_millis() as u64)
}

/// Lists model ids from `GET <base>/models` (OpenAI and Anthropic shapes).
pub fn list_models(base_url: &str, key: Option<&str>, api: &str) -> Result<Vec<String>, String> {
    let mut req = client()?.get(models_url(base_url));
    if let Some(k) = key {
        req = if api == "anthropic" {
            req.header("x-api-key", k).header("anthropic-version", "2023-06-01")
        } else {
            req.bearer_auth(k)
        };
    }
    let resp = req.send().map_err(|e| if e.is_timeout() { crate::i18n::l("请求超时", "Request timed out").to_string() } else { tr!("连接失败：{e}", "Connection failed: {e}") })?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => tr!("密钥无效或没有权限（HTTP {}）", "Invalid API key or no permission (HTTP {})", status.as_str()),
            404 => crate::i18n::l("这个地址没有 /models 接口（HTTP 404）", "This URL has no /models endpoint (HTTP 404)").to_string(),
            _ => format!("HTTP {status}"),
        });
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| crate::i18n::l("返回的不是 JSON", "Response is not JSON").to_string())?;
    let list = v.get("data").or_else(|| v.get("models")).and_then(|d| d.as_array()).ok_or(crate::i18n::l("返回里没有模型列表", "No model list in the response"))?;
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(|x| x.as_str()).map(String::from))
        .collect();
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

fn short(s: &str, n: usize) -> String {
    let t: String = s.trim().chars().take(n).collect();
    if s.trim().chars().count() > n { format!("{t}…") } else { t }
}

/// Sends one tiny real request with the provider's key and model, so the address,
/// key, protocol and model are all checked (the request uses a few tokens).
pub fn test_call(base_url: &str, key: Option<&str>, api: &str, model: &str) -> TestResult {
    let base = base_url.trim_end_matches('/');
    let prompt = "Reply with exactly one word: pong";
    let (url, body) = match api {
        "anthropic" => (
            format!("{base}/messages"),
            serde_json::json!({ "model": model, "max_tokens": 32, "messages": [{ "role": "user", "content": prompt }] }),
        ),
        "chat" => (
            format!("{base}/chat/completions"),
            serde_json::json!({ "model": model, "max_tokens": 32, "stream": false, "messages": [{ "role": "user", "content": prompt }] }),
        ),
        _ => (
            format!("{base}/responses"),
            serde_json::json!({ "model": model, "input": prompt, "max_output_tokens": 64, "stream": false }),
        ),
    };
    let mut r = TestResult { ok: false, status: None, ms: 0, model: model.into(), url: url.clone(), reply: None, error: None, usage: None };
    let client = match reqwest::blocking::Client::builder().timeout(Duration::from_secs(45)).build() {
        Ok(c) => c,
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    let mut req = client.post(&url).header("content-type", "application/json").body(body.to_string());
    if let Some(k) = key {
        req = if api == "anthropic" { req.header("x-api-key", k).header("anthropic-version", "2023-06-01") } else { req.bearer_auth(k) };
    }
    let t0 = Instant::now();
    let resp = match req.send() {
        Ok(x) => x,
        Err(e) => {
            r.ms = t0.elapsed().as_millis() as u64;
            r.error = Some(if e.is_timeout() { crate::i18n::l("请求超时（45 秒）", "Request timed out (45 s)").into() } else if e.is_connect() { crate::i18n::l("连接失败：地址不可达", "Connection failed: URL unreachable").into() } else { tr!("请求失败：{e}", "Request failed: {e}") });
            return r;
        }
    };
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    r.ms = t0.elapsed().as_millis() as u64;
    r.status = Some(status.as_u16());
    let v: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        let msg = v
            .as_ref()
            .and_then(|v| v.pointer("/error/message").or_else(|| v.get("message")).or_else(|| v.get("error")).or_else(|| v.get("detail")))
            .map(|m| m.as_str().map(String::from).unwrap_or_else(|| m.to_string()))
            .unwrap_or_else(|| short(&text, 160));
        let hint = match status.as_u16() {
            401 | 403 => crate::i18n::l("密钥无效或没有权限", "Invalid API key or no permission"),
            404 => crate::i18n::l("地址或接口类型不对，或者没有这个模型", "Wrong base URL or API type, or no such model"),
            429 => crate::i18n::l("请求太频繁或额度用完", "Too many requests or quota exhausted"),
            400 | 422 => crate::i18n::l("请求被拒绝，可能是模型名不对或接口类型不匹配", "Request rejected; the model name may be wrong or the API type mismatched"),
            s if s >= 500 => crate::i18n::l("服务端出错", "Server error"),
            _ => crate::i18n::l("请求失败", "Request failed"),
        };
        r.error = Some(if msg.is_empty() { tr!("{hint}（HTTP {status}）", "{hint} (HTTP {status})") } else { tr!("{hint}（HTTP {}）：{}", "{hint} (HTTP {}): {}", status.as_u16(), short(&msg, 200)) });
        return r;
    }
    let Some(v) = v else {
        r.error = Some(tr!("返回的不是 JSON：{}", "Response is not JSON: {}", short(&text, 120)));
        return r;
    };
    let reply = match api {
        "anthropic" => v.get("content").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<String>()),
        "chat" => v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).map(String::from),
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
    r.reply = reply.filter(|s| !s.trim().is_empty()).map(|s| short(&s, 80));
    let u = v.get("usage");
    let num = |k: &[&str]| k.iter().find_map(|k| u.and_then(|u| u.get(*k)).and_then(|x| x.as_u64()));
    if let (Some(i), Some(o)) = (num(&["input_tokens", "prompt_tokens"]), num(&["output_tokens", "completion_tokens"])) {
        r.usage = Some((i, o));
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
}
