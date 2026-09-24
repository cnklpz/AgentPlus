//! Error circuit breaker, one per forward. After `threshold` upstream failures in a row
//! (cannot connect, 5xx, 429, 401/402/403, a broken stream…) the forward is paused: requests
//! get a 503 that says which error tripped it, and the unified entry skips it. When the pause
//! is over one request is let through as a probe; success closes the breaker, another
//! failure pauses it again for twice as long (up to ten minutes).

use crate::i18n::l;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_COOLDOWN: u64 = 600;
/// A probe that never reports back (e.g. a stream still running) frees its slot after this.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Failures in a row that pause a forward.
    #[serde(default = "three")]
    pub threshold: u32,
    /// First pause, in seconds; doubles on every trip in a row.
    #[serde(default = "sixty")]
    pub cooldown_secs: u64,
}

fn yes() -> bool {
    true
}
fn three() -> u32 {
    3
}
fn sixty() -> u64 {
    60
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, threshold: 3, cooldown_secs: 60 }
    }
}

#[derive(Default)]
struct State {
    /// Failures in a row since the last success.
    fails: u32,
    /// Trips in a row (sets the pause length).
    trips: u32,
    open_until: Option<Instant>,
    probing: Option<Instant>,
    /// Error that tripped the breaker (or the latest one while it is still closed).
    reason: Option<String>,
    /// Wall-clock time of the trip, for display.
    at: Option<String>,
    paused_secs: u64,
}

fn states() -> &'static Mutex<HashMap<String, State>> {
    static S: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What happened to one call, as far as the breaker is concerned.
pub enum Outcome {
    /// The upstream answered normally (a 4xx about the request itself counts too).
    Ok,
    /// Upstream-side failure, with a short description.
    Fault(String),
    /// Nothing learned about the upstream (the client's request was bad, …).
    Neutral,
}

/// Why a paused forward refused a call.
fn refusal(id: &str, s: &State) -> String {
    let left = s.open_until.map(|u| u.saturating_duration_since(Instant::now())).unwrap_or_default();
    let why = s.reason.as_deref().unwrap_or(l("上游出错", "upstream error"));
    if !left.is_zero() {
        let left = left.as_secs() + 1;
        tr!(
            "转发「{id}」因连续出错已暂停（熔断）：{why}。约 {left} 秒后自动重试；也可以在 AgentPlus「本地网关」里立即恢复。",
            "Forward \"{id}\" is paused after repeated errors (circuit breaker): {why}. It retries automatically in about {left} s; you can also resume it now in AgentPlus › Local gateway."
        )
    } else {
        // Pause over, a probe request is on its way.
        tr!(
            "转发「{id}」因连续出错已暂停（熔断）：{why}。正在用一个请求试探是否恢复，请稍后再试。",
            "Forward \"{id}\" is paused after repeated errors (circuit breaker): {why}. A trial request is checking whether it has recovered; try again shortly."
        )
    }
}

/// Whether the forward is paused right now (does not claim the probe slot).
pub fn is_open(cfg: &Config, id: &str) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let map = states().lock().unwrap();
    let s = map.get(id)?;
    let until = s.open_until?;
    let paused = Instant::now() < until || s.probing.is_some_and(|p| p.elapsed() < PROBE_TIMEOUT);
    paused.then(|| refusal(id, s))
}

/// Lets a call through, or says why not. Once the pause is over the first caller becomes
/// the probe and the others keep being refused until it reports.
pub fn admit(cfg: &Config, id: &str) -> Result<(), String> {
    if !cfg.enabled {
        return Ok(());
    }
    let mut map = states().lock().unwrap();
    let Some(s) = map.get_mut(id) else { return Ok(()) };
    let Some(until) = s.open_until else { return Ok(()) };
    if Instant::now() < until || s.probing.is_some_and(|p| p.elapsed() < PROBE_TIMEOUT) {
        return Err(refusal(id, s));
    }
    s.probing = Some(Instant::now());
    Ok(())
}

/// Records a call's outcome. Returns a note for the request log when this call tripped the breaker.
pub fn record(cfg: &Config, id: &str, outcome: Outcome) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let mut map = states().lock().unwrap();
    match outcome {
        Outcome::Ok => {
            map.remove(id);
            None
        }
        Outcome::Neutral => {
            if let Some(s) = map.get_mut(id) {
                s.probing = None;
            }
            None
        }
        Outcome::Fault(why) => {
            let s = map.entry(id.to_string()).or_default();
            // While paused, stragglers that were already in flight do not extend the pause.
            if s.open_until.is_some_and(|u| Instant::now() < u) {
                return None;
            }
            let probe_failed = s.probing.take().is_some();
            s.fails += 1;
            s.reason = Some(why);
            if !probe_failed && s.fails < cfg.threshold.max(1) {
                return None;
            }
            s.trips += 1;
            let secs = cfg.cooldown_secs.max(5).saturating_mul(1u64 << (s.trips - 1).min(10)).min(MAX_COOLDOWN.max(cfg.cooldown_secs));
            s.open_until = Some(Instant::now() + Duration::from_secs(secs));
            s.paused_secs = secs;
            s.at = Some(chrono::Local::now().format("%H:%M:%S").to_string());
            let n = s.fails;
            s.fails = 0;
            Some(if probe_failed {
                tr!("恢复试探失败，转发再暂停 {secs} 秒", "Recovery probe failed; forward paused again for {secs} s")
            } else {
                tr!("连续 {n} 次出错，转发暂停 {secs} 秒", "{n} errors in a row; forward paused for {secs} s")
            })
        }
    }
}

