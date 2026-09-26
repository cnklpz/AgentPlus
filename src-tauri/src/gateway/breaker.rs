//! Error circuit breaker, one per forward. After `threshold` upstream failures in a row
//! (cannot connect, 5xx, 429, 401/402/403, a broken stream…) the forward is paused: requests
//! get a 503 that says which error tripped it, and the unified entry skips it. When the pause
//! is over one request is let through as a probe; success closes the breaker, another
//! failure pauses it again for twice as long (up to ten minutes).

use super::{clip, convert, lock};
use crate::i18n::l;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const MAX_COOLDOWN: u64 = 600;
/// Longest first pause a config may ask for.
const MAX_COOLDOWN_SETTING: u64 = 3600;
/// A probe that never reports back (e.g. a stream still running) frees its slot after this.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Missing fields take their value from `Default`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub enabled: bool,
    /// Failures in a row that pause a forward.
    pub threshold: u32,
    /// First pause, in seconds; doubles on every trip in a row.
    pub cooldown_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, threshold: 3, cooldown_secs: 60 }
    }
}

impl Config {
    /// Limits in range (store.json may have been edited by hand).
    pub fn clamped(self) -> Self {
        Config { threshold: self.threshold.clamp(1, 100), cooldown_secs: self.cooldown_secs.clamp(5, MAX_COOLDOWN_SETTING), ..self }
    }
}

#[derive(Default)]
struct State {
    /// Failures in a row since the last success.
    fails: u32,
    /// Trips in a row (sets the pause length).
    trips: u32,
    open_until: Option<Instant>,
    /// The probe under way: when it was let through, and its ticket.
    probing: Option<(Instant, u64)>,
    /// Error that tripped the breaker (or the latest one while it is still closed).
    reason: Option<String>,
    /// Wall-clock time of the trip, for display.
    at: Option<String>,
    paused_secs: u64,
}

fn states() -> MutexGuard<'static, HashMap<String, State>> {
    static S: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    lock(S.get_or_init(|| Mutex::new(HashMap::new())))
}

/// What `admit` handed a call; `record` needs it back. Only the current probe (or AgentPlus's
/// own test) can close or re-pause a tripped forward: calls that were already in flight when
/// it tripped report late and are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticket(u64);

impl Ticket {
    /// AgentPlus's own test: skips `admit` and counts like a probe.
    pub const TEST: Ticket = Ticket(u64::MAX);

