//! The gateway's usage ledger: one JSON line per request, in
//! `~/.agentplus/usage/gateway-YYYY-MM.jsonl`. It records the conversation id the agent
//! sent, so `scan` can leave those calls out of the agent's own logs.

use super::Rec;
use crate::gateway::server::LogEntry;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Unix seconds when the request came in and when it finished.
    pub t0: i64,
    pub t: i64,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub route: String,
    /// Input tokens, cache reads included.
    #[serde(default, rename = "in")]
    pub input: u64,
    #[serde(default, rename = "out")]
    pub output: u64,
    #[serde(default)]
    pub cached: u64,
    #[serde(default)]
    pub status: u16,
}

impl Row {
    pub fn as_rec(&self) -> Rec {
        Rec {
            t: self.t,
            model: self.model.clone(),
            session: self.session.clone().unwrap_or_default(),
            input: self.input.saturating_sub(self.cached),
            output: self.output,
            cache_read: self.cached.min(self.input),
            official: false,
            calls: 1,
            ..Default::default()
        }
    }

    /// A row for a finished request; None for what isn't a model call by an agent.
    pub fn from_log(t0: i64, t: i64, log: &LogEntry) -> Option<Row> {
        let agent = log.agent.clone()?;
        if log.method != "POST" || log.path.ends_with("/count_tokens") {
            return None;
        }
        // A failed call that used no tokens is not usage (the gateway page counts failures).
        if log.status >= 400 && log.usage.is_none_or(|(i, o)| i + o == 0) {
            return None;
        }
        let (input, output) = log.usage.unwrap_or((0, 0));
        Some(Row { t0, t, agent, session: log.session.clone(), model: log.model.clone(), route: log.route.clone(), input, output, cached: log.cached, status: log.status })
    }
}

pub fn dir() -> PathBuf {
    crate::util::agentplus_dir().join("usage")
}

fn file_for(dir: &Path, t: i64) -> PathBuf {
    use chrono::TimeZone;
    let month = chrono::Local.timestamp_opt(t, 0).single().map_or_else(|| "unknown".into(), |d| d.format("%Y-%m").to_string());
    dir.join(format!("gateway-{month}.jsonl"))
}

static WRITE: Mutex<()> = Mutex::new(());

/// Appends the finished request to the ledger. Errors only reach the diagnostic log: a
/// full disk must not fail the request.
pub fn record(t0: i64, log: &LogEntry) {
    // Gateway tests run real requests; they must not write to the user's ledger.
    if cfg!(test) && crate::util::test_home().is_none() {
        return;
    }
    let t = chrono::Local::now().timestamp();
    let Some(row) = Row::from_log(t0, t, log) else { return };
    if let Err(e) = append(&dir(), &row) {
        crate::applog::warn("usage", format!("ledger write failed: {e:#}"));
    }
}

fn append(dir: &Path, row: &Row) -> anyhow::Result<()> {
    let _g = crate::util::lock(&WRITE);
    fs::create_dir_all(dir)?;
    let path = file_for(dir, row.t);
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    // A line cut short (crash, full disk) would swallow this one: start on a new line.
    if !ends_with_newline(&path) {
        line.insert(0, '\n');
    }
    OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())?;
    Ok(())
}

/// True for a missing or empty file too.
fn ends_with_newline(p: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = fs::File::open(p) else { return true };
    if f.metadata().map_or(0, |m| m.len()) == 0 || f.seek(SeekFrom::End(-1)).is_err() {
        return true;
    }
    let mut b = [0u8];
    f.read_exact(&mut b).is_err() || b[0] == b'\n'
}

pub struct Ledger {
    pub rows: Vec<Row>,
    pub errors: Vec<String>,
}

pub fn read_all() -> Ledger {
    read_dir(&dir())
}

fn read_dir(dir: &Path) -> Ledger {
    let mut out = Ledger { rows: vec![], errors: vec![] };
    let Ok(rd) = fs::read_dir(dir) else { return out };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("gateway-") && n.ends_with(".jsonl")))
        .collect();
    files.sort();
    for p in files {
        match fs::read_to_string(&p) {
            // A line cut short by a crash is skipped; the rest still counts.
            Ok(text) => out.rows.extend(text.lines().filter_map(|l| serde_json::from_str::<Row>(l).ok())),
            Err(e) => out.errors.push(tr!("Can't read {}: {e}", "读取 {} 失败：{e}", p.display())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> LogEntry {
        LogEntry {
            method: "POST".into(),
            path: "/glm/v1/messages".into(),
            route: "glm".into(),
            model: "glm-5".into(),
            status: 200,
            usage: Some((120, 8)),
            cached: 100,
            agent: Some("claude".into()),
            session: Some("abc".into()),
            ..Default::default()
        }
    }

    #[test]
    fn rows_only_for_agent_model_calls() {
        let r = Row::from_log(1, 3, &entry()).unwrap();
        assert_eq!((r.t0, r.t, r.agent.as_str(), r.session.as_deref(), r.input, r.cached), (1, 3, "claude", Some("abc"), 120, 100));
        let rec = r.as_rec();
        assert_eq!((rec.input, rec.cache_read, rec.output, rec.official), (20, 100, 8, false));
        let mut e = entry();
        e.agent = None;
        assert!(Row::from_log(1, 3, &e).is_none(), "refused request");
        let mut e = entry();
        e.method = "GET".into();
        assert!(Row::from_log(1, 3, &e).is_none(), "model list");
        let mut e = entry();
        e.path = "/glm/v1/messages/count_tokens".into();
        assert!(Row::from_log(1, 3, &e).is_none());
        let mut e = entry();
        (e.status, e.usage) = (502, None);
        assert!(Row::from_log(1, 3, &e).is_none(), "a failed call without tokens");
        e.usage = Some((30, 2));
        assert!(Row::from_log(1, 3, &e).is_some(), "a stream that broke after using tokens");
    }

    #[test]
    fn append_and_read_back_skipping_broken_lines() {
        let d = std::env::temp_dir().join(format!("agp-ledger-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let r = Row::from_log(1_800_000_000, 1_800_000_005, &entry()).unwrap();
        append(&d, &r).unwrap();
        let f = file_for(&d, r.t);
        let mut text = fs::read_to_string(&f).unwrap();
        text.push_str("{\"t0\":1,\"t\":");
        fs::write(&f, text).unwrap();
        let mut r2 = r.clone();
        r2.session = None;
        append(&d, &r2).unwrap();
        let got = read_dir(&d);
        assert_eq!(got.rows, vec![r.clone(), r2.clone()]);
        assert!(got.errors.is_empty());
        assert!(serde_json::to_string(&r2).unwrap().contains("\"in\":120"));
        let _ = fs::remove_dir_all(&d);
    }
}
