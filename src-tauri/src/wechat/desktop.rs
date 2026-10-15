//! Notices for turns that finish outside the bridge (Codex desktop, the CLI): the sessions
//! updated in the last hour are read from Codex's state database, and the new lines of
//! their rollout files are checked for the end of a turn (`task_complete`, `turn_aborted`).
//! A file seen for the first time is only read from its current end, so nothing old is
//! reported.

use crate::sessions::RecentThread;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Sessions updated this long ago or less are watched.
const WINDOW_MS: i64 = 60 * 60 * 1000;
/// Most read from one file in one check; a bigger jump only moves the offset.
const MAX_READ: u64 = 4 * 1024 * 1024;

/// A turn that ended in a watched session.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub thread: String,
    pub title: String,
    pub turn: String,
    pub aborted: bool,
    pub duration_ms: Option<u64>,
    /// Codex's last message of the turn, when the file has one.
    pub message: Option<String>,
}

#[derive(Default)]
pub struct Watch {
    /// Thread id → the file being followed.
    files: HashMap<String, Followed>,
}

struct Followed {
    path: PathBuf,
    /// Bytes already read.
    offset: u64,
    /// Codex's last message so far in the turn being read (it may end in a later check).
    last: Option<String>,
}

impl Watch {
    /// The turns that ended since the last check, in sessions `skip` doesn't exclude.
    pub fn check(&mut self, skip: impl Fn(&str) -> bool) -> Vec<Finished> {
        let since = chrono::Utc::now().timestamp_millis() - WINDOW_MS;
        match crate::sessions::recent_threads(since) {
            Ok(threads) => self.check_threads(&threads, skip),
            Err(e) => {
                crate::applog::warn("wechat", format!("desktop sessions: {e:#}"));
                vec![]
            }
        }
    }

    pub fn check_threads(&mut self, threads: &[RecentThread], skip: impl Fn(&str) -> bool) -> Vec<Finished> {
        let mut out = vec![];
        self.files.retain(|id, _| threads.iter().any(|t| &t.id == id));
        for t in threads {
            let len = crate::util::file_len(&t.rollout);
            let Some(f) = self.files.get_mut(&t.id) else {
                self.files.insert(t.id.clone(), Followed { path: t.rollout.clone(), offset: len, last: None });
                continue;
            };
            if f.path != t.rollout || len < f.offset {
                *f = Followed { path: t.rollout.clone(), offset: len, last: None };
                continue;
            }
            if len == f.offset {
                continue;
            }
            let (ends, read) = read_ends(&f.path, f.offset, &mut f.last);
            f.offset += read;
            if skip(&t.id) {
                continue;
            }
            out.extend(ends.into_iter().map(|e| Finished { thread: t.id.clone(), title: t.title.clone(), ..e }));
        }
        out
    }
}

/// The turn ends in a file after `offset`, and how many bytes of whole lines were read.
fn read_ends(path: &Path, offset: u64, last: &mut Option<String>) -> (Vec<Finished>, u64) {
    let Ok(mut f) = std::fs::File::open(path) else { return (vec![], 0) };
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return (vec![], 0);
    }
    let mut buf = vec![];
    if f.take(MAX_READ).read_to_end(&mut buf).is_err() {
        return (vec![], 0);
    }
    // Only whole lines: one still being written is read next time.
    let Some(end) = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1) else {
        return (vec![], if buf.len() as u64 >= MAX_READ { buf.len() as u64 } else { 0 });
    };
    (turn_ends(&String::from_utf8_lossy(&buf[..end]), last), end as u64)
}

/// `task_complete` / `turn_aborted` events in rollout lines; the last assistant message
/// before each stands in when the event doesn't carry one. `last` carries that message
/// from one read to the next.
pub fn turn_ends(text: &str, last: &mut Option<String>) -> Vec<Finished> {
    let mut out = vec![];
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("response_item"), Some("message")) if p["role"] == "assistant" => {
                let text: Vec<&str> = p["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
                if !text.is_empty() {
                    *last = Some(text.join("\n"));
                }
            }
            (Some("event_msg"), Some(kind @ ("task_complete" | "turn_aborted"))) => {
                let message = p["last_agent_message"].as_str().map(String::from).filter(|s| !s.trim().is_empty()).or_else(|| last.take());
                out.push(Finished {
                    thread: String::new(),
                    title: String::new(),
                    turn: p["turn_id"].as_str().unwrap_or("").to_string(),
                    aborted: kind == "turn_aborted",
                    duration_ms: p["duration_ms"].as_u64(),
                    message: if kind == "turn_aborted" { None } else { message },
                });
                *last = None;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const DONE: &str = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"duration_ms":170966}}"#;

    fn said(text: &str) -> String {
        format!(r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{text}"}}]}}}}"#)
    }

    #[test]
    fn finds_turn_ends_with_their_last_message() {
        let text = [said("working"), said("All done"), DONE.to_string(), r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t2"}}"#.to_string()].join("\n");
        let ends = turn_ends(&text, &mut None);
        assert_eq!(ends.len(), 2);
        assert_eq!((ends[0].turn.as_str(), ends[0].message.as_deref(), ends[0].duration_ms), ("t1", Some("All done"), Some(170966)));
        assert!(ends[1].aborted && ends[1].message.is_none());
        let carried = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t3","last_agent_message":"Fixed"}}"#;
        assert_eq!(turn_ends(&format!("{}\n{carried}", said("x")), &mut None)[0].message.as_deref(), Some("Fixed"));
        assert!(turn_ends("not json\n{}", &mut None).is_empty());
    }

    #[test]
    fn reports_only_what_is_new_and_whole() {
        let h = crate::util::TestHome::new("wechat-desktop");
        let path = h.0.join("rollout.jsonl");
        std::fs::write(&path, format!("{DONE}\n")).unwrap();
        let threads = vec![RecentThread { id: "a".into(), title: "Fix it".into(), cwd: String::new(), rollout: path.clone() }];
        let mut w = Watch::default();
        assert!(w.check_threads(&threads, |_| false).is_empty(), "seen first: from its end");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{}\n{}", said("Second"), &DONE[..40]).unwrap();
        assert!(w.check_threads(&threads, |_| false).is_empty(), "the turn hasn't ended yet");
        writeln!(f, "{}", &DONE[40..]).unwrap();
        let got = w.check_threads(&threads, |_| false);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].thread.as_str(), got[0].title.as_str(), got[0].message.as_deref()), ("a", "Fix it", Some("Second")));
        assert!(w.check_threads(&threads, |_| false).is_empty(), "reported once");
        // A skipped session still moves on, so it isn't reported later either.
        writeln!(f, "{DONE}").unwrap();
        assert!(w.check_threads(&threads, |_| true).is_empty());
        assert!(w.check_threads(&threads, |_| false).is_empty());
        // Gone from the recent list: forgotten.
        w.check_threads(&[], |_| false);
        assert!(w.files.is_empty());
    }
}
