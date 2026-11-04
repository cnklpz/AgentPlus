//! Completion notifications: when an agent finishes a turn, push a message to the user's
//! phone or another service (Bark, ntfy, ServerChan, a webhook).
//!
//! - `watch` finds finished turns by reading the agents' own session records; nothing in
//!   their config changes.
//! - Settings live in `~/.agentplus/notify.json`, with keys, tokens and the webhook URL
//!   protected like the sync password (DPAPI on Windows). Not part of `store.json`, so sync
//!   never carries them.
//! - A watcher thread runs while notifications are on and at least one channel is chosen.

mod push;
mod watch;

use crate::i18n::l;
use crate::util::lock;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// A finished turn.
#[derive(Debug, Clone)]
pub struct Done {
    /// Agent id; empty for a row that is only there to move the watermark.
    pub agent: &'static str,
    /// The project folder's name.
    pub project: String,
    /// The agent's final reply.
    pub message: String,
    pub duration_ms: Option<i64>,
    pub end_ms: i64,
}

impl Done {
    fn none() -> Done {
        Done { agent: "", project: String::new(), message: String::new(), duration_ms: None, end_ms: 0 }
    }
}

/// What a channel sends.
pub struct Message {
    pub title: String,
    pub body: String,
    pub agent: String,
    pub project: String,
    pub duration_ms: Option<i64>,
}

