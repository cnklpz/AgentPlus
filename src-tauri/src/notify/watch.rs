//! Finds finished agent turns by reading what each agent writes as it works, without
//! changing any agent's config:
//!
//! - Codex: rollout files `sessions/YYYY/MM/DD/rollout-*.jsonl`; an `event_msg` of type
//!   `task_complete` ends a turn (`turn_aborted` ends one without a result). Only threads the
//!   user started count (not reviews, subagents or `codex exec`).
//! - Claude Code: transcripts `projects/<project>/<session>.jsonl`; an assistant entry with
//!   `stop_reason: "end_turn"` outside a sidechain ends a turn.
//! - OpenCode and ZCode: their SQLite `message` table; an assistant message with
//!   `finish: "stop"` in a top-level session ends a turn.
//!
//! Only turns that end after watching starts are reported: files already there are read from
//! their end, and the databases from the current time.

use super::Done;
use crate::util::home;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Agents the watcher can follow, in display order.
pub const AGENTS: [&str; 4] = ["codex", "claude", "opencode", "zcode"];

/// Files untouched for longer than this aren't looked for (they are still read if tracked).
const RECENT: Duration = Duration::from_secs(15 * 60);
/// How often the folders are listed again for new session files.
const RESCAN: Duration = Duration::from_secs(20);
/// A turn that ended longer ago than this is old news (a file found late, a clock jump).
const STALE_MS: i64 = 10 * 60 * 1000;
/// At most this much of a file is read per poll; the rest waits for the next one.
const READ_CAP: u64 = 8 << 20;

pub fn codex_sessions() -> PathBuf {
    crate::adapters::codex::codex_home().join("sessions")
}
pub fn claude_projects() -> PathBuf {
    crate::adapters::claude::dir().join("projects")
}
pub fn opencode_db() -> PathBuf {
    home().join(".local").join("share").join("opencode").join("opencode.db")
}
/// ZCode keeps its desktop settings in `~/.zcode/v2` and its sessions next to it in `cli/db`.
pub fn zcode_db() -> PathBuf {
    let d = crate::adapters::zcode::dir();
    let root = if d.ends_with("v2") { d.parent().map(Path::to_path_buf).unwrap_or(d) } else { d };
    root.join("cli").join("db").join("db.sqlite")
}

