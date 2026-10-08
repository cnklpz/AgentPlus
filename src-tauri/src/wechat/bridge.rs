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
use super::command::{self, Cmd, Decision};
use super::format::{self, TurnStats};
use super::ilink::{self, Inbound};
use super::{log, set_sessions, set_state, stats, update_saved, Event, Saved, SessionView, Stats};
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
const CALL: Duration = Duration::from_secs(60);

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
}

struct Turn {
    id: Option<String>,
    started: Instant,
    stats: TurnStats,
    final_answer: Option<String>,
    last_message: Option<String>,
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
    /// A message that needs Codex, waiting for the user to say whether to start it.
    deferred: Option<Inbound>,
}

fn thread_title(t: &Value) -> String {
    let name = t["name"].as_str().unwrap_or("").trim();
    let s = if name.is_empty() { t["preview"].as_str().unwrap_or("").trim() } else { name };
    s.lines().next().unwrap_or("").to_string()
}

impl Bridge {
    pub fn new(wx: ilink::Client, user: String, saved: Saved, events: mpsc::Sender<Event>) -> Bridge {
        Bridge { wx, user, saved, events, codex: None, generation: 0, live: HashMap::new(), pending: vec![], changes: HashMap::new(), typing: None, run: 0, deferred: None }
    }

