//! Reads token usage out of the agents' own session logs. Each file's records are cached
//! (in memory and in `~/.agentplus/usage/scan-cache.json`) by size and mtime, so only new
//! or grown files are parsed again.

use super::{Rec, SourceInfo};
use crate::adapters::{claude, codex, gemini, hermes, zcode};
use crate::util::home;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Codex,
    Claude,
    Gemini,
    OpenCode,
    ZCode,
    Hermes,
}

impl Kind {
    fn agent(self) -> &'static str {
        match self {
            Kind::Codex => codex::ID,
            Kind::Claude => claude::ID,
            Kind::Gemini => gemini::ID,
            Kind::OpenCode => crate::adapters::opencode::ID,
            Kind::ZCode => zcode::ID,
            Kind::Hermes => hermes::ID,
        }
    }
}

/// Bump when parsing changes, so cached records are read again.
const CACHE_VERSION: u32 = 2;
const MAX_ERRORS: usize = 5;

#[derive(Serialize, Deserialize, Clone)]
struct Cached {
    len: u64,
    mtime: u64,
    recs: Vec<Rec>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    /// Hash of a fixed value: record keys are only comparable under the same hasher.
    probe: u64,
    files: HashMap<String, Cached>,
}

static CACHE: Mutex<Option<CacheFile>> = Mutex::new(None);
static LAST_SAVE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

pub fn key_of(parts: &[&str]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish().max(1)
}

fn probe() -> u64 {
    key_of(&["agentplus-usage-probe"])
}

fn cache_path() -> PathBuf {
    super::ledger::dir().join("scan-cache.json")
}

fn load_cache() -> CacheFile {
    let fresh = CacheFile { version: CACHE_VERSION, probe: probe(), files: HashMap::new() };
    let Ok(text) = fs::read(cache_path()) else { return fresh };
    match serde_json::from_slice::<CacheFile>(&text) {
        Ok(c) if c.version == CACHE_VERSION && c.probe == fresh.probe => c,
        _ => fresh,
    }
}

fn save_cache(c: &CacheFile) {
    let p = cache_path();
    let r = (|| -> anyhow::Result<()> {
        fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(c)?)?;
        fs::rename(&tmp, &p)?;
        Ok(())
    })();
    if let Err(e) = r {
        crate::applog::warn("usage", format!("scan cache write failed: {e:#}"));
    }
}

pub struct Scanned {
    /// Records per agent, in file order.
    pub agents: Vec<(String, Vec<Rec>)>,
    pub sources: Vec<SourceInfo>,
    pub errors: Vec<String>,
}

/// Files to read for each kind of log, sorted (Codex and Gemini names start with the date).
fn files(kind: Kind) -> (PathBuf, Vec<PathBuf>) {
    let mut out = vec![];
    let root = match kind {
        Kind::Codex => {
            let h = codex::codex_home();
            walk(&h.join("sessions"), &mut out, &|p| ext_is(p, "jsonl"));
            walk(&h.join("archived_sessions"), &mut out, &|p| ext_is(p, "jsonl"));
            h.join("sessions")
        }
        Kind::Claude => {
            let mut roots = vec![claude::dir().join("projects")];
            let alt = home().join(".config").join("claude").join("projects");
            if !roots.contains(&alt) {
                roots.push(alt);
            }
            for r in &roots {
                walk(r, &mut out, &|p| ext_is(p, "jsonl"));
            }
            roots.swap_remove(0)
        }
        Kind::Gemini => {
            let tmp = gemini::dir().join("tmp");
            walk(&tmp, &mut out, &|p| ext_is(p, "json") && p.parent().and_then(Path::file_name).is_some_and(|n| n == "chats"));
            tmp
        }
        Kind::OpenCode => one_file(&mut out, home().join(".local").join("share").join("opencode").join("opencode.db")),
        // The desktop app's folder is ~/.zcode/v2; its agent keeps usage in ~/.zcode/cli.
        Kind::ZCode => {
            let d = zcode::dir();
            one_file(&mut out, d.parent().unwrap_or(&d).join("cli").join("db").join("db.sqlite"))
        }
        Kind::Hermes => one_file(&mut out, hermes::dir().join("state.db")),
    };
    out.sort();
    (root, out)
}

