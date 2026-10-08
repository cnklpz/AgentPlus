//! WeChat bridge: talk to Codex sessions from WeChat through ClawBot (the bot WeChat lets an
//! account add for OpenClaw-style agents).
//!
//! - Sign-in scans a QR code with the phone; the bot token, the account's API base and the
//!   WeChat user who scanned are kept in `~/.agentplus/wechat.json` (token protected with
//!   DPAPI on Windows). This file is not part of `store.json`, so sync never carries it.
//! - While on, a poll thread long-polls the relay and a bridge thread owns all state: it
//!   reads the messages (only from the user who scanned), runs them through `command`, and
//!   drives Codex through a `codex app-server` child. See `bridge` for the routing rules.

mod appserver;
mod bridge;
mod command;
mod format;
mod ilink;

use crate::i18n::l;
use crate::util::lock;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

/// What the bridge thread reacts to.
pub enum Event {
    Inbound(ilink::Inbound),
    /// Start Codex now (the page's button).
    StartCodex,
    Cursor(String),
    /// The relay rejected the token (scan again).
    Expired,
    PollError(String),
    CodexNotify { generation: u64, method: String, params: Value },
    CodexRequest { generation: u64, id: Value, method: String, params: Value },
    CodexExited { generation: u64 },
    Stop,
}