/// Whether the agent's records exist on this machine.
pub fn available(agent: &str) -> bool {
    match agent {
        "codex" => codex_sessions().is_dir(),
        "claude" => claude_projects().is_dir(),
        "opencode" => opencode_db().is_file(),
        "zcode" => zcode_db().is_file(),
        _ => false,
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn ts_ms(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|d| d.timestamp_millis())
}

fn folder_name(cwd: &str) -> String {
    let t = cwd.trim_end_matches(['/', '\\']);
    t.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(t).to_string()
}

// ---------------------------------------------------------------- line parsers

/// What a session file has said so far about the turn in progress.
#[derive(Default, Debug)]
pub struct Turn {
    pub cwd: String,
    /// Codex: whether the thread is one the user started.
    pub skip: bool,
    pub start_ms: Option<i64>,
    /// Turn or message ids already reported.
    pub done: HashSet<String>,
    /// Claude Code: a finished message whose later entries (its text after its thinking)
    /// may still follow; `flush` hands it over once the read is through.
    pub pending: Option<(String, Done)>,
}

/// The turn held back by the last lines read, if any.
pub fn flush(t: &mut Turn) -> Option<Done> {
    t.pending.take().map(|p| p.1)
}

fn remember(t: &mut Turn, id: String) -> bool {
    if t.done.len() > 256 {
        t.done.clear();
    }
    t.done.insert(id)
}

/// One line of a Codex rollout file.
pub fn codex_line(t: &mut Turn, line: &str) -> Option<Done> {
    let v: Value = serde_json::from_str(line).ok()?;
    let p = &v["payload"];
    match (v["type"].as_str()?, p["type"].as_str()) {
        ("session_meta", _) => {
            t.cwd = p["cwd"].as_str().unwrap_or_default().to_string();
            let source = match &p["source"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            t.skip = crate::sessions::kind_of(&source, p["thread_source"].as_str().unwrap_or_default()) != "user";
            None
        }
        ("turn_context", _) => {
            if let Some(c) = p["cwd"].as_str() {
                t.cwd = c.to_string();
            }
            None
        }
        ("event_msg", Some("task_started")) => {
            t.start_ms = p["started_at"].as_i64().map(|s| s * 1000).or_else(|| ts_ms(&v["timestamp"]));
            None
        }
        ("event_msg", Some("turn_aborted")) => {
            t.start_ms = None;
            None
        }
        ("event_msg", Some("task_complete")) => {
            let end = ts_ms(&v["timestamp"]).unwrap_or_else(now_ms);
            let duration = p["duration_ms"].as_i64().or_else(|| t.start_ms.map(|s| (end - s).max(0)));
            t.start_ms = None;
            let id = p["turn_id"].as_str().map(str::to_string).unwrap_or_else(|| end.to_string());
            if t.skip || !remember(t, id) {
                return None;
            }
            Some(Done {
                agent: "codex",
                project: folder_name(&t.cwd),
                message: p["last_agent_message"].as_str().unwrap_or_default().to_string(),
                duration_ms: duration,
                end_ms: end,
            })
        }
        _ => None,
    }
}

/// Whether a Claude Code user entry is a prompt (not a tool result or injected context).
fn is_prompt(v: &Value) -> bool {
    if v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
        return false;
    }
    match &v["message"]["content"] {
        Value::String(s) => !s.starts_with("<command-") && !s.starts_with("<local-command"),
        Value::Array(a) => a.iter().any(|b| b["type"] == "text") && !a.iter().any(|b| b["type"] == "tool_result"),
        _ => false,
    }
}

/// One line of a Claude Code transcript.
pub fn claude_line(t: &mut Turn, line: &str) -> Option<Done> {
    let v: Value = serde_json::from_str(line).ok()?;
    if let Some(c) = v["cwd"].as_str() {
        t.cwd = c.to_string();
    }
    match v["type"].as_str()? {
        "user" if is_prompt(&v) => {
            t.start_ms = ts_ms(&v["timestamp"]);
            None
        }
        "assistant" if v["isSidechain"].as_bool() != Some(true) && v["message"]["stop_reason"] == "end_turn" => {
            let end = ts_ms(&v["timestamp"]).unwrap_or_else(now_ms);
            let id = v["message"]["id"].as_str().or(v["uuid"].as_str()).map(str::to_string).unwrap_or_else(|| end.to_string());
            let text = v["message"]["content"].as_array().map(|a| a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
            // One message is written as several entries (thinking, then text): gather them.
            if let Some((_, d)) = t.pending.as_mut().filter(|(pid, _)| *pid == id) {
                // The same entry read twice (a file rewritten in place) adds nothing.
                if !text.is_empty() && !d.message.ends_with(&text) {
                    d.message = if d.message.is_empty() { text } else { format!("{}\n{text}", d.message) };
                }
                return None;
            }
            let before = flush(t);
            if remember(t, id.clone()) {
                let d = Done { agent: "claude", project: folder_name(&t.cwd), message: text, duration_ms: t.start_ms.map(|s| (end - s).max(0)), end_ms: end };
                t.pending = Some((id, d));
            }
            before
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- JSONL files

struct Tracked {
    offset: u64,
    /// The end of the last read when it stopped mid-line (bytes: a read can end inside a
    /// character).
    partial: Vec<u8>,
    turn: Turn,
}

/// Session files of one agent, followed by appending reads.
pub struct Files {
    agent: &'static str,
    parse: fn(&mut Turn, &str) -> Option<Done>,
    /// By `path_key`, so one file reached by two spellings is followed once.
    files: HashMap<String, (PathBuf, Tracked)>,
    scanned: Option<std::time::Instant>,
    /// When watching started: files created before it are read from their end.
    started: SystemTime,
    /// Codex rollouts from its database, and when they were read.
    threads: (Vec<PathBuf>, Option<std::time::Instant>),
}

/// The same file under one key: Windows paths ignore case and take either slash.
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s.into_owned()
    }
}

/// When a session file's first entry was written (both agents stamp their entries).
fn first_entry_ms(line: &str) -> Option<i64> {
    let v: Value = serde_json::from_str(line).ok()?;
    ts_ms(&v["timestamp"]).or_else(|| ts_ms(&v["payload"]["timestamp"]))
}

/// Modified within `RECENT`. `fs::metadata` rather than the directory listing's copy: on
/// Windows the listing can show a stale time for a file another process keeps open.
fn recent(p: &Path) -> bool {
    fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|a| a < RECENT)
}

impl Files {
    pub fn new(agent: &'static str) -> Files {
        let parse = if agent == "codex" { codex_line } else { claude_line };
        Files { agent, parse, files: HashMap::new(), scanned: None, started: SystemTime::now(), threads: (Vec::new(), None) }
    }

    fn candidates(&mut self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let mut out = Vec::new();
        if self.agent == "codex" {
            // A resumed thread appends to its rollout in the folder of the day it began:
            // Codex's database knows where. Today's and yesterday's folders cover threads
            // the database doesn't list (yet). In WSL the database is read from a copy, so
            // less often.
            let every = if crate::env::is_wsl() { Duration::from_secs(120) } else { RESCAN };
            if self.threads.1.is_none_or(|t| t.elapsed() >= every) {
                let since = now_ms() - RECENT.as_millis() as i64;
                self.threads = (crate::sessions::recent_threads(since).unwrap_or_default().into_iter().map(|t| t.rollout).collect(), Some(std::time::Instant::now()));
            }
            out.extend(self.threads.0.iter().cloned());
            let root = codex_sessions();
            let today = chrono::Local::now().date_naive();
            for d in [today, today - chrono::Days::new(1)] {
                dirs.push(root.join(d.format("%Y").to_string()).join(d.format("%m").to_string()).join(d.format("%d").to_string()));
            }
        } else if let Ok(rd) = fs::read_dir(claude_projects()) {
            dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        // The listing's own times (free) narrow it down first; `recent` opens each file left.
        let today = |e: &fs::DirEntry| e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_none_or(|a| a < Duration::from_secs(86_400));
        for d in dirs {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            out.extend(rd.flatten().filter(|e| today(e)).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")));
        }
        let mut seen = HashSet::new();
        out.retain(|p| seen.insert(path_key(p)) && recent(p));
        out
    }

    pub fn poll(&mut self, out: &mut Vec<Done>) {
        if self.scanned.is_none_or(|s| s.elapsed() >= RESCAN) {
            self.scanned = Some(std::time::Instant::now());
            for p in self.candidates() {
                let key = path_key(&p);
                if self.files.contains_key(&key) {
                    continue;
                }
                let mut turn = Turn::default();
                // A file begun since watching started is read whole; an older one (a session
                // resumed after a break) from its end, keeping what its first line says.
                // Without a creation time (the WSL share) its first entry's time decides.
                let meta = fs::metadata(&p).ok();
                let first = File::open(&p).ok().and_then(|f| BufReader::new(f).lines().next()).and_then(Result::ok);
                let started_ms = self.started.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                let fresh = match meta.as_ref().map(|m| m.created()) {
                    Some(Ok(c)) => c >= self.started,
                    _ => first.as_deref().and_then(first_entry_ms).is_some_and(|t| t >= started_ms),
                };
                let offset = if fresh {
                    0
                } else {
                    if let Some(first) = &first {
                        let _ = (self.parse)(&mut turn, first);
                    }
                    meta.map(|m| m.len()).unwrap_or(0)
                };
                self.files.insert(key, (p, Tracked { offset, partial: Vec::new(), turn }));
            }
            // Forget files that stopped changing long ago.
            self.files.retain(|_, (p, _)| fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|a| a < RECENT * 2));
        }
        let cutoff = now_ms() - STALE_MS;
        for (p, f) in self.files.values_mut() {
            let lines = read_new(p, f);
            let found: Vec<Done> = lines.iter().filter_map(|l| (self.parse)(&mut f.turn, l)).collect();
            out.extend(found.into_iter().chain(flush(&mut f.turn)).filter(|d| d.end_ms >= cutoff));
        }
    }
}

/// The complete lines appended since the last read.
fn read_new(p: &Path, f: &mut Tracked) -> Vec<String> {
    let Ok(len) = fs::metadata(p).map(|m| m.len()) else { return Vec::new() };
    if len < f.offset {
        // Rewritten from scratch: start over.
        f.offset = 0;
        f.partial.clear();
    }
    if len == f.offset {
        return Vec::new();
    }
    let Ok(mut file) = File::open(p) else { return Vec::new() };
    if file.seek(SeekFrom::Start(f.offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let Ok(n) = file.take(READ_CAP).read_to_end(&mut buf) else { return Vec::new() };
    f.offset += n as u64;
    split_lines(&mut f.partial, &buf)
}

/// Appends `chunk` to `partial` and takes out every complete line. Lines are decoded only
/// once whole, so a character split between two reads stays intact.
pub fn split_lines(partial: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    partial.extend_from_slice(chunk);
    let Some(last) = partial.iter().rposition(|b| *b == b'\n') else { return Vec::new() };
    let rest = partial.split_off(last + 1);
    let lines = partial
        .split(|b| *b == b'\n')
        .map(|l| String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)).into_owned())
        .filter(|l| !l.is_empty())
        .collect();
    *partial = rest;
    lines
}

// ---------------------------------------------------------------- SQLite (OpenCode, ZCode)

pub struct Db {
    agent: &'static str,
    path: fn() -> PathBuf,
    /// Messages updated after this (ms) are looked at.
    since: i64,
    seen: HashSet<String>,
    stamp: Option<(SystemTime, Option<SystemTime>)>,
}

impl Db {
    pub fn new(agent: &'static str) -> Db {
        let path = if agent == "opencode" { opencode_db } else { zcode_db };
        Db { agent, path, since: now_ms(), seen: HashSet::new(), stamp: None }
    }

    pub fn poll(&mut self, out: &mut Vec<Done>) -> Result<(), String> {
        let p = (self.path)();
        let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
        let Some(main) = mtime(&p) else { return Ok(()) };
        let stamp = (main, mtime(&PathBuf::from(format!("{}-wal", p.display()))));
        if self.stamp == Some(stamp) {
            return Ok(());
        }
        let conn = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_millis(500)).map_err(|e| e.to_string())?;
        let found = query_done(&conn, self.agent, self.since).map_err(|e| e.to_string())?;
        // A full page leaves rows for the next poll, even if the database doesn't change.
        self.stamp = (found.len() < PAGE).then_some(stamp);
        let cutoff = now_ms() - STALE_MS;
        for (id, updated, d) in found {
            self.since = self.since.max(updated);
            if self.seen.len() > 512 {
                self.seen.clear();
            }
            if !d.agent.is_empty() && d.end_ms >= cutoff && self.seen.insert(id) {
                out.push(d);
            }
        }
        Ok(())
    }
}

/// Rows read per poll.
const PAGE: usize = 500;

/// Finished top-level assistant turns updated at or after `since` (rows sharing the last
/// time seen are read again; `seen` drops the repeats): (message id, updated, turn).
pub fn query_done(conn: &Connection, agent: &'static str, since: i64) -> rusqlite::Result<Vec<(String, i64, Done)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT m.id, m.time_updated, m.data, s.directory, s.parent_id FROM message m JOIN session s ON s.id = m.session_id \
         WHERE m.time_updated >= ?1 ORDER BY m.time_updated LIMIT {PAGE}"
    ))?;
    let rows: Vec<(String, i64, String, String, Option<String>)> = stmt
        .query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (id, updated, data, dir, parent) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
        let end = v["time"]["completed"].as_i64();
        if v["role"] != "assistant" || v["finish"] != "stop" || end.is_none() || parent.is_some_and(|p| !p.is_empty()) {
            // Still remember how far we got.
            out.push((id, updated, Done::none()));
            continue;
        }
        let end = end.unwrap_or(updated);
        let start: Option<i64> = v["parentID"].as_str().and_then(|u| conn.query_row("SELECT time_created FROM message WHERE id = ?1", params![u], |r| r.get(0)).ok());
        let message = last_text(conn, &id).unwrap_or_default();
        out.push((id, updated, Done { agent, project: folder_name(&dir), message, duration_ms: start.map(|s| (end - s).max(0)), end_ms: end }));
    }
    Ok(out)
}

