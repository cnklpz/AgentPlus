//! The bridge thread: routes WeChat messages to Codex sessions and Codex's answers back.
//!
//! All sessions share the one chat with the bot, so each has a number that never changes
//! (`Saved::numbers`) and every bot message starts with its tag `【#3 title】`:
//! - Plain text goes to the current session (`/use 3`); `#3 text` goes to session 3 once;
//!   replying to (quoting) a bot message goes to that message's session.
//! - A session runs one turn at a time; text sent meanwhile is queued and sent as the
//!   next turn. Each turn ends with one message: the final answer plus a summary line.
//! - Approvals and questions are relayed with the session number (`y3` / `n3` / `a3`).
//! - A session that Codex desktop may have open (Codex is running and the session changed
//!   in the last 30 minutes, not by the bridge) is locked: both would write the same session.
//!   `#3! text` or `/use 3!` sends anyway.

use super::appserver::AppServer;
use super::desktop;
use super::command::{self, Cmd, Decision, ModelCmd, Spec};
use super::format::{self, TurnStats};
use super::ilink::{self, Inbound};
use super::{log, set_sessions, set_state, stats, update_saved, Event, Options, Saved, SessionView, Stats};
use crate::i18n::l;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A session changed this recently, with Codex desktop running, may be open there.
const LOCK_WINDOW: i64 = 30 * 60;
const QUOTES: usize = 300;
const LIST: usize = 10;
/// Folders listed by `/dirs`.
const DIRS: usize = 10;
const CALL: Duration = Duration::from_secs(60);
/// How often the bridge checks timers (typing, heartbeats) between events.
const TICK: Duration = Duration::from_secs(5);
/// WeChat drops the "typing…" line after a while: send it again this often during a turn.
const TYPING_EVERY: Duration = Duration::from_secs(20);
/// Tries per message part before giving up on it.
const SEND_TRIES: u32 = 3;
/// Characters of a quoted message passed on to Codex.
const QUOTE_CHARS: usize = 500;
/// Lines `/diff` shows.
const DIFF_LINES: usize = 30;
/// How often sessions outside the bridge are checked for finished turns.
const DESKTOP_EVERY: Duration = Duration::from_secs(15);
/// Characters of Codex's last message in a desktop notice.
const NOTICE_CHARS: usize = 300;
/// The text sent with a picture that came alone (request content: not translated).
const IMAGE_ONLY: &str = "(See the attached picture.)";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Default)]
struct Live {
    title: String,
    cwd: String,
    /// Unix seconds, from Codex's thread list.
    updated: i64,
    /// Resumed in our app-server, so turns can start.
    loaded: bool,
    turn: Option<Turn>,
    queue: Vec<String>,
    /// Pictures sent with the queued messages.
    queue_images: Vec<std::path::PathBuf>,
    /// As Codex last reported them.
    model: String,
    effort: String,
    /// Answered "allow for this session" (`a3`): later command and file approvals are accepted
    /// without asking. Codex's own `acceptForSession` only covers that exact command.
    allow_all: bool,
    /// The last turn's reply, for `/last`.
    last_reply: Option<String>,
    /// Files the last turn changed, for `/diff` outside a Git repository.
    last_files: Vec<String>,
}

struct Turn {
    id: Option<String>,
    started: Instant,
    stats: TurnStats,
    final_answer: Option<String>,
    last_message: Option<String>,
    /// `last_message` was already relayed as progress.
    relayed: bool,
    /// Heartbeats sent so far.
    beats: u32,
    /// `/stop` came before Codex said which turn it started: interrupt once it does.
    stop_asked: bool,
}

impl Turn {
    fn new(id: Option<String>) -> Turn {
        Turn { id, started: Instant::now(), stats: TurnStats::default(), final_answer: None, last_message: None, relayed: false, beats: 0, stop_asked: false }
    }
}

/// When heartbeat `n` (from 0) of a running turn is due: 5 minutes in, then every 10.
fn beat_at(n: u32) -> Duration {
    Duration::from_secs(5 * 60 + 10 * 60 * u64::from(n))
}

enum Ask {
    Command,
    File,
    Permissions(Value),
    Questions(Vec<Value>),
}

struct Pending {
    id: Value,
    thread: String,
    ask: Ask,
}

pub struct Bridge {
    wx: ilink::Client,
    user: String,
    saved: Saved,
    events: mpsc::Sender<Event>,
    codex: Option<AppServer>,
    generation: u64,
    live: HashMap<String, Live>,
    pending: Vec<Pending>,
    /// File changes seen when their item started, by item id (approvals don't list them).
    changes: HashMap<String, Vec<String>>,
    typing: Option<String>,
    /// This run of the bridge; once over, nothing more is saved.
    run: u64,
    /// Messages that need Codex, waiting for the user to say whether to start it.
    deferred: Vec<Inbound>,
    /// Session messages accepted before Codex stopped, waiting for a restart.
    retry: Vec<(String, String)>,
    /// The list the last reply showed: a bare number in the next message picks from it.
    pick: Option<Pick>,
    /// The folders `/dirs` showed, in order, so `/new N` gets the folder that was shown.
    dirs: Vec<String>,
    /// `saved` changed since it was last written.
    dirty: bool,
    /// Tests: messages are kept here instead of being sent.
    outbox: Option<Vec<String>>,
    /// Messages held until the user's first message gives a reply context, with their
    /// session numbers.
    unsent: Vec<(Vec<String>, Option<u32>)>,
    /// Codex's `model/list`, read once per Codex start.
    models: Option<Vec<Value>>,
    /// When "typing…" was last sent.
    typed_at: Option<Instant>,
    /// A message didn't fully arrive: the next one says so.
    lost: bool,
    /// Sessions running outside the bridge, for "finished" notices.
    desktop: desktop::Watch,
    desktop_at: Option<Instant>,
    /// Notices waiting for a reply context: the latest per session only.
    held_notices: std::collections::BTreeMap<String, String>,
    /// Pictures of the message being handled, for the turn it starts.
    attach: Vec<std::path::PathBuf>,
    /// A message's pictures are downloading; later messages wait in `held`.
    downloading: bool,
    held: std::collections::VecDeque<Inbound>,
}

/// A numbered list the bot just showed.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    /// `/ls`: numbers are session numbers.
    Sessions,
    /// `/dirs`: numbers pick a folder for `/new`.
    Dirs,
    /// `/model`: numbers pick a model for this session (None: no session selected).
    Models(Option<String>),
}

/// What a bare number means right after a list.
fn pick_cmd(pick: &Pick, n: u32) -> (Cmd, Option<String>) {
    match pick {
        Pick::Sessions => (Cmd::Use { no: n, force: false }, None),
        Pick::Dirs => (Cmd::New(Some(n.to_string())), None),
        Pick::Models(thread) => (Cmd::Model(ModelCmd::Set(n.to_string())), thread.clone()),
    }
}

fn thread_title(t: &Value) -> String {
    let name = t["name"].as_str().unwrap_or("").trim();
    let s = if name.is_empty() { t["preview"].as_str().unwrap_or("").trim() } else { name };
    s.lines().next().unwrap_or("").to_string()
}

impl Bridge {
    pub fn new(wx: ilink::Client, user: String, saved: Saved, events: mpsc::Sender<Event>) -> Bridge {
        Bridge { wx, user, saved, events, codex: None, generation: 0, live: HashMap::new(), pending: vec![], changes: HashMap::new(), typing: None, run: 0, deferred: vec![], retry: vec![], pick: None, dirs: vec![], dirty: false, outbox: None, unsent: vec![], models: None, typed_at: None, lost: false, desktop: desktop::Watch::default(), desktop_at: None, held_notices: Default::default(), attach: vec![], downloading: false, held: Default::default() }
    }

