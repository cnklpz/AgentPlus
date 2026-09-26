//! AgentPlus's own diagnostic log, for finding out what went wrong on a user's machine:
//! `~/.agentplus/logs/agentplus-YYYY-MM-DD.log`, one file a day, one line an entry.
//!
//! - Everything is scrubbed before it is written ([`scrub`]): API keys and tokens, hosts
//!   other than this machine and the well-known vendors, IP addresses, e-mail addresses and
//!   the account name in paths. An export is scrubbed again, so it can be sent as it is.
//! - Files older than the kept number of days are removed at start and each new day, and
//!   the folder never grows past [`MAX_TOTAL`].
//! - Lines are for developers: English, not translated (only the settings UI is).
//! - Settings live in `store.json` → `appLog` (`enabled`, `days`); on unless turned off.

use crate::util::agentplus_dir;
use chrono::{Local, NaiveDate};
use regex::{Captures, Regex};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// Days kept when nothing is set.
pub const DEFAULT_DAYS: u32 = 7;
/// The whole folder stays under this; the oldest files go first.
const MAX_TOTAL: u64 = 50 * 1024 * 1024;
/// One day's file stops growing here (a loop that fails every second can't fill the disk).
const MAX_FILE: u64 = 10 * 1024 * 1024;
/// A single entry is cut here.
const MAX_LINE: usize = 4000;
const PREFIX: &str = "agentplus-";

static ENABLED: AtomicBool = AtomicBool::new(true);
static DAYS: AtomicU32 = AtomicU32::new(DEFAULT_DAYS);
/// The day the last entry was written: a new day cleans up first.
static WRITE: Mutex<Option<NaiveDate>> = Mutex::new(None);

pub fn dir() -> PathBuf {
    agentplus_dir().join("logs")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogInfo {
    pub enabled: bool,
    pub days: u32,
    pub files: usize,
    pub bytes: u64,
    pub dir: String,
}

/// Reads the settings and removes expired files; called once at start.
pub fn init() {
    let root = crate::store::load();
    let s = root.get("appLog");
    ENABLED.store(s.and_then(|s| s.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true), Ordering::Relaxed);
    DAYS.store(s.and_then(|s| s.get("days")).and_then(|v| v.as_u64()).map(|d| clamp_days(d as u32)).unwrap_or(DEFAULT_DAYS), Ordering::Relaxed);
    if ENABLED.load(Ordering::Relaxed) {
        cleanup(&dir(), DAYS.load(Ordering::Relaxed), Local::now().date_naive());
    }
}

fn clamp_days(d: u32) -> u32 {
    d.clamp(1, 90)
}

pub fn status() -> LogInfo {
    let d = dir();
    let files = log_files(&d);
    LogInfo {
        enabled: ENABLED.load(Ordering::Relaxed),
        days: DAYS.load(Ordering::Relaxed),
        files: files.len(),
        bytes: files.iter().map(|(_, _, n)| n).sum(),
        dir: crate::util::display_path(&d),
    }
}

pub fn set(enabled: bool, days: u32) -> anyhow::Result<LogInfo> {
    let days = clamp_days(days);
    crate::store::update(|root| {
        root["appLog"] = json!({ "enabled": enabled, "days": days });
        Ok(())
    })?;
    let was = ENABLED.swap(enabled, Ordering::Relaxed);
    DAYS.store(days, Ordering::Relaxed);
    if enabled && !was {
        info("log", format!("logging turned on, keeping {days} day(s)"));
    }
    cleanup(&dir(), days, Local::now().date_naive());
    Ok(status())
}

/// Removes every log file.
pub fn clear() -> anyhow::Result<LogInfo> {
    let _g = crate::util::lock(&WRITE);
    for (p, _, _) in log_files(&dir()) {
        fs::remove_file(&p)?;
    }
    drop(_g);
    Ok(status())
}

pub fn error(source: &str, msg: impl AsRef<str>) {
    write("ERROR", source, msg.as_ref());
}

pub fn warn(source: &str, msg: impl AsRef<str>) {
    write("WARN", source, msg.as_ref());
}

pub fn info(source: &str, msg: impl AsRef<str>) {
    write("INFO", source, msg.as_ref());
}

/// An entry from the page ("error" | "warn" | anything else = info).
pub fn client(level: &str, source: &str, msg: &str) {
    let level = match level {
        "error" => "ERROR",
        "warn" => "WARN",
        _ => "INFO",
    };
    let source: String = source.chars().filter(|c| c.is_ascii_alphanumeric() || "_-.:".contains(*c)).take(40).collect();
    write(level, &format!("ui:{source}"), msg);
}

fn write(level: &str, source: &str, msg: &str) {
    // Tests never write into the real log.
    if cfg!(test) || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let now = Local::now();
    let line = format_line(&now.format("%Y-%m-%d %H:%M:%S%.3f").to_string(), level, source, msg);
    let mut last = crate::util::lock(&WRITE);
    let today = now.date_naive();
    let d = dir();
    if *last != Some(today) {
        let _ = fs::create_dir_all(&d);
        cleanup(&d, DAYS.load(Ordering::Relaxed), today);
        *last = Some(today);
    }
    append(&d.join(file_name(today)), &line);
}

fn format_line(time: &str, level: &str, source: &str, msg: &str) -> String {
    let msg = scrub(msg.trim());
    // One entry, one line: a multi-line error chain is kept readable with " ⏎ ".
    let msg = msg.replace("\r\n", "\n").split('\n').map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ⏎ ");
    format!("{time} {level:<5} [{source}] {}\n", crate::util::clip(&msg, MAX_LINE))
}

fn append(path: &Path, line: &str) {
    let len = crate::util::file_len(path);
    if len >= MAX_FILE {
        return;
    }
    let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) else { return };
    let _ = if len + line.len() as u64 >= MAX_FILE {
        f.write_all(b"-- this day's log is full; later entries are dropped --\n")
    } else {
        f.write_all(line.as_bytes())
    };
}