fn one_file(out: &mut Vec<PathBuf>, p: PathBuf) -> PathBuf {
    if p.is_file() {
        out.push(p.clone());
    }
    p
}

fn ext_is(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(&p, out, keep);
        } else if ft.is_file() && keep(&p) {
            out.push(p);
        }
    }
}

/// Size and mtime (µs) of a file; for OpenCode's database, of the database plus its WAL.
fn stat(kind: Kind, p: &Path) -> Option<(u64, u64)> {
    let one = |p: &Path| -> Option<(u64, u64)> {
        let m = fs::metadata(p).ok()?;
        Some((m.len(), m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_micros() as u64))
    };
    let (mut len, mut mtime) = one(p)?;
    if matches!(kind, Kind::OpenCode | Kind::ZCode | Kind::Hermes) {
        if let Some((l, t)) = one(&PathBuf::from(format!("{}-wal", p.display()))) {
            len += l;
            mtime = mtime.max(t);
        }
    }
    Some((len, mtime))
}

fn parse(kind: Kind, p: &Path) -> anyhow::Result<Vec<Rec>> {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    match kind {
        Kind::Codex => Ok(codex_lines(lines(p)?, codex_file_session(stem))),
        Kind::Claude => Ok(claude_lines(lines(p)?, stem)),
        Kind::Gemini => Ok(gemini_chat(&fs::read_to_string(p)?)),
        Kind::OpenCode => opencode_db(p),
        Kind::ZCode => zcode_db(p),
        Kind::Hermes => hermes_db(p),
    }
}

/// Lines of a file, skipping any that are not UTF-8.
fn lines(p: &Path) -> anyhow::Result<impl Iterator<Item = String>> {
    let f = fs::File::open(p)?;
    Ok(BufReader::with_capacity(1 << 16, f).split(b'\n').map_while(Result::ok).filter_map(|b| String::from_utf8(b).ok()))
}

/// The thread id in `rollout-<time>-<id>[_<segment>]`.
fn codex_file_session(stem: &str) -> String {
    let s = stem.split('_').next().unwrap_or(stem);
    // The id is the last 5 groups of the UUID.
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() >= 5 { parts[parts.len() - 5..].join("-") } else { s.to_string() }
}

fn ts(s: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s?).ok().map(|d| d.timestamp())
}

// ---------------------------------------------------------------- Codex

#[derive(Deserialize)]
struct CxLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: CxPayload,
}

#[derive(Deserialize, Default)]
struct CxPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    /// The root session of a subagent thread (`id` is the thread's own).
    session_id: Option<String>,
    model_provider: Option<String>,
    model: Option<String>,
    info: Option<CxInfo>,
}

#[derive(Deserialize)]
struct CxInfo {
    total_token_usage: Option<CxUsage>,
    last_token_usage: Option<CxUsage>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(default)]
struct CxUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