    pub fn run(&mut self, rx: mpsc::Receiver<Event>, run: u64) {
        self.run = run;
        stats(|s| *s = Stats { since: Some(chrono::Utc::now().timestamp_millis()), ..Default::default() });
        set_state("running", None);
        log("info", l("Connected to WeChat", "已连接微信"));
        self.publish();
        let current = || super::RUN.load(std::sync::atomic::Ordering::SeqCst) == run;
        // Timers run every TICK whether or not events keep coming (a turn streams them).
        let mut ticked = Instant::now();
        loop {
            let wait = TICK.saturating_sub(ticked.elapsed());
            let ev = match rx.recv_timeout(wait) {
                Ok(ev) => Some(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if !current() {
                break;
            }
            if ticked.elapsed() >= TICK {
                ticked = Instant::now();
                self.tick();
            }
            let Some(ev) = ev else {
                self.flush();
                self.publish();
                continue;
            };
            match ev {
                Event::Stop => break,
                Event::Options(o) => self.set_options(o),
                Event::Inbound(m) => self.on_inbound(m),
                Event::Pictures { message, paths, failed } => self.on_pictures(message, paths, failed),
                Event::StartCodex => self.start_codex_now(),
                Event::Cursor(c) => {
                    if self.saved.cursor != c {
                        self.save(|s| s.cursor = c);
                    }
                }
                Event::Expired => {
                    set_state("expired", Some(l("WeChat signed the bot out. Scan the QR code again", "微信已让机器人下线，请重新扫码").into()));
                    set_sessions(vec![], false);
                    super::RUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                Event::PollError(e) => {
                    log("error", &e);
                    crate::applog::warn("wechat", e);
                }
                Event::CodexNotify { generation, method, params } if generation == self.generation => self.on_notify(&method, &params),
                Event::CodexRequest { generation, id, method, params } if generation == self.generation => self.on_request(id, &method, params),
                Event::CodexExited { generation } if generation == self.generation => self.on_codex_exit(),
                _ => {}
            }
            self.flush();
            self.publish();
        }
        self.flush();
        self.stop_typing();
        self.codex = None;
        stats(|s| s.codex_exe = None);
    }

    /// Changes the state; it is written once the current event is handled.
    fn save(&mut self, f: impl FnOnce(&mut Saved)) {
        f(&mut self.saved);
        self.dirty = true;
    }

    fn flush(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let s = self.saved.clone();
        let run = self.run;
        // Keep fields the page changes (folder, switch) as they are in the file. A run that
        // is over (disconnected, signed in again) must not write its account back.
        if let Err(e) = update_saved(|file| {
            if super::RUN.load(std::sync::atomic::Ordering::SeqCst) != run {
                return;
            }
            let (enabled, cwd, options) = (file.enabled, file.default_cwd.clone(), file.options.clone());
            *file = s;
            file.enabled = enabled;
            file.default_cwd = cwd;
            file.options = options;
        }) {
            crate::applog::warn("wechat", format!("save: {e:#}"));
        }
    }

    fn publish(&self) {
        let mut v: Vec<SessionView> = self
            .saved
            .numbers
            .iter()
            .filter(|(_, t)| self.live.contains_key(*t) || self.saved.current.as_ref() == Some(*t))
            .map(|(no, t)| {
                let live = self.live.get(t);
                let waiting = self.pending.iter().any(|p| &p.thread == t);
                SessionView {
                    no: *no,
                    title: live.map(|l| l.title.clone()).unwrap_or_default(),
                    cwd: live.map(|l| l.cwd.clone()).unwrap_or_default(),
                    state: if waiting { "waiting" } else if live.is_some_and(|l| l.turn.is_some()) { "running" } else { "idle" },
                    queued: live.map(|l| l.queue.len()).unwrap_or(0) + self.retry.iter().filter(|(thread, _)| thread == t).count(),
                    current: self.saved.current.as_ref() == Some(t),
                    model: live.map(|l| l.model.clone()).unwrap_or_default(),
                    effort: live.map(|l| l.effort.clone()).unwrap_or_default(),
                    next_model: self.saved.next.get(t).and_then(|n| n.model.clone()),
                    next_effort: self.saved.next.get(t).and_then(|n| n.effort.clone()),
                    mode: self.saved.modes.get(t).cloned(),
                }
            })
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.no));
        set_sessions(v, self.codex.as_ref().is_some_and(|c| c.alive()));
    }

    // ------------------------------------------------------------ sending

    fn send(&mut self, texts: Vec<String>, no: Option<u32>) {
        // A fresh connection has no reply context until the user sends a message; hold the
        // messages until one arrives.
        let Some(ctx) = self.reply_context() else {
            if self.unsent.is_empty() {
                log("info", l("Send a message to the bot in WeChat before it can reply.", "请先在微信里给机器人发一条消息，机器人才能回复。"));
            }
            for t in &texts {
                log("info", t);
            }
            self.unsent.push((texts, no));
            return;
        };
        let mut texts = texts;
        if std::mem::take(&mut self.lost) {
            if let Some(first) = texts.first_mut() {
                first.push_str(l("\n\n(The previous reply didn't fully arrive. Send /last to get it again.)", "\n\n（上一条回复没有完整送达，发 /last 重发。）"));
            }
        }
        if let Some(out) = &mut self.outbox {
            out.extend(texts);
            return;
        }
        let mut ids = vec![];
        for (i, t) in texts.iter().enumerate() {
            let mut tries = 0;
            let sent = loop {
                tries += 1;
                match self.wx.send_text(&self.user, t, Some(&ctx)) {
                    Ok(id) => break Some(id),
                    // The saved reply context no longer works: retrying can't help. Hold this
                    // part and the rest until the user's next message brings a new one.
                    Err(e) if e.downcast_ref::<ilink::SendRefused>().is_some_and(|r| r.code == ilink::STALE_CONTEXT) => {
                        crate::applog::warn("wechat", format!("send: {e:#}; reply context dropped"));
                        self.save(|s| s.context_token = None);
                        self.typing = None;
                        self.send(texts[i..].to_vec(), no);
                        break None;
                    }
                    Err(e) if tries < SEND_TRIES => {
                        crate::applog::warn("wechat", format!("send (try {tries}): {e:#}"));
                        std::thread::sleep(Duration::from_secs(u64::from(tries)));
                    }
                    Err(e) => {
                        log("error", format!("{e:#}"));
                        crate::applog::warn("wechat", format!("send: {e:#}"));
                        break None;
                    }
                }
            };
            match sent {
                Some(id) => {
                    log("out", t);
                    if let (Some(id), Some(no)) = (id, no) {
                        ids.push((id, no));
                    }
                }
                // Held for a new reply context: nothing is lost.
                None if self.reply_context().is_none() => break,
                None => {
                    // The rest would read as if this part was there; the next message says so.
                    self.lost = true;
                    break;
                }
            }
        }
        if !ids.is_empty() {
            self.save(|s| {
                s.quotes.extend(ids);
                while s.quotes.len() > QUOTES {
                    s.quotes.pop_front();
                }
            });
        }
    }

    /// The context token replies echo; None until the user has sent a message.
    fn reply_context(&self) -> Option<String> {
        self.saved.context_token.clone().filter(|ctx| !ctx.trim().is_empty())
    }

    /// Sends what was held for a reply context, once there is one.
    fn flush_unsent(&mut self) {
        if self.reply_context().is_some() {
            for (texts, no) in std::mem::take(&mut self.unsent) {
                self.send(texts, no);
            }
            for (thread, body) in std::mem::take(&mut self.held_notices) {
                self.note(&thread, &body);
            }
        }
    }

    /// The bridge's own text, a line per paragraph.
    fn say(&mut self, text: impl AsRef<str>) {
        self.send(format::plain(&format::breaks(text.as_ref())), None);
    }

    /// The bridge's own note about a session (approvals, questions, errors).
    fn note(&mut self, thread: &str, body: &str) {
        self.say_from(thread, &format::breaks(body));
    }

    /// A Codex answer: its Markdown goes as written.
    fn say_from(&mut self, thread: &str, body: &str) {
        let no = self.number(thread);
        let title = self.live.get(thread).map(|l| l.title.clone()).unwrap_or_default();
        self.send(format::tagged(no, &title, body), Some(no));
    }

    fn start_typing(&mut self) {
        if self.outbox.is_some() {
            return;
        }
        if self.typing.is_none() {
            let Some(ctx) = self.reply_context() else { return };
            self.typing = self.wx.typing_ticket(&self.user, Some(&ctx)).ok().flatten();
        }
        self.typed_at = Some(Instant::now());
        if let Some(t) = &self.typing {
            if self.wx.send_typing(&self.user, t, true).is_err() {
                // The ticket may have run out: fetch a new one next time.
                self.typing = None;
            }
        }
    }

    fn stop_typing(&mut self) {
        if self.live.values().any(|l| l.turn.is_some()) {
            return;
        }
        self.typed_at = None;
        if let (Some(t), None) = (&self.typing, &self.outbox) {
            let _ = self.wx.send_typing(&self.user, t, false);
        }
    }

    /// Timers between events: keeps "typing…" on during turns and sends heartbeats for
    /// long ones.
    fn tick(&mut self) {
        if self.saved.options.notify_desktop && self.desktop_at.is_none_or(|t| t.elapsed() >= DESKTOP_EVERY) {
            self.desktop_at = Some(Instant::now());
            self.check_desktop();
        }
        if !self.live.values().any(|l| l.turn.is_some()) {
            return;
        }
        if self.typed_at.is_none_or(|t| t.elapsed() >= TYPING_EVERY) {
            self.start_typing();
        }
        let due: Vec<(String, String)> = self
            .live
            .iter_mut()
            .filter_map(|(thread, l)| {
                let turn = l.turn.as_mut()?;
                let elapsed = turn.started.elapsed();
                if elapsed < beat_at(turn.beats) {
                    return None;
                }
                while elapsed >= beat_at(turn.beats) {
                    turn.beats += 1;
                }
                let stats = TurnStats { duration_ms: Some(elapsed.as_millis() as u64), ..turn.stats.clone() };
                Some((thread.clone(), format::summary(&stats).unwrap_or_default()))
            })
            .collect();
        // Without a reply context they would pile up for later; the turn's reply says it all.
        if self.reply_context().is_none() {
            return;
        }
        for (thread, summary) in due {
            self.note(&thread, &tr!("⏳ Still running · {summary}", "⏳ 仍在运行 · {summary}"));
        }
    }

    fn set_options(&mut self, o: Options) {
        if o.notify_desktop && !self.saved.options.notify_desktop {
            // Start from what the files hold now: nothing that ended before is reported.
            self.desktop = desktop::Watch::default();
            self.desktop_at = None;
        }
        self.saved.options = o;
    }

    /// Tells WeChat about turns that ended in sessions the bridge isn't driving.
    fn check_desktop(&mut self) {
        if crate::env::is_wsl() {
            return;
        }
        let live = &self.live;
        let ends = self.desktop.check(|t| live.get(t).is_some_and(|l| l.loaded || l.turn.is_some()));
        for e in ends {
            let live = self.live.entry(e.thread.clone()).or_default();
            if !e.title.is_empty() {
                live.title = e.title.clone();
            }
            live.updated = now();
            let no = self.number(&e.thread);
            let body = desktop_notice(&e, no);
            if self.reply_context().is_some() {
                self.note(&e.thread, &body);
            } else {
                self.held_notices.insert(e.thread, body);
            }
        }
    }

    // ------------------------------------------------------------ sessions

    fn number(&mut self, thread: &str) -> u32 {
        if let Some((no, _)) = self.saved.numbers.iter().find(|(_, t)| *t == thread) {
            return *no;
        }
        let no = self.saved.numbers.keys().next_back().copied().unwrap_or(0) + 1;
        let t = thread.to_string();
        self.save(|s| {
            s.numbers.insert(no, t);
        });
        no
    }

    fn thread_of(&self, no: u32) -> Option<String> {
        self.saved.numbers.get(&no).cloned()
    }

    fn codex(&mut self) -> Result<&AppServer> {
        if !self.codex.as_ref().is_some_and(|c| c.alive()) {
            let exes = crate::process::codex_clis();
            if exes.is_empty() {
                bail!("{}", l("No Codex program found on this computer", "这台电脑上没有找到 Codex 程序"));
            }
            // Some systems won't let other programs run the copy inside WindowsApps: try the next.
            let mut last = None;
            for exe in &exes {
                self.generation += 1;
                self.models = None;
                match AppServer::spawn(exe, self.events.clone(), self.generation) {
                    Ok(s) => {
                        self.codex = Some(s);
                        let shown = crate::util::display_path(exe);
                        stats(|s| s.codex_exe = Some(shown));
                        break;
                    }
                    Err(e) => {
                        crate::applog::warn("wechat", format!("app-server via {}: {e:#}", exe.display()));
                        last = Some(e);
                    }
                }
            }
            if self.codex.is_none() {
                return Err(last.expect("tried at least one"));
            }
            for l in self.live.values_mut() {
                l.loaded = false;
            }
            log("info", l("Codex started", "Codex 已启动"));
        }
        Ok(self.codex.as_ref().expect("just started"))
    }

    fn remember(&mut self, t: &Value) -> Option<String> {
        let id = t["id"].as_str()?.to_string();
        let live = self.live.entry(id.clone()).or_default();
        live.title = thread_title(t);
        live.cwd = t["cwd"].as_str().unwrap_or("").to_string();
        live.updated = t["updatedAt"].as_i64().unwrap_or(live.updated);
        if let Some(m) = t["model"].as_str() {
            live.model = m.to_string();
        }
        if let Some(e) = t["reasoningEffort"].as_str() {
            live.effort = e.to_string();
        }
        Some(id)
    }

    /// Fresh title, folder and update time of one session.
    fn refresh(&mut self, thread: &str) -> Result<()> {
        let v = self.codex()?.request("thread/read", json!({ "threadId": thread, "includeTurns": false }), CALL)?;
        self.remember(&v["thread"]).ok_or_else(|| anyhow!(l("Codex didn't return the session", "Codex 没有返回这个会话")))?;
        Ok(())
    }

    /// Whether Codex desktop (running when `desktop`) may have this session open: it
    /// changed lately, and not through the bridge.
    fn locked(&self, thread: &str, desktop: bool) -> bool {
        let Some(live) = self.live.get(thread) else { return false };
        let ours = self.saved.owned.get(thread).is_some_and(|t| live.updated <= t + 60);
        desktop && !live.loaded && !ours && now() - live.updated < LOCK_WINDOW
    }

    /// Why sending to this session could clash with Codex desktop, if it could.
    fn lock_reason(&self, thread: &str) -> Option<String> {
        let live = self.live.get(thread)?;
        // Asks the system for Codex's process only when the rest of the rule holds.
        (self.locked(thread, true) && crate::process::detect_codex().running).then(|| format::ago(live.updated, now()))
    }

    fn list(&mut self, n: usize) -> Result<String> {
        let v = self.codex()?.request(
            "thread/list",
            // Every provider; sessions from the desktop app, the CLI and this bridge.
            json!({ "limit": n, "sortKey": "updated_at", "modelProviders": [], "sourceKinds": ["vscode", "cli", "appServer"], "useStateDbOnly": true }),
            CALL,
        )?;
        let threads = v["data"].as_array().cloned().unwrap_or_default();
        if threads.is_empty() {
            return Ok(l("No Codex sessions yet. Send /new to start one.", "还没有 Codex 会话。发 /new 新建一个。").into());
        }
        let desktop = crate::process::detect_codex().running;
        let mut lines = vec![l("Recent Codex sessions:", "最近的 Codex 会话：").to_string()];
        for t in &threads {
            let Some(id) = self.remember(t) else { continue };
            let no = self.number(&id);
            let live = &self.live[&id];
            let mark = if self.saved.current.as_deref() == Some(id.as_str()) { "▶ " } else { "" };
            let state = if live.turn.is_some() {
                " ⏳"
            } else if self.locked(&id, desktop) {
                " 🔒"
            } else {
                ""
            };
            lines.push(format!(
                "{mark}#{no} {} · {} · {}{state}",
                format::short_title(&live.title),
                format::folder_name(&live.cwd),
                format::ago(live.updated, now())
            ));
        }
        lines.push(String::new());
        lines.push(l("/use N or just N switch · #N text send once · /new start one · 🔒 may be open in Codex", "/use 编号（或直接回编号）切换 · #编号 消息 临时发送 · /new 新建 · 🔒 可能正开在 Codex 里").into());
        self.pick = Some(Pick::Sessions);
        Ok(lines.join("\n"))
    }

    fn use_session(&mut self, no: u32, force: bool) -> Result<String> {
        let thread = self.thread_of(no).ok_or_else(|| anyhow!(tr!("There is no session #{no}. Send /ls to see the list", "没有 #{no} 这个会话。发 /ls 查看列表")))?;
        self.refresh(&thread)?;
        if !force {
            if let Some(when) = self.lock_reason(&thread) {
                bail!("{}", locked_text(no, &when));
            }
        }
        self.save(|s| s.current = Some(thread.clone()));
        let live = &self.live[&thread];
        Ok(tr!(
            "Now talking to {tag}\nFolder: {cwd}\nUpdated {ago}",
            "已切换到 {tag}\n目录：{cwd}\n更新于 {ago}",
            tag = format::tag(no, &live.title),
            cwd = live.cwd,
            ago = format::ago(live.updated, now())
        ))
    }

    /// Folders of recent sessions, newest first, plus the default folder for new sessions.
    fn recent_dirs(&mut self) -> Result<Vec<String>> {
        let v = self.codex()?.request(
            "thread/list",
            json!({ "limit": 60, "sortKey": "updated_at", "modelProviders": [], "sourceKinds": ["vscode", "cli", "appServer"], "useStateDbOnly": true }),
            CALL,
        )?;
        let cwds: Vec<String> = v["data"].as_array().into_iter().flatten().filter_map(|t| t["cwd"].as_str().map(String::from)).collect();
        let default = super::load_saved().default_cwd.unwrap_or_else(|| super::fallback_cwd().to_string_lossy().into_owned());
        Ok(with_default(distinct_dirs(cwds.iter().map(String::as_str), DIRS, |p| std::path::Path::new(p).is_dir()), &default, DIRS))
    }

    fn dirs_text(&mut self) -> Result<String> {
        let dirs = self.recent_dirs()?;
        let mut lines = vec![l("Recent folders:", "最近的工作目录：").to_string()];
        for (i, d) in dirs.iter().enumerate() {
            lines.push(format!("{}. {} · {d}", i + 1, format::folder_name(d)));
        }
        lines.push(l("/new N (or just N) starts a session in folder N", "/new 编号（或直接回编号）在对应目录新建会话").into());
        self.dirs = dirs;
        self.pick = Some(Pick::Dirs);
        Ok(lines.join("\n"))
    }

    fn new_session(&mut self, path: Option<String>) -> Result<String> {
        // `/new 2`: the second folder of `/dirs`.
        let path = match path.as_deref().map(str::trim).and_then(|p| p.parse::<usize>().ok()) {
            Some(n) => {
                let dirs = if self.dirs.is_empty() { self.recent_dirs()? } else { self.dirs.clone() };
                Some(n.checked_sub(1).and_then(|i| dirs.get(i)).cloned().ok_or_else(|| {
                    anyhow!(tr!("There is no folder {n}. Send /dirs to see the list", "没有第 {n} 个目录。发 /dirs 查看列表"))
                })?)
            }
            None => path,
        };
        let cwd = path
            .or_else(|| self.saved.current.as_ref().and_then(|t| self.live.get(t)).map(|l| l.cwd.clone()).filter(|c| !c.is_empty()))
            .or_else(|| super::load_saved().default_cwd)
            .unwrap_or_else(|| {
                let dir = super::fallback_cwd();
                let _ = std::fs::create_dir_all(&dir);
                dir.to_string_lossy().into_owned()
            });
        let cwd = cwd.trim().trim_matches('"').to_string();
        if !std::path::Path::new(&cwd).is_dir() {
            bail!("{}", tr!("Folder not found: {cwd}", "找不到文件夹：{cwd}"));
        }
        let v = self.codex()?.request("thread/start", json!({ "cwd": cwd }), CALL)?;
        let id = self.remember(&v["thread"]).ok_or_else(|| anyhow!(l("Codex didn't start a session", "Codex 没有新建会话")))?;
        let no = self.number(&id);
        if let Some(l) = self.live.get_mut(&id) {
            l.loaded = true;
            l.updated = now();
        }
        self.save(|s| {
            s.current = Some(id.clone());
            s.owned.insert(id.clone(), now());
            if let Some(m) = s.options.default_mode.clone() {
                s.modes.insert(id.clone(), m);
            }
        });
        Ok(tr!(
            "New session #{no} in {cwd}. Send a message to start.",
            "已新建会话 #{no}，目录：{cwd}。直接发消息开始。"
        ))
    }

    /// Sends `text` to a session as a turn, or queues it while one runs.
    fn send_to(&mut self, thread: &str, text: String, force: bool) -> Result<()> {
        let no = self.number(thread);
        if let Some(live) = self.live.get_mut(thread) {
            if live.turn.is_some() {
                live.queue.push(text);
                live.queue_images.append(&mut self.attach);
                let n = live.queue.len();
                self.say(tr!("Queued for #{no} ({n} waiting); it runs when the current turn ends. #{no} /now sends it into this turn now · /stop {no} interrupts.", "已排队到 #{no}（共 {n} 条），当前这轮结束后发送。#{no} /now 立即插入这一轮 · /stop {no} 中断。"));
                return Ok(());
            }
        }
        if !self.live.get(thread).is_some_and(|l| l.loaded) {
            self.refresh(thread)?;
            if !force {
                if let Some(when) = self.lock_reason(thread) {
                    bail!("{}", locked_text(no, &when));
                }
            }
            self.codex()?.request("thread/resume", json!({ "threadId": thread, "excludeTurns": true }), CALL)?;
            if let Some(l) = self.live.get_mut(thread) {
                l.loaded = true;
            }
        }
        self.start_turn(thread, text)
    }

    fn start_turn(&mut self, thread: &str, text: String) -> Result<()> {
        let mut input = vec![json!({ "type": "text", "text": text, "text_elements": [] })];
        input.extend(std::mem::take(&mut self.attach).into_iter().map(|p| json!({ "type": "localImage", "path": p })));
        let mut params = json!({ "threadId": thread, "input": input });
        let next = self.saved.next.get(thread).cloned().unwrap_or_default();
        if let Some(m) = &next.model {
            params["model"] = json!(m);
        }
        if let Some(e) = &next.effort {
            params["effort"] = json!(e);
        }
        if let Some((approval, sandbox)) = self.saved.modes.get(thread).and_then(|m| mode_policy(m)) {
            params["approvalPolicy"] = approval;
            params["sandboxPolicy"] = sandbox;
        }
        let v = self.codex()?.request("turn/start", params, CALL)?;
        let live = self.live.entry(thread.to_string()).or_default();
        if let Some(m) = next.model {
            live.model = m;
        }
        if let Some(e) = next.effort {
            live.effort = e;
        }
        live.turn = Some(Turn::new(v["turn"]["id"].as_str().map(String::from)));
        let t = thread.to_string();
        self.save(|s| {
            s.next.remove(&t);
            s.owned.insert(t, now());
        });
        self.start_typing();
        Ok(())
    }

    /// The session a command names, or the current one.
    fn session(&self, no: Option<u32>) -> Result<String> {
        match no {
            Some(n) => self.thread_of(n).ok_or_else(|| anyhow!(tr!("There is no session #{n}. Send /ls to see the list", "没有 #{n} 这个会话。发 /ls 查看列表"))),
            None => self.saved.current.clone().ok_or_else(|| anyhow!(l("No session selected. /use N first", "还没有选择会话，先发 /use 编号"))),
        }
    }

    fn stop(&mut self, no: Option<u32>) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let Some(turn) = self.live.get_mut(&thread).and_then(|l| l.turn.as_mut()) else {
            return Ok(tr!("#{no} isn't running", "#{no} 没有在运行"));
        };
        let Some(turn_id) = turn.id.clone() else {
            // Codex hasn't said which turn it started yet: interrupt it once it does.
            turn.stop_asked = true;
            self.live.get_mut(&thread).expect("found above").queue.clear();
            return Ok(tr!("Stopping #{no}…", "正在停止 #{no}…"));
        };
        if let Some(l) = self.live.get_mut(&thread) {
            l.queue.clear();
        }
        self.codex()?.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn_id }), CALL)?;
        Ok(tr!("Stopping #{no}…", "正在停止 #{no}…"))
    }

    fn stop_all(&mut self) -> Result<String> {
        let running: Vec<u32> = {
            let threads: Vec<String> = self.live.iter().filter(|(_, l)| l.turn.is_some()).map(|(t, _)| t.clone()).collect();
            threads.iter().map(|t| self.number(t)).collect()
        };
        if running.is_empty() {
            return Ok(l("Nothing is running.", "没有正在运行的会话。").into());
        }
        let mut failed = vec![];
        for no in &running {
            if let Err(e) = self.stop(Some(*no)) {
                failed.push(format!("#{no}: {e:#}"));
            }
        }
        let list: Vec<String> = running.iter().map(|n| format!("#{n}")).collect();
        let mut text = tr!("Stopping {list}…", "正在停止 {list}…", list = crate::i18n::join(&list));
        for f in failed {
            text.push_str(&format!("\n❗ {f}"));
        }
        Ok(text)
    }

    fn current_text(&mut self) -> String {
        match self.saved.current.clone() {
            Some(t) => {
                let no = self.number(&t);
                let (title, cwd) = self.live.get(&t).map(|l| (l.title.clone(), l.cwd.clone())).unwrap_or_default();
                let mut text = tr!("Current: {tag}", "当前：{tag}", tag = format::tag(no, &title));
                if !cwd.is_empty() {
                    text.push_str(&tr!("\nFolder: {cwd}", "\n目录：{cwd}"));
                }
                text.push_str(l("\n/use N switches · /ls lists sessions", "\n/use 编号 切换 · /ls 查看会话"));
                text
            }
            None => l("No session selected. /ls lists sessions, /use N picks one.", "还没有选择会话。发 /ls 查看会话，/use 编号 选择。").into(),
        }
    }

    fn last(&mut self, no: Option<u32>) -> Result<Option<String>> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        match self.live.get(&thread).and_then(|l| l.last_reply.clone()) {
            Some(body) => {
                self.say_from(&thread, &body);
                Ok(None)
            }
            None => Ok(Some(tr!("#{no} has no reply from this run of the bridge yet.", "#{no} 在本次连接里还没有回复。"))),
        }
    }

    fn queue(&mut self, no: Option<u32>, clear: bool) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let live = self.live.entry(thread.clone()).or_default();
        if live.queue.is_empty() {
            return Ok(tr!("Nothing is queued for #{no}.", "#{no} 没有排队的消息。"));
        }
        if clear {
            let n = std::mem::take(&mut live.queue).len();
            return Ok(trn!(n, "Dropped {n} queued message for #{no}.", "Dropped {n} queued messages for #{no}.", "已清空 #{no} 的 {n} 条排队消息。"));
        }
        let mut lines = vec![tr!("Queued for #{no} (sent together when the turn ends):", "#{no} 排队中（这一轮结束后一起发送）：")];
        for (i, q) in live.queue.iter().enumerate() {
            lines.push(format!("{}. {}", i + 1, crate::util::clip(&q.replace('\n', " "), 80)));
        }
        lines.push(tr!("/queue {no} clear drops them", "/queue {no} clear 清空"));
        Ok(lines.join("\n"))
    }

    /// `/now`: `text` (or, without text, what is queued) goes into the running turn of the
    /// session right away (`turn/steer`). Without a running turn, `text` starts one.
    fn now(&mut self, no: Option<u32>, text: Option<String>) -> Result<Option<String>> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let turn = self.live.get(&thread).and_then(|l| l.turn.as_ref()).map(|t| t.id.clone());
        let (text, from_queue) = match text {
            Some(t) => (t, false),
            None => {
                let queue = self.live.get(&thread).map(|l| l.queue.clone()).unwrap_or_default();
                if queue.is_empty() {
                    return Ok(Some(tr!("Nothing is queued for #{no}. /now text adds a message to its running turn.", "#{no} 没有排队的消息。发 /now 消息 可以插入到正在进行的这一轮。")));
                }
                (queue.join("\n\n"), true)
            }
        };
        let turn_id = match turn {
            None => {
                self.send_to(&thread, text, false)?;
                return Ok(None);
            }
            Some(None) => bail!("{}", tr!("#{no} is just starting; send it again in a moment.", "#{no} 刚开始运行，请稍后再发。")),
            Some(Some(id)) => id,
        };
        let images = if from_queue { self.live.get(&thread).map(|l| l.queue_images.clone()).unwrap_or_default() } else { vec![] };
        let mut input = vec![json!({ "type": "text", "text": text, "text_elements": [] })];
        input.extend(images.iter().map(|p| json!({ "type": "localImage", "path": p })));
        self.codex()?.request("turn/steer", json!({ "threadId": thread, "expectedTurnId": turn_id, "input": input }), CALL)?;
        let live = self.live.entry(thread.clone()).or_default();
        let sent = if from_queue {
            live.queue_images.clear();
            std::mem::take(&mut live.queue).len()
        } else {
            1
        };
        Ok(Some(trn!(sent, "Added {n} message to #{no}'s running turn.", "Added {n} messages to #{no}'s running turn.", "已把 {n} 条消息插入 #{no} 正在进行的这一轮。")))
    }

    fn rename(&mut self, no: Option<u32>, title: String) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        self.codex()?.request("thread/name/set", json!({ "threadId": thread, "name": title }), CALL)?;
        self.live.entry(thread).or_default().title = title.clone();
        Ok(tr!("Renamed #{no} to {title}", "已把 #{no} 改名为 {title}"))
    }

    fn archive(&mut self, no: Option<u32>) -> Result<String> {
        let Some(n) = no else {
            bail!("{}", usage_text(command::spec("archive").expect("listed")));
        };
        let thread = self.session(Some(n))?;
        if self.live.get(&thread).is_some_and(|l| l.turn.is_some()) {
            bail!("{}", tr!("#{n} is running. /stop {n} first", "#{n} 正在运行，先发 /stop {n}"));
        }
        self.codex()?.request("thread/archive", json!({ "threadId": thread }), CALL)?;
        self.live.remove(&thread);
        self.pending.retain(|p| p.thread != thread);
        // The number stays taken: old messages tagged with it must not reach another session.
        self.save(|s| {
            s.modes.remove(&thread);
            s.next.remove(&thread);
            if s.current.as_ref() == Some(&thread) {
                s.current = None;
            }
        });
        Ok(tr!("Archived #{n}. It can be restored in Codex desktop.", "已归档 #{n}，可以在 Codex 桌面版里恢复。"))
    }

    /// The session's folder, read from Codex when the bridge doesn't know it yet.
    fn cwd_of(&mut self, thread: &str) -> Result<String> {
        if self.live.get(thread).is_none_or(|l| l.cwd.is_empty()) {
            self.refresh(thread)?;
        }
        Ok(self.live.get(thread).map(|l| l.cwd.clone()).unwrap_or_default())
    }

    fn cwd(&mut self, no: Option<u32>) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let cwd = self.cwd_of(&thread)?;
        Ok(tr!("#{no} folder: {cwd}", "#{no} 的目录：{cwd}"))
    }

    fn diff(&mut self, no: Option<u32>) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let cwd = self.cwd_of(&thread)?;
        if let Some(stat) = git_diff_stat(&cwd)? {
            return Ok(if stat.trim().is_empty() {
                tr!("#{no}: no uncommitted changes in {cwd}", "#{no}：{cwd} 没有未提交的改动")
            } else {
                format!("{}\n```\n{}\n```", tr!("#{no} uncommitted changes:", "#{no} 未提交的改动："), clip_lines(&stat, DIFF_LINES))
            });
        }
        let files = self.live.get(&thread).map(|l| l.last_files.clone()).unwrap_or_default();
        Ok(if files.is_empty() {
            tr!("#{no}: {cwd} isn't a Git repository, and the last turn changed no files.", "#{no}：{cwd} 不是 Git 仓库，上一轮也没有改动文件。")
        } else {
            format!("{}\n{}", tr!("#{no}: files the last turn changed:", "#{no} 上一轮改动的文件："), clip_lines(&files.join("\n"), DIFF_LINES))
        })
    }

    fn mode(&mut self, no: Option<u32>, mode: Option<String>) -> Result<String> {
        let thread = self.session(no)?;
        let no = self.number(&thread);
        let Some(m) = mode else {
            let now = self.saved.modes.get(&thread).map(|m| mode_label(m).to_string()).unwrap_or_else(|| l("Codex's own setting", "Codex 自己的设置").into());
            return Ok(format!("{}\n{}", tr!("#{no} mode: {now}", "#{no} 的模式：{now}"), usage_text(command::spec("mode").expect("listed"))));
        };
        if m == "default" {
            let t = thread.clone();
            self.save(|s| {
                s.modes.remove(&t);
            });
            return Ok(tr!(
                "WeChat no longer sets a mode for #{no}. Codex may keep the last one it was given; /mode sets one again.",
                "微信不再为 #{no} 指定模式。Codex 可能沿用上一次收到的模式；发 /mode 可重新指定。"
            ));
        }
        let (t, mv) = (thread.clone(), m.clone());
        self.save(|s| {
            s.modes.insert(t, mv);
        });
        let mut text = tr!("#{no} mode: {m}, from the next message.", "#{no} 的模式已改为 {m}，下一条消息生效。", m = mode_label(&m));
        if m == "full" {
            text.push_str(l(
                "\n⚠️ Codex now runs any command and changes any file without asking. /mode auto goes back.",
                "\n⚠️ Codex 现在执行任何命令、修改任何文件都不再询问。发 /mode auto 恢复。",
            ));
        }
        Ok(text)
    }

    fn status_text(&mut self) -> String {
        let mut lines = vec![];
        let current = self.saved.current.clone();
        match &current {
            Some(t) => {
                let no = self.number(t);
                let title = self.live.get(t).map(|l| l.title.clone()).unwrap_or_default();
                lines.push(tr!("Current: {tag}", "当前：{tag}", tag = format::tag(no, &title)));
            }
            None => lines.push(l("No session selected", "还没有选择会话").into()),
        }
        let running: Vec<(String, usize, u64)> =
            self.live.iter().filter_map(|(t, l)| l.turn.as_ref().map(|turn| (t.clone(), l.queue.len(), turn.started.elapsed().as_millis() as u64))).collect();
        for (t, queued, ms) in running {
            let no = self.number(&t);
            let title = self.live.get(&t).map(|l| l.title.clone()).unwrap_or_default();
            let mut line = tr!("⏳ {tag} running for {d}", "⏳ {tag} 已运行 {d}", tag = format::tag(no, &title), d = format::duration(ms));
            if queued > 0 {
                line.push_str(&tr!(", {queued} queued", "，排队 {queued} 条"));
            }
            lines.push(line);
        }
        let waiting: Vec<String> = self.pending.iter().map(|p| p.thread.clone()).collect();
        for t in waiting {
            let no = self.number(&t);
            lines.push(tr!("⚠️ #{no} is waiting for your answer", "⚠️ #{no} 在等你确认"));
        }
        lines.join("\n")
    }

    // ------------------------------------------------------------ models

    fn models(&mut self) -> Result<Vec<Value>> {
        if self.models.is_none() {
            let v = self.codex()?.request("model/list", json!({ "limit": 200, "includeHidden": false }), CALL)?;
            self.models = Some(v["data"].as_array().cloned().unwrap_or_default());
        }
        Ok(self.models.clone().unwrap_or_default())
    }

    /// The model a reply names: its number in `/model`'s list, or its name.
    fn find_model(&mut self, arg: &str) -> Result<Value> {
        let models = self.models()?;
        pick_model(&models, arg)
            .cloned()
            .ok_or_else(|| anyhow!(tr!("No model \"{arg}\". Send /model to see the choices", "没有「{arg}」这个模型。发 /model 查看可选模型")))
    }

    fn model_cmd(&mut self, thread: Option<String>, cmd: ModelCmd) -> Result<String> {
        match cmd {
            ModelCmd::Show => self.model_overview(thread),
            ModelCmd::Default(None) => Ok(match default_model() {
                Some(m) => tr!("Default model for new sessions: {m}", "新会话的默认模型：{m}"),
                None => l("No default model set in config.toml; Codex picks one.", "config.toml 里没有设置默认模型，由 Codex 自己选择。").into(),
            }),
            ModelCmd::Default(Some(arg)) => {
                let name = model_name(&self.find_model(&arg)?);
                set_default_model(&name)?;
                Ok(tr!(
                    "Default model is now {name}. New sessions use it; existing ones keep theirs.",
                    "默认模型已改为 {name}。之后新建的会话使用它，已有会话不变。"
                ))
            }
            ModelCmd::Set(arg) => {
                let thread = thread.ok_or_else(|| anyhow!(l("No session selected. /use N first", "还没有选择会话，先发 /use 编号")))?;
                let m = self.find_model(&arg)?;
                let name = model_name(&m);
                let no = self.number(&thread);
                // An effort the new model doesn't support would be refused: let Codex pick one.
                let effort = self.saved.next.get(&thread).and_then(|n| n.effort.clone()).or_else(|| self.live.get(&thread).map(|l| l.effort.clone()));
                let keep = effort.is_some_and(|e| supports_effort(&m, &e));
                let (t, n) = (thread.clone(), name.clone());
                self.save(|s| {
                    let next = s.next.entry(t).or_default();
                    next.model = Some(n);
                    if !keep {
                        next.effort = None;
                    }
                });
                Ok(if self.live.get(&thread).is_some_and(|l| l.turn.is_some()) {
                    tr!("#{no} switches to {name} after the running turn.", "#{no} 将在当前这轮结束后改用 {name}。")
                } else {
                    tr!("#{no} switched to {name}, from your next message.", "#{no} 已切换为 {name}，下一条消息生效。")
                })
            }
        }
    }

    fn model_overview(&mut self, thread: Option<String>) -> Result<String> {
        let models = self.models()?;
        let mut lines = vec![];
        let mut using = String::new();
        if let Some(t) = &thread {
            self.refresh(t)?;
            let no = self.number(t);
            let live = &self.live[t];
            using = live.model.clone();
            lines.push(format::tag(no, &live.title));
            lines.push(tr!("Now: {m} · {e}", "当前：{m} · {e}", m = or_dash(&live.model), e = or_dash(&live.effort)));
            if let Some(n) = self.saved.next.get(t) {
                let m = n.model.clone().unwrap_or_else(|| live.model.clone());
                let e = n.effort.clone().unwrap_or_else(|| l("default", "默认").into());
                lines.push(tr!("From the next message: {m} · {e}", "下一条消息起：{m} · {e}"));
                using = m;
            }
        } else {
            lines.push(l("No session selected: /use N first to switch its model.", "还没有选择会话：先发 /use 编号，才能切换它的模型。").into());
        }
        if let Some(d) = default_model() {
            lines.push(tr!("Default (config.toml): {d}", "默认（config.toml）：{d}"));
        }
        lines.push(l("Models:", "可选模型：").into());
        for (i, m) in models.iter().enumerate() {
            let name = model_name(m);
            let mark = if name == using { " ✓" } else { "" };
            let efforts = efforts_of(m);
            let efforts = if efforts.is_empty() { String::new() } else { format!(" · {}", efforts.join("/")) };
            lines.push(format!("{}. {name}{mark}{efforts}", i + 1));
        }
        lines.push(
            l(
                "/model N switch this session · /effort high reasoning · /model default N new sessions' default",
                "/model 编号 切换当前会话 · /effort high 推理强度 · /model default 编号 改新会话的默认",
            )
            .into(),
        );
        self.pick = Some(Pick::Models(thread));
        Ok(lines.join("\n"))
    }

    fn effort_cmd(&mut self, thread: Option<String>, effort: Option<String>) -> Result<String> {
        let thread = thread.ok_or_else(|| anyhow!(l("No session selected. /use N first", "还没有选择会话，先发 /use 编号")))?;
        if !self.live.contains_key(&thread) {
            self.refresh(&thread)?;
        }
        let model = self.saved.next.get(&thread).and_then(|n| n.model.clone()).unwrap_or_else(|| self.live[&thread].model.clone());
        let models = self.models()?;
        let choices = pick_model(&models, &model).map(efforts_of).unwrap_or_default();
        let no = self.number(&thread);
        let Some(e) = effort.map(|e| e.trim().to_lowercase()) else {
            let now = or_dash(&self.live[&thread].effort).to_string();
            let c = choices.join(" / ");
            return Ok(tr!("#{no} reasoning: {now}. Choices for {model}: {c}", "#{no} 推理强度：{now}。{model} 可选：{c}", c = or_dash(&c)));
        };
        if e == "default" {
            let t = thread.clone();
            let had = self.saved.next.get(&t).is_some_and(|n| n.effort.is_some());
            self.save(|s| {
                if let Some(n) = s.next.get_mut(&t) {
                    n.effort = None;
                    if n.model.is_none() {
                        s.next.remove(&t);
                    }
                }
            });
            let now = or_dash(&self.live[&thread].effort).to_string();
            return Ok(if had {
                tr!("#{no} keeps its reasoning effort ({now}).", "#{no} 保持原来的推理强度（{now}）。")
            } else {
                tr!("#{no} has no reasoning change waiting; it uses {now}.", "#{no} 没有待生效的推理强度修改，当前为 {now}。")
            });
        }
        if !choices.is_empty() && !choices.contains(&e) {
            bail!("{}", tr!("{model} supports {c}", "{model} 支持的推理强度：{c}", c = choices.join(" / ")));
        }
        let (t, ev) = (thread.clone(), e.clone());
        self.save(|s| s.next.entry(t).or_default().effort = Some(ev));
        Ok(tr!("#{no} reasoning set to {e}, from your next message.", "#{no} 推理强度已改为 {e}，下一条消息生效。"))
    }

    // ------------------------------------------------------------ inbound

    fn session_of_quote(&self, m: &Inbound) -> Option<u32> {
        if let Some(id) = &m.quote_id {
            if let Some((_, no)) = self.saved.quotes.iter().rev().find(|(q, _)| q == id) {
                return Some(*no);
            }
        }
        m.quote_text.as_deref().and_then(command::tag_in)
    }

    fn on_inbound(&mut self, m: Inbound) {
        if m.from != self.user {
            log("info", tr!("Ignored a message from another WeChat user", "忽略了其他微信用户的消息"));
            return;
        }
        if let Some(ctx) = m.context_token.clone() {
            if self.saved.context_token.as_ref() != Some(&ctx) {
                self.save(|s| s.context_token = Some(ctx));
            }
        }
        self.flush_unsent();
        let mut m = m;
        let shown = match (m.text.trim().is_empty(), m.images.is_empty(), m.media) {
            (true, false, _) => trn!(m.images.len(), "[{n} picture]", "[{n} pictures]", "[{n} 张图片]"),
            (true, true, true) => l("[media]", "[媒体]").to_string(),
            _ => m.text.clone(),
        };
        log("in", shown);
        if m.voice_blank {
            self.say(l("WeChat sent no text for that voice message. Say it again, or type it.", "微信没有给出这条语音的文字，请重说一遍或直接打字。"));
        }
        if m.text.trim().is_empty() {
            if m.images.is_empty() {
                if m.media {
                    self.say(l("Only text, voice and pictures can be passed to Codex for now.", "目前只能把文字、语音和图片转给 Codex。"));
                }
                return;
            }
            // A picture alone goes to the current session with a line for Codex.
            m.text = IMAGE_ONLY.to_string();
        }
        if m.media {
            self.say(l("Files and videos can't be passed to Codex yet; the rest of that message is sent.", "文件和视频暂时无法转给 Codex，这条消息的其他内容已发送。"));
        }
        // Messages keep their order: later ones wait while pictures download, except those
        // that start no turn (stop, approvals…), which mustn't wait for a slow download.
        if self.downloading && !self.urgent(&m) {
            self.held.push_back(m);
            return;
        }
        self.dispatch(m);
    }

    /// A message that acts right away and never starts a turn, so it can't change the order
    /// of what reaches a session.
    fn urgent(&self, m: &Inbound) -> bool {
        if !m.images.is_empty() {
            return false;
        }
        match command::parse(&m.text) {
            Cmd::Stop(_) | Cmd::StopAll | Cmd::Status | Cmd::Help(_) | Cmd::Queue { .. } | Cmd::Last(_) | Cmd::Current | Cmd::Usage(_) | Cmd::Unknown(..) => true,
            // An answer only while something waits for one; otherwise it is text for a session.
            Cmd::Approve { .. } => self.pending.iter().any(|p| !matches!(p.ask, Ask::Questions(_))) || !self.deferred.is_empty() || !self.retry.is_empty(),
            _ => false,
        }
    }

    /// A message with pictures waits for them (downloaded in the background); others go on.
    fn dispatch(&mut self, m: Inbound) {
        if m.images.is_empty() {
            return self.process(m);
        }
        self.downloading = true;
        let tx = self.events.clone();
        let spawned = std::thread::Builder::new().name("agentplus-wechat-pictures".into()).spawn(move || {
            let images = m.images.clone();
            // Whatever happens here, the bridge must hear back: it holds later messages until then.
            let got = std::panic::catch_unwind(|| {
                let mut paths = vec![];
                let mut failed = 0;
                for img in &images {
                    match ilink::download_image(img).and_then(|data| super::save_media(&data, ilink::image_ext(&data))) {
                        Ok(p) => paths.push(p),
                        Err(e) => {
                            failed += 1;
                            crate::applog::warn("wechat", format!("picture: {e:#}"));
                            log("error", format!("{e:#}"));
                        }
                    }
                }
                (paths, failed)
            });
            let (paths, failed) = got.unwrap_or_else(|_| {
                crate::applog::error("wechat", "picture download panicked");
                (vec![], images.len())
            });
            let _ = tx.send(Event::Pictures { message: m, paths, failed });
        });
        if let Err(e) = spawned {
            self.downloading = false;
            self.say(format!("❗ {e:#}"));
        }
    }

    /// The pictures of a message are in: it goes on with them, then the messages that waited.
    fn on_pictures(&mut self, mut m: Inbound, paths: Vec<std::path::PathBuf>, failed: usize) {
        self.downloading = false;
        m.images.clear();
        if paths.is_empty() && m.text == IMAGE_ONLY {
            self.say(l("❗ The picture couldn't be downloaded from WeChat; nothing was sent. Send it again.", "❗ 无法从微信下载图片，消息没有发送，请重发。"));
        } else {
            if failed > 0 {
                self.say(trn!(failed, "❗ {n} picture couldn't be downloaded; the rest was sent.", "❗ {n} pictures couldn't be downloaded; the rest was sent.", "❗ 有 {n} 张图片下载失败，其余内容已发送。"));
            }
            m.pictures = paths;
            self.process(m);
        }
        while !self.downloading {
            let Some(next) = self.held.pop_front() else { break };
            self.dispatch(next);
        }
    }

    /// A message ready to act on.
    fn process(&mut self, m: Inbound) {
        let (cmd, target) = self.resolve(&m.text);
        if !self.codex.as_ref().is_some_and(|c| c.alive()) {
            // The answer to "Start Codex?".
            if !self.deferred.is_empty() || !self.retry.is_empty() {
                match cmd {
                    Cmd::Approve { decision: Decision::Decline, .. } => {
                        let n = std::mem::take(&mut self.deferred).len() + std::mem::take(&mut self.retry).len();
                        self.say(trn!(n, "OK, Codex stays off; {n} message dropped.", "OK, Codex stays off; {n} messages dropped.", "好的，先不启动 Codex，{n} 条消息已取消。"));
                        return;
                    }
                    Cmd::Approve { .. } => return self.start_codex_now(),
                    _ => {}
                }
            }
            if self.needs_codex(&cmd, &m) {
                self.deferred.push(m);
                let n = self.deferred.len() + self.retry.len();
                self.say(if n == 1 {
                    l("Codex isn't running. Reply y to start it and run your message, or n to cancel.", "Codex 还没启动。回复 y 启动并执行刚才的消息，回复 n 取消。").to_string()
                } else {
                    tr!("Codex isn't running; {n} messages are waiting. Reply y to start it and run them in order, or n to cancel.", "Codex 还没启动，已有 {n} 条消息在等。回复 y 启动并按顺序执行，回复 n 全部取消。")
                });
                return;
            }
        }
        self.handle(m, cmd, target);
    }

    /// The command a message asks for. A bare number right after a list picks from that list
    /// (unless a session waits for an answer: then it is that answer). Also returns the
    /// session a picked model is for.
    fn resolve(&mut self, text: &str) -> (Cmd, Option<String>) {
        let cmd = command::parse(text);
        let pick = self.pick.take();
        match (&cmd, pick, text.trim().parse::<u32>()) {
            (Cmd::Text(_), Some(p), Ok(n)) if n > 0 && !self.pending.iter().any(|x| matches!(x.ask, Ask::Questions(_))) => pick_cmd(&p, n),
            _ => (cmd, None),
        }
    }

    /// Whether this message has to reach Codex (and so start it).
    fn needs_codex(&self, cmd: &Cmd, m: &Inbound) -> bool {
        match cmd {
            Cmd::Help(_) | Cmd::Status | Cmd::Unknown(..) | Cmd::Usage(_) | Cmd::Stop(_) | Cmd::StopAll | Cmd::Current | Cmd::Last(_) | Cmd::Queue { .. } | Cmd::Mode { .. } => false,
            Cmd::List(_) | Cmd::Dirs | Cmd::New(_) | Cmd::Model(_) | Cmd::Effort(_) | Cmd::Rename { .. } | Cmd::Archive(_) | Cmd::Diff(_) | Cmd::Cwd(_) | Cmd::Now { .. } => true,
            // A session number that doesn't exist fails without Codex.
            Cmd::Use { no, .. } | Cmd::To { no, .. } => self.thread_of(*no).is_some(),
            // Text only goes somewhere with a session to go to.
            Cmd::Text(_) | Cmd::Approve { .. } => self.saved.current.is_some() || self.session_of_quote(m).is_some_and(|n| self.thread_of(n).is_some()),
        }
    }

    /// Starts Codex (asked from WeChat or the page), then runs the message that waited for it.
    fn start_codex_now(&mut self) {
        let deferred = std::mem::take(&mut self.deferred);
        let retry = std::mem::take(&mut self.retry);
        match self.codex().map(|_| ()) {
            Ok(()) => {
                self.say(l("Codex started.", "Codex 已启动。"));
                for m in deferred {
                    let (cmd, target) = self.resolve(&m.text);
                    self.handle(m, cmd, target);
                }
                let mut failed = vec![];
                for (thread, text) in retry {
                    if let Err(e) = self.send_to(&thread, text.clone(), false) {
                        self.say(format!("❗ {e:#}"));
                        failed.push((thread, text));
                    }
                }
                self.retry = failed;
            }
            Err(e) => {
                // Starting Codex is retryable; keep the messages that were waiting for it.
                self.deferred = deferred;
                self.retry = retry;
                self.say(format!("❗ {e:#}"));
            }
        }
    }

    /// `target`: the session a model picked from `/model`'s list is for.
    fn handle(&mut self, m: Inbound, cmd: Cmd, target: Option<String>) {
        let quoted = self.session_of_quote(&m);
        // Pictures go with the text of this message; a command doesn't use them.
        self.attach = if matches!(cmd, Cmd::Text(_) | Cmd::To { .. } | Cmd::Approve { .. }) { m.pictures.clone() } else { vec![] };
        let r = match cmd {
            Cmd::To { no, force, text } => match self.thread_of(no) {
                // `#3 /stop`: a command for that session; it never goes to Codex as text.
                Some(t) if text.starts_with(['/', '／']) => match command::for_session(command::parse(&text), no) {
                    Some(c) => self.run_cmd(c, Some(t)),
                    None => Ok(Some(tr!("{text} doesn't take a session with #{no}. Send it on its own.", "{text} 不能配合 #{no} 使用，请单独发送。"))),
                },
                Some(t) => self.send_or_answer(&t, with_quote(&m, &text, quoted.is_some()), force).map(|_| None),
                None => Err(anyhow!(tr!("There is no session #{no}. Send /ls to see the list", "没有 #{no} 这个会话。发 /ls 查看列表"))),
            },
            Cmd::Approve { no, decision } => match self.answer_approval(no.or(quoted), decision) {
                Some(r) => r.map(Some),
                None => self.route_text(with_quote(&m, m.text.trim(), quoted.is_some()), quoted, false),
            },
            Cmd::Text(t) => self.route_text(with_quote(&m, &t, quoted.is_some()), quoted, false),
            // A model picked from the list goes to the session the list was for; otherwise a
            // quoted session, then the current one.
            c => {
                let session = target.or_else(|| quoted.and_then(|n| self.thread_of(n)));
                self.run_cmd(c, session)
            }
        };
        match r {
            Ok(Some(text)) => self.say(text),
            Ok(None) => {}
            Err(e) => self.say(format!("❗ {e:#}")),
        }
    }

    /// A command. `session`: the one it is aimed at (`#3 /stop`, a quoted message), else the
    /// current one.
    fn run_cmd(&mut self, cmd: Cmd, session: Option<String>) -> Result<Option<String>> {
        let no_of = |b: &Self, n: Option<u32>| n.or_else(|| session.as_ref().and_then(|t| b.saved.numbers.iter().find(|(_, x)| *x == t).map(|(n, _)| *n)));
        match cmd {
            Cmd::Help(None) => Ok(Some(help_text())),
            Cmd::Help(Some(s)) => Ok(Some(help_of(s))),
            Cmd::Usage(s) => Ok(Some(usage_text(s))),
            Cmd::Unknown(c, Some(near)) => Ok(Some(tr!("Unknown command {c}. Did you mean /{near}? Send /help for the list.", "不认识的命令 {c}，是要发 /{near} 吗？发 /help 查看说明。"))),
            Cmd::Unknown(c, None) => Ok(Some(tr!("Unknown command {c}. Send /help for the list.", "不认识的命令 {c}。发 /help 查看说明。"))),
            Cmd::List(n) => self.list(n.unwrap_or(LIST).min(30)).map(Some),
            Cmd::Use { no, force } => self.use_session(no, force).map(Some),
            Cmd::Current => Ok(Some(self.current_text())),
            Cmd::New(p) => self.new_session(p).map(Some),
            Cmd::Dirs => self.dirs_text().map(Some),
            Cmd::Stop(no) => {
                let no = no_of(self, no);
                self.stop(no).map(Some)
            }
            Cmd::StopAll => self.stop_all().map(Some),
            Cmd::Status => Ok(Some(self.status_text())),
            Cmd::Last(no) => {
                let no = no_of(self, no);
                self.last(no)
            }
            Cmd::Queue { no, clear } => {
                let no = no_of(self, no);
                self.queue(no, clear).map(Some)
            }
            Cmd::Now { no, text } => {
                let no = no_of(self, no);
                self.now(no, text)
            }
            Cmd::Rename { no, title } => {
                let no = no_of(self, no);
                self.rename(no, title).map(Some)
            }
            Cmd::Archive(no) => self.archive(no).map(Some),
            Cmd::Diff(no) => {
                let no = no_of(self, no);
                self.diff(no).map(Some)
            }
            Cmd::Cwd(no) => {
                let no = no_of(self, no);
                self.cwd(no).map(Some)
            }
            Cmd::Mode { no, mode } => {
                let no = no_of(self, no);
                self.mode(no, mode).map(Some)
            }
            Cmd::Model(c) => self.model_cmd(session.or_else(|| self.saved.current.clone()), c).map(Some),
            Cmd::Effort(e) => self.effort_cmd(session.or_else(|| self.saved.current.clone()), e).map(Some),
            Cmd::To { .. } | Cmd::Approve { .. } | Cmd::Text(_) => unreachable!("handled by handle()"),
        }
    }

    fn route_text(&mut self, text: String, quoted: Option<u32>, force: bool) -> Result<Option<String>> {
        let thread = match quoted.and_then(|n| self.thread_of(n)) {
            Some(t) => t,
            None => match self.saved.current.clone() {
                Some(t) => t,
                None => {
                    return Ok(Some(l(
                        "No session selected. Send /ls to see recent sessions, then /use N; or /new to start one. /help lists everything.",
                        "还没有选择会话。发 /ls 查看最近的会话，再发 /use 编号 选择；或发 /new 新建。/help 查看全部命令。",
                    ).into()))
                }
            },
        };
        self.send_or_answer(&thread, text, force)?;
        Ok(None)
    }

    /// Text for a session: the answer to its open question, else a new turn.
    fn send_or_answer(&mut self, thread: &str, text: String, force: bool) -> Result<()> {
        if let Some(i) = self.pending.iter().position(|p| p.thread == thread && matches!(p.ask, Ask::Questions(_))) {
            let Ask::Questions(qs) = &self.pending[i].ask else { unreachable!() };
            match question_answers(qs, &text) {
                Ok(answers) => {
                    let p = self.pending.remove(i);
                    self.codex()?.respond(&p.id, json!({ "answers": answers }))?;
                }
                Err(missing) => {
                    let (n, total) = (missing.len(), qs.len());
                    let mut body = tr!("Codex asked {total} questions; {n} have no answer yet:", "Codex 问了 {total} 个问题，还有 {n} 个没有回答：");
                    for q in &missing {
                        body.push_str(&format!("\n- {q}"));
                    }
                    body.push_str(l("\nReply again with every answer, one per line.", "\n请重新回复，每行回答一个问题。"));
                    self.note(thread, &body);
                }
            }
            return Ok(());
        }
        self.send_to(thread, text, force)
    }

    /// None when there is nothing to approve (the text is then an ordinary message).
    fn answer_approval(&mut self, no: Option<u32>, d: Decision) -> Option<Result<String>> {
        let approvals: Vec<usize> = (0..self.pending.len()).filter(|i| !matches!(self.pending[*i].ask, Ask::Questions(_))).collect();
        let pick = match no {
            Some(n) => {
                let t = self.thread_of(n)?;
                approvals.into_iter().find(|i| self.pending[*i].thread == t)?
            }
            None => match approvals.as_slice() {
                [] => return None,
                [one] => *one,
                many => {
                    let threads: Vec<String> = many.iter().map(|i| self.pending[*i].thread.clone()).collect();
                    let nos: Vec<String> = threads.iter().map(|t| format!("#{}", self.number(t))).collect();
                    return Some(Ok(tr!(
                        "Several sessions are waiting ({list}). Add the number, e.g. y{first}.",
                        "有多个会话在等确认（{list}），请带上编号，例如 y{first}。",
                        list = crate::i18n::join(&nos),
                        first = nos[0].trim_start_matches('#')
                    )));
                }
            },
        };
        let p = self.pending.remove(pick);
        if d == Decision::Always && auto_reply(&p.ask).is_some() {
            self.live.entry(p.thread.clone()).or_default().allow_all = true;
        }
        let result = match &p.ask {
            Ask::Command | Ask::File => json!({ "decision": match d { Decision::Accept => "accept", Decision::Always => "acceptForSession", Decision::Decline => "decline" } }),
            Ask::Permissions(asked) => match d {
                Decision::Decline => json!({ "permissions": {}, "scope": "turn" }),
                _ => json!({ "permissions": granted(asked), "scope": if d == Decision::Always { "session" } else { "turn" } }),
            },
            Ask::Questions(_) => unreachable!(),
        };
        let no = self.number(&p.thread);
        Some(self.codex().and_then(|c| c.respond(&p.id, result)).map(|_| match d {
            Decision::Accept => tr!("Allowed #{no}", "已允许 #{no}"),
            Decision::Always => tr!("Allowed #{no} for the rest of the session", "已允许 #{no}，本会话内不再询问"),
            Decision::Decline => tr!("Declined #{no}", "已拒绝 #{no}"),
        }))
    }

    // ------------------------------------------------------------ from Codex

    fn on_notify(&mut self, method: &str, p: &Value) {
        let thread = p["threadId"].as_str().unwrap_or("").to_string();
        match method {
            "turn/started" => {
                let Some(t) = self.live.get_mut(&thread).and_then(|l| l.turn.as_mut()) else { return };
                t.id = t.id.clone().or_else(|| p["turn"]["id"].as_str().map(String::from));
                if let (true, Some(id)) = (std::mem::take(&mut t.stop_asked), t.id.clone()) {
                    if let Err(e) = self.codex().and_then(|c| c.request("turn/interrupt", json!({ "threadId": thread, "turnId": id }), CALL)) {
                        self.note(&thread, &format!("❗ {e:#}"));
                    }
                }
            }
            "item/started" => {
                let item = &p["item"];
                if item["type"] == "fileChange" {
                    let paths = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str().map(String::from)).collect();
                    if let Some(id) = item["id"].as_str() {
                        self.changes.insert(id.to_string(), paths);
                    }
                }
            }
            "item/completed" => {
                let item = &p["item"];
                let relay = self.saved.options.relay_progress;
                let Some(turn) = self.live.get_mut(&thread).and_then(|l| l.turn.as_mut()) else { return };
                match item["type"].as_str().unwrap_or("") {
                    "agentMessage" => {
                        let text = item["text"].as_str().unwrap_or("").to_string();
                        let is_final = item["phase"] == "final_answer";
                        if is_final {
                            turn.final_answer = Some(text.clone());
                        }
                        if !text.trim().is_empty() {
                            turn.last_message = Some(text.clone());
                            let relayed = relay && !is_final;
                            turn.relayed = relayed;
                            if relayed {
                                self.say_from(&thread, &text);
                            }
                        }
                    }
                    "commandExecution" => turn.stats.commands += 1,
                    "fileChange" => {
                        if item["status"] != "declined" {
                            for c in item["changes"].as_array().into_iter().flatten() {
                                if let Some(path) = c["path"].as_str() {
                                    turn.stats.files.insert(path.to_string());
                                }
                            }
                        }
                        if let Some(id) = item["id"].as_str() {
                            self.changes.remove(id);
                        }
                    }
                    "mcpToolCall" | "dynamicToolCall" | "webSearch" => turn.stats.tools += 1,
                    _ => {}
                }
            }
            "turn/completed" => self.finish_turn(&thread, &p["turn"]),
            "thread/name/updated" => {
                if let (Some(l), Some(name)) = (self.live.get_mut(&thread), p["threadName"].as_str()) {
                    l.title = name.to_string();
                }
            }
            "serverRequest/resolved" => {
                let id = &p["requestId"];
                self.pending.retain(|x| &x.id != id);
            }
            "error" if p["willRetry"] != true => {
                let msg = p["error"]["message"].as_str().unwrap_or(l("Codex error", "Codex 出错")).to_string();
                log("error", &msg);
                // A failed turn reports its error when it ends; others would go unseen.
                if self.live.get(&thread).is_some_and(|l| l.turn.is_none()) {
                    self.note(&thread, &tr!("❗ Codex: {msg}", "❗ Codex：{msg}"));
                }
            }
            _ => {}
        }
    }

    fn finish_turn(&mut self, thread: &str, t: &Value) {
        let Some(live) = self.live.get_mut(thread) else { return };
        let Some(mut turn) = live.turn.take() else { return };
        turn.stats.duration_ms = t["durationMs"].as_u64().or(Some(turn.started.elapsed().as_millis() as u64));
        let status = t["status"].as_str().unwrap_or("completed");
        stats(|s| if status == "failed" { s.turns_failed += 1 } else { s.turns_done += 1 });
        let body = match status {
            "failed" => {
                let msg = t["error"]["message"].as_str().unwrap_or("");
                tr!("❌ The turn failed: {msg}", "❌ 这一轮失败了：{msg}")
            }
            "interrupted" => l("⏹ Stopped", "⏹ 已停止").to_string(),
            _ => {
                // An answer already relayed as progress isn't sent twice.
                let answer = match (turn.final_answer, turn.relayed) {
                    (Some(a), _) => a,
                    (None, true) => l("✅ Done", "✅ 完成").to_string(),
                    (None, false) => turn.last_message.unwrap_or_else(|| l("(no reply)", "（没有回复）").to_string()),
                };
                match format::summary(&turn.stats) {
                    Some(s) => format!("{answer}\n\n— {s}"),
                    None => answer,
                }
            }
        };
        live.last_reply = Some(body.clone());
        live.last_files = turn.stats.files.iter().cloned().collect();
        let queued = std::mem::take(&mut live.queue);
        let images = std::mem::take(&mut live.queue_images);
        self.pending.retain(|p| p.thread != thread);
        let t = thread.to_string();
        self.save(|s| {
            s.owned.insert(t, now());
        });
        self.say_from(thread, &body);
        if !queued.is_empty() {
            self.attach = images.clone();
            if let Err(e) = self.start_turn(thread, queued.join("\n\n")) {
                let live = self.live.entry(thread.to_string()).or_default();
                live.queue = queued;
                live.queue_images = images;
                self.say(format!("❗ {e:#}"));
            }
        }
        self.stop_typing();
    }

    fn on_request(&mut self, id: Value, method: &str, p: Value) {
        let thread = p["threadId"].as_str().unwrap_or("").to_string();
        let ask = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                let mut body = tr!("⚠️ Wants to run:\n{cmd}", "⚠️ 要执行命令：\n{cmd}", cmd = p["command"].as_str().unwrap_or("?"));
                if let Some(cwd) = p["cwd"].as_str() {
                    body.push_str(&tr!("\nFolder: {cwd}", "\n目录：{cwd}"));
                }
                if let Some(r) = p["reason"].as_str().filter(|r| !r.is_empty()) {
                    body.push_str(&tr!("\nWhy: {r}", "\n原因：{r}"));
                }
                Some((Ask::Command, body))
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                let files = p["itemId"].as_str().and_then(|i| self.changes.get(i)).cloned().unwrap_or_default();
                let mut body = if files.is_empty() {
                    l("⚠️ Wants to change files", "⚠️ 要修改文件").to_string()
                } else {
                    tr!("⚠️ Wants to change:\n{f}", "⚠️ 要修改文件：\n{f}", f = files.join("\n"))
                };
                if let Some(r) = p["reason"].as_str().filter(|r| !r.is_empty()) {
                    body.push_str(&tr!("\nWhy: {r}", "\n原因：{r}"));
                }
                Some((Ask::File, body))
            }
            "item/permissions/requestApproval" => {
                let perms = &p["permissions"];
                let mut what = vec![];
                if perms["network"]["enabled"] == true {
                    what.push(l("network access", "联网").to_string());
                }
                for path in perms["fileSystem"]["write"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    what.push(tr!("write {path}", "写入 {path}"));
                }
                for path in perms["fileSystem"]["read"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    what.push(tr!("read {path}", "读取 {path}"));
                }
                let mut body = tr!("⚠️ Asks for more permissions: {w}", "⚠️ 申请更多权限：{w}", w = crate::i18n::join(&what));
                if let Some(r) = p["reason"].as_str().filter(|r| !r.is_empty()) {
                    body.push_str(&tr!("\nWhy: {r}", "\n原因：{r}"));
                }
                Some((Ask::Permissions(perms.clone()), body))
            }
            "item/tool/requestUserInput" => {
                let qs = p["questions"].as_array().cloned().unwrap_or_default();
                let mut body = vec![l("❓ Codex asks:", "❓ Codex 在问：").to_string()];
                for q in &qs {
                    body.push(q["question"].as_str().unwrap_or("").to_string());
                    for (i, o) in q["options"].as_array().into_iter().flatten().enumerate() {
                        body.push(format!("  {}. {}", i + 1, o["label"].as_str().unwrap_or("")));
                    }
                }
                body.push(if qs.len() > 1 {
                    l("Reply with one answer per line.", "请每行回答一个问题。").to_string()
                } else {
                    l("Reply with your answer (or the option's number).", "直接回复答案（或选项编号）。").to_string()
                });
                Some((Ask::Questions(qs), body.join("\n")))
            }
            "mcpServer/elicitation/request" => {
                let server = p["serverName"].as_str().unwrap_or("MCP").to_string();
                if let Some(c) = &self.codex {
                    let _ = c.respond(&id, json!({ "action": "decline", "content": null, "_meta": null }));
                }
                self.note(&thread, &tr!("{server} asked for a form, which WeChat can't show; declined.", "{server} 请求填写表单，微信里无法填写，已拒绝。"));
                None
            }
            _ => {
                if let Some(c) = &self.codex {
                    let _ = c.respond_error(&id, "not supported by this client");
                }
                if !thread.is_empty() {
                    self.note(&thread, &tr!("Codex asked for {method}, which WeChat can't handle; declined.", "Codex 请求了 {method}，微信里无法处理，已拒绝。"));
                }
                None
            }
        };
        let Some((ask, mut body)) = ask else { return };
        let no = self.number(&thread);
        if self.live.get(&thread).is_some_and(|l| l.allow_all) {
            if let Some(reply) = auto_reply(&ask) {
                if let Some(c) = &self.codex {
                    let _ = c.respond(&id, reply);
                }
                let what = body.lines().skip(1).find(|l| !l.trim().is_empty()).or_else(|| body.lines().next()).unwrap_or("").trim().to_string();
                self.note(&thread, &tr!("✅ Allowed #{no} (allowed for this session): {what}", "✅ 已自动允许 #{no}（本会话都允许）：{what}"));
                return;
            }
        }
        if !matches!(ask, Ask::Questions(_)) {
            body.push_str(&tr!("\n\nReply y{no} to allow · a{no} allow for this session · n{no} decline", "\n\n回复 y{no} 允许 · a{no} 本会话都允许 · n{no} 拒绝"));
        }
        if !matches!(ask, Ask::Questions(_)) {
            stats(|s| s.approvals += 1);
        }
        self.pending.push(Pending { id, thread: thread.clone(), ask });
        self.note(&thread, &body);
    }

    fn on_codex_exit(&mut self) {
        self.codex = None;
        stats(|s| s.codex_exe = None);
        self.pending.clear();
        let mut running = vec![];
        let mut queued = vec![];
        let mut queued_counts = vec![];
        for (t, l) in &mut self.live {
            l.loaded = false;
            if l.turn.take().is_some() {
                running.push(t.clone());
            }
            let texts = std::mem::take(&mut l.queue);
            if !texts.is_empty() {
                queued_counts.push((t.clone(), texts.len()));
                queued.extend(texts.into_iter().map(|text| (t.clone(), text)));
            }
        }
        self.retry.extend(queued);
        log("error", l("Codex stopped", "Codex 已退出"));
        for t in running {
            self.note(&t, l("❗ Codex stopped unexpectedly; this turn was lost. Send the message again to retry.", "❗ Codex 意外退出，这一轮没有完成。重新发送消息即可重试。"));
        }
        for (t, n) in queued_counts {
            self.note(
                &t,
                &trn!(
                    n,
                    "❗ Codex stopped before {n} queued message could run. Reply y to restart and retry it, or n to cancel.",
                    "❗ Codex stopped before {n} queued messages could run. Reply y to restart and retry them, or n to cancel.",
                    "❗ Codex 在 {n} 条排队消息执行前退出了。回复 y 重启并重试，回复 n 取消。"
                ),
            );
        }
        self.stop_typing();
    }
}

