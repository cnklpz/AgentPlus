//! Provider latency: time to first response header of `GET <base>/models`.
//! Any HTTP status counts (an unauthenticated 401 still measures the round trip).

use std::time::{Duration, Instant};

pub fn latency(base_url: &str) -> Result<u64, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    client.get(&url).send().map_err(|e| {
        if e.is_timeout() { "超时".to_string() } else { "连接失败".to_string() }
    })?;
    Ok(t0.elapsed().as_millis() as u64)
}