impl CxUsage {
    fn minus(self, o: CxUsage) -> CxUsage {
        CxUsage {
            input_tokens: self.input_tokens.saturating_sub(o.input_tokens),
            cached_input_tokens: self.cached_input_tokens.saturating_sub(o.cached_input_tokens),
            cache_write_input_tokens: self.cache_write_input_tokens.saturating_sub(o.cache_write_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(o.output_tokens),
            total_tokens: self.total_tokens.saturating_sub(o.total_tokens),
        }
    }
}

/// Codex writes a `token_count` event after each model call: the running total and the
/// call's own usage (`last_token_usage`). An event repeating the previous total is a
/// re-report, not a call. Input counts include cached tokens; output includes reasoning.
fn codex_lines(lines: impl Iterator<Item = String>, file_session: String) -> Vec<Rec> {
    let mut out = vec![];
    let (mut session, mut provider, mut model) = (None::<String>, None::<String>, None::<String>);
    let mut root = String::new();
    let mut prev = CxUsage::default();
    for line in lines {
        if !(line.contains("\"token_count\"") || line.contains("\"session_meta\"") || line.contains("\"turn_context\"")) {
            continue;
        }
        let Ok(l) = serde_json::from_str::<CxLine>(&line) else { continue };
        match l.kind.as_str() {
            "session_meta" => {
                // A fork copies its parent's items after its own meta: keep the first.
                if session.is_none() {
                    root = l.payload.session_id.filter(|r| Some(r) != l.payload.id.as_ref()).unwrap_or_default();
                    session = l.payload.id;
                    provider = l.payload.model_provider;
                }
            }
            "turn_context" => {
                if let Some(m) = l.payload.model.filter(|m| !m.is_empty()) {
                    model = Some(m);
                }
            }
            "event_msg" if l.payload.kind.as_deref() == Some("token_count") => {
                let Some(info) = l.payload.info else { continue };
                let Some(total) = info.total_token_usage else { continue };
                if total == prev {
                    continue;
                }
                // A running total that drops was reset (the call itself still counts).
                let reset = total.total_tokens < prev.total_tokens;
                let last = info.last_token_usage.unwrap_or_else(|| if reset { total } else { total.minus(prev) });
                prev = total;
                let Some(t) = ts(l.timestamp.as_deref()) else { continue };
                let cached = last.cached_input_tokens.min(last.input_tokens);
                let write = last.cache_write_input_tokens.min(last.input_tokens - cached);
                let stamp = l.timestamp.unwrap_or_default();
                out.push(Rec {
                    t,
                    model: model.clone().unwrap_or_else(|| "unknown".into()),
                    session: session.clone().unwrap_or_else(|| file_session.clone()),
                    alt_session: root.clone(),
                    input: last.input_tokens - cached - write,
                    output: last.output_tokens,
                    cache_read: cached,
                    cache_write: write,
                    official: provider.as_deref().unwrap_or("openai") == "openai",
                    key: key_of(&[&stamp, &total.total_tokens.to_string(), &last.total_tokens.to_string()]),
            calls: 1,
                });
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- Claude Code

#[derive(Deserialize)]
struct ClLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClMsg>,
}

#[derive(Deserialize)]
struct ClMsg {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
}

/// Claude Code writes one line per content block of a reply, each carrying the reply's
/// usage: lines with the same message and request id are one call (the largest counts win).
fn claude_lines(lines: impl Iterator<Item = String>, file_session: &str) -> Vec<Rec> {
    let mut out: Vec<Rec> = vec![];
    let mut at: HashMap<u64, usize> = HashMap::new();
    for line in lines {
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(l) = serde_json::from_str::<ClLine>(&line) else { continue };
        if l.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(m) = l.message else { continue };
        let Some(u) = m.usage else { continue };
        let model = m.model.unwrap_or_default();
        if model.is_empty() || model == "<synthetic>" {
            continue;
        }
        if u.input_tokens + u.output_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens == 0 {
            continue;
        }
        let Some(t) = ts(l.timestamp.as_deref()) else { continue };
        let key = match (&m.id, &l.request_id) {
            (Some(id), req) => key_of(&[id, req.as_deref().unwrap_or("")]),
            (None, _) => 0,
        };
        let rec = Rec {
            t,
            official: model.starts_with("claude-"),
            model,
            session: l.session_id.unwrap_or_else(|| file_session.to_string()),
            alt_session: String::new(),
            input: u.input_tokens,
            output: u.output_tokens,
            cache_read: u.cache_read_input_tokens,
            cache_write: u.cache_creation_input_tokens,
            key,
            calls: 1,
        };
        if key != 0 {
            if let Some(&i) = at.get(&key) {
                let r = &mut out[i];
                r.input = r.input.max(rec.input);
                r.output = r.output.max(rec.output);
                r.cache_read = r.cache_read.max(rec.cache_read);
                r.cache_write = r.cache_write.max(rec.cache_write);
                continue;
            }
            at.insert(key, out.len());
        }
        out.push(rec);
    }
    out
}

// ---------------------------------------------------------------- Gemini CLI

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GmChat {
    session_id: Option<String>,
    #[serde(default)]
    messages: Vec<GmMsg>,
}

#[derive(Deserialize)]
struct GmMsg {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    tokens: Option<GmTokens>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GmTokens {
    input: u64,
    output: u64,
    cached: u64,
    thoughts: u64,
    tool: u64,
}

/// Gemini CLI keeps a chat per file; `input` includes the cached tokens.
fn gemini_chat(text: &str) -> Vec<Rec> {
    let Ok(c) = serde_json::from_str::<GmChat>(text) else { return vec![] };
    let session = c.session_id.unwrap_or_default();
    c.messages
        .into_iter()
        .filter(|m| m.kind.as_deref() == Some("gemini"))
        .filter_map(|m| {
            let tk = m.tokens?;
            let t = ts(m.timestamp.as_deref())?;
            let model = m.model.unwrap_or_else(|| "unknown".into());
            let cached = tk.cached.min(tk.input);
            let rec = Rec {
                t,
                official: model.starts_with("gemini-"),
                model,
                session: session.clone(),
                alt_session: String::new(),
                input: tk.input - cached + tk.tool,
                output: tk.output + tk.thoughts,
                cache_read: cached,
                cache_write: 0,
                key: m.id.as_deref().map_or(0, |id| key_of(&[&session, id])),
            calls: 1,
            };
            (rec.tokens() > 0).then_some(rec)
        })
        .collect()
}

// ---------------------------------------------------------------- OpenCode

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OcMsg {
    role: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    time: Option<OcTime>,
    tokens: Option<OcTokens>,
}

#[derive(Deserialize)]
struct OcTime {
    created: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OcTokens {
    input: u64,
    output: u64,
    reasoning: u64,
    cache: OcCache,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OcCache {
    read: u64,
    write: u64,
}

/// Providers that are the model vendor's own service (subscription or its API).
const OFFICIAL: &[&str] = &[
    "anthropic", "openai", "google", "gemini", "github-copilot", "deepseek", "zhipuai", "zai", "zhipu", "bigmodel", "moonshot",
    "moonshotai", "kimi", "xai", "mistral", "dashscope", "qwen", "alibaba", "minimax", "xiaomi", "mimo", "openai-codex",
];

fn is_official(provider: &str) -> bool {
    let p = provider.trim().to_ascii_lowercase();
    OFFICIAL.contains(&p.as_str())
}

/// One assistant message of OpenCode (its `message.data` JSON); `input` excludes the cache.
fn opencode_msg(id: &str, session: &str, data: &str) -> Option<Rec> {
    let m: OcMsg = serde_json::from_str(data).ok()?;
    if m.role.as_deref() != Some("assistant") {
        return None;
    }
    let tk = m.tokens?;
    let t = m.time?.created? / 1000;
    let rec = Rec {
        t,
        model: m.model_id.unwrap_or_else(|| "unknown".into()),
        session: session.to_string(),
        alt_session: String::new(),
        input: tk.input,
        output: tk.output + tk.reasoning,
        cache_read: tk.cache.read,
        cache_write: tk.cache.write,
        official: m.provider_id.as_deref().is_some_and(is_official),
        key: key_of(&["opencode", id]),
            calls: 1,
    };
    (rec.tokens() > 0).then_some(rec)
}

/// Whether SQLite can't lock `p`: it is on the WSL file share (the WSL environment, or a
/// folder picked there by hand).
fn on_wsl_share(p: &Path) -> bool {
    let s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    s.starts_with(r"\\wsl.localhost\") || s.starts_with(r"\\wsl$\")
}

fn open_db(p: &Path) -> anyhow::Result<rusqlite::Connection> {
    use rusqlite::{Connection, OpenFlags};
    // Over the WSL share every open fails with "database is locked": read a copy instead.
    if crate::env::is_wsl() || on_wsl_share(p) {
        return crate::sessions::open_snapshot(p);
    }
    let c = Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(c)
}

fn opencode_db(p: &Path) -> anyhow::Result<Vec<Rec>> {
    let c = open_db(p)?;
    let mut st = c.prepare("SELECT id, session_id, data FROM message WHERE data LIKE '%\"tokens\"%' ORDER BY time_created")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    Ok(rows.filter_map(Result::ok).filter_map(|(id, s, d)| opencode_msg(&id, &s, &d)).collect())
}

// ---------------------------------------------------------------- ZCode

/// One row of ZCode's `model_usage` (one model call): input includes the cache, output
/// includes reasoning (`provider_total_tokens` = input + output).
#[allow(clippy::too_many_arguments)]
fn zcode_row(id: &str, session: &str, provider: &str, model: &str, at_ms: i64, input: i64, output: i64, cache_read: i64, cache_write: i64) -> Option<Rec> {
    let u = |x: i64| x.max(0) as u64;
    let (input, cache_read) = (u(input), u(cache_read));
    let cache_read = cache_read.min(input);
    let cache_write = u(cache_write).min(input - cache_read);
    let rec = Rec {
        t: at_ms / 1000,
        model: model.to_string(),
        session: session.to_string(),
        alt_session: String::new(),
        input: input - cache_read - cache_write,
        output: u(output),
        cache_read,
        cache_write,
        official: is_official(provider),
        key: key_of(&["zcode", id]),
        calls: 1,
    };
    (rec.tokens() > 0).then_some(rec)
}

fn zcode_db(p: &Path) -> anyhow::Result<Vec<Rec>> {
    let c = open_db(p)?;
    let mut st = c.prepare(
        "SELECT id, session_id, provider_id, model_id, COALESCE(completed_at, started_at), input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens \
         FROM model_usage ORDER BY started_at",
    )?;
    let rows = st.query_map([], |r| {
        Ok(zcode_row(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?, &r.get::<_, String>(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
    })?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

// ---------------------------------------------------------------- Hermes

/// Hermes keeps totals per session, model and billing route (`session_model_usage`), not
/// per call: each row is one record at the time it was last used. Input excludes the cache.
#[allow(clippy::too_many_arguments)]
fn hermes_row(session: &str, model: &str, provider: &str, task: &str, last_seen: f64, calls: i64, input: i64, output: i64, cache_read: i64, cache_write: i64) -> Option<Rec> {
    let u = |x: i64| x.max(0) as u64;
    let rec = Rec {
        t: last_seen as i64,
        model: model.to_string(),
        session: session.to_string(),
        alt_session: String::new(),
        input: u(input),
        output: u(output),
        cache_read: u(cache_read),
        cache_write: u(cache_write),
        official: is_official(provider),
        key: key_of(&["hermes", session, model, provider, task]),
        calls: u(calls).max(1),
    };
    (rec.tokens() > 0 && last_seen > 0.0).then_some(rec)
}

fn hermes_db(p: &Path) -> anyhow::Result<Vec<Rec>> {
    let c = open_db(p)?;
    let mut st = c.prepare(
        "SELECT u.session_id, u.model, u.billing_provider, u.task, COALESCE(u.last_seen, u.first_seen, s.started_at, 0), u.api_call_count, \
         u.input_tokens, u.output_tokens, u.cache_read_tokens, u.cache_write_tokens \
         FROM session_model_usage u LEFT JOIN sessions s ON s.id = u.session_id ORDER BY 5",
    )?;
    let rows = st.query_map([], |r| {
        Ok(hermes_row(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?, &r.get::<_, String>(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))
    })?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

// ---------------------------------------------------------------- scanning

static SCAN: Mutex<()> = Mutex::new(());

/// Reads every agent's logs, parsing only files that changed since the last scan.
pub fn scan_all() -> Scanned {
    let _one = crate::util::lock(&SCAN);
    let mut guard = crate::util::lock(&CACHE);
    let cache = guard.get_or_insert_with(load_cache);
    let mut out = Scanned { agents: vec![], sources: vec![], errors: vec![] };
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut changed = false;
    for kind in [Kind::Codex, Kind::Claude, Kind::Gemini, Kind::OpenCode, Kind::ZCode, Kind::Hermes] {
        let (root, list) = files(kind);
        let mut todo: Vec<Job> = vec![];
        for p in &list {
            let k = p.to_string_lossy().to_string();
            seen.insert(k.clone());
            let Some(st) = stat(kind, p) else { continue };
            if cache.files.get(&k).is_none_or(|c| (c.len, c.mtime) != st) {
                todo.push((k, p.clone(), st));
            }
        }
        changed |= !todo.is_empty();
        for (k, res, (len, mtime)) in parse_many(kind, todo) {
            match res {
                Ok(recs) => {
                    cache.files.insert(k, Cached { len, mtime, recs });
                }
                Err(e) => {
                    if out.errors.len() < MAX_ERRORS {
                        out.errors.push(tr!("Can't read {k}: {e:#}", "读取 {k} 失败：{e:#}"));
                    }
                }
            }
        }
        if list.is_empty() {
            continue;
        }
        let recs: Vec<Rec> = list.iter().filter_map(|p| cache.files.get(&*p.to_string_lossy())).flat_map(|c| c.recs.iter().cloned()).collect();
        out.sources.push(SourceInfo { agent: kind.agent().into(), files: list.len(), path: root.display().to_string() });
        out.agents.push((kind.agent().into(), recs));
    }
    let before = cache.files.len();
    cache.files.retain(|k, _| seen.contains(k));
    // Agents append to their logs all the time: write the (multi-MB) cache at most once a minute
    // for that; a removed file or the first scan saves at once.
    let due = LAST_SAVE.lock().map(|t| t.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60))).unwrap_or(true);
    if (changed && due) || cache.files.len() != before {
        *crate::util::lock(&LAST_SAVE) = Some(std::time::Instant::now());
        save_cache(cache);
    }
    out
}

/// A file to parse: cache key, path, (size, mtime).
type Job = (String, PathBuf, (u64, u64));
type Parsed = (String, anyhow::Result<Vec<Rec>>, (u64, u64));

/// Parses files on a few threads (the first scan can be gigabytes of logs).
fn parse_many(kind: Kind, todo: Vec<Job>) -> Vec<Parsed> {
    let n = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8).min(todo.len().max(1));
    let chunks: Vec<Vec<Job>> = {
        let mut c: Vec<Vec<_>> = (0..n).map(|_| vec![]).collect();
        for (i, t) in todo.into_iter().enumerate() {
            c[i % n].push(t);
        }
        c
    };
    std::thread::scope(|s| {
        let hs: Vec<_> = chunks.into_iter().map(|chunk| s.spawn(move || chunk.into_iter().map(|(k, p, st)| (k, parse(kind, &p), st)).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(s: &[&str]) -> impl Iterator<Item = String> {
        s.iter().map(|x| x.to_string()).collect::<Vec<_>>().into_iter()
    }

    const META: &str = r#"{"timestamp":"2026-11-10T14:06:01.305Z","type":"session_meta","payload":{"id":"01a1c5c7-861e-78b0-bd26-5a127b4adfa0","model_provider":"openai","base_instructions":{"text":"..."}}}"#;
    const TURN: &str = r#"{"timestamp":"2026-11-10T14:06:02Z","type":"turn_context","payload":{"model":"gpt-5.1-codex","cwd":"x"}}"#;

    fn tc(stamp: &str, total: (u64, u64, u64), last: (u64, u64, u64)) -> String {
        let u = |(i, c, o): (u64, u64, u64)| format!(r#"{{"input_tokens":{i},"cached_input_tokens":{c},"output_tokens":{o},"reasoning_output_tokens":0,"total_tokens":{}}}"#, i + o);
        format!(r#"{{"timestamp":"{stamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{},"model_context_window":258400}},"rate_limits":{{}}}}}}"#, u(total), u(last))
    }

    #[test]
    fn codex_counts_each_call_once() {
        let a = tc("2026-11-10T14:06:08Z", (100, 60, 10), (100, 60, 10));
        let again = tc("2026-11-10T14:06:09Z", (100, 60, 10), (100, 60, 10));
        let b = tc("2026-11-10T14:06:20Z", (250, 160, 30), (150, 100, 20));
        let null_info = r#"{"timestamp":"2026-11-10T14:06:21Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#;
        let r = codex_lines(it(&[META, TURN, &a, &again, null_info, &b]), "f".into());
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].input, r[0].cache_read, r[0].output), (40, 60, 10));
        assert_eq!((r[1].input, r[1].cache_read, r[1].output), (50, 100, 20));
        assert_eq!(r[0].model, "gpt-5.1-codex");
        assert_eq!(r[0].session, "01a1c5c7-861e-78b0-bd26-5a127b4adfa0");
        assert!(r[0].official);
        assert_ne!(r[0].key, r[1].key);
    }

    #[test]
    fn codex_without_last_usage_uses_the_difference_and_third_party_provider() {
        let meta = META.replace("\"openai\"", "\"agentplus\"");
        let a = tc("2026-11-10T14:06:08Z", (100, 0, 10), (100, 0, 10));
        let b = r#"{"timestamp":"2026-11-10T14:06:30Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":50,"output_tokens":40,"total_tokens":340}}}}"#;
        let r = codex_lines(it(&[&meta, &a, b]), "f".into());
        assert_eq!((r[1].input, r[1].cache_read, r[1].output), (150, 50, 30));
        assert!(!r[1].official);
        assert_eq!(r[1].model, "unknown");
    }

    #[test]
    fn codex_fork_copies_share_keys() {
        let call = tc("2026-11-10T14:06:08Z", (100, 0, 10), (100, 0, 10));
        let parent = codex_lines(it(&[META, &call]), "p".into());
        let fork_meta = META.replace("01a1c5c7-861e-78b0-bd26-5a127b4adfa0", "aaaaaaaa-0000-0000-0000-000000000000");
        let fork = codex_lines(it(&[&fork_meta, META, &call]), "c".into());
        assert_eq!(parent[0].key, fork[0].key);
        assert_eq!(fork[0].session, "aaaaaaaa-0000-0000-0000-000000000000", "first meta wins");
    }

    #[test]
    fn codex_keeps_the_root_session_and_counts_the_call_after_a_reset() {
        let meta = META.replace(r#""id":"01a1c5c7-861e-78b0-bd26-5a127b4adfa0""#, r#""id":"thread-2","session_id":"root-1""#);
        let a = tc("2026-11-10T14:06:08Z", (500, 0, 50), (500, 0, 50));
        let reset = tc("2026-11-10T14:07:00Z", (100, 0, 10), (100, 0, 10));
        let r = codex_lines(it(&[&meta, &a, &reset]), "f".into());
        assert_eq!(r.len(), 2, "the call after the reset counts");
        assert_eq!((r[1].input, r[1].output), (100, 10));
        assert_eq!((r[0].session.as_str(), r[0].alt_session.as_str()), ("thread-2", "root-1"));
        let plain = codex_lines(it(&[META, &a]), "f".into());
        assert!(plain[0].alt_session.is_empty());
    }

    #[test]
    fn codex_session_from_file_name() {
        assert_eq!(codex_file_session("rollout-2026-11-10T22-06-01-01a1c5c7-861e-78b0-bd26-5a127b4adfa0_01a1c5c7-b258-79d1-aa13-8b9e4751afaa"), "01a1c5c7-861e-78b0-bd26-5a127b4adfa0");
    }

    fn cl(id: &str, req: &str, model: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-11-11T01:00:00Z","sessionId":"s1","requestId":"{req}","message":{{"model":"{model}","id":"{id}","role":"assistant","content":[{{"type":"text","text":"usage"}}],"usage":{{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000,"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn claude_merges_content_block_lines() {
        let user = r#"{"type":"user","message":{"role":"user","content":"what is \"usage\" of assistant"},"timestamp":"2026-11-11T01:00:00Z"}"#;
        let synthetic = cl("m0", "r0", "<synthetic>", 0);
        let lines = [user.to_string(), cl("m1", "r1", "claude-opus-5-5", 5), cl("m1", "r1", "claude-opus-5-5", 458), cl("m2", "r2", "glm-5", 9), synthetic];
        let r = claude_lines(lines.into_iter(), "file");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].input, r[0].cache_write, r[0].cache_read, r[0].output), (2, 300, 4000, 458));
        assert!(r[0].official && !r[1].official);
        assert_eq!(r[0].session, "s1");
    }

    #[test]
    fn gemini_chat_tokens() {
        let text = r#"{"sessionId":"g1","messages":[
            {"id":"u","timestamp":"2026-05-22T12:12:06Z","type":"user","content":[]},
            {"id":"a","timestamp":"2026-05-22T12:12:09Z","type":"gemini","model":"gemini-2.5-pro","tokens":{"input":1000,"output":50,"cached":800,"thoughts":20,"tool":5,"total":1075}},
            {"id":"b","timestamp":"2026-05-22T12:12:10Z","type":"gemini","content":"no tokens"}]}"#;
        let r = gemini_chat(text);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].input, r[0].cache_read, r[0].output, r[0].session.as_str()), (205, 800, 70, "g1"));
        assert!(r[0].official);
        assert!(gemini_chat("not json").is_empty());
    }

    #[test]
    fn opencode_assistant_messages() {
        let d = r#"{"role":"assistant","cost":0,"tokens":{"total":76432,"input":891,"output":483,"reasoning":370,"cache":{"write":0,"read":74688}},"modelID":"mimo-v2.6-flash-free","providerID":"opencode","time":{"created":1792178912239}}"#;
        let r = opencode_msg("msg_1", "ses_1", d).unwrap();
        assert_eq!((r.t, r.input, r.output, r.cache_read), (1792178912, 891, 853, 74688));
        assert!(!r.official);
        let a = d.replace("\"opencode\"", "\"anthropic\"");
        assert!(opencode_msg("msg_1", "ses_1", &a).unwrap().official);
        assert!(opencode_msg("m", "s", r#"{"role":"user","time":{"created":1}}"#).is_none());
    }

    #[test]
    fn wsl_share_paths_are_read_from_a_copy() {
        assert!(on_wsl_share(Path::new(r"\\wsl.localhost\Ubuntu\home\me\.zcode\cli\db\db.sqlite")));
        assert!(on_wsl_share(Path::new(r"\\WSL$\Ubuntu\home\me\x.db")));
        assert!(on_wsl_share(Path::new("//wsl.localhost/Ubuntu/home/me/x.db")));
        assert!(!on_wsl_share(Path::new(r"C:\Users\me\.zcode\cli\db\db.sqlite")));
        assert!(!on_wsl_share(Path::new(r"\\nas\share\x.db")));
    }

    /// A database opened through a snapshot reads the same rows, the WAL's included.
    #[test]
    fn snapshot_reads_what_the_database_holds() {
        let h = crate::util::TestHome::new("usage-snapshot");
        let db = h.0.join("db.sqlite");
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);").unwrap();
        c.execute(
            "INSERT INTO message VALUES ('m1', 's1', 1, ?1)",
            [r#"{"role":"assistant","modelID":"glm-5","providerID":"x","time":{"created":5000},"tokens":{"input":7,"output":3,"reasoning":0,"cache":{"read":0,"write":0}}}"#],
        )
        .unwrap();
        // The writer is still open (the row may sit in the WAL) while the snapshot is taken.
        let snap = crate::sessions::open_snapshot(&db).unwrap();
        let (id, s, d): (String, String, String) = snap.query_row("SELECT id, session_id, data FROM message", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        let rec = opencode_msg(&id, &s, &d).unwrap();
        assert_eq!((rec.input, rec.output, rec.t), (7, 3, 5));
    }

    #[test]
    fn zcode_rows_split_the_cache_out_of_input() {
        let r = zcode_row("u1", "session_1", "deepseek", "deepseek-flash", 1_793_793_013_800, 67757, 337, 67584, 0).unwrap();
        assert_eq!((r.t, r.input, r.cache_read, r.output, r.calls), (1_793_793_013, 173, 67584, 337, 1));
        assert!(r.official);
        assert!(!zcode_row("u2", "s", "my-relay", "glm-5", 1000, 10, 1, 0, 0).unwrap().official);
        assert!(zcode_row("u3", "s", "x", "m", 1000, 0, 0, 0, 0).is_none(), "a failed call without tokens");
        let odd = zcode_row("u4", "s", "x", "m", 1000, 10, 1, 50, 50).unwrap();
        assert_eq!((odd.input, odd.cache_read, odd.cache_write), (0, 10, 0), "cache counts beyond the input are capped");
    }

    #[test]
    fn hermes_rows_are_session_totals() {
        let r = hermes_row("s1", "claude-opus-5-5", "anthropic", "", 1_800_000_000.5, 12, 100, 50, 900, 10).unwrap();
        assert_eq!((r.t, r.calls, r.input, r.cache_read, r.cache_write), (1_800_000_000, 12, 100, 900, 10));
        assert!(r.official);
        assert_ne!(r.key, hermes_row("s1", "claude-opus-5-5", "anthropic", "compress", 1.0, 1, 1, 1, 0, 0).unwrap().key);
        assert!(hermes_row("s1", "m", "custom", "", 0.0, 1, 5, 5, 0, 0).is_none(), "no time");
    }

    /// Scans the real logs of this machine: `cargo test real_usage -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_usage() {
        let t = std::time::Instant::now();
        let s = scan_all();
        for (a, r) in &s.agents {
            println!("{a}: {} records, {} tokens", r.len(), r.iter().map(Rec::tokens).sum::<u64>());
        }
        println!("{:?} errors={:?} in {:?}", s.sources, s.errors, t.elapsed());
        let t = std::time::Instant::now();
        let _ = scan_all();
        println!("cached rescan {:?}", t.elapsed());
    }
}