fn last_text(conn: &Connection, message_id: &str) -> Option<String> {
    let mut stmt = conn.prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY time_created").ok()?;
    let parts = stmt.query_map(params![message_id], |r| r.get::<_, String>(0)).ok()?;
    parts.flatten().filter_map(|d| serde_json::from_str::<Value>(&d).ok()).filter(|v| v["type"] == "text").filter_map(|v| v["text"].as_str().map(str::to_string)).last()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(parse: fn(&mut Turn, &str) -> Option<Done>, lines: &[&str]) -> Vec<Done> {
        let mut t = Turn::default();
        let mut got: Vec<Done> = lines.iter().filter_map(|l| parse(&mut t, l)).collect();
        got.extend(flush(&mut t));
        got
    }

    #[test]
    fn codex_turns() {
        let meta = r#"{"type":"session_meta","payload":{"cwd":"D:\\xm\\proj","source":"vscode","thread_source":""}}"#;
        let start = r#"{"timestamp":"2026-11-04T07:38:39.268Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1","started_at":1793777919}}"#;
        let done = r#"{"timestamp":"2026-11-04T07:38:42.652Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"All set","duration_ms":3384}}"#;
        let got = feed(codex_line, &[meta, start, done, done]);
        assert_eq!(got.len(), 1, "a repeated turn id reports once");
        assert_eq!(got[0].project, "proj");
        assert_eq!(got[0].message, "All set");
        assert_eq!(got[0].duration_ms, Some(3384));

        // Without duration_ms: from task_started.
        let old = r#"{"timestamp":"2026-11-04T07:38:49.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","last_agent_message":null}}"#;
        let got = feed(codex_line, &[meta, start, old]);
        assert_eq!(got[0].duration_ms, Some(10_000));
        assert_eq!(got[0].message, "");
    }

    #[test]
    fn codex_skips_reviews_and_subagents() {
        let review = r#"{"type":"session_meta","payload":{"cwd":"/p","source":"vscode","thread_source":"guardian_review"}}"#;
        let sub = r#"{"type":"session_meta","payload":{"cwd":"/p","source":{"subagent":"x"},"thread_source":""}}"#;
        let exec = r#"{"type":"session_meta","payload":{"cwd":"/p","source":"exec"}}"#;
        let done = r#"{"timestamp":"2026-11-04T07:38:42.652Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#;
        for meta in [review, sub, exec] {
            assert!(feed(codex_line, &[meta, done]).is_empty(), "{meta}");
        }
    }

    #[test]
    fn claude_turns() {
        let prompt = r#"{"type":"user","cwd":"/home/u/app","timestamp":"2026-11-04T08:00:00.000Z","message":{"role":"user","content":"fix it"}}"#;
        let tool = r#"{"type":"assistant","cwd":"/home/u/app","timestamp":"2026-11-04T08:00:05.000Z","message":{"id":"m1","stop_reason":"tool_use","content":[{"type":"tool_use"}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-11-04T08:00:06.000Z","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#;
        let think = r#"{"type":"assistant","timestamp":"2026-11-04T08:01:00.000Z","message":{"id":"m2","stop_reason":"end_turn","content":[{"type":"thinking"}]}}"#;
        let text = r#"{"type":"assistant","timestamp":"2026-11-04T08:01:00.100Z","message":{"id":"m2","stop_reason":"end_turn","content":[{"type":"text","text":"Fixed."}]}}"#;
        let side = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-11-04T08:01:00.000Z","message":{"id":"m3","stop_reason":"end_turn","content":[]}}"#;
        let got = feed(claude_line, &[prompt, tool, result, side, think, text]);
        assert_eq!(got.len(), 1, "one report per message, none for sidechains");
        assert_eq!(got[0].project, "app");
        assert_eq!(got[0].duration_ms, Some(60_000), "timed from the prompt, not the tool result");
        assert_eq!(got[0].message, "Fixed.", "the text entry after the thinking one joins it");
        // Two turns in one read both report, each once.
        let prompt2 = r#"{"type":"user","timestamp":"2026-11-04T08:02:00.000Z","message":{"role":"user","content":[{"type":"text","text":"again"}]}}"#;
        let text2 = r#"{"type":"assistant","timestamp":"2026-11-04T08:02:03.000Z","message":{"id":"m4","stop_reason":"end_turn","content":[{"type":"text","text":"Again."}]}}"#;
        let got = feed(claude_line, &[prompt, text, prompt2, text2, text2]);
        let got: Vec<_> = got.iter().map(|d| (d.message.as_str(), d.duration_ms)).collect();
        assert_eq!(got, vec![("Fixed.", Some(60_100)), ("Again.", Some(3000))]);
    }

    #[test]
    fn lines_split_across_reads() {
        let mut partial = Vec::new();
        assert!(split_lines(&mut partial, b"{\"a\":1}\r\n{\"b\"").len() == 1);
        assert_eq!(partial, b"{\"b\"");
        assert_eq!(split_lines(&mut partial, b":2}\n\n"), vec!["{\"b\":2}".to_string()]);
        assert!(partial.is_empty());
        // "中" is three bytes; the read stops after the first one.
        let line = "{\"c\":\"中\"}\n".as_bytes();
        assert!(split_lines(&mut partial, &line[..7]).is_empty());
        assert_eq!(split_lines(&mut partial, &line[7..]), vec!["{\"c\":\"中\"}".to_string()]);
    }

    /// Reads this machine's newest records: `cargo test real_records -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_records() {
        let newest = |dir: PathBuf, depth: usize| {
            let mut stack = vec![(dir, 0)];
            let mut all: Vec<(SystemTime, PathBuf)> = Vec::new();
            while let Some((d, n)) = stack.pop() {
                for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() && n < depth {
                        stack.push((p, n + 1));
                    } else if p.extension().is_some_and(|x| x == "jsonl") {
                        all.push((e.metadata().unwrap().modified().unwrap(), p));
                    }
                }
            }
            all.sort_by_key(|x| std::cmp::Reverse(x.0));
            all.into_iter().take(6).map(|x| x.1).collect::<Vec<_>>()
        };
        for (agent, dir, depth) in [("codex", codex_sessions(), 3), ("claude", claude_projects(), 1)] {
            for p in newest(dir, depth) {
                let parse = if agent == "codex" { codex_line } else { claude_line };
                let mut t = Turn::default();
                let text = fs::read_to_string(&p).unwrap();
                let mut got: Vec<Done> = text.lines().filter_map(|l| parse(&mut t, l)).collect();
                got.extend(flush(&mut t));
                println!("{agent} {}: {} turns, last {:?}", p.display(), got.len(), got.last().map(|d| (&d.project, d.duration_ms, d.message.chars().take(40).collect::<String>())));
            }
        }
        for (agent, p) in [("opencode", opencode_db()), ("zcode", zcode_db())] {
            let Ok(c) = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
            let got: Vec<_> = query_done(&c, agent, 0).unwrap().into_iter().filter(|x| !x.2.agent.is_empty()).collect();
            println!("{agent}: {} turns, last {:?}", got.len(), got.last().map(|d| (&d.2.project, d.2.duration_ms, d.2.message.chars().take(40).collect::<String>())));
        }
    }

    #[test]
    fn path_keys_and_first_entries() {
        if cfg!(windows) {
            assert_eq!(path_key(Path::new("C:/Users/Me/.codex/a.jsonl")), path_key(Path::new(r"c:\users\me\.codex\A.jsonl")));
        }
        assert_eq!(first_entry_ms(r#"{"timestamp":"2026-11-04T07:38:39.268Z","type":"session_meta"}"#), Some(1_793_777_919_268));
        assert_eq!(first_entry_ms(r#"{"type":"session_meta","payload":{"timestamp":"2026-11-04T07:38:39.268Z"}}"#), Some(1_793_777_919_268));
        assert_eq!(first_entry_ms("not json"), None);
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("D:\\xm\\proj\\"), "proj");
        assert_eq!(folder_name("/home/u/app"), "app");
        assert_eq!(folder_name(""), "");
    }

    #[test]
    fn sqlite_turns() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE session (id text primary key, directory text not null, parent_id text);
             CREATE TABLE message (id text primary key, session_id text, time_created integer, time_updated integer, data text);
             CREATE TABLE part (id text primary key, message_id text, time_created integer, data text);
             INSERT INTO session VALUES ('s1', 'F:/Project/kanban', NULL), ('s2', 'F:/Project/kanban', 's1');
             INSERT INTO message VALUES
               ('u1', 's1', 1000, 1000, '{\"role\":\"user\"}'),
               ('a1', 's1', 2000, 3000, '{\"role\":\"assistant\",\"parentID\":\"u1\",\"finish\":\"tool-calls\",\"time\":{\"completed\":3000}}'),
               ('a2', 's1', 3000, 9000, '{\"role\":\"assistant\",\"parentID\":\"u1\",\"finish\":\"stop\",\"time\":{\"completed\":9000}}'),
               ('a3', 's2', 3000, 9500, '{\"role\":\"assistant\",\"parentID\":\"u1\",\"finish\":\"stop\",\"time\":{\"completed\":9500}}'),
               ('a4', 's1', 9600, 9600, '{\"role\":\"assistant\",\"finish\":\"stop\",\"time\":{}}');
             INSERT INTO part VALUES ('p1', 'a2', 1, '{\"type\":\"text\",\"text\":\"first\"}'), ('p2', 'a2', 2, '{\"type\":\"text\",\"text\":\"Done here\"}');",
        )
        .unwrap();
        let got = query_done(&c, "opencode", 0).unwrap();
        assert_eq!(got.last().unwrap().1, 9600, "the watermark moves past every row");
        let real: Vec<_> = got.iter().filter(|(_, _, d)| !d.agent.is_empty()).collect();
        assert_eq!(real.len(), 1, "tool calls, subagent sessions and unfinished messages don't count");
        let d = &real[0].2;
        assert_eq!((d.project.as_str(), d.message.as_str(), d.duration_ms), ("kanban", "Done here", Some(8000)));
        // Rows at the watermark are read again (the caller's `seen` drops them).
        let again = query_done(&c, "opencode", 9000).unwrap();
        assert_eq!(again.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["a2", "a3", "a4"]);
        assert!(query_done(&c, "opencode", 9601).unwrap().is_empty());
    }
}