// ---------------------------------------------------------------- settings

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Bark {
    pub server: String,
    /// Device key, or the whole URL the Bark app shows.
    pub key: String,
    pub sound: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Ntfy {
    pub server: String,
    pub topic: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Gotify {
    pub server: String,
    /// The application token.
    pub token: String,
    /// 0–10; blank = 5.
    pub priority: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerChan {
    pub key: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Webhook {
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Channels {
    pub bark: Bark,
    pub ntfy: Ntfy,
    pub gotify: Gotify,
    pub serverchan: ServerChan,
    pub webhook: Webhook,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub enabled: bool,
    /// Agents to watch (`watch::AGENTS` ids).
    pub agents: Vec<String>,
    /// Channels to send to (`push::KINDS` ids).
    pub channels: Vec<String>,
    /// Turns shorter than this aren't reported (0 = all).
    pub min_secs: u32,
    /// Put the agent's reply in the message (off by default: it goes to the push service).
    pub include_reply: bool,
    /// With `include_reply`: a model's summary of the reply instead of its first lines.
    pub summary: bool,
    /// Where the summarizing provider is configured: an agent id, or `library` (the
    /// provider library).
    pub summary_agent: String,
    /// That provider's id.
    pub summary_provider: String,
    /// Its model; blank = the entry's first.
    pub summary_model: String,
    pub settings: Channels,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            enabled: false,
            agents: watch::AGENTS.iter().map(|s| s.to_string()).collect(),
            channels: Vec::new(),
            min_secs: 0,
            include_reply: false,
            summary: false,
            summary_agent: String::new(),
            summary_provider: String::new(),
            summary_model: String::new(),
            settings: Channels::default(),
        }
    }
}

/// JSON pointers of the fields stored protected.
const SECRETS: [&str; 5] = ["/settings/bark/key", "/settings/ntfy/token", "/settings/gotify/token", "/settings/serverchan/key", "/settings/webhook/url"];

fn path() -> PathBuf {
    crate::util::agentplus_dir().join("notify.json")
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);
/// Held from reading the settings to saving them, so two changes can't drop each other.
static WRITE: Mutex<()> = Mutex::new(());
/// Told after every saved change (the tray menu's switch follows it).
static ON_CHANGE: Mutex<Option<fn()>> = Mutex::new(None);

pub fn on_change(f: fn()) {
    *lock(&ON_CHANGE) = Some(f);
}

/// The saved settings (read once, then cached).
pub fn config() -> Config {
    let mut c = lock(&CONFIG);
    c.get_or_insert_with(load).clone()
}

fn load() -> Config {
    let Ok(text) = std::fs::read_to_string(path()) else { return Config::default() };
    let Ok(mut v) = serde_json::from_str::<Value>(&text) else {
        crate::applog::warn("notify", "notify.json unreadable; using defaults");
        return Config::default();
    };
    for p in SECRETS {
        if let Some(slot) = v.pointer_mut(p) {
            if slot.is_object() {
                *slot = Value::String(crate::seal::unprotect(slot).unwrap_or_else(|e| {
                    crate::applog::warn("notify", format!("A saved secret ({p}) can't be read: {e}"));
                    String::new()
                }));
            }
        }
    }
    serde_json::from_value(v).unwrap_or_default()
}

fn save(cfg: &Config) -> Result<()> {
    let mut v = serde_json::to_value(cfg)?;
    for p in SECRETS {
        if let Some(slot) = v.pointer_mut(p) {
            if let Some(s) = slot.as_str().filter(|s| !s.is_empty()) {
                *slot = crate::seal::protect(s)?;
            }
        }
    }
    crate::util::ensure_private_dir(&crate::util::agentplus_dir())?;
    crate::util::write_private_atomic(&path(), serde_json::to_string_pretty(&v)?.as_bytes())?;
    *lock(&CONFIG) = Some(cfg.clone());
    let f = *lock(&ON_CHANGE);
    if let Some(f) = f {
        f();
    }
    Ok(())
}

/// Drops ids this version doesn't know and repeats.
fn clean(mut cfg: Config) -> Config {
    let keep = |list: &mut Vec<String>, ok: &[&str]| {
        let mut seen = std::collections::HashSet::new();
        list.retain(|s| ok.contains(&s.as_str()) && seen.insert(s.clone()));
    };
    keep(&mut cfg.agents, &watch::AGENTS);
    keep(&mut cfg.channels, &push::KINDS.map(|k| k.0));
    cfg.min_secs = cfg.min_secs.min(24 * 3600);
    cfg
}

// ---------------------------------------------------------------- page state

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub channel: String,
    /// None = delivered.
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub at_ms: i64,
    pub agent: String,
    pub project: String,
    pub title: String,
    pub body: String,
    pub sent: Vec<Sent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSource {
    pub id: String,
    /// Its session records exist on this machine.
    pub found: bool,
}

/// Turns reported in one hour, for the page's chart.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Hour {
    /// Start of the hour (Unix seconds).
    pub t: i64,
    pub turns: u32,
    /// Turns at least one channel failed to deliver.
    pub failed: u32,
    /// Agent id → turns.
    pub agents: BTreeMap<String, u32>,
    /// When the last turn of the hour ended (ms).
    pub last_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub config: Config,
    pub agents: Vec<AgentSource>,
    /// Newest first.
    pub recent: Vec<Record>,
    /// The watcher is following the agents right now.
    pub watching: bool,
    /// When the watcher started following (ms), while it does.
    pub since_ms: Option<i64>,
    /// The last 7 days, oldest first; hours without turns are left out.
    pub hours: Vec<Hour>,
    /// When the last reported turn ended (ms); kept across restarts.
    pub last_ms: Option<i64>,
    pub now_ms: i64,
}

/// How long the hourly counts are kept.
const KEEP_HOURS: i64 = 7 * 24;

fn stats_path() -> PathBuf {
    crate::util::agentplus_dir().join("notify-stats.json")
}

static STATS: Mutex<Option<Vec<Hour>>> = Mutex::new(None);
static SINCE: Mutex<Option<i64>> = Mutex::new(None);

fn with_stats<T>(f: impl FnOnce(&mut Vec<Hour>) -> T) -> T {
    let mut g = lock(&STATS);
    let hours = g.get_or_insert_with(|| std::fs::read_to_string(stats_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    f(hours)
}

/// Counts one reported turn in its hour and drops hours older than `KEEP_HOURS`.
pub fn count_turn(hours: &mut Vec<Hour>, at_ms: i64, agent: &str, failed: bool) {
    let t = at_ms.div_euclid(3_600_000) * 3600;
    let i = match hours.iter().position(|h| h.t == t) {
        Some(i) => i,
        None => {
            let i = hours.partition_point(|h| h.t < t);
            hours.insert(i, Hour { t, ..Hour::default() });
            i
        }
    };
    let h = &mut hours[i];
    h.turns += 1;
    h.failed += failed as u32;
    h.last_ms = h.last_ms.max(at_ms);
    *h.agents.entry(agent.to_string()).or_default() += 1;
    let newest = hours.last().map_or(t, |h| h.t);
    hours.retain(|h| h.t > newest - KEEP_HOURS * 3600);
}

const RECENT_MAX: usize = 30;
static RECENT: Mutex<VecDeque<Record>> = Mutex::new(VecDeque::new());

/// At least one agent and one channel chosen.
pub fn ready(cfg: &Config) -> bool {
    !cfg.channels.is_empty() && !cfg.agents.is_empty()
}

fn active(cfg: &Config) -> bool {
    cfg.enabled && ready(cfg)
}

pub fn status() -> Status {
    let config = config();
    Status {
        watching: active(&config),
        agents: watch::AGENTS.iter().map(|a| AgentSource { id: a.to_string(), found: watch::available(a) }).collect(),
        recent: lock(&RECENT).iter().cloned().collect(),
        since_ms: *lock(&SINCE),
        hours: with_stats(|h| h.clone()),
        last_ms: with_stats(|h| h.iter().map(|x| x.last_ms).max().filter(|&ms| ms > 0)),
        now_ms: watch::now_ms(),
        config,
    }
}

pub fn set_config(cfg: Config) -> Result<Status> {
    {
        let _w = lock(&WRITE);
        save(&clean(cfg))?;
    }
    Ok(status())
}

/// Turns notifications on or off, keeping the other settings (the tray menu's switch).
pub fn set_enabled(on: bool) -> Result<Status> {
    {
        let _w = lock(&WRITE);
        save(&Config { enabled: on, ..config() })?;
    }
    Ok(status())
}

pub fn clear_recent() -> Status {
    lock(&RECENT).clear();
    status()
}

/// Sends a sample message through one channel, with the settings as typed (not yet saved).
pub fn test(cfg: &Config, channel: &str) -> Result<()> {
    if !push::KINDS.iter().any(|k| k.0 == channel) {
        bail!("{}", tr!("Unknown push channel: {channel}", "未知的推送渠道：{channel}"));
    }
    let m = Message {
        title: l("AgentPlus test notification", "AgentPlus 测试通知").to_string(),
        body: l("Completion notifications are set up.", "完成通知已配置好。").to_string(),
        agent: "AgentPlus".into(),
        project: String::new(),
        duration_ms: None,
    };
    push::send(&cfg.settings, channel, &m)
}

/// Summarizes a sample reply with the summary settings as typed (not yet saved).
pub fn test_summary(cfg: &Config) -> Result<String> {
    summarize(cfg, "I traced the failing login to a token cache that was never cleared after logout. \
        I added an invalidation step in auth/session.rs, wrote two regression tests, and ran the full suite: \
        all 214 tests pass. I did not touch the database migrations. Next, the old cache flag in settings.json can be removed.")
}

// ---------------------------------------------------------------- messages

fn agent_name(id: &str) -> &'static str {
    match id {
        "codex" => "Codex",
        "claude" => "Claude Code",
        "opencode" => "OpenCode",
        "zcode" => "ZCode",
        _ => "Agent",
    }
}

pub fn fmt_duration(ms: i64) -> String {
    let s = (ms.max(0) + 500) / 1000;
    let (h, m, s) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        tr!("{h}h {m}m", "{h} 小时 {m} 分")
    } else if m > 0 {
        tr!("{m}m {s}s", "{m} 分 {s} 秒")
    } else {
        tr!("{s}s", "{s} 秒")
    }
}

/// The first `max` characters of the reply, on one paragraph's worth of lines.
fn snippet(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    format!("{}…", t.chars().take(max).collect::<String>().trim_end())
}

/// What the prompt asks the summarizing model for; the reply is cut to its first 6000
/// characters.
pub fn summary_prompt(reply: &str, chinese: bool) -> String {
    let reply: String = reply.trim().chars().take(6000).collect();
    let lang = if chinese { "Simplified Chinese" } else { "English" };
    format!(
        "Summarize the final reply of a coding agent below for a phone notification. \
         At most two short sentences, plain text without Markdown, in {lang}. \
         Say what was done and anything the user must act on. Output only the summary.\n\n---\n{reply}"
    )
}

/// A model's summary of `reply`, using the provider chosen in the settings (one of an
/// agent's, or a provider library entry).
fn summarize(cfg: &Config, reply: &str) -> Result<String> {
    if cfg.summary_agent.trim().is_empty() || cfg.summary_provider.trim().is_empty() {
        bail!("{}", l("Choose a provider for the summary", "请选择用于总结的供应商"));
    }
    let model = cfg.summary_model.trim();
    if model.is_empty() {
        bail!("{}", l("Choose a model for the summary", "请选择用于总结的模型"));
    }
    let (base, key, api) = crate::adapters::provider_endpoint(&cfg.summary_agent, &cfg.summary_provider)?;
    let prompt = summary_prompt(reply, !crate::i18n::is_en());
    // Short: the notification waits for the summary.
    let text = crate::net::ask(&base, key.as_deref(), &api, model, &prompt, 400, 20).map_err(|e| anyhow::anyhow!(e))?;
    Ok(text.trim().trim_matches('"').trim().to_string())
}

/// The reply as the message carries it: none, its first lines, or a summary (its first
/// lines again when summarizing fails).
fn reply_text(cfg: &Config, d: &Done) -> Option<String> {
    if !cfg.include_reply || d.message.trim().is_empty() {
        return None;
    }
    if cfg.summary {
        match summarize(cfg, &d.message) {
            Ok(s) if !s.is_empty() => return Some(snippet(&s, 300)),
            Ok(_) => {}
            Err(e) => crate::applog::warn("notify", format!("Summary failed, sending the reply's start instead: {e}")),
        }
    }
    Some(snippet(&d.message, 300))
}

pub fn message(d: &Done, reply: Option<&str>) -> Message {
    let agent = agent_name(d.agent);
    let title = if d.project.is_empty() { tr!("{agent} finished", "{agent} 已完成") } else { tr!("{agent} finished · {project}", "{agent} 已完成 · {project}", project = d.project) };
    let mut lines = Vec::new();
    if let Some(ms) = d.duration_ms {
        lines.push(tr!("Took {d}", "用时 {d}", d = fmt_duration(ms)));
    }
    if let Some(r) = reply.filter(|r| !r.trim().is_empty()) {
        lines.push(r.to_string());
    }
    if lines.is_empty() {
        lines.push(l("The turn is done.", "本轮任务已结束。").to_string());
    }
    Message { title, body: lines.join("\n"), agent: agent.to_string(), project: d.project.clone(), duration_ms: d.duration_ms }
}

/// Whether a finished turn is worth a message under these settings.
pub fn wanted(cfg: &Config, d: &Done) -> bool {
    cfg.agents.iter().any(|a| a == d.agent) && (cfg.min_secs == 0 || d.duration_ms.is_none_or(|ms| ms >= cfg.min_secs as i64 * 1000))
}

fn dispatch(cfg: Config, d: Done) {
    let m = message(&d, reply_text(&cfg, &d).as_deref());
    let sent: Vec<Sent> = cfg
        .channels
        .iter()
        .map(|c| Sent { channel: c.clone(), error: push::send(&cfg.settings, c, &m).err().map(|e| e.to_string()) })
        .collect();
    for s in &sent {
        if let Some(e) = &s.error {
            crate::applog::warn("notify", format!("{} push failed: {e}", s.channel));
        }
    }
    crate::applog::info("notify", format!("{} turn finished; sent to {}/{} channels", d.agent, sent.iter().filter(|s| s.error.is_none()).count(), sent.len()));
    let failed = sent.iter().any(|s| s.error.is_some());
    // Written under the lock: turns are dispatched on their own threads, and an older copy
    // must not land after a newer one.
    with_stats(|h| {
        count_turn(h, d.end_ms, d.agent, failed);
        if let Err(e) = serde_json::to_string(h).map_err(anyhow::Error::from).and_then(|s| crate::util::write_private_atomic(&stats_path(), s.as_bytes())) {
            crate::applog::warn("notify", format!("Can't save notify-stats.json: {e}"));
        }
    });
    let mut r = lock(&RECENT);
    r.push_front(Record { at_ms: d.end_ms, agent: d.agent.to_string(), project: d.project, title: m.title, body: m.body, sent });
    r.truncate(RECENT_MAX);
}

// ---------------------------------------------------------------- watcher

enum Source {
    Files(watch::Files),
    Db(watch::Db, Option<String>),
}

/// The watcher thread (started with the app; idle while notifications are off).
pub fn autostart() {
    let mut sources: HashMap<&'static str, Source> = HashMap::new();
    loop {
        let cfg = config();
        if !active(&cfg) {
            // Starting again begins from "now", not from where it stopped.
            sources.clear();
            *lock(&SINCE) = None;
        } else {
            lock(&SINCE).get_or_insert_with(watch::now_ms);
            sources.retain(|a, _| cfg.agents.iter().any(|x| x == a));
            let mut found = Vec::new();
            for a in watch::AGENTS.iter().copied().filter(|a| cfg.agents.iter().any(|x| x == a)) {
                let src = sources.entry(a).or_insert_with(|| match a {
                    "codex" | "claude" => Source::Files(watch::Files::new(a)),
                    _ => Source::Db(watch::Db::new(a), None),
                });
                match src {
                    Source::Files(f) => f.poll(&mut found),
                    Source::Db(db, last) => match db.poll(&mut found) {
                        Ok(()) => *last = None,
                        Err(e) => {
                            if last.as_deref() != Some(e.as_str()) {
                                crate::applog::warn("notify", format!("Can't read {a} sessions: {e}"));
                                *last = Some(e);
                            }
                        }
                    },
                }
            }
            for d in found.into_iter().filter(|d| wanted(&cfg, d)) {
                let cfg = cfg.clone();
                std::thread::spawn(move || dispatch(cfg, d));
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(agent: &'static str, ms: Option<i64>, msg: &str) -> Done {
        Done { agent, project: "proj".into(), message: msg.into(), duration_ms: ms, end_ms: 0 }
    }

    #[test]
    fn filters_by_agent_and_length() {
        let mut cfg = Config { agents: vec!["codex".into()], ..Config::default() };
        assert!(wanted(&cfg, &done("codex", Some(1000), "")));
        assert!(!wanted(&cfg, &done("claude", Some(1000), "")));
        cfg.min_secs = 30;
        assert!(!wanted(&cfg, &done("codex", Some(29_999), "")));
        assert!(wanted(&cfg, &done("codex", Some(30_000), "")));
        assert!(wanted(&cfg, &done("codex", None, "")), "an unknown length still reports");
    }

    #[test]
    fn message_text() {
        let with_reply = Config { include_reply: true, ..Config::default() };
        let d = done("claude", Some(125_000), "  All tests pass.  ");
        let m = message(&d, reply_text(&with_reply, &d).as_deref());
        assert_eq!(m.title, "Claude Code 已完成 · proj");
        assert_eq!(m.body, "用时 2 分 5 秒\nAll tests pass.");
        // The reply stays out unless asked for.
        let quiet = Config::default();
        assert!(!quiet.include_reply);
        let d = Done { project: String::new(), ..done("zcode", None, "secret") };
        let m = message(&d, reply_text(&quiet, &d).as_deref());
        assert_eq!((m.title.as_str(), m.body.as_str()), ("ZCode 已完成", "本轮任务已结束。"));
        let long = done("codex", None, &"字".repeat(400));
        assert_eq!(message(&long, reply_text(&with_reply, &long).as_deref()).body.chars().count(), 301);
    }

    #[test]
    fn failed_summary_falls_back_to_the_reply() {
        let cfg = Config { include_reply: true, summary: true, ..Config::default() };
        assert!(summarize(&cfg, "x").is_err(), "no provider chosen");
        assert_eq!(reply_text(&cfg, &done("codex", None, "Done.")).as_deref(), Some("Done."));
        assert_eq!(reply_text(&cfg, &done("codex", None, "  ")), None);
    }

    #[test]
    fn summary_prompts() {
        let p = summary_prompt(&format!("  {}", "a".repeat(7000)), true);
        assert!(p.contains("Simplified Chinese"));
        assert!(p.ends_with(&"a".repeat(6000)) && !p.contains(&"a".repeat(6001)));
        assert!(summary_prompt("x", false).contains("in English"));
    }

    #[test]
    fn hourly_counts() {
        let h0 = 1_793_772_000_000; // on the hour
        let mut hours = Vec::new();
        count_turn(&mut hours, h0 + 5_000, "codex", false);
        count_turn(&mut hours, h0 + 3_599_999, "claude", true);
        count_turn(&mut hours, h0 - 1, "codex", false);
        assert_eq!(hours.iter().map(|h| (h.t, h.turns, h.failed)).collect::<Vec<_>>(), vec![(h0 / 1000 - 3600, 1, 0), (h0 / 1000, 2, 1)]);
        assert_eq!(hours[1].agents.get("claude"), Some(&1));
        assert_eq!(hours[1].last_ms, h0 + 3_599_999);
        // A turn a week later drops what is older than 7 days.
        count_turn(&mut hours, h0 + KEEP_HOURS * 3_600_000, "zcode", false);
        assert_eq!(hours.len(), 1);
        assert_eq!(hours[0].agents.get("zcode"), Some(&1));
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(400), "0 秒");
        assert_eq!(fmt_duration(59_600), "1 分 0 秒");
        assert_eq!(fmt_duration(3_725_000), "1 小时 2 分");
    }

    #[test]
    fn clean_drops_unknown_and_repeated_ids() {
        let cfg = clean(Config { agents: vec!["codex".into(), "nope".into(), "codex".into()], channels: vec!["bark".into(), "fax".into()], min_secs: u32::MAX, ..Config::default() });
        assert_eq!(cfg.agents, vec!["codex"]);
        assert_eq!(cfg.channels, vec!["bark"]);
        assert_eq!(cfg.min_secs, 86_400);
    }

    #[test]
    fn secrets_are_stored_protected() {
        let _h = crate::util::TestHome::new("notify-secrets");
        let cfg = Config { enabled: true, channels: vec!["bark".into()], settings: Channels { bark: Bark { key: "DEVICEKEY".into(), ..Bark::default() }, ..Channels::default() }, ..Config::default() };
        save(&cfg).unwrap();
        let text = std::fs::read_to_string(path()).unwrap();
        if crate::seal::SYSTEM_PROTECTED {
            assert!(!text.contains("DEVICEKEY"));
        }
        *lock(&CONFIG) = None;
        assert_eq!(load(), cfg);
        // Old or partial files fill in the defaults.
        std::fs::write(path(), r#"{"enabled":true}"#).unwrap();
        assert_eq!(load().agents.len(), watch::AGENTS.len());
        *lock(&CONFIG) = None;
    }
}