// ---------------------------------------------------------------- saved state

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    pub bot_id: String,
    /// The WeChat user who scanned: the only one the bot listens to.
    pub user_id: String,
    pub base_url: String,
    /// `seal::protect` form.
    pub token: Value,
    pub bound_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub enabled: bool,
    pub account: Option<Account>,
    /// `get_updates_buf`: where polling resumes.
    pub cursor: String,
    /// From the user's latest message; every reply must carry one.
    pub context_token: Option<String>,
    /// Session numbers, kept for good: number → Codex thread id.
    pub numbers: BTreeMap<u32, String>,
    pub current: Option<String>,
    /// Folder for `/new` without a path.
    pub default_cwd: Option<String>,
    /// Thread id → when the bridge last ran a turn on it (Unix seconds).
    pub owned: BTreeMap<String, i64>,
    /// Recent bot messages: relay message id → session number, for quoted replies.
    pub quotes: VecDeque<(String, u32)>,
    /// Model / effort chosen from WeChat, sent with the session's next turn: thread id →
    /// override. Codex keeps them for the session from then on.
    pub next: BTreeMap<String, NextTurn>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NextTurn {
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn saved_path() -> PathBuf {
    crate::util::agentplus_dir().join("wechat.json")
}

/// Serializes load … save of `wechat.json` between the bridge thread and the UI's commands.
static FILE: Mutex<()> = Mutex::new(());

pub fn load_saved() -> Saved {
    std::fs::read_to_string(saved_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_saved(s: &Saved) -> Result<()> {
    crate::util::ensure_private_dir(&crate::util::agentplus_dir())?;
    crate::util::write_private_atomic(&saved_path(), serde_json::to_string_pretty(s)?.as_bytes())
}

/// Changes the saved state in one locked load … save.
pub fn update_saved<T>(f: impl FnOnce(&mut Saved) -> T) -> Result<T> {
    let _g = lock(&FILE);
    let mut s = load_saved();
    let out = f(&mut s);
    write_saved(&s)?;
    Ok(out)
}

// ---------------------------------------------------------------- status for the page

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub at: i64,
    /// "in" | "out" | "info" | "error"
    pub kind: &'static str,
    pub text: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub no: u32,
    pub title: String,
    pub cwd: String,
    /// "idle" | "running" | "waiting" (an approval or a question)
    pub state: &'static str,
    pub queued: usize,
    /// The model and reasoning effort Codex reports for the session ("" when unknown).
    pub model: String,
    pub effort: String,
    /// Chosen from WeChat, taking effect with the next turn.
    pub next_model: Option<String>,
    pub next_effort: Option<String>,
    pub current: bool,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginView {
    /// "wait" | "scanned" | "needCode" | "badCode" | "done" | "failed"
    pub state: String,
    pub qr_svg: Option<String>,
    pub message: Option<String>,
}

/// Counters of the current run (since the bridge last started).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Unix ms.
    pub since: Option<i64>,
    pub last_in: Option<i64>,
    pub last_out: Option<i64>,
    pub received: u32,
    pub sent: u32,
    pub turns_done: u32,
    pub turns_failed: u32,
    pub approvals: u32,
    pub errors: u32,
    /// The Codex program the bridge runs, while it runs.
    pub codex_exe: Option<String>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub bound: bool,
    /// "off" | "starting" | "running" | "expired" | "error"
    pub state: String,
    pub error: Option<String>,
    pub bot_id: Option<String>,
    pub user_id: Option<String>,
    pub bound_at: Option<i64>,
    pub default_cwd: Option<String>,
    /// Used when `default_cwd` isn't set.
    pub fallback_cwd: String,
    pub codex_ready: bool,
    pub login: Option<LoginView>,
    pub sessions: Vec<SessionView>,
    pub log: Vec<LogLine>,
    pub stats: Stats,
}

#[derive(Default)]
struct Shared {
    state: String,
    error: Option<String>,
    codex_ready: bool,
    sessions: Vec<SessionView>,
    log: VecDeque<LogLine>,
    login: Option<LoginView>,
    stats: Stats,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn shared<T>(f: impl FnOnce(&mut Shared) -> T) -> T {
    let mut g = lock(&SHARED);
    f(g.get_or_insert_with(|| Shared { state: "off".into(), ..Default::default() }))
}

const LOG_LINES: usize = 80;

pub(crate) fn log(kind: &'static str, text: impl AsRef<str>) {
    let at = chrono::Utc::now().timestamp_millis();
    let line = LogLine { at, kind, text: crate::util::clip(text.as_ref(), 300) };
    shared(|s| {
        match kind {
            "in" => {
                s.stats.received += 1;
                s.stats.last_in = Some(at);
            }
            "out" => {
                s.stats.sent += 1;
                s.stats.last_out = Some(at);
            }
            "error" => s.stats.errors += 1,
            _ => {}
        }
        s.log.push_back(line);
        while s.log.len() > LOG_LINES {
            s.log.pop_front();
        }
    });
}

pub(crate) fn stats(f: impl FnOnce(&mut Stats)) {
    shared(|s| f(&mut s.stats));
}

pub(crate) fn set_state(state: &str, error: Option<String>) {
    if let Some(e) = &error {
        crate::applog::warn("wechat", e);
    }
    shared(|s| {
        s.state = state.into();
        s.error = error;
    });
}

pub(crate) fn set_sessions(v: Vec<SessionView>, codex_ready: bool) {
    shared(|s| {
        s.sessions = v;
        s.codex_ready = codex_ready;
    });
}

pub fn status() -> Status {
    let saved = load_saved();
    let acc = saved.account.as_ref();
    shared(|s| Status {
        enabled: saved.enabled,
        bound: acc.is_some(),
        state: s.state.clone(),
        error: s.error.clone(),
        bot_id: acc.map(|a| a.bot_id.clone()),
        user_id: acc.map(|a| a.user_id.clone()),
        bound_at: acc.map(|a| a.bound_at),
        default_cwd: saved.default_cwd.clone(),
        fallback_cwd: crate::util::display_path(&fallback_cwd()),
        codex_ready: s.codex_ready,
        login: s.login.clone(),
        sessions: s.sessions.clone(),
        log: s.log.iter().rev().cloned().collect(),
        stats: s.stats.clone(),
    })
}

// ---------------------------------------------------------------- running

/// Bumped by every start and stop; a thread whose run is over stops at its next check.
static RUN: AtomicU64 = AtomicU64::new(0);
static CONTROL: Mutex<Option<mpsc::Sender<Event>>> = Mutex::new(None);

fn token_of(acc: &Account) -> Result<String> {
    crate::seal::unprotect(&acc.token).map_err(|_| anyhow!(l("The saved WeChat sign-in can't be read on this device. Scan again", "本机保存的微信登录信息读不出来，请重新扫码")))
}

fn start_locked(ctl: &mut Option<mpsc::Sender<Event>>) -> Result<()> {
    if let Some(tx) = ctl.take() {
        let _ = tx.send(Event::Stop);
    }
    let run = RUN.fetch_add(1, Ordering::SeqCst) + 1;
    let saved = load_saved();
    let Some(acc) = saved.account.clone() else {
        set_state("off", None);
        return Ok(());
    };
    if !saved.enabled {
        set_state("off", None);
        return Ok(());
    }
    let token = match token_of(&acc) {
        Ok(t) => t,
        Err(e) => {
            set_state("error", Some(e.to_string()));
            return Err(e);
        }
    };
    let wx = ilink::Client::new(&acc.base_url, Some(token.clone()))?;
    let poll = ilink::Client::new(&acc.base_url, Some(token))?;
    let (tx, rx) = mpsc::channel();
    set_state("starting", None);
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("agentplus-wechat-poll".into()).spawn(move || poll_loop(poll, run, tx))?;
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("agentplus-wechat".into()).spawn(move || {
            let mut b = bridge::Bridge::new(wx, acc.user_id.clone(), saved, tx);
            b.run(rx, run);
        })?;
    }
    *ctl = Some(tx);
    Ok(())
}

/// Held while a poll thread runs. A stopped run's thread can still be inside a long poll for
/// up to its hold time: the next run waits for it, so polls never overlap and its
/// "stop" notice can't follow the new run's "start".
static POLL: Mutex<()> = Mutex::new(());

fn poll_loop(wx: ilink::Client, run: u64, tx: mpsc::Sender<Event>) {
    let current = || RUN.load(Ordering::SeqCst) == run;
    let _one = lock(&POLL);
    if !current() {
        return;
    }
    // Read now: the previous run may have moved it while this one waited.
    let mut cursor = load_saved().cursor;
    wx.notify(true);
    let mut hold = ilink::LONG_POLL;
    let mut failures = 0u32;
    while current() {
        match wx.get_updates(&cursor, hold) {
            Ok(v) => {
                if !current() {
                    break;
                }
                if let Some((code, msg)) = ilink::updates_error(&v) {
                    if code == ilink::STALE_TOKEN {
                        let _ = tx.send(Event::Expired);
                        return;
                    }
                    failures += 1;
                    let _ = tx.send(Event::PollError(format!("getupdates {code} {msg}")));
                    std::thread::sleep(Duration::from_secs(if failures >= 3 { 30 } else { 2 }));
                    continue;
                }
                failures = 0;
                if let Some(ms) = v["longpolling_timeout_ms"].as_u64().filter(|ms| *ms > 0) {
                    hold = Duration::from_millis(ms.min(120_000));
                }
                if let Some(c) = v["get_updates_buf"].as_str().filter(|c| !c.is_empty()) {
                    cursor = c.to_string();
                    let _ = tx.send(Event::Cursor(cursor.clone()));
                }
                for m in v["msgs"].as_array().into_iter().flatten() {
                    if let Some(i) = ilink::parse_message(m) {
                        let _ = tx.send(Event::Inbound(i));
                    }
                }
            }
            Err(e) => {
                if !current() {
                    break;
                }
                failures += 1;
                let _ = tx.send(Event::PollError(e.to_string()));
                std::thread::sleep(Duration::from_secs(if failures >= 3 { 30 } else { 2 }));
            }
        }
    }
    wx.notify(false);
}

fn stop_locked(ctl: &mut Option<mpsc::Sender<Event>>) {
    RUN.fetch_add(1, Ordering::SeqCst);
    if let Some(tx) = ctl.take() {
        let _ = tx.send(Event::Stop);
    }
    set_sessions(vec![], false);
    set_state("off", None);
}

/// Starts the bridge at launch when it was left on.
pub fn autostart() {
    if let Err(e) = start_locked(&mut lock(&CONTROL)) {
        crate::applog::warn("wechat", format!("autostart: {e:#}"));
    }
}

/// Stops the bridge (and its Codex) when AgentPlus quits.
pub fn shutdown() {
    stop_locked(&mut lock(&CONTROL));
}

pub fn set_enabled(on: bool) -> Result<()> {
    let mut ctl = lock(&CONTROL);
    if on && load_saved().account.is_none() {
        bail!("{}", l("Connect a WeChat account first", "请先扫码连接微信"));
    }
    update_saved(|s| s.enabled = on)?;
    if on { start_locked(&mut ctl) } else {
        stop_locked(&mut ctl);
        Ok(())
    }
}

/// Starts Codex for the running bridge (and sends a message that waited for it).
pub fn start_codex() -> Result<()> {
    match lock(&CONTROL).as_ref() {
        Some(tx) => tx.send(Event::StartCodex).map_err(|_| anyhow!(l("The WeChat bridge isn't running", "微信桥接没有在运行"))),
        None => bail!("{}", l("The WeChat bridge isn't running", "微信桥接没有在运行")),
    }
}

/// Where `/new` starts a session when no folder is given and none is set:
/// DocumentsAgentPlus (created when first used).
pub fn fallback_cwd() -> PathBuf {
    dirs::document_dir().unwrap_or_else(crate::util::home).join("AgentPlus")
}

pub fn set_default_cwd(path: Option<String>) -> Result<()> {
    if let Some(p) = &path {
        crate::util::require_dir(std::path::Path::new(p))?;
    }
    update_saved(|s| s.default_cwd = path)
}

/// Forgets the account: stops the bridge and deletes the token and session numbers.
pub fn unbind() -> Result<()> {
    let mut ctl = lock(&CONTROL);
    stop_locked(&mut ctl);
    update_saved(|s| {
        let cwd = s.default_cwd.take();
        *s = Saved { default_cwd: cwd, ..Default::default() };
    })
}

// ---------------------------------------------------------------- sign-in

static LOGIN: AtomicU64 = AtomicU64::new(0);
static VERIFY: Mutex<Option<String>> = Mutex::new(None);

fn qr_svg(content: &str) -> Result<String> {
    let code = qrcode::QrCode::new(content.as_bytes())?;
    Ok(code.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
}

fn set_login(state: &str, qr: Option<String>, message: Option<String>) {
    shared(|s| {
        let qr = qr.or_else(|| s.login.as_ref().and_then(|l| l.qr_svg.clone()));
        s.login = Some(LoginView { state: state.into(), qr_svg: qr, message });
    });
}

/// Shows a new QR code and waits (in the background) for the phone to confirm.
pub fn login_start() -> Result<()> {
    let attempt = LOGIN.fetch_add(1, Ordering::SeqCst) + 1;
    *lock(&VERIFY) = None;
    let client = ilink::Client::new(ilink::LOGIN_BASE, None)?;
    let previous: Vec<String> = load_saved().account.and_then(|a| token_of(&a).ok()).into_iter().collect();
    let (qrcode, content) = client.qr_code(&previous)?;
    set_login("wait", Some(qr_svg(&content)?), None);
    std::thread::Builder::new().name("agentplus-wechat-login".into()).spawn(move || {
        if let Err(e) = login_wait(client, qrcode, attempt, previous) {
            if LOGIN.load(Ordering::SeqCst) == attempt {
                set_login("failed", None, Some(format!("{e:#}")));
            }
        }
    })?;
    Ok(())
}

/// The number the phone shows, when the relay asks for it.
pub fn login_verify(code: String) {
    *lock(&VERIFY) = Some(code.trim().to_string());
}

pub fn login_cancel() {
    LOGIN.fetch_add(1, Ordering::SeqCst);
    shared(|s| s.login = None);
}

fn login_wait(mut client: ilink::Client, mut qrcode: String, attempt: u64, previous: Vec<String>) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(8 * 60);
    let mut refreshes = 0;
    let mut sent_code: Option<String> = None;
    while LOGIN.load(Ordering::SeqCst) == attempt {
        if std::time::Instant::now() > deadline {
            bail!("{}", l("The QR code timed out. Try again", "二维码已超时，请重试"));
        }
        let code = lock(&VERIFY).clone();
        if sent_code.is_some() && code == sent_code {
            // Waiting for a new number after a wrong one.
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let v = client.qr_status(&qrcode, code.as_deref())?;
        if LOGIN.load(Ordering::SeqCst) != attempt {
            return Ok(());
        }
        match v["status"].as_str().unwrap_or("wait") {
            "wait" => {}
            "scaned" => {
                if code.is_some() {
                    *lock(&VERIFY) = None;
                    sent_code = None;
                }
                set_login("scanned", None, None);
            }
            "need_verifycode" => {
                let wrong = code.is_some();
                sent_code = code;
                set_login(if wrong { "badCode" } else { "needCode" }, None, None);
                if !wrong {
                    // Wait for the number before polling again.
                    while LOGIN.load(Ordering::SeqCst) == attempt && lock(&VERIFY).is_none() {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
            }
            "expired" | "verify_code_blocked" => {
                refreshes += 1;
                if refreshes >= 3 {
                    bail!("{}", l("The QR code expired several times. Try again later", "二维码多次失效，请稍后再试"));
                }
                *lock(&VERIFY) = None;
                sent_code = None;
                client = ilink::Client::new(ilink::LOGIN_BASE, None)?;
                let (id, content) = client.qr_code(&previous)?;
                qrcode = id;
                set_login("wait", Some(qr_svg(&content)?), Some(l("The QR code expired; here is a new one", "二维码已过期，已换新的").into()));
            }
            "binded_redirect" => {
                set_login("done", None, Some(l("This WeChat is already connected", "这个微信已经连接过了").into()));
                return Ok(());
            }
            "scaned_but_redirect" => {
                if let Some(h) = v["redirect_host"].as_str().filter(|h| !h.is_empty()) {
                    let base = format!("https://{h}");
                    if !ilink::trusted_base(&base) {
                        bail!("{}", tr!("Unexpected sign-in host {h}", "登录服务器不正常：{h}"));
                    }
                    client = ilink::Client::new(&base, None)?;
                }
            }
            "confirmed" => {
                let (Some(token), Some(bot), Some(user)) = (v["bot_token"].as_str(), v["ilink_bot_id"].as_str(), v["ilink_user_id"].as_str()) else {
                    bail!("{}", l("WeChat confirmed the sign-in but sent no account", "微信确认了登录，但没有返回账号信息"));
                };
                let base = v["baseurl"].as_str().filter(|b| !b.is_empty()).unwrap_or(ilink::LOGIN_BASE).to_string();
                if !ilink::trusted_base(&base) {
                    bail!("{}", tr!("Unexpected WeChat server {base}", "微信服务器地址不正常：{base}"));
                }
                let acc = Account { bot_id: bot.into(), user_id: user.into(), base_url: base, token: crate::seal::protect(token)?, bound_at: chrono::Utc::now().timestamp() };
                let mut ctl = lock(&CONTROL);
                // The running bridge (old account) stops saving before the file changes.
                stop_locked(&mut ctl);
                update_saved(|s| {
                    // Another WeChat user: the old numbers and reply token were theirs.
                    if s.account.as_ref().is_some_and(|a| a.user_id != acc.user_id) {
                        let cwd = s.default_cwd.take();
                        *s = Saved { default_cwd: cwd, ..Default::default() };
                    }
                    s.cursor.clear();
                    s.account = Some(acc);
                    s.enabled = true;
                })?;
                set_login("done", None, None);
                crate::applog::info("wechat", "account connected");
                return start_locked(&mut ctl);
            }
            other => crate::applog::warn("wechat", format!("sign-in status {other}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    #[test]
    fn saved_state_round_trips_and_unbind_keeps_the_folder() {
        let _h = TestHome::new("wechat-saved");
        assert!(load_saved().account.is_none());
        update_saved(|s| {
            s.enabled = true;
            s.account = Some(Account { bot_id: "b".into(), user_id: "u".into(), base_url: ilink::LOGIN_BASE.into(), token: json!({ "plain": "t" }), bound_at: 1 });
            s.numbers.insert(3, "thread".into());
            s.default_cwd = Some("C:/code".into());
            s.quotes.push_back(("m1".into(), 3));
        })
        .unwrap();
        let s = load_saved();
        assert_eq!(s.numbers.get(&3).map(String::as_str), Some("thread"));
        assert_eq!(s.quotes.front(), Some(&("m1".to_string(), 3)));
        assert_eq!(token_of(s.account.as_ref().unwrap()).unwrap(), "t");

        unbind().unwrap();
        let s = load_saved();
        assert!(s.account.is_none() && !s.enabled && s.numbers.is_empty() && s.quotes.is_empty());
        assert_eq!(s.default_cwd.as_deref(), Some("C:/code"));
        // Nothing to turn on without an account.
        assert!(set_enabled(true).is_err());
    }

    #[test]
    fn unreadable_file_reads_as_empty() {
        let h = TestHome::new("wechat-bad");
        std::fs::create_dir_all(h.0.join(".agentplus")).unwrap();
        std::fs::write(h.0.join(".agentplus").join("wechat.json"), "{ not json").unwrap();
        assert!(load_saved().account.is_none());
    }
}