/// Clears one forward's breaker, or all of them.
pub fn reset(id: Option<&str>) {
    let mut map = states().lock().unwrap();
    match id {
        Some(id) => {
            map.remove(id);
        }
        None => map.clear(),
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// "open" (paused) | "probe" (pause over, waiting for / running a trial request) | "closed".
    pub state: &'static str,
    pub remaining_secs: u64,
    /// Length of the current pause.
    pub paused_secs: u64,
    pub reason: Option<String>,
    /// When it tripped (HH:MM:SS).
    pub at: Option<String>,
    /// Failures in a row so far (closed state).
    pub fails: u32,
    pub trips: u32,
}

pub fn view(cfg: &Config, id: &str) -> Option<View> {
    if !cfg.enabled {
        return None;
    }
    let map = states().lock().unwrap();
    let s = map.get(id)?;
    let now = Instant::now();
    let state = match s.open_until {
        Some(u) if now < u => "open",
        Some(_) => "probe",
        None => "closed",
    };
    Some(View {
        state,
        remaining_secs: s.open_until.map(|u| u.saturating_duration_since(now).as_secs()).unwrap_or(0),
        paused_secs: s.paused_secs,
        reason: s.reason.clone(),
        at: s.at.clone(),
        fails: s.fails,
        trips: s.trips,
    })
}

/// Statuses that say the upstream (or the account on it) is in trouble, not the request.
pub fn is_fault_status(status: u16) -> bool {
    status >= 500 || matches!(status, 401 | 402 | 403 | 408 | 429)
}

/// "HTTP 401 密钥无效：invalid api key" from a status and the upstream's error body.
pub fn describe(status: u16, body: &str) -> String {
    let label = match status {
        401 => l(" 密钥无效或未授权", " invalid key or unauthorized"),
        402 => l(" 余额不足", " insufficient balance"),
        403 => l(" 没有权限", " forbidden"),
        408 => l(" 上游超时", " upstream timeout"),
        429 => l(" 请求过多（被限流）", " too many requests (rate limited)"),
        502..=504 => l(" 上游不可用", " upstream unavailable"),
        500..=599 => l(" 上游服务出错", " upstream server error"),
        _ => "",
    };
    let msg = brief(body);
    if msg.is_empty() { format!("HTTP {status}{label}") } else { tr!("HTTP {status}{label}：{msg}", "HTTP {status}{label}: {msg}") }
}

/// The message out of an error body (JSON `error.message`, `message`, …), shortened.
pub fn brief(body: &str) -> String {
    let body = body.trim();
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            [v.pointer("/error/message"), v.get("message"), v.get("error"), v.get("detail")]
                .into_iter()
                .flatten()
                .find_map(|m| m.as_str().map(String::from))
        })
        .unwrap_or_else(|| body.to_string());
    let msg = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = msg.chars().take(160).collect();
    if msg.chars().count() > 160 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_after_threshold_and_recovers() {
        let cfg = Config { enabled: true, threshold: 2, cooldown_secs: 60 };
        let id = "breaker-unit";
        reset(Some(id));
        assert!(admit(&cfg, id).is_ok());
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 503".into())), None);
        // A success in between starts the count over.
        record(&cfg, id, Outcome::Ok);
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 503".into())), None);
        let note = record(&cfg, id, Outcome::Fault(describe(401, r#"{"error":{"message":"invalid api key"}}"#)));
        assert_eq!(note.as_deref(), Some("连续 2 次出错，转发暂停 60 秒"));
        let why = admit(&cfg, id).unwrap_err();
        assert!(why.contains("HTTP 401 密钥无效或未授权：invalid api key"), "{why}");
        assert!(is_open(&cfg, id).is_some());
        assert_eq!(view(&cfg, id).unwrap().state, "open");

        // Pause over: one probe goes through, the rest wait for it.
        states().lock().unwrap().get_mut(id).unwrap().open_until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(view(&cfg, id).unwrap().state, "probe");
        assert!(admit(&cfg, id).is_ok());
        assert!(admit(&cfg, id).is_err());
        // Probe fails: paused again, twice as long.
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 502".into())).as_deref(), Some("恢复试探失败，转发再暂停 120 秒"));
        states().lock().unwrap().get_mut(id).unwrap().open_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(admit(&cfg, id).is_ok());
        record(&cfg, id, Outcome::Ok);
        assert!(view(&cfg, id).is_none());
        assert!(admit(&cfg, id).is_ok());
    }

    #[test]
    fn disabled_breaker_never_trips() {
        let cfg = Config { enabled: false, threshold: 1, cooldown_secs: 60 };
        for _ in 0..5 {
            assert_eq!(record(&cfg, "breaker-off", Outcome::Fault("x".into())), None);
        }
        assert!(admit(&cfg, "breaker-off").is_ok());
    }

    #[test]
    fn classifies_and_describes() {
        assert!(is_fault_status(503) && is_fault_status(429) && is_fault_status(401));
        assert!(!is_fault_status(400) && !is_fault_status(404));
        assert_eq!(describe(429, "rate limited"), "HTTP 429 请求过多（被限流）：rate limited");
        assert_eq!(describe(500, ""), "HTTP 500 上游服务出错");
        assert_eq!(brief(r#"{"message":"quota\n exceeded"}"#), "quota exceeded");
    }
}