fn file_name(day: NaiveDate) -> String {
    format!("{PREFIX}{}.log", day.format("%Y-%m-%d"))
}

/// The log files in `dir`, oldest first: (path, day, size).
fn log_files(dir: &Path) -> Vec<(PathBuf, NaiveDate, u64)> {
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<(PathBuf, NaiveDate, u64)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let day = NaiveDate::parse_from_str(name.strip_prefix(PREFIX)?.strip_suffix(".log")?, "%Y-%m-%d").ok()?;
            Some((e.path(), day, e.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    v.sort_by_key(|(_, d, _)| *d);
    v
}

/// Removes files from before the last `days` days (today counts as one), then the oldest
/// while the folder is over [`MAX_TOTAL`] (today's file stays).
fn cleanup(dir: &Path, days: u32, today: NaiveDate) {
    let first = today - chrono::Days::new(u64::from(days.max(1)) - 1);
    let mut files = log_files(dir);
    files.retain(|(p, d, _)| {
        let old = *d < first;
        if old {
            let _ = fs::remove_file(p);
        }
        !old
    });
    let mut total: u64 = files.iter().map(|(_, _, n)| n).sum();
    for (p, d, n) in &files {
        if total <= MAX_TOTAL || *d >= today {
            break;
        }
        if fs::remove_file(p).is_ok() {
            total -= n;
        }
    }
}

/// Writes every log file (oldest first, scrubbed again) under a header describing this
/// machine into one text file for sending; returns its path. It goes to Downloads when
/// there is one, else to `~/.agentplus/exports`.
pub fn export(header: &str) -> anyhow::Result<PathBuf> {
    let files = log_files(&dir());
    let mut out = String::new();
    out.push_str(&scrub(header));
    out.push('\n');
    if files.is_empty() {
        out.push_str("(no log entries)\n");
    }
    for (p, _, _) in &files {
        out.push_str(&format!("\n===== {} =====\n", p.file_name().unwrap_or_default().to_string_lossy()));
        let text = fs::read(p).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
        for line in text.lines() {
            out.push_str(&scrub(line));
            out.push('\n');
        }
    }
    let target = dirs::download_dir().filter(|d| d.is_dir()).unwrap_or_else(|| agentplus_dir().join("exports"));
    fs::create_dir_all(&target)?;
    let path = target.join(format!("AgentPlus-log-{}.txt", Local::now().format("%Y%m%d-%H%M%S")));
    fs::write(&path, out)?;
    Ok(path)
}

// ---- scrubbing ------------------------------------------------------------------------

const MASK: &str = "***";

/// Hosts kept in the log: this machine, and public API vendors whose name helps tell
/// which upstream failed without saying anything about the user. Anything else (a relay,
/// a company proxy) is masked.
const PUBLIC_HOSTS: &[&str] = &[
    "openai.com", "chatgpt.com", "anthropic.com", "claude.ai", "googleapis.com", "google.com", "github.com",
    "githubusercontent.com", "openrouter.ai", "deepseek.com", "bigmodel.cn", "z.ai", "moonshot.cn", "moonshot.ai",
    "kimi.com", "xiaomimimo.com", "aliyuncs.com", "volces.com", "minimax.io", "minimaxi.com", "siliconflow.cn",
    "x.ai", "mistral.ai", "groq.com", "together.xyz", "opencode.ai", "models.dev", "npmjs.org", "factory.ai",
];

fn local_host(h: &str) -> bool {
    let h = h.to_ascii_lowercase();
    h == "localhost" || h == "0.0.0.0" || h == "[::1]" || h.starts_with("127.")
}

fn keep_host(h: &str) -> bool {
    let h = h.to_ascii_lowercase();
    local_host(&h) || PUBLIC_HOSTS.iter().any(|p| h == *p || h.ends_with(&format!(".{p}")))
}

struct Rules {
    kv: Regex,
    key: Regex,
    url: Regex,
    query: Regex,
    email: Regex,
    ipv4: Regex,
    user_dir: Regex,
    token: Regex,
    home: Option<Regex>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).filter(|h| h.len() > 3).map(|h| {
            // Either slash style, any case: `C:\Users\me`, `C:/Users/me`. It must end the path
            // component (kept in group 1), so `C:\Users\me2` is someone else's, not `~2`.
            let parts: Vec<String> = h.split(['\\', '/']).map(regex::escape).collect();
            Regex::new(&format!(r#"(?i){}([\\/\s"'<>:]|$)"#, parts.join(r"[\\/]+"))).unwrap()
        });
        Rules {
            // key=value, "key": "value", Header: value for anything that names a secret.
            kv: Regex::new(r#"(?i)(\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|secret|password|passwd|authorization|x-api-key|x-goog-api-key|cookie|session[_-]?token|token)["']?\s*[:=]\s*["']?)(?:Bearer\s+)?[^\s"',;&}]{3,}"#).unwrap(),
            key: Regex::new(r"\b(?:(?:sk|ak|pk|rk)-[A-Za-z0-9_-]{6,}|AIza[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9._-]+)|(\bBearer\s+)[A-Za-z0-9._~+/=-]{8,}").unwrap(),
            url: Regex::new(r"(?i)\b((?:https?|wss?)://)(?:[^\s/?#@]*@)?(\[[0-9a-f:]+\]|[^\s/?#:\x22'<>()，。）\]]+)").unwrap(),
            query: Regex::new(r"(\?[^\s#\x22'<>]*)").unwrap(),
            email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),
            ipv4: Regex::new(r"\b\d{1,3}(?:\.\d{1,3}){3}\b").unwrap(),
            user_dir: Regex::new(r"(?i)((?:^|[\\/])(?:Users|home)[\\/]+)([^\\/\s\x22'<>:]+)").unwrap(),
            token: Regex::new(r"\b[A-Za-z0-9_]{32,}\b").unwrap(),
            home,
        }
    })
}

/// `text` with everything that could identify the user or unlock an account masked.
pub fn scrub(text: &str) -> String {
    let r = rules();
    let mut s = text.to_string();
    if let Some(h) = &r.home {
        s = h.replace_all(&s, "~${1}").into_owned();
    }
    s = r.kv.replace_all(&s, |c: &Captures| format!("{}{MASK}", &c[1])).into_owned();
    s = r.key.replace_all(&s, |c: &Captures| format!("{}{MASK}", c.get(1).map_or("", |m| m.as_str()))).into_owned();
    // A query string can carry a key (`?key=…`); the path stays, it says which endpoint failed.
    s = r
        .url
        .replace_all(&s, |c: &Captures| {
            let host = &c[2];
            format!("{}{}", &c[1], if keep_host(host) { host } else { MASK })
        })
        .into_owned();
    s = mask_queries(&s, &r.query);
    s = r.email.replace_all(&s, MASK).into_owned();
    s = r.ipv4.replace_all(&s, |c: &Captures| if local_host(&c[0]) { c[0].to_string() } else { MASK.into() }).into_owned();
    s = r.user_dir.replace_all(&s, |c: &Captures| format!("{}{MASK}", &c[1])).into_owned();
    // Long random-looking strings (letters and digits mixed): keys without a known prefix.
    s = r
        .token
        .replace_all(&s, |c: &Captures| {
            let t = &c[0];
            if t.bytes().any(|b| b.is_ascii_digit()) && t.bytes().any(|b| b.is_ascii_alphabetic()) { MASK.to_string() } else { t.to_string() }
        })
        .into_owned();
    s
}

/// Masks the query string of each URL (`https://h/p?key=1` → `https://h/p?***`).
fn mask_queries(s: &str, query: &Regex) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let end = tail.find(|c: char| c.is_whitespace() || "\"'<>".contains(c)).unwrap_or(tail.len());
        let (url, after) = tail.split_at(end);
        out.push_str(&query.replace(url, |c: &Captures| if c[1].len() > 1 { format!("?{MASK}") } else { c[1].to_string() }));
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_masks_secrets_hosts_and_users() {
        let s = scrub("POST https://relay.example.com:8443/v1/chat?key=abc123 failed: Bearer sk-abcdef123456 (api_key=\"zz-9999-xyz\")");
        assert_eq!(s, "POST https://***:8443/v1/chat?*** failed: Bearer *** (api_key=\"***\")");
        assert_eq!(scrub("upstream https://api.openai.com/v1/responses 401"), "upstream https://api.openai.com/v1/responses 401");
        assert_eq!(scrub("gateway http://127.0.0.1:18650/relay/v1"), "gateway http://127.0.0.1:18650/relay/v1");
        assert_eq!(scrub("connect 64.83.33.80:8080 refused"), "connect ***:8080 refused");
        assert_eq!(scrub(r"C:\Users\someone\.codex\config.toml"), r"C:\Users\***\.codex\config.toml");
        // Someone else's home: a name that is not the test runner's, whose own home becomes `~` first.
        assert_eq!(scrub("/home/someone/.config/opencode"), "/home/***/.config/opencode");
        assert_eq!(scrub("signed in as a.b@example.org"), "signed in as ***");
        assert_eq!(scrub(r#"{"apiKey": "k-123456"}"#), r#"{"apiKey": "***"}"#);
        assert_eq!(scrub("x-api-key: abcdefgh"), "x-api-key: ***");
        assert_eq!(scrub("token 3f9a0c7be21d44aa9e0d1c2b3a4f5e6d7c8b"), "token ***");
        // Ordinary words, versions and short ids stay.
        assert_eq!(scrub("Codex 26.924.2738.0 gpt-5.5 took 1.2s"), "Codex 26.924.2738.0 gpt-5.5 took 1.2s");
    }

    #[test]
    fn scrub_replaces_own_home() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(scrub(&format!("{home}/.agentplus/store.json")), "~/.agentplus/store.json");
        assert_eq!(scrub(&format!("{}\\x", home.replace('/', "\\"))), "~\\x");
        assert_eq!(scrub(&format!("cwd {home}")), "cwd ~");
        assert_eq!(scrub(&format!("\"{home}\": ok")), "\"~\": ok");
        // A longer name that merely starts with ours is another user.
        assert!(!scrub(&format!("{home}2/x")).starts_with('~'));
    }

    #[test]
    fn lines_are_single_and_clipped() {
        let l = format_line("T", "ERROR", "cmd", "first\r\n  second\n\nthird  ");
        assert_eq!(l, "T ERROR [cmd] first ⏎   second ⏎ third\n");
        assert!(format_line("T", "INFO", "x", &"a".repeat(MAX_LINE * 2)).len() < MAX_LINE + 40);
    }

    #[test]
    fn cleanup_keeps_recent_days() {
        let d = std::env::temp_dir().join(format!("agentplus-applog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        for back in 0..10u64 {
            fs::write(d.join(file_name(today - chrono::Days::new(back))), "x\n").unwrap();
        }
        fs::write(d.join("other.txt"), "keep").unwrap();
        cleanup(&d, 7, today);
        let left: Vec<NaiveDate> = log_files(&d).into_iter().map(|(_, day, _)| day).collect();
        assert_eq!(left.len(), 7);
        assert_eq!(left.first(), Some(&(today - chrono::Days::new(6))));
        assert!(d.join("other.txt").exists());
        // One day keeps only today.
        cleanup(&d, 1, today);
        assert_eq!(log_files(&d).len(), 1);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn append_stops_at_the_file_cap() {
        let d = std::env::temp_dir().join(format!("agentplus-applog-cap-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("a.log");
        fs::write(&p, vec![b'x'; (MAX_FILE - 10) as usize]).unwrap();
        append(&p, "a line that does not fit\n");
        let len = crate::util::file_len(&p);
        assert!(fs::read_to_string(&p).unwrap().ends_with("dropped --\n"));
        append(&p, "more\n");
        assert_eq!(crate::util::file_len(&p), len);
        fs::remove_dir_all(&d).unwrap();
    }
}
