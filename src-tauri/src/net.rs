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
    let t0 = Instant::now();
    client()?
        .get(models_url(base_url))
        .timeout(Duration::from_secs(8))
        .send()
        .map_err(|e| if e.is_timeout() { "超时".to_string() } else { "连接失败".to_string() })?;
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
    let resp = req.send().map_err(|e| if e.is_timeout() { "请求超时".to_string() } else { format!("连接失败：{e}") })?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "密钥无效或没有权限（HTTP ".to_string() + status.as_str() + "）",
            404 => "这个地址没有 /models 接口（HTTP 404）".to_string(),
            _ => format!("HTTP {status}"),
        });
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "返回的不是 JSON".to_string())?;
    let list = v.get("data").or_else(|| v.get("models")).and_then(|d| d.as_array()).ok_or("返回里没有模型列表")?;
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(|x| x.as_str()).map(String::from))
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}