/// "✅ Finished in Codex · 2m 50s", Codex's last words, and how to go on from here.
fn desktop_notice(e: &desktop::Finished, no: u32) -> String {
    let mut head = if e.aborted { l("⏹ Stopped in Codex", "⏹ 已在 Codex 中停止").to_string() } else { l("✅ Finished in Codex", "✅ 已在 Codex 中完成").to_string() };
    if let Some(ms) = e.duration_ms.filter(|ms| *ms >= 1000) {
        head.push_str(&format!(" · {}", format::duration(ms)));
    }
    let mut lines = vec![head];
    if let Some(m) = e.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        lines.push(crate::util::clip(&m.replace('\n', " "), NOTICE_CHARS));
    }
    // It is likely still open in Codex desktop, which locks it: say how to go ahead.
    lines.push(tr!(
        "To continue here, send #{no}! text (it may still be open in Codex desktop).",
        "要在微信里继续，发 #{no}! 消息（它可能还开在 Codex 桌面版里）。"
    ));
    lines.join("\n")
}

fn locked_text(no: u32, when: &str) -> String {
    tr!(
        "#{no} may be open in Codex desktop (it changed {when}). Writing to it from here could clash. Close it there, or send #{no}! text (or /use {no}!) to go ahead anyway.",
        "#{no} 可能正开在 Codex 桌面版里（{when}有更新），从这里写入可能冲突。请先在桌面版里关掉它，或发 #{no}! 消息（或 /use {no}!）强制发送。"
    )
}

