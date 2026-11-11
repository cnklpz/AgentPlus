//! Token usage across agents, from two sources that never count the same request twice:
//!
//! - `ledger`: every request through the local gateway, written as it finishes, with the
//!   conversation id the agent sent.
//! - `scan`: the agents' own session logs (Codex rollouts, Claude Code transcripts, Gemini
//!   CLI chats, OpenCode's database), which also cover what never passes the gateway:
//!   subscriptions (ChatGPT, Claude Pro/Max, Gemini) and providers set up directly.
//!
//! A log record whose conversation went through the gateway at that time belongs to the
//! gateway and is left out of the local side. Costs are API-equivalent prices (`price`),
//! so subscription usage shows what it would have cost on the API.

pub mod ledger;
pub mod price;
pub mod scan;

use crate::i18n::l;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where a token count comes from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// The model vendor's own service (subscription or the vendor's API key).
    #[default]
    Official,
    /// A provider set up directly in the agent, not through the gateway.
    ThirdParty,
    /// The local gateway (from its ledger).
    Gateway,
}

/// One model call from an agent's log.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Rec {
    /// Unix seconds.
    pub t: i64,
    #[serde(rename = "m")]
    pub model: String,
    #[serde(rename = "s")]
    pub session: String,
    /// Another id the agent may send for the same conversation (Codex: the root session of a
    /// subagent thread); empty when there is none.
    #[serde(rename = "a", default, skip_serializing_if = "String::is_empty")]
    pub alt_session: String,
    /// Input tokens not read from or written to the cache.
    #[serde(rename = "i")]
    pub input: u64,
    /// Output tokens, reasoning included.
    #[serde(rename = "o")]
    pub output: u64,
    #[serde(rename = "cr")]
    pub cache_read: u64,
    #[serde(rename = "cw")]
    pub cache_write: u64,
    /// The vendor's own service; else a third-party provider.
    #[serde(rename = "of")]
    pub official: bool,
    /// Identifies the call across files (copies of a conversation repeat it); 0 = none.
    #[serde(rename = "k")]
    pub key: u64,
    /// Model calls this record adds up (0 counts as 1): Hermes keeps totals, not calls.
    #[serde(rename = "n", default, skip_serializing_if = "is_zero")]
    pub calls: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Rec {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One cell of the report: a time bucket × agent × source × model.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// Unix seconds at the start of the bucket (local time).
    pub t: i64,
    pub agent: String,
    pub source: Source,
    pub model: String,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// API-equivalent cost in USD of the requests with a known price.
    pub cost: f64,
    /// Requests whose model has no price.
    pub unpriced: u64,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub from: i64,
    pub to: i64,
    pub bucket: String,
    pub cells: Vec<Cell>,
    /// Log records left out because the gateway counted them.
    pub deduped_requests: u64,
    pub deduped_tokens: u64,
    /// Log records counted once although several files repeat them (forks, resumes).
    pub repeated_requests: u64,
    /// When the gateway ledger starts (None: nothing recorded yet).
    pub ledger_since: Option<i64>,
    /// Agents whose logs were found, and how many files each.
    pub sources: Vec<SourceInfo>,
    /// When the logs were read (Unix seconds): the page re-reads them when this is old.
    pub scanned_at: i64,
    pub errors: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub agent: String,
    pub files: usize,
    pub path: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bucket {
    Hour,
    Day,
}

impl Bucket {
    pub fn parse(s: &str) -> Bucket {
        if s == "hour" { Bucket::Hour } else { Bucket::Day }
    }

    /// Start of the local-time bucket holding `t`.
    pub fn start(self, t: i64) -> i64 {
        use chrono::{Local, TimeZone, Timelike};
        let Some(dt) = Local.timestamp_opt(t, 0).single() else { return t - t.rem_euclid(3600) };
        match self {
            Bucket::Hour => t - i64::from(dt.minute()) * 60 - i64::from(dt.second()),
            Bucket::Day => {
                // Where DST starts at midnight that time doesn't exist: the day starts at the first
                // valid time after it (01:00), not at UTC midnight.
                let date = dt.date_naive();
                let midnight = (0..=3).find_map(|h| date.and_hms_opt(h, 0, 0).and_then(|n| Local.from_local_datetime(&n).earliest()));
                midnight.map_or(t - t.rem_euclid(86400), |m| m.timestamp())
            }
        }
    }
}

/// Seconds around a gateway request within which a log record of the same conversation
/// is taken to be that request (logs stamp a call when it starts or ends).
const MATCH_SLACK: i64 = 120;
/// Longest a single request is assumed to take, for the interval search.
const MAX_REQUEST: i64 = 3600;

/// Gateway requests by (agent, conversation): (start, end) sorted by start.
pub struct GatewaySessions(HashMap<(String, String), Vec<(i64, i64)>>);

impl GatewaySessions {
    pub fn new(rows: &[ledger::Row]) -> Self {
        let mut m: HashMap<(String, String), Vec<(i64, i64)>> = HashMap::new();
        for r in rows {
            if let Some(s) = r.session.as_deref().filter(|s| !s.is_empty()) {
                m.entry((r.agent.clone(), s.to_string())).or_default().push((r.t0.min(r.t), r.t));
            }
        }
        for v in m.values_mut() {
            v.sort_unstable();
        }
        GatewaySessions(m)
    }

    /// The record's conversation had a gateway request around its time.
    pub fn covers(&self, agent: &str, session: &str, t: i64) -> bool {
        let Some(v) = self.0.get(&(agent.to_string(), session.to_string())) else { return false };
        let end = v.partition_point(|&(s, _)| s - MATCH_SLACK <= t);
        v[..end].iter().rev().take_while(|&&(s, _)| s >= t - MAX_REQUEST - MATCH_SLACK).any(|&(_, e)| t <= e + MATCH_SLACK)
    }
}

/// Adds up log records and ledger rows in [from, to) into cells.
pub fn build(
    from: i64,
    to: i64,
    bucket: Bucket,
    logs: &[(String, &[Rec])],
    ledger: &[ledger::Row],
    prices: &price::Table,
) -> Report {
    let gw = GatewaySessions::new(ledger);
    let mut cells: BTreeMap<(i64, String, Source, String), Cell> = BTreeMap::new();
    let mut rep = Report { from, to, bucket: if bucket == Bucket::Hour { "hour" } else { "day" }.into(), ..Default::default() };
    let mut seen: HashSet<u64> = HashSet::new();
    let mut add = |t: i64, agent: &str, source: Source, r: &Rec| {
        let b = bucket.start(t);
        let c = cells.entry((b, agent.to_string(), source, r.model.clone())).or_insert_with(|| Cell {
            t: b,
            agent: agent.to_string(),
            source,
            model: r.model.clone(),
            ..Default::default()
        });
        c.requests += r.calls.max(1);
        c.input += r.input;
        c.output += r.output;
        c.cache_read += r.cache_read;
        c.cache_write += r.cache_write;
        match prices.cost(&r.model, r) {
            Some(x) => c.cost += x,
            None => c.unpriced += r.calls.max(1),
        }
    };
    for (agent, recs) in logs {
        for r in recs.iter() {
            if r.key != 0 && !seen.insert(r.key) {
                if (from..to).contains(&r.t) {
                    rep.repeated_requests += 1;
                }
                continue;
            }
            if !(from..to).contains(&r.t) {
                continue;
            }
            if gw.covers(agent, &r.session, r.t) || (!r.alt_session.is_empty() && gw.covers(agent, &r.alt_session, r.t)) {
                rep.deduped_requests += 1;
                rep.deduped_tokens += r.tokens();
                continue;
            }
            add(r.t, agent, if r.official { Source::Official } else { Source::ThirdParty }, r);
        }
    }
    for row in ledger {
        if (from..to).contains(&row.t) {
            add(row.t, &row.agent, Source::Gateway, &row.as_rec());
        }
    }
    rep.ledger_since = ledger.iter().map(|r| r.t).min();
    rep.cells = cells.into_values().collect();
    rep
}

/// The report for [from, to): the agents' logs (read again unless a scan at most `max_age`
/// seconds old exists) and the gateway ledger.
pub fn report(from: i64, to: i64, bucket: &str, max_age: u64) -> anyhow::Result<Report> {
    let bucket = Bucket::parse(bucket);
    let scanned = scan::scan_cached(std::time::Duration::from_secs(max_age));
    let ledger = ledger::read_all();
    let prices = price::Table::load();
    let logs: Vec<(String, &[Rec])> = scanned.agents.iter().map(|(a, recs)| (a.clone(), recs.as_slice())).collect();
    let mut rep = build(from, to, bucket, &logs, &ledger.rows, &prices);
    rep.sources = scanned.sources.clone();
    rep.errors = scanned.errors.clone();
    rep.scanned_at = scanned.at;
    rep.errors.extend(ledger.errors);
    if from >= to {
        rep.errors.push(l("The time range is empty", "时间范围为空").into());
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(t: i64, session: &str, model: &str, input: u64, key: u64) -> Rec {
        Rec { t, model: model.into(), session: session.into(), input, output: 10, official: true, key, ..Default::default() }
    }

    fn row(t0: i64, t: i64, agent: &str, session: Option<&str>) -> ledger::Row {
        ledger::Row { t0, t, agent: agent.into(), session: session.map(String::from), model: "glm-5".into(), input: 100, output: 5, ..Default::default() }
    }

    #[test]
    fn covers_matches_session_and_time() {
        let g = GatewaySessions::new(&[row(1000, 1300, "claude", Some("s1")), row(5000, 5010, "claude", Some("s1"))]);
        assert!(g.covers("claude", "s1", 1000));
        assert!(g.covers("claude", "s1", 1300 + MATCH_SLACK));
        assert!(g.covers("claude", "s1", 1000 - MATCH_SLACK));
        assert!(!g.covers("claude", "s1", 3000), "between two gateway requests");
        assert!(!g.covers("claude", "s2", 1000), "other conversation");
        assert!(!g.covers("codex", "s1", 1000), "other agent");
        assert!(g.covers("claude", "s1", 5005));
    }

    #[test]
    fn the_alternate_session_id_matches_too() {
        let prices = price::Table::builtin();
        let mut r = rec(1000, "thread-2", "gpt-5", 100, 1);
        r.alt_session = "root-1".into();
        let ledger = vec![row(990, 1005, "codex", Some("root-1"))];
        let rep = build(0, 10_000, Bucket::Day, &[("codex".into(), &[r])], &ledger, &prices);
        assert_eq!(rep.deduped_requests, 1);
    }

    #[test]
    fn covers_long_overlapping_requests() {
        // A long request that started first, then a short one inside it.
        let g = GatewaySessions::new(&[row(0, 2000, "codex", Some("s")), row(100, 110, "codex", Some("s"))]);
        assert!(g.covers("codex", "s", 1500));
        assert!(!g.covers("codex", "s", 2000 + MATCH_SLACK + 1));
    }

    #[test]
    fn gateway_rows_replace_their_log_records() {
        let prices = price::Table::builtin();
        let logs = vec![rec(1000, "s1", "glm-5", 100, 1), rec(9000, "s1", "claude-opus-5-5", 50, 2), rec(9100, "s2", "claude-opus-5-5", 50, 3)];
        let ledger = vec![row(990, 1005, "claude", Some("s1")), row(100, 120, "claude", None)];
        let r = build(0, 100_000, Bucket::Day, &[("claude".into(), &logs)], &ledger, &prices);
        assert_eq!((r.deduped_requests, r.deduped_tokens), (1, 110));
        let by = |s: Source| r.cells.iter().filter(|c| c.source == s).map(|c| c.requests).sum::<u64>();
        assert_eq!((by(Source::Official), by(Source::ThirdParty), by(Source::Gateway)), (2, 0, 2));
        assert_eq!(r.ledger_since, Some(120));
    }

    #[test]
    fn repeated_keys_count_once_and_range_is_half_open() {
        let prices = price::Table::builtin();
        let a = vec![rec(10, "s", "m", 1, 7), rec(20, "s", "m", 1, 0), rec(30, "s", "m", 1, 0)];
        let b = vec![rec(10, "s2", "m", 1, 7)];
        let r = build(10, 30, Bucket::Day, &[("codex".into(), &a), ("codex".into(), &b)], &[], &prices);
        assert_eq!(r.cells.iter().map(|c| c.requests).sum::<u64>(), 2);
        assert_eq!(r.repeated_requests, 1);
        assert_eq!(r.cells[0].unpriced, 2, "model m has no price");
    }

    #[test]
    fn third_party_and_cost() {
        let prices = price::Table::builtin();
        let mut x = rec(5, "s", "claude-opus-5-5", 1_000_000, 0);
        x.output = 1_000_000;
        let mut y = x.clone();
        y.official = false;
        let r = build(0, 10, Bucket::Hour, &[("claude".into(), &[x, y])], &[], &prices);
        assert_eq!(r.cells.len(), 2);
        assert!(r.cells.iter().all(|c| (c.cost - 24.0).abs() < 1e-9), "{:?}", r.cells);
        assert_eq!(r.bucket, "hour");
    }

    #[test]
    fn buckets_start_on_the_hour_and_local_midnight() {
        use chrono::{Local, TimeZone};
        let t = Local.with_ymd_and_hms(2026, 3, 4, 15, 42, 7).unwrap().timestamp();
        assert_eq!(Bucket::Hour.start(t), Local.with_ymd_and_hms(2026, 3, 4, 15, 0, 0).unwrap().timestamp());
        assert_eq!(Bucket::Day.start(t), Local.with_ymd_and_hms(2026, 3, 4, 0, 0, 0).unwrap().timestamp());
    }
}