    fn probe() -> Ticket {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ticket(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    fn is_probe_of(self, s: &State) -> bool {
        self == Ticket::TEST || (self.0 != 0 && s.probing.is_some_and(|(_, t)| t == self.0))
    }
}

fn probing(s: &State) -> bool {
    s.probing.is_some_and(|(at, _)| at.elapsed() < PROBE_TIMEOUT)
}

/// Tripped and still paused: the pause isn't over, or a probe is under way.
fn paused(s: &State) -> bool {
    s.open_until.is_some_and(|u| Instant::now() < u || probing(s))
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
    let why = s.reason.as_deref().unwrap_or(l("upstream error", "上游出错"));
    if !left.is_zero() {
        let left = left.as_secs() + 1;
        tr!(
            "Forward \"{id}\" is paused after repeated errors (circuit breaker): {why}. It retries automatically in about {left} s; you can also resume it now in AgentPlus › Local gateway.",
            "转发「{id}」因连续出错已暂停（熔断）：{why}。约 {left} 秒后自动重试；也可以在 AgentPlus「本地网关」里立即恢复。"
        )
    } else {
        // Pause over, a probe request is on its way.
        tr!(
            "Forward \"{id}\" is paused after repeated errors (circuit breaker): {why}. A trial request is checking whether it has recovered; try again shortly.",
            "转发「{id}」因连续出错已暂停（熔断）：{why}。正在用一个请求试探是否恢复，请稍后再试。"
        )
    }
}

/// Whether the forward is paused right now (does not claim the probe slot).
pub fn is_open(cfg: &Config, id: &str) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let map = states();
    let s = map.get(id)?;
    paused(s).then(|| refusal(id, s))
}

/// Lets a call through, or says why not. Once the pause is over the first caller becomes
/// the probe and the others keep being refused until it reports.
pub fn admit(cfg: &Config, id: &str) -> Result<Ticket, String> {
    if !cfg.enabled {
        return Ok(Ticket::default());
    }
    let mut map = states();
    let Some(s) = map.get_mut(id).filter(|s| s.open_until.is_some()) else { return Ok(Ticket::default()) };
    if paused(s) {
        return Err(refusal(id, s));
    }
    let t = Ticket::probe();
    s.probing = Some((Instant::now(), t.0));
    Ok(t)
}

/// Records a call's outcome. Returns a note for the request log when this call tripped the breaker.
pub fn record(cfg: &Config, id: &str, outcome: Outcome, ticket: Ticket) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let mut map = states();
    // Tripped: only the probe's report counts; stragglers that were already in flight do not.
    let stale = map.get(id).is_some_and(|s| s.open_until.is_some() && !ticket.is_probe_of(s));
    match outcome {
        _ if stale => None,
        Outcome::Ok => {
            map.remove(id);
            None
        }
        Outcome::Neutral => {
            if let Some(s) = map.get_mut(id).filter(|s| s.probing.is_some_and(|(_, t)| t == ticket.0)) {
                s.probing = None;
            }
            None
        }
        Outcome::Fault(why) => {
            let s = map.entry(id.to_string()).or_default();
            // AgentPlus's test while paused does not extend the pause.
            if s.open_until.is_some_and(|u| Instant::now() < u) {
                return None;
            }
            let probe_failed = s.open_until.is_some();
            s.probing = None;
            s.fails += 1;
            s.reason = Some(why);
            if !probe_failed && s.fails < cfg.threshold.max(1) {
                return None;
            }
            s.trips += 1;
            let cfg = cfg.clone().clamped();
            let secs = cfg.cooldown_secs.saturating_mul(1u64 << (s.trips - 1).min(10)).min(MAX_COOLDOWN.max(cfg.cooldown_secs));
            let now = Instant::now();
            s.open_until = Some(now.checked_add(Duration::from_secs(secs)).unwrap_or(now));
            s.paused_secs = secs;
            s.at = Some(chrono::Local::now().format("%H:%M:%S").to_string());
            let n = s.fails;
            s.fails = 0;
            Some(if probe_failed {
                tr!("Recovery probe failed; forward paused again for {secs} s", "恢复试探失败，转发再暂停 {secs} 秒")
            } else {
                tr!("{n} errors in a row; forward paused for {secs} s", "连续 {n} 次出错，转发暂停 {secs} 秒")
            })
        }
    }
}