fn help_text() -> String {
    l(
        "AgentPlus · Codex over WeChat\n\
         /ls [N] recent sessions · /use N switch · /status what's running\n\
         /dirs recent folders · /new [folder | N] start a session\n\
         #N text send to session N once · #N /command a command for session N\n\
         /stop [N | all] interrupt · /queue [N] [clear] waiting messages\n\
         /now [text] add to the running turn instead of queuing\n\
         /last [N] the last reply again · /diff [N] changes · /cwd [N] folder\n\
         /rename [N] title · /archive N\n\
         /mode [N] ask|auto|full|readonly what it may do without asking\n\
         /model [N] see or switch the model · /effort [level] reasoning effort\n\
         y N / a N / n N allow / allow for session / decline\n\
         Text, voice and pictures go to Codex; files and videos not yet.\n\
         /help command for details. Reply to (quote) a bot message to answer that session; other text goes to the current session.",
        "AgentPlus · 微信使用 Codex\n\
         /ls [数量] 最近的会话 · /use 编号 切换 · /status 运行情况\n\
         /dirs 最近的目录 · /new [目录 | 编号] 新建会话\n\
         #编号 消息 临时发给某个会话 · #编号 /命令 对该会话执行命令\n\
         /stop [编号 | all] 中断 · /queue [编号] [clear] 排队的消息\n\
         /now [消息] 不排队，直接插入正在进行的这一轮\n\
         /last [编号] 重发上次回复 · /diff [编号] 改动 · /cwd [编号] 目录\n\
         /rename [编号] 标题 改名 · /archive 编号 归档\n\
         /mode [编号] ask|auto|full|readonly 无需询问可做的事\n\
         /model [编号] 查看或切换模型 · /effort [强度] 推理强度\n\
         y编号 / a编号 / n编号 允许 / 本会话都允许 / 拒绝\n\
         文字、语音和图片会转给 Codex，文件和视频暂不支持。\n\
         /help 命令 查看详细说明。引用机器人的某条消息回复，会发给那条消息的会话；其他消息发给当前会话。",
    )
    .to_string()
}