    pub fn run(&mut self, rx: mpsc::Receiver<Event>, run: u64) {
        self.run = run;
        stats(|s| *s = Stats { since: Some(chrono::Utc::now().timestamp_millis()), ..Default::default() });
        set_state("running", None);
        log("info", l("Connected to WeChat", "已连接微信"));
        self.publish();
        let current = || super::RUN.load(std::sync::atomic::Ordering::SeqCst) == run;
        while let Ok(ev) = rx.recv() {
            if !current() {
                break;
            }
            match ev {
                Event::Stop => break,
                Event::Inbound(m) => self.on_inbound(m),
                Event::StartCodex => self.start_codex_now(),
                Event::Cursor(c) => self.save(|s| s.cursor = c),
                Event::Expired => {
                    set_state("expired", Some(l("WeChat signed the bot out. Scan the QR code again", "微信已让机器人下线，请重新扫码").into()));
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
            self.publish();
        }
        self.stop_typing();
        self.codex = None;
        stats(|s| s.codex_exe = None);
    }

    fn save(&mut self, f: impl FnOnce(&mut Saved)) {
        f(&mut self.saved);
        let s = self.saved.clone();
        let run = self.run;
        // Keep fields the page changes (folder, switch) as they are in the file. A run that
        // is over (disconnected, signed in again) must not write its account back.
        if let Err(e) = update_saved(|file| {
            if super::RUN.load(std::sync::atomic::Ordering::SeqCst) != run {
                return;
            }
            let (enabled, cwd) = (file.enabled, file.default_cwd.clone());
            *file = s;
            file.enabled = enabled;
            file.default_cwd = cwd;
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
                    queued: live.map(|l| l.queue.len()).unwrap_or(0),
                    current: self.saved.current.as_ref() == Some(t),
                }
            })
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.no));
        set_sessions(v, self.codex.as_ref().is_some_and(|c| c.alive()));
    }

    // ------------------------------------------------------------ sending

    fn send(&mut self, texts: Vec<String>, no: Option<u32>) {
        let ctx = self.saved.context_token.clone();
        let mut ids = vec![];
        for t in texts {
            match self.wx.send_text(&self.user, &t, ctx.as_deref()) {
                Ok(id) => {
                    log("out", &t);
                    if let (Some(id), Some(no)) = (id, no) {
                        ids.push((id, no));
                    }
                }
                Err(e) => {
                    log("error", format!("{e:#}"));
                    crate::applog::warn("wechat", format!("send: {e:#}"));
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
        if self.typing.is_none() {
            self.typing = self.wx.typing_ticket(&self.user, self.saved.context_token.as_deref()).ok().flatten();
        }
        if let Some(t) = &self.typing {
            let _ = self.wx.send_typing(&self.user, t, true);
        }
    }

    fn stop_typing(&mut self) {
        if self.live.values().any(|l| l.turn.is_some()) {
            return;
        }
        if let Some(t) = &self.typing {
            let _ = self.wx.send_typing(&self.user, t, false);
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
        Some(id)
    }

    /// Fresh title, folder and update time of one session.
    fn refresh(&mut self, thread: &str) -> Result<()> {
        let v = self.codex()?.request("thread/read", json!({ "threadId": thread, "includeTurns": false }), CALL)?;
        self.remember(&v["thread"]).ok_or_else(|| anyhow!(l("Codex didn't return the session", "Codex 没有返回这个会话")))?;
        Ok(())
    }

    /// Why sending to this session could clash with Codex desktop, if it could.
    fn lock_reason(&self, thread: &str) -> Option<String> {
        let live = self.live.get(thread)?;
        if live.loaded {
            return None;
        }
        let ours = self.saved.owned.get(thread).is_some_and(|t| live.updated <= t + 60);
        let age = now() - live.updated;
        if ours || age >= LOCK_WINDOW || !crate::process::detect_codex().running {
            return None;
        }
        Some(format::ago(live.updated, now()))
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
            } else if desktop && live.updated > now() - LOCK_WINDOW && !self.saved.owned.get(&id).is_some_and(|o| live.updated <= o + 60) && !live.loaded {
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
        lines.push(l("/use N switch · #N text send once · /new start one · 🔒 may be open in Codex", "/use 编号 切换 · #编号 消息 临时发送 · /new 新建 · 🔒 可能正开在 Codex 里").into());
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

    fn new_session(&mut self, path: Option<String>) -> Result<String> {
        let cwd = path
            .or_else(|| self.saved.current.as_ref().and_then(|t| self.live.get(t)).map(|l| l.cwd.clone()).filter(|c| !c.is_empty()))
            .or_else(|| super::load_saved().default_cwd)
            .unwrap_or_else(|| crate::util::home().to_string_lossy().into_owned());
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
                let n = live.queue.len();
                self.say(tr!("Queued for #{no} ({n} waiting); it runs when the current turn ends. /stop {no} interrupts.", "已排队到 #{no}（共 {n} 条），当前这轮结束后发送。/stop {no} 可中断。"));
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
        let v = self.codex()?.request(
            "turn/start",
            json!({ "threadId": thread, "input": [{ "type": "text", "text": text, "text_elements": [] }] }),
            CALL,
        )?;
        let live = self.live.entry(thread.to_string()).or_default();
        live.turn = Some(Turn { id: v["turn"]["id"].as_str().map(String::from), started: Instant::now(), stats: TurnStats::default(), final_answer: None, last_message: None });
        let t = thread.to_string();
        self.save(|s| {
            s.owned.insert(t, now());
        });
        self.start_typing();
        Ok(())
    }

    fn stop(&mut self, no: Option<u32>) -> Result<String> {
        let thread = match no {
            Some(n) => self.thread_of(n).ok_or_else(|| anyhow!(tr!("There is no session #{n}", "没有 #{n} 这个会话")))?,
            None => self.saved.current.clone().ok_or_else(|| anyhow!(l("No session selected", "还没有选择会话")))?,
        };
        let no = self.number(&thread);
        let Some(turn_id) = self.live.get(&thread).and_then(|l| l.turn.as_ref()).and_then(|t| t.id.clone()) else {
            return Ok(tr!("#{no} isn't running", "#{no} 没有在运行"));
        };
        if let Some(l) = self.live.get_mut(&thread) {
            l.queue.clear();
        }
        self.codex()?.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn_id }), CALL)?;
        Ok(tr!("Stopping #{no}…", "正在停止 #{no}…"))
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
        log("in", if m.text.is_empty() && m.media { l("[media]", "[媒体]") } else { m.text.as_str() });
        if m.text.trim().is_empty() {
            if m.media {
                self.say(l("Only text and voice messages can be passed to Codex for now.", "目前只能把文字和语音消息转给 Codex。"));
            }
            return;
        }
        let cmd = command::parse(&m.text);
        if !self.codex.as_ref().is_some_and(|c| c.alive()) {
            // The answer to "Start Codex?".
            if self.deferred.is_some() {
                match cmd {
                    Cmd::Approve { decision: Decision::Decline, .. } => {
                        self.deferred = None;
                        self.say(l("OK, Codex stays off.", "好的，先不启动 Codex。"));
                        return;
                    }
                    Cmd::Approve { .. } => return self.start_codex_now(),
                    _ => {}
                }
            }
            if self.needs_codex(&cmd, &m) {
                self.deferred = Some(m);
                self.say(l(
                    "Codex isn't running. Reply y to start it and run your message, or n to cancel.",
                    "Codex 还没启动。回复 y 启动并执行刚才的消息，回复 n 取消。",
                ));
                return;
            }
        }
        self.handle(m, cmd);
    }

    /// Whether this message has to reach Codex (and so start it).
    fn needs_codex(&self, cmd: &Cmd, m: &Inbound) -> bool {
        match cmd {
            Cmd::Help | Cmd::Status | Cmd::Unknown(_) | Cmd::Stop(_) => false,
            Cmd::List(_) | Cmd::Use { .. } | Cmd::New(_) | Cmd::To { .. } => true,
            // Text only goes somewhere with a session to go to.
            Cmd::Text(_) | Cmd::Approve { .. } => self.saved.current.is_some() || self.session_of_quote(m).is_some_and(|n| self.thread_of(n).is_some()),
        }
    }

    /// Starts Codex (asked from WeChat or the page), then runs the message that waited for it.
    fn start_codex_now(&mut self) {
        let deferred = self.deferred.take();
        match self.codex().map(|_| ()) {
            Ok(()) => {
                self.say(l("Codex started.", "Codex 已启动。"));
                if let Some(m) = deferred {
                    let cmd = command::parse(&m.text);
                    self.handle(m, cmd);
                }
            }
            Err(e) => self.say(format!("❗ {e:#}")),
        }
    }

    fn handle(&mut self, m: Inbound, cmd: Cmd) {
        let quoted = self.session_of_quote(&m);
        let r = match cmd {
            Cmd::Help => Ok(Some(help_text())),
            Cmd::List(n) => self.list(n.unwrap_or(LIST).min(30)).map(Some),
            Cmd::Use { no, force } => self.use_session(no, force).map(Some),
            Cmd::New(p) => self.new_session(p).map(Some),
            Cmd::Stop(no) => self.stop(no).map(Some),
            Cmd::Status => Ok(Some(self.status_text())),
            Cmd::Unknown(c) => Ok(Some(tr!("Unknown command {c}. Send /help for the list.", "不认识的命令 {c}。发 /help 查看说明。"))),
            Cmd::Approve { no, decision } => match self.answer_approval(no.or(quoted), decision) {
                Some(r) => r.map(Some),
                None => self.route_text(m.text.trim().to_string(), quoted, false),
            },
            Cmd::To { no, force, text } => match self.thread_of(no) {
                Some(t) => self.send_or_answer(&t, text, force).map(|_| None),
                None => Err(anyhow!(tr!("There is no session #{no}. Send /ls to see the list", "没有 #{no} 这个会话。发 /ls 查看列表"))),
            },
            Cmd::Text(t) => self.route_text(t, quoted, false),
        };
        match r {
            Ok(Some(text)) => self.say(text),
            Ok(None) => {}
            Err(e) => self.say(format!("❗ {e:#}")),
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
            let p = self.pending.remove(i);
            let Ask::Questions(qs) = &p.ask else { unreachable!() };
            let answers = question_answers(qs, &text);
            self.codex()?.respond(&p.id, json!({ "answers": answers }))?;
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
                if let Some(t) = self.live.get_mut(&thread).and_then(|l| l.turn.as_mut()) {
                    t.id = t.id.clone().or_else(|| p["turn"]["id"].as_str().map(String::from));
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
                let Some(turn) = self.live.get_mut(&thread).and_then(|l| l.turn.as_mut()) else { return };
                match item["type"].as_str().unwrap_or("") {
                    "agentMessage" => {
                        let text = item["text"].as_str().unwrap_or("").to_string();
                        if item["phase"] == "final_answer" {
                            turn.final_answer = Some(text.clone());
                        }
                        if !text.trim().is_empty() {
                            turn.last_message = Some(text);
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
            "error" if p["willRetry"] != true => log("error", p["error"]["message"].as_str().unwrap_or("Codex error")),
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
                let answer = turn.final_answer.or(turn.last_message).unwrap_or_else(|| l("(no reply)", "（没有回复）").to_string());
                match format::summary(&turn.stats) {
                    Some(s) => format!("{answer}\n\n— {s}"),
                    None => answer,
                }
            }
        };
        let queued = std::mem::take(&mut live.queue);
        self.pending.retain(|p| p.thread != thread);
        let t = thread.to_string();
        self.save(|s| {
            s.owned.insert(t, now());
        });
        self.say_from(thread, &body);
        if !queued.is_empty() {
            if let Err(e) = self.start_turn(thread, queued.join("\n\n")) {
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
                None
            }
        };
        let Some((ask, mut body)) = ask else { return };
        let no = self.number(&thread);
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
        let running: Vec<String> = self.live.iter_mut().filter_map(|(t, l)| {
            l.loaded = false;
            l.queue.clear();
            l.turn.take().map(|_| t.clone())
        }).collect();
        log("error", l("Codex stopped", "Codex 已退出"));
        for t in running {
            self.note(&t, l("❗ Codex stopped unexpectedly; this turn was lost. Send the message again to retry.", "❗ Codex 意外退出，这一轮没有完成。重新发送消息即可重试。"));
        }
        self.stop_typing();
    }
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
         /ls [N] recent sessions\n\
         /use N switch to session N\n\
         /new [folder] start a session\n\
         #N text send to session N once\n\
         /stop [N] interrupt a turn\n\
         /status what's running\n\
         y N / a N / n N allow / allow for session / decline\n\
         Reply to (quote) a bot message to answer that session. Other text goes to the current session.",
        "AgentPlus · 微信使用 Codex\n\
         /ls [数量] 最近的会话\n\
         /use 编号 切换会话\n\
         /new [目录] 新建会话\n\
         #编号 消息 临时发给某个会话\n\
         /stop [编号] 中断当前这轮\n\
         /status 查看运行情况\n\
         y编号 / a编号 / n编号 允许 / 本会话都允许 / 拒绝\n\
         引用机器人的某条消息回复，会发给那条消息的会话；其他消息发给当前会话。",
    )
    .to_string()
}

/// Grants what was asked (fields left out stay unchanged).
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
/// number picks that option.
fn question_answers(qs: &[Value], text: &str) -> Value {
    let lines: Vec<&str> = if qs.len() > 1 { text.lines().map(str::trim).filter(|l| !l.is_empty()).collect() } else { vec![text.trim()] };
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
    Value::Object(out)
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
    fn answers_questions() {
        let qs = vec![json!({ "id": "q1", "options": [{ "label": "Yes" }, { "label": "No" }] })];
        assert_eq!(question_answers(&qs, "2"), json!({ "q1": { "answers": ["No"] } }));
        assert_eq!(question_answers(&qs, "7"), json!({ "q1": { "answers": ["7"] } }));
        assert_eq!(question_answers(&qs, "0"), json!({ "q1": { "answers": ["0"] } }));
        assert_eq!(question_answers(&qs, " maybe \nlater"), json!({ "q1": { "answers": ["maybe \nlater"] } }));
        let two = vec![json!({ "id": "a", "options": null }), json!({ "id": "b", "options": [{ "label": "X" }] })];
        assert_eq!(question_answers(&two, "first\n\n1"), json!({ "a": { "answers": ["first"] }, "b": { "answers": ["X"] } }));
        assert_eq!(question_answers(&two, "only one"), json!({ "a": { "answers": ["only one"] }, "b": { "answers": [""] } }));
    }

    #[test]
    fn grants_what_was_asked() {
        assert_eq!(granted(&json!({ "network": { "enabled": true }, "fileSystem": null })), json!({ "network": { "enabled": true } }));
        assert_eq!(granted(&json!({})), json!({}));
    }
}