/// Clears one forward's breaker, or all of them.
pub fn reset(id: Option<&str>) {
    let mut map = states();
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
    let map = states();
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

/// "HTTP 401 invalid key or unauthorized: invalid api key" from a status and the
/// upstream's error body.
pub fn describe(status: u16, body: &str) -> String {
    let label = match status {
        401 => l(" invalid key or unauthorized", " 密钥无效或未授权"),
        402 => l(" insufficient balance", " 余额不足"),
        403 => l(" forbidden", " 没有权限"),
        408 => l(" upstream timeout", " 上游超时"),
        429 => l(" too many requests (rate limited)", " 请求过多（被限流）"),
        502..=504 => l(" upstream unavailable", " 上游不可用"),
        500..=599 => l(" upstream server error", " 上游服务出错"),
        _ => "",
    };
    let msg = brief(body);
    if msg.is_empty() { format!("HTTP {status}{label}") } else { tr!("HTTP {status}{label}: {msg}", "HTTP {status}{label}：{msg}") }
}

/// The message out of an error body (see `convert::error_message`), on one line and
/// shortened. Its own output comes back unchanged.
pub fn brief(body: &str) -> String {
    let body = body.trim();
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| convert::error_message(&v)).unwrap_or_else(|| body.to_string());
    clip(&msg.split_whitespace().collect::<Vec<_>>().join(" "), 160)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_after_threshold_and_recovers() {
        let cfg = Config { enabled: true, threshold: 2, cooldown_secs: 60 };
        let id = "breaker-unit";
        reset(Some(id));
        let t = admit(&cfg, id).unwrap();
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 503".into()), t), None);
        // A success in between starts the count over.
        record(&cfg, id, Outcome::Ok, t);
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 503".into()), t), None);
        let note = record(&cfg, id, Outcome::Fault(describe(401, r#"{"error":{"message":"invalid api key"}}"#)), t);
        assert_eq!(note.as_deref(), Some("连续 2 次出错，转发暂停 60 秒"));
        let why = admit(&cfg, id).unwrap_err();
        assert!(why.contains("HTTP 401 密钥无效或未授权：invalid api key"), "{why}");
        assert!(is_open(&cfg, id).is_some());
        assert_eq!(view(&cfg, id).unwrap().state, "open");

        // Pause over: one probe goes through, the rest wait for it.
        states().get_mut(id).unwrap().open_until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(view(&cfg, id).unwrap().state, "probe");
        let probe = admit(&cfg, id).unwrap();
        assert!(admit(&cfg, id).is_err());
        // Probe fails: paused again, twice as long.
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 502".into()), probe).as_deref(), Some("恢复试探失败，转发再暂停 120 秒"));
        states().get_mut(id).unwrap().open_until = Some(Instant::now() - Duration::from_secs(1));
        let probe = admit(&cfg, id).unwrap();
        record(&cfg, id, Outcome::Ok, probe);
        assert!(view(&cfg, id).is_none());
        assert!(admit(&cfg, id).is_ok());
    }

    /// Calls admitted before the trip report late: they neither close the breaker, count as
    /// the probe failing, nor free the probe slot. Only the probe (or the test) decides.
    #[test]
    fn stragglers_do_not_decide_the_probe() {
        let cfg = Config { enabled: true, threshold: 1, cooldown_secs: 60 };
        let id = "breaker-stragglers";
        reset(Some(id));
        let old = admit(&cfg, id).unwrap();
        assert!(record(&cfg, id, Outcome::Fault("HTTP 503".into()), admit(&cfg, id).unwrap()).is_some());
        // An older call succeeding right after the trip does not close it.
        assert_eq!(record(&cfg, id, Outcome::Ok, old), None);
        assert_eq!(view(&cfg, id).unwrap().state, "open");

        states().get_mut(id).unwrap().open_until = Some(Instant::now() - Duration::from_secs(1));
        let probe = admit(&cfg, id).unwrap();
        // A straggler's fault or neutral outcome during the probe window changes nothing.
        assert_eq!(record(&cfg, id, Outcome::Fault("HTTP 502".into()), old), None);
        record(&cfg, id, Outcome::Neutral, old);
        assert!(admit(&cfg, id).is_err(), "probe slot still taken");
        assert_eq!(view(&cfg, id).unwrap().trips, 1);
        // The probe's own neutral outcome frees the slot for the next caller.
        record(&cfg, id, Outcome::Neutral, probe);
        let probe = admit(&cfg, id).unwrap();
        record(&cfg, id, Outcome::Ok, probe);
        assert!(view(&cfg, id).is_none());

        // AgentPlus's test closes a paused forward.
        assert!(record(&cfg, id, Outcome::Fault("HTTP 503".into()), Ticket::default()).is_some());
        record(&cfg, id, Outcome::Ok, Ticket::TEST);
        assert!(view(&cfg, id).is_none());
    }

    /// A huge cooldown from a hand-edited store.json neither panics nor poisons the lock.
    #[test]
    fn huge_cooldown_is_clamped() {
        let cfg = Config { enabled: true, threshold: 1, cooldown_secs: u64::MAX };
        let id = "breaker-huge";
        reset(Some(id));
        let note = record(&cfg, id, Outcome::Fault("x".into()), Ticket::default()).unwrap();
        assert!(note.contains("3600"), "{note}");
        assert_eq!(cfg.clone().clamped().cooldown_secs, 3600);
        assert!(admit(&cfg, id).is_err());
        reset(Some(id));
    }

    #[test]
    fn disabled_breaker_never_trips() {
        let cfg = Config { enabled: false, threshold: 1, cooldown_secs: 60 };
        for _ in 0..5 {
            assert_eq!(record(&cfg, "breaker-off", Outcome::Fault("x".into()), Ticket::default()), None);
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
        // The message of a long JSON body, never a fragment of the JSON itself.
        let long = format!(r#"{{"error":{{"message":"{}","type":"x"}}}}"#, "too long ".repeat(50));
        let b = brief(&long);
        assert!(b.starts_with("too long") && b.ends_with('…') && b.chars().count() == 161, "{b}");
        assert_eq!(brief(&b), b);
        assert_eq!(describe(402, &b), format!("HTTP 402 余额不足：{b}"));
        assert_eq!(brief(r#"{"error":{"message":""},"detail":"d"}"#), "d");
    }

    /// Missing config fields take the defaults.
    #[test]
    fn config_defaults() {
        let c: Config = serde_json::from_str(r#"{"threshold": 5}"#).unwrap();
        assert!(c.enabled);
        assert_eq!((c.threshold, c.cooldown_secs), (5, 60));
        let d: Config = serde_json::from_str("{}").unwrap();
        assert_eq!((d.enabled, d.threshold, d.cooldown_secs), (true, 3, 60));
    }
}