/// A folder for comparing: any case on Windows, trailing slashes ignored.
fn dir_key(p: &str) -> String {
    let p = p.trim().trim_end_matches(['/', '\\']);
    if cfg!(windows) { p.replace('/', "\\").to_lowercase() } else { p.to_string() }
}

/// `dirs` with `default` always in it: last, taking the place of the oldest when full.
fn with_default(mut dirs: Vec<String>, default: &str, limit: usize) -> Vec<String> {
    if !dirs.iter().any(|d| dir_key(d) == dir_key(default)) {
        dirs.truncate(limit.saturating_sub(1));
        dirs.push(default.to_string());
    }
    dirs
}

/// Each folder once (see `dir_key`), in order, up to `limit`, keeping only those `keep`
/// accepts.
fn distinct_dirs<'a>(dirs: impl Iterator<Item = &'a str>, limit: usize, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let key = dir_key;
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for d in dirs {
        if out.len() >= limit {
            break;
        }
        if d.trim().is_empty() || !keep(d) || !seen.insert(key(d)) {
            continue;
        }
        out.push(d.trim().to_string());
    }
    out
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

/// The name Codex takes for a model in `model/list`.
fn model_name(m: &Value) -> String {
    m["model"].as_str().or_else(|| m["id"].as_str()).unwrap_or("").to_string()
}

fn efforts_of(m: &Value) -> Vec<String> {
    m["supportedReasoningEfforts"].as_array().into_iter().flatten().filter_map(|e| e["reasoningEffort"].as_str().map(String::from)).collect()
}

fn supports_effort(m: &Value, e: &str) -> bool {
    let list = efforts_of(m);
    list.is_empty() || list.iter().any(|x| x == e)
}

/// A model by its number (from 1) in the list, or by name, id or display name (any case).
fn pick_model<'a>(models: &'a [Value], arg: &str) -> Option<&'a Value> {
    let arg = arg.trim();
    if let Ok(n) = arg.parse::<usize>() {
        return n.checked_sub(1).and_then(|i| models.get(i));
    }
    let a = arg.to_lowercase();
    models.iter().find(|m| ["model", "id", "displayName"].iter().any(|k| m[*k].as_str().is_some_and(|s| s.to_lowercase() == a)))
}

/// `model` in Codex's config.toml.
fn default_model() -> Option<String> {
    let text = std::fs::read_to_string(crate::adapters::codex::config_path()).ok()?;
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("model").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// Sets `model` in Codex's config.toml after a backup, keeping the rest of the file as it is.
fn set_default_model(name: &str) -> Result<()> {
    let path = crate::adapters::codex::config_path();
    let (text, meta) = crate::util::read_text_or_new(&path)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| anyhow!("config.toml: {e}"))?;
    doc["model"] = toml_edit::value(name);
    if path.exists() {
        crate::util::backup_tagged("codex", std::slice::from_ref(&path), l("WeChat: default model", "微信：默认模型"))?;
    }
    crate::util::write_text_atomic(&path, &doc.to_string(), meta)
}

/// The answer to an approval once its session is allowed throughout, or None when it still asks.
fn auto_reply(ask: &Ask) -> Option<Value> {
    match ask {
        Ask::Command | Ask::File => Some(json!({ "decision": "acceptForSession" })),
        Ask::Permissions(asked) => Some(json!({ "permissions": granted(asked), "scope": "session" })),
        Ask::Questions(_) => None,
    }
}

/// `/mode`: (approval policy, sandbox policy) for `turn/start`.
fn mode_policy(mode: &str) -> Option<(Value, Value)> {
    Some(match mode {
        "ask" => (json!("untrusted"), json!({ "type": "workspaceWrite" })),
        "auto" => (json!("on-request"), json!({ "type": "workspaceWrite" })),
        "full" => (json!("never"), json!({ "type": "dangerFullAccess" })),
        "readonly" => (json!("on-request"), json!({ "type": "readOnly" })),
        _ => return None,
    })
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "ask" => l("ask (asks for anything not known safe)", "ask（不确定安全的都问）"),
        "auto" => l("auto (works in its folder, asks for more)", "auto（目录内自由工作，超出才问）"),
        "full" => l("full (no sandbox, never asks)", "full（无沙箱、从不询问）"),
        "readonly" => l("readonly (reads only)", "readonly（只读）"),
        _ => l("unknown", "未知"),
    }
}

fn usage_text(s: &Spec) -> String {
    tr!("Usage: {u}", "用法：{u}", u = l(s.usage.0, s.usage.1))
}

/// `/help stop`: usage, what it does and its other names.
fn help_of(s: &Spec) -> String {
    let mut lines = vec![l(s.usage.0, s.usage.1).to_string(), l(s.about.0, s.about.1).to_string()];
    if s.names.len() > 1 {
        let names: Vec<String> = s.names.iter().map(|n| format!("/{n}")).collect();
        lines.push(tr!("Also: {n}", "也可以发：{n}", n = names[1..].join(" ")));
    }
    lines.join("\n")
}

/// The message for Codex, with the quoted text in front: the part the user selected, or a
/// whole message of their own or someone else's. A whole bot message (`bot`, or tagged
/// `【#3 …】`) is only quoted to pick its session: Codex already has its text.
fn with_quote(m: &Inbound, text: &str, bot: bool) -> String {
    let q = match (m.quote_part.as_deref().map(str::trim), m.quote_text.as_deref().map(str::trim)) {
        (Some(part), _) if !part.is_empty() => part,
        (_, Some(q)) if !bot && !q.is_empty() && command::tag_in(q).is_none() => q,
        _ => return text.to_string(),
    };
    let quoted: String = crate::util::clip(q, QUOTE_CHARS).lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n");
    format!("{quoted}\n\n{text}")
}

/// `git diff` gives up after this; it runs on the bridge thread.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// `git diff --stat` in a folder; None when it isn't a Git working tree.
/// Settings that make git run other programs (fsmonitor, external diff, text conversion)
/// are turned off: the folder is whatever the session uses, and this is asked from a phone.
fn git_diff_stat(cwd: &str) -> Result<Option<String>> {
    use std::io::Read;
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "-C", cwd, "diff", "HEAD", "--stat", "--no-color", "--no-ext-diff", "--no-textconv"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::process::no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| anyhow!(tr!("Can't run git ({e}). Is Git installed?", "无法运行 git（{e}）。装了 Git 吗？")))?;
    let mut stdout = child.stdout.take().expect("piped");
    // Read while it runs, so a long output can't fill the pipe and stall git.
    let reader = std::thread::spawn(move || {
        let mut out = vec![];
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{}", l("git took too long in this folder", "git 在这个目录里运行太久，已放弃"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = reader.join().unwrap_or_default();
    Ok(status.success().then(|| String::from_utf8_lossy(&out).trim_end().to_string()))
}

/// The first `n` lines, and how many more there are.
fn clip_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.to_string();
    }
    let more = lines.len() - n;
    format!("{}\n{}", lines[..n].join("\n"), trn!(more, "… {n} more line", "… {n} more lines", "……还有 {n} 行"))
}

fn granted(asked: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in ["network", "fileSystem"] {
        if asked[k].is_object() {
            out.insert(k.into(), asked[k].clone());
        }
    }
    Value::Object(out)
}

/// `{ questionId: { answers: [..] } }` from the user's reply: one line per question; a
/// number picks that option. Too few lines: Err with the questions left without an answer.
fn question_answers(qs: &[Value], text: &str) -> Result<Value, Vec<String>> {
    let lines: Vec<&str> = if qs.len() > 1 { text.lines().map(str::trim).filter(|l| !l.is_empty()).collect() } else { vec![text.trim()] };
    if lines.len() < qs.len() {
        return Err(qs[lines.len()..].iter().map(|q| q["question"].as_str().unwrap_or("").to_string()).collect());
    }
    let mut out = serde_json::Map::new();
    for (i, q) in qs.iter().enumerate() {
        let Some(id) = q["id"].as_str() else { continue };
        let line = lines.get(i).copied().unwrap_or("");
        let options = q["options"].as_array();
        let answer = line
            .parse::<usize>()
            .ok()
            .and_then(|n| options.and_then(|o| o.get(n.wrapping_sub(1))))
            .and_then(|o| o["label"].as_str())
            .unwrap_or(line);
        out.insert(id.into(), json!({ "answers": [answer] }));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_prefer_the_name() {
        assert_eq!(thread_title(&json!({ "name": "Fix updater", "preview": "hi" })), "Fix updater");
        assert_eq!(thread_title(&json!({ "name": null, "preview": "first line\nsecond" })), "first line");
        assert_eq!(thread_title(&json!({})), "");
    }

    #[test]
    fn allowing_a_session_covers_approvals_but_not_questions() {
        let yes = json!({ "decision": "acceptForSession" });
        assert_eq!(auto_reply(&Ask::Command), Some(yes.clone()));
        assert_eq!(auto_reply(&Ask::File), Some(yes));
        assert_eq!(
            auto_reply(&Ask::Permissions(json!({ "network": { "enabled": true } }))),
            Some(json!({ "permissions": { "network": { "enabled": true } }, "scope": "session" }))
        );
        assert_eq!(auto_reply(&Ask::Questions(vec![])), None);
    }

    #[test]
    fn modes_map_to_codex_policies() {
        assert_eq!(mode_policy("readonly"), Some((json!("on-request"), json!({ "type": "readOnly" }))));
        assert_eq!(mode_policy("full"), Some((json!("never"), json!({ "type": "dangerFullAccess" }))));
        assert!(mode_policy("ask").is_some() && mode_policy("auto").is_some());
        assert_eq!(mode_policy("yolo"), None);
        for s in command::SPECS {
            assert!(!help_of(s).is_empty());
        }
    }

    #[test]
    fn heartbeats_come_at_5_then_every_10_minutes() {
        assert_eq!(beat_at(0), Duration::from_secs(300));
        assert_eq!(beat_at(1), Duration::from_secs(900));
        assert_eq!(beat_at(2), Duration::from_secs(1500));
    }

    #[test]
    fn quotes_go_in_front_unless_they_are_the_bots() {
        let m = |q: Option<&str>| Inbound { quote_text: q.map(String::from), ..msg("why?") };
        assert_eq!(with_quote(&m(None), "why?", false), "why?");
        assert_eq!(with_quote(&m(Some("line one\nline two")), "why?", false), "> line one\n> line two\n\nwhy?");
        // The bot's own messages only pick the session.
        assert_eq!(with_quote(&m(Some("【#2 demo】 | line one")), "why?", false), "why?");
        assert_eq!(with_quote(&m(Some("plain")), "why?", true), "why?");
        // A selected part of the bot's message is passed on.
        let part = Inbound { quote_part: Some("step 3".into()), ..m(Some("【#2 demo】 | step 3 fails")) };
        assert_eq!(with_quote(&part, "why?", true), "> step 3\n\nwhy?");
        let long = "x".repeat(QUOTE_CHARS * 2);
        assert!(with_quote(&m(Some(&long)), "q", false).chars().count() < QUOTE_CHARS + 20);
        assert_eq!(clip_lines("a\nb\nc", 2), "a\nb\n……还有 1 行");
        assert_eq!(clip_lines("a\nb", 2), "a\nb");
    }

    #[test]
    fn session_commands_never_reach_codex_as_text() {
        let _h = crate::util::TestHome::new("wechat-to-cmd");
        let mut b = bridge();
        b.saved.numbers.insert(3, "t3".into());
        b.live.insert("t3".into(), Live { queue: vec!["a".into(), "b".into()], ..Default::default() });
        b.handle(msg("#3 /queue"), command::parse("#3 /queue"), None);
        let out = sent(&mut b);
        assert!(out[0].contains("#3 排队中") && out[0].contains("1. a"), "{out:?}");
        b.handle(msg("#3 /queue clear"), command::parse("#3 /queue clear"), None);
        assert!(sent(&mut b)[0].contains("已清空 #3 的 2 条"));
        assert!(b.live["t3"].queue.is_empty());
        b.handle(msg("#3 /ls"), command::parse("#3 /ls"), None);
        assert!(sent(&mut b)[0].contains("不能配合 #3"));
        b.handle(msg("#3 /stauts"), command::parse("#3 /stauts"), None);
        assert!(sent(&mut b)[0].contains("/status"));
        // `/stop` before Codex named the turn: remembered for when it does.
        b.live.get_mut("t3").unwrap().turn = Some(Turn::new(None));
        b.handle(msg("#3 /stop"), command::parse("#3 /stop"), None);
        assert!(sent(&mut b)[0].contains("正在停止 #3"));
        assert!(b.live["t3"].turn.as_ref().unwrap().stop_asked);
    }

    #[test]
    fn quoted_session_gets_the_command() {
        let _h = crate::util::TestHome::new("wechat-quote-cmd");
        let mut b = bridge();
        b.saved.numbers.insert(1, "t1".into());
        b.saved.numbers.insert(2, "t2".into());
        b.saved.current = Some("t1".into());
        b.live.insert("t2".into(), Live { last_reply: Some("the answer".into()), ..Default::default() });
        let m = Inbound { quote_text: Some("【#2 x】 | the answer".into()), ..msg("/last") };
        b.handle(m, Cmd::Last(None), None);
        assert_eq!(sent(&mut b), ["【#2】\n\nthe answer"]);
        b.handle(msg("/last"), Cmd::Last(None), None);
        assert!(sent(&mut b)[0].contains("#1 在本次连接里还没有回复"));
        b.handle(msg("/mode 2 readonly"), command::parse("/mode 2 readonly"), None);
        assert_eq!(b.saved.modes.get("t2").map(String::as_str), Some("readonly"));
        assert!(sent(&mut b)[0].contains("readonly"));
        b.handle(msg("/use"), Cmd::Current, None);
        assert!(sent(&mut b)[0].contains("当前：【#1】"));
    }

    #[test]
    fn desktop_notices_say_how_it_ended() {
        let e = desktop::Finished { thread: "t".into(), title: "x".into(), turn: "1".into(), aborted: false, duration_ms: Some(170_966), message: Some("Line one\nline two".into()), at: None };
        let text = desktop_notice(&e, 4);
        assert!(text.starts_with("✅ 已在 Codex 中完成 · 2 分 50 秒\nLine one line two\n"), "{text}");
        assert!(text.contains("#4!"), "{text}");
        let stopped = desktop_notice(&desktop::Finished { aborted: true, message: None, duration_ms: None, ..e }, 4);
        assert!(stopped.starts_with("⏹ 已在 Codex 中停止\n"), "{stopped}");
    }

    #[test]
    fn notices_without_a_reply_context_keep_the_latest() {
        let _h = crate::util::TestHome::new("wechat-held");
        let mut b = bridge();
        b.saved.context_token = None;
        b.held_notices.insert("t1".into(), "first".into());
        b.held_notices.insert("t1".into(), "second".into());
        b.saved.context_token = Some("ctx".into());
        b.flush_unsent();
        let out = sent(&mut b);
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("second"), "{out:?}");
    }

    #[test]
    fn now_explains_when_it_has_nothing_to_add() {
        let _h = crate::util::TestHome::new("wechat-now");
        let mut b = bridge();
        b.saved.numbers.insert(3, "t3".into());
        b.saved.current = Some("t3".into());
        b.live.insert("t3".into(), Live { turn: Some(Turn::new(None)), ..Default::default() });
        b.handle(msg("/now"), command::parse("/now"), None);
        assert!(sent(&mut b)[0].contains("#3 没有排队的消息"));
        // Codex hasn't named the turn yet: it can't be steered.
        b.handle(msg("#3 /now go on"), command::parse("#3 /now go on"), None);
        assert!(sent(&mut b)[0].contains("#3 刚开始运行"));
        // While a turn runs, the queue notice points to /now.
        b.live.get_mut("t3").unwrap().turn.as_mut().unwrap().id = Some("turn".into());
        b.send_to("t3", "later".into(), false).unwrap();
        assert!(sent(&mut b)[0].contains("#3 /now 立即插入"));
    }

    #[test]
    fn messages_wait_for_pictures_in_order() {
        let _h = crate::util::TestHome::new("wechat-pictures");
        let mut b = bridge();
        b.downloading = true;
        b.on_inbound(msg("what is it?"));
        b.on_inbound(msg("/new"));
        assert!(sent(&mut b).is_empty(), "held while a picture downloads");
        // What starts no turn doesn't wait.
        b.on_inbound(msg("/status"));
        assert!(sent(&mut b)[0].contains("还没有选择会话"));
        b.on_inbound(msg("n"));
        assert_eq!(b.held.len(), 3, "n with nothing to answer is text");
        // Every picture failed and there was no text: nothing goes on, then the rest runs.
        b.on_pictures(Inbound { text: IMAGE_ONLY.into(), ..msg("") }, vec![], 1);
        let out = sent(&mut b);
        assert!(out[0].contains("无法从微信下载图片"), "{out:?}");
        assert!(out[1].contains("还没有选择会话") && out[2].contains("Codex 还没启动"), "{out:?}");
        assert!(!b.downloading && b.held.is_empty());
    }

    #[test]
    fn a_failed_part_is_mentioned_next_time() {
        let _h = crate::util::TestHome::new("wechat-lost");
        let mut b = bridge();
        b.lost = true;
        b.say("hi");
        assert_eq!(sent(&mut b), ["hi\n\n（上一条回复没有完整送达，发 /last 重发。）"]);
        b.say("again");
        assert_eq!(sent(&mut b), ["again"]);
    }

    #[test]
    fn answers_questions() {
        let qs = vec![json!({ "id": "q1", "options": [{ "label": "Yes" }, { "label": "No" }] })];
        assert_eq!(question_answers(&qs, "2"), Ok(json!({ "q1": { "answers": ["No"] } })));
        assert_eq!(question_answers(&qs, "7"), Ok(json!({ "q1": { "answers": ["7"] } })));
        assert_eq!(question_answers(&qs, "0"), Ok(json!({ "q1": { "answers": ["0"] } })));
        assert_eq!(question_answers(&qs, " maybe \nlater"), Ok(json!({ "q1": { "answers": ["maybe \nlater"] } })));
        let two = vec![json!({ "id": "a", "question": "Name?", "options": null }), json!({ "id": "b", "question": "Pick", "options": [{ "label": "X" }] })];
        assert_eq!(question_answers(&two, "first\n\n1"), Ok(json!({ "a": { "answers": ["first"] }, "b": { "answers": ["X"] } })));
        // Too few lines: nothing is answered, the missing questions come back.
        assert_eq!(question_answers(&two, "only one"), Err(vec!["Pick".to_string()]));
    }

    #[test]
    fn picks_models_by_number_or_name() {
        let models = vec![
            json!({ "id": "a", "model": "gpt-6-sol", "displayName": "GPT-6 Sol", "supportedReasoningEfforts": [{ "reasoningEffort": "low" }, { "reasoningEffort": "high" }] }),
            json!({ "id": "b", "model": "deepseek-v4-pro", "displayName": "DeepSeek V4 Pro", "supportedReasoningEfforts": [] }),
        ];
        assert_eq!(pick_model(&models, "2").map(model_name).as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(pick_model(&models, " GPT-6-SOL ").map(model_name).as_deref(), Some("gpt-6-sol"));
        assert_eq!(pick_model(&models, "gpt-6 sol").map(model_name).as_deref(), Some("gpt-6-sol"));
        assert_eq!(pick_model(&models, "b").map(model_name).as_deref(), Some("deepseek-v4-pro"));
        assert!(pick_model(&models, "0").is_none() && pick_model(&models, "3").is_none() && pick_model(&models, "x").is_none());
        assert_eq!(efforts_of(&models[0]), vec!["low", "high"]);
        assert!(supports_effort(&models[0], "high") && !supports_effort(&models[0], "xhigh"));
        // A model that lists no efforts takes any.
        assert!(supports_effort(&models[1], "xhigh"));
    }

    #[test]
    fn default_model_keeps_the_rest_of_config_toml() {
        let _h = crate::util::TestHome::new("wechat-model");
        let path = crate::adapters::codex::config_path();
        assert_eq!(default_model(), None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine\nmodel = \"old\"\n\n[model_providers.x]\nname = \"x\" # keep\n").unwrap();
        assert_eq!(default_model().as_deref(), Some("old"));
        set_default_model("gpt-6-luna").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(default_model().as_deref(), Some("gpt-6-luna"));
        assert!(text.contains("# mine") && text.contains("name = \"x\" # keep"), "{text}");
    }

    #[test]
    fn lists_each_folder_once() {
        let dirs = [r"C:\code\a", r"C:\code\b\", r"c:\code\A", "", r"C:\gone", r"C:\code\b", "C:/code/c"];
        let got = distinct_dirs(dirs.into_iter(), 10, |p| p != r"C:\gone");
        if cfg!(windows) {
            assert_eq!(got, vec![r"C:\code\a", r"C:\code\b\", "C:/code/c"]);
        }
        assert_eq!(distinct_dirs(dirs.into_iter(), 1, |_| true), vec![r"C:\code\a"]);
    }

    /// A bridge for user "me" that keeps what it would send, with Codex not running.
    fn bridge() -> Bridge {
        let wx = ilink::Client::new("http://127.0.0.1:9", None).unwrap();
        let (tx, _rx) = mpsc::channel();
        let saved = Saved { context_token: Some("test-context".into()), ..Default::default() };
        let mut b = Bridge::new(wx, "me".into(), saved, tx);
        b.outbox = Some(vec![]);
        b
    }

    fn msg(text: &str) -> Inbound {
        Inbound { from: "me".into(), text: text.into(), ..Default::default() }
    }

    fn sent(b: &mut Bridge) -> Vec<String> {
        std::mem::take(b.outbox.as_mut().unwrap())
    }

    #[test]
    fn messages_wait_for_a_usable_reply_context() {
        let _h = crate::util::TestHome::new("wechat-reply-context");
        let mut b = bridge();
        for ctx in [None, Some(String::new()), Some(" \t\n".into())] {
            b.saved.context_token = ctx;
            b.say("Codex started.");
            assert!(sent(&mut b).is_empty());
        }
        assert_eq!(b.unsent.len(), 3, "held, not dropped");
        b.on_inbound(Inbound { context_token: Some("test-context".into()), ..msg("/help") });
        let out = sent(&mut b);
        assert_eq!(out[..3], ["Codex started.", "Codex started.", "Codex started."], "held messages go first, in order");
        assert!(out[3].contains("/model"));
        assert!(b.unsent.is_empty());
        b.say("Codex started.");
        assert_eq!(sent(&mut b), ["Codex started."]);
    }

    #[test]
    fn only_the_connected_users_message_enables_replies() {
        let _h = crate::util::TestHome::new("wechat-inbound-context");
        let mut b = bridge();
        b.saved.context_token = None;
        b.on_inbound(Inbound { from: "other".into(), text: "/help".into(), context_token: Some("other-context".into()), ..Default::default() });
        assert!(b.saved.context_token.is_none() && sent(&mut b).is_empty());
        for ctx in ["first-context", "next-context"] {
            b.on_inbound(Inbound { context_token: Some(ctx.into()), ..msg("/help") });
            assert_eq!(b.saved.context_token.as_deref(), Some(ctx));
            assert!(sent(&mut b)[0].contains("/model"));
        }
        b.on_inbound(msg("/help"));
        assert_eq!(b.saved.context_token.as_deref(), Some("next-context"));
        assert!(!sent(&mut b).is_empty());
    }

    #[test]
    fn a_bare_number_picks_from_the_last_list() {
        let _h = crate::util::TestHome::new("wechat-pick");
        let mut b = bridge();
        assert_eq!(b.resolve("9").0, Cmd::Text("9".into()));
        b.pick = Some(Pick::Sessions);
        assert_eq!(b.resolve(" 9 ").0, Cmd::Use { no: 9, force: false });
        // Only the next message.
        assert_eq!(b.resolve("9").0, Cmd::Text("9".into()));
        b.pick = Some(Pick::Dirs);
        assert_eq!(b.resolve("2").0, Cmd::New(Some("2".into())));
        b.pick = Some(Pick::Models(Some("t1".into())));
        assert_eq!(b.resolve("3"), (Cmd::Model(ModelCmd::Set("3".into())), Some("t1".into())));
        b.pick = Some(Pick::Sessions);
        assert_eq!(b.resolve("0").0, Cmd::Text("0".into()));
        b.pick = Some(Pick::Sessions);
        assert_eq!(b.resolve("hi").0, Cmd::Text("hi".into()));
        // A session asking a question gets the number as its answer.
        b.pending.push(Pending { id: json!(1), thread: "t1".into(), ask: Ask::Questions(vec![]) });
        b.pick = Some(Pick::Sessions);
        assert_eq!(b.resolve("2").0, Cmd::Text("2".into()));
    }

    #[test]
    fn needs_codex_only_for_sessions_that_exist() {
        let _h = crate::util::TestHome::new("wechat-needs");
        let mut b = bridge();
        b.saved.numbers.insert(3, "t3".into());
        let m = msg("x");
        assert!(!b.needs_codex(&Cmd::Use { no: 99, force: false }, &m));
        assert!(b.needs_codex(&Cmd::Use { no: 3, force: false }, &m));
        assert!(!b.needs_codex(&Cmd::To { no: 99, force: false, text: "x".into() }, &m));
        assert!(!b.needs_codex(&Cmd::Help(None), &m) && !b.needs_codex(&Cmd::Status, &m));
        // Text with no session to go to is answered with a hint, without Codex.
        assert!(!b.needs_codex(&Cmd::Text("x".into()), &m));
        b.saved.current = Some("t3".into());
        assert!(b.needs_codex(&Cmd::Text("x".into()), &m));
    }

    #[test]
    fn messages_wait_for_codex_in_order_and_n_drops_them() {
        let _h = crate::util::TestHome::new("wechat-defer");
        let mut b = bridge();
        b.on_inbound(msg("/ls"));
        assert_eq!(b.deferred.len(), 1);
        assert!(sent(&mut b)[0].contains("回复 y 启动"));
        b.on_inbound(msg("/dirs"));
        assert_eq!(b.deferred.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["/ls", "/dirs"]);
        assert!(sent(&mut b)[0].contains("已有 2 条消息在等"));
        // Commands that don't need Codex still answer right away.
        b.on_inbound(msg("/help"));
        assert!(sent(&mut b)[0].contains("/model"));
        assert_eq!(b.deferred.len(), 2);
        b.on_inbound(msg("n"));
        assert!(b.deferred.is_empty());
        assert!(sent(&mut b)[0].contains("2 条消息已取消"));
        // Someone else's message is ignored.
        b.on_inbound(Inbound { from: "other".into(), text: "/ls".into(), ..Default::default() });
        assert!(b.deferred.is_empty() && sent(&mut b).is_empty());
    }

    #[test]
    fn codex_exit_keeps_queued_messages_for_retry() {
        let _h = crate::util::TestHome::new("wechat-retry");
        let mut b = bridge();
        b.saved.numbers.insert(1, "t1".into());
        b.live.insert("t1".into(), Live { queue: vec!["one".into(), "two".into()], ..Default::default() });

        b.on_codex_exit();

        assert_eq!(b.retry, vec![("t1".into(), "one".into()), ("t1".into(), "two".into())]);
        assert!(sent(&mut b).iter().any(|text| text.contains("排队消息")));
        // The start prompt counts the messages waiting for a retry too.
        b.on_inbound(msg("/ls"));
        assert!(sent(&mut b)[0].contains("已有 3 条消息在等"));
    }

    #[test]
    fn locks_sessions_codex_desktop_may_have_open() {
        let _h = crate::util::TestHome::new("wechat-lock");
        let mut b = bridge();
        b.live.insert("t".into(), Live { updated: now() - 60, ..Default::default() });
        assert!(b.locked("t", true));
        assert!(!b.locked("t", false));
        b.saved.owned.insert("t".into(), now() - 100);
        assert!(!b.locked("t", true), "changed by the bridge itself");
        b.saved.owned.clear();
        b.live.get_mut("t").unwrap().updated = now() - LOCK_WINDOW - 1;
        assert!(!b.locked("t", true), "too long ago");
        assert!(!b.locked("unknown", true));
    }

    #[test]
    fn the_default_folder_is_always_listed() {
        let ten: Vec<String> = (0..10).map(|i| format!("C:/p{i}")).collect();
        let got = with_default(ten.clone(), "C:/home", 10);
        assert_eq!(got.len(), 10);
        assert_eq!(got.last().map(String::as_str), Some("C:/home"));
        assert_eq!(got[8], "C:/p8");
        // Already there (another spelling on Windows): unchanged.
        let mut with = ten[..3].to_vec();
        with.push("C:/home/".into());
        if cfg!(windows) {
            assert_eq!(with_default(with.clone(), "c:\\HOME", 10), with);
        }
        assert_eq!(with_default(vec![], "C:/home", 10), vec!["C:/home"]);
    }

    #[test]
    fn grants_what_was_asked() {
        assert_eq!(granted(&json!({ "network": { "enabled": true }, "fileSystem": null })), json!({ "network": { "enabled": true } }));
        assert_eq!(granted(&json!({})), json!({}));
    }
}
