//! Codex sessions: browse, health check, safe cleanup and provider repair.
//!
//! Sources of truth: `state_5.sqlite` (`threads`, schema v55) plus rollout files
//! `sessions/YYYY/MM/DD/rollout-*-<id>[_<segment>].jsonl`. Writes only happen with
//! Codex closed, after a backup, and repairs keep an undo log.

use crate::adapters::codex::{codex_home, configured_provider};
use crate::i18n::l;
use crate::process;
use crate::util::*;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const STATE_DB: &str = "state_5.sqlite";
const LOGS_DB: &str = "logs_2.sqlite";
const HISTORY_DB: &str = "thread_history_1.sqlite";
/// Schema versions this code was written against; anything else is read-only.
const STATE_VERSION: i64 = 55;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub provider: String,
    pub model: String,
    /// user | automation | subagent | review | exec | agent
    pub kind: String,
    pub archived: bool,
    pub updated_ms: i64,
    pub size: u64,
    pub rollout_path: String,
    pub rollout_exists: bool,
    /// Why Codex desktop may not show it in its lists (empty = shown).
    pub hidden: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionList {
    pub sessions: Vec<SessionRow>,
    pub current_provider: String,
    pub providers: Vec<(String, usize)>,
    /// Providers sessions can be moved to: "openai" plus every [model_providers.*].
    pub targets: Vec<String>,
    pub codex_running: bool,
    pub writable: bool,
    pub note: Option<String>,
    pub last_repair: Option<RepairSummary>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepairSummary {
    pub stamp: String,
    pub target: String,
    pub count: usize,
    pub undone: bool,
}

fn state_path() -> PathBuf {
    codex_home().join(STATE_DB)
}

fn open_ro(p: &Path) -> Result<Connection> {
    if crate::env::is_wsl() {
        return open_snapshot(p);
    }
    Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| tr!("Failed to open {}", "打开 {} 失败", p.display()))
}

/// SQLite cannot lock over the WSL file share, so WSL databases are read from a
/// fresh copy (db + wal) on the Windows side.
fn open_snapshot(p: &Path) -> Result<Connection> {
    // One folder per call: the session list and the health check run at the same time, and
    // copying over a database another connection has open corrupts what it reads.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = agentplus_dir().join("tmp").join("wsl-snapshot");
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = base.join(format!("{}-{n}", std::process::id()));
    // Clear out old snapshots: ones from an earlier run, or long finished. Never a recent one
    // of this run — another thread may have copied it and not opened it yet.
    let mine = format!("{}-", std::process::id());
    let old = |e: &fs::DirEntry| e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > std::time::Duration::from_secs(600));
    if let Ok(rd) = fs::read_dir(&base) {
        for e in rd.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with(&mine) || old(e)) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    fs::create_dir_all(&dir)?;
    let name = p.file_name().ok_or_else(|| anyhow!(l("Invalid path", "路径无效")))?;
    let dst = dir.join(name);
    let dst_wal = wal_of(&dst);
    fs::copy(p, &dst).with_context(|| tr!("Failed to copy {}", "复制 {} 失败", p.display()))?;
    if wal_of(p).exists() {
        fs::copy(wal_of(p), &dst_wal)?;
    }
    Connection::open(&dst).with_context(|| tr!("Failed to open {}", "打开 {} 失败", dst.display()))
}

/// Writes need real locks; refuse them for WSL.
fn deny_wsl() -> Result<()> {
    if crate::env::is_wsl() {
        return Err(anyhow!(l("Session databases in WSL are read-only for now: Windows can't lock files in WSL, so writing isn't safe", "WSL 里的会话数据库目前只读：Windows 侧无法对 WSL 文件加锁，写入不安全")));
    }
    Ok(())
}

fn migration_version(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT max(version) FROM _sqlx_migrations", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
}

fn norm_path(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

/// A rollout path as this side can open it: the WSL database stores Linux paths
/// (`/home/me/.codex/...`), reached over `\\wsl.localhost\<distro>\...`.
fn local_path(p: &str) -> String {
    let p = norm_path(p);
    if crate::env::is_wsl() && p.starts_with('/') {
        return crate::env::resolve_path(&p).to_string_lossy().into_owned();
    }
    p
}

fn kind_of(source: &str, thread_source: &str) -> &'static str {
    match (source, thread_source) {
        (_, "guardian_review") => "review",
        (_, "subagent") => "subagent",
        (_, "agent_created_thread") => "agent",
        ("exec", _) => "exec",
        (_, "automation") => "automation",
        _ if source.starts_with('{') => "subagent",
        _ => "user",
    }
}

fn config_doc() -> Option<toml_edit::DocumentMut> {
    crate::adapters::codex::load_doc().ok().map(|(d, _)| d)
}

fn current_provider_id() -> String {
    config_doc().map(|d| configured_provider(&d)).unwrap_or_else(|| "openai".into())
}

/// "openai" (built in) plus every provider defined in config.toml.
fn defined_providers() -> Vec<String> {
    let mut out = vec!["openai".to_string()];
    if let Some(d) = config_doc() {
        if let Some(t) = d.get("model_providers").and_then(|i| i.as_table_like()) {
            out.extend(t.iter().map(|(k, _)| k.to_string()));
        }
    }
    out
}

/// Codex is "busy" when its desktop package or any codex.exe (CLI/app-server) runs.
/// The always-on Windows sandbox service does not count.
pub fn codex_busy() -> bool {
    if process::detect("codex").running {
        return true;
    }
    if crate::env::is_wsl() {
        return false;
    }
    process::any_process(|name, path| {
        let n = name.to_lowercase();
        (n == "codex.exe" || n == "codex") && !path.to_lowercase().contains("sandbox")
    })
}

pub fn list() -> Result<SessionList> {
    let conn = open_ro(&state_path())?;
    let version = migration_version(&conn);
    let cur = current_provider_id();
    // Older Codex builds (e.g. the CLI inside WSL) lack some columns; read what exists.
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('threads')")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|c| c.ok())
        .collect();
    let col = |c: &str| if cols.iter().any(|x| x == c) { format!("coalesce({c},'')") } else { "''".into() };
    let updated = match (cols.iter().any(|c| c == "updated_at_ms"), cols.iter().any(|c| c == "updated_at")) {
        (true, true) => "coalesce(updated_at_ms, updated_at*1000, 0)",
        (true, false) => "coalesce(updated_at_ms, 0)",
        (false, true) => "coalesce(updated_at*1000, 0)",
        (false, false) => "0",
    };
    let archived = if cols.iter().any(|c| c == "archived") { "archived" } else { "0" };
    let sql = format!(
        "SELECT id, {}, {}, {}, {}, {}, {}, {}, {}, {archived}, {updated}, {} FROM threads ORDER BY {updated} DESC",
        col("name"), col("title"), col("preview"), col("cwd"), col("model_provider"),
        col("model"), col("source"), col("thread_source"), col("rollout_path"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?,
            r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?,
            r.get::<_, String>(8)?, r.get::<_, i64>(9)?, r.get::<_, i64>(10)?, r.get::<_, String>(11)?,
        ))
    })?;
    let mut sessions = vec![];
    let mut counts: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let (id, name, title, preview, cwd, provider, model, source, tsrc, archived, updated, rollout) = row?;
        let rp = local_path(&rollout);
        let meta = fs::metadata(&rp).ok();
        let kind = kind_of(&source, &tsrc);
        let mut hidden = vec![];
        if archived != 0 {
            hidden.push(l("Archived", "已归档").to_string());
        }
        if !matches!(kind, "user" | "automation") {
            hidden.push(l("Subagent / review / exec sessions don't appear in the sidebar", "子代理 / 审查 / exec 会话不进侧边栏").to_string());
        } else if preview.is_empty() {
            hidden.push(l("Empty; the desktop app doesn't show it", "没有内容，桌面端不显示").to_string());
        }
        if !provider.is_empty() && provider != cur {
            hidden.push(tr!("Belongs to \"{provider}\" but the current provider is \"{cur}\": may not appear in Recent or Archived", "属于「{provider}」，当前是「{cur}」：最近列表和归档里可能看不到"));
        }
        if meta.is_none() {
            hidden.push(l("Session file is missing and can't be restored", "会话文件缺失，无法恢复").to_string());
        }
        let shown_title = [name, title, preview].into_iter().find(|s| !s.trim().is_empty()).unwrap_or_else(|| l("(Untitled)", "(无标题)").into());
        *counts.entry(provider.clone()).or_default() += 1;
        sessions.push(SessionRow {
            id,
            title: shown_title.chars().take(80).collect(),
            cwd: norm_path(&cwd),
            provider,
            model,
            kind: kind.into(),
            archived: archived != 0,
            updated_ms: updated,
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            rollout_exists: meta.is_some(),
            rollout_path: rp,
            hidden,
        });
    }
    let mut providers: Vec<(String, usize)> = counts.into_iter().collect();
    providers.sort_by_key(|p| std::cmp::Reverse(p.1));
    let writable = version == Some(STATE_VERSION) && !crate::env::is_wsl();
    Ok(SessionList {
        sessions,
        current_provider: cur,
        providers,
        targets: defined_providers(),
        codex_running: codex_busy(),
        writable,
        note: if writable { None } else if crate::env::is_wsl() { Some(l("Sessions in WSL can only be browsed: Windows can't lock the databases in WSL, so repair, migration and cleanup aren't available.", "WSL 里的会话只能浏览：Windows 侧无法对 WSL 里的数据库加锁，修复、迁移和清理暂不提供。").into()) } else { Some(tr!("state_5.sqlite is version {version:?}, not the verified {STATE_VERSION}; shown read-only.", "state_5.sqlite 版本是 {version:?}，不是已验证的 {STATE_VERSION}，只读显示。")) },
        last_repair: last_repair(),
    })
}

// ---------------------------------------------------------------- health

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthItem {
    pub key: String,
    pub title: String,
    /// ok | warn | error | info
    pub status: String,
    pub detail: String,
}

fn item(key: &str, title: &str, status: &str, detail: impl Into<String>) -> HealthItem {
    HealthItem { key: key.into(), title: title.into(), status: status.into(), detail: detail.into() }
}

fn tmp_leftovers() -> Vec<(PathBuf, u64)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(codex_home()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("..codex-global-state.json") && n.contains(".tmp-") {
                out.push((e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    out
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_048_576.0)
}

pub fn health() -> Result<Vec<HealthItem>> {
    let home = codex_home();
    let mut out = vec![];

    // Schema versions
    let state = open_ro(&state_path())?;
    let v = migration_version(&state);
    out.push(if v == Some(STATE_VERSION) {
        item("schema", l("Database version", "数据库版本"), "ok", tr!("state_5 v{STATE_VERSION}, verified", "state_5 v{STATE_VERSION}，已验证"))
    } else {
        item("schema", l("Database version", "数据库版本"), "warn", tr!("state_5 version {v:?} is unverified; write operations are disabled", "state_5 版本 {v:?} 未验证，写入类操作已禁用"))
    });

    // Integrity (quick_check is read-only)
    let qc: String = state.query_row("PRAGMA quick_check", [], |r| r.get(0)).unwrap_or_else(|e| e.to_string());
    out.push(if qc == "ok" { item("integrity", l("Session index integrity", "会话索引完整性"), "ok", l("quick_check passed", "quick_check 通过")) } else { item("integrity", l("Session index integrity", "会话索引完整性"), "error", qc) });

    // Rollout files
    let list = list()?;
    let missing = list.sessions.iter().filter(|s| !s.rollout_exists).count();
    out.push(if missing == 0 {
        item("rollout", l("Session files", "会话文件"), "ok", tr!("Files for all {} session(s) are present", "{} 个会话的文件都在", list.sessions.len()))
    } else {
        item("rollout", l("Session files", "会话文件"), "error", tr!("Files for {missing} session(s) are missing and can't be restored", "{missing} 个会话的文件不见了，无法恢复"))
    });

    // Other providers
    let others: Vec<String> = list.providers.iter().filter(|(p, _)| p != &list.current_provider && !p.is_empty()).map(|(p, n)| tr!("{p}: {n}", "{p} {n} 个")).collect();
    out.push(if others.is_empty() {
        item("provider", l("Session providers", "会话供应商"), "ok", tr!("All belong to the current provider \"{}\"", "全部属于当前供应商「{}」", list.current_provider))
    } else {
        item("provider", l("Session providers", "会话供应商"), "warn", tr!("{}. After switching they may not appear in Recent or Archived; fix them in one click under Sessions", "{}，切换后在最近列表和归档里可能看不到，可在「会话」里一键修复", crate::i18n::join(&others)))
    });

    // Providers used by sessions but missing from config
    let defined = defined_providers();
    let undefined: Vec<String> = list.providers.iter().map(|(p, _)| p.clone()).filter(|p| !p.is_empty() && !defined.contains(p)).collect();
    if !undefined.is_empty() {
        out.push(item("undefined", l("Missing provider config", "缺少供应商配置"), "warn", tr!("{} used by sessions is not defined in config.toml; resuming those sessions will fail", "会话用到的 {} 在 config.toml 里没有定义，恢复这些会话会失败", crate::i18n::join(&undefined))));
    }

    // Projection progress (thread_history)
    if let Ok(h) = open_ro(&home.join(HISTORY_DB)) {
        let mut behind = 0;
        if let Ok(mut st) = h.prepare("SELECT thread_id, next_rollout_byte_offset FROM thread_history_projection_state") {
            let paths: HashMap<&str, &SessionRow> = list.sessions.iter().map(|s| (s.id.as_str(), s)).collect();
            if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for (id, off) in rows.flatten() {
                    if let Some(s) = paths.get(id.as_str()) {
                        if s.rollout_exists && (off as u64) + 4096 < s.size {
                            behind += 1;
                        }
                    }
                }
            }
        }
        out.push(if behind == 0 {
            item("projection", l("History projection", "历史投影"), "ok", l("History for all sessions is synced", "所有会话的历史都已同步"))
        } else {
            item("projection", l("History projection", "历史投影"), "info", tr!("History for {behind} session(s) isn't fully synced yet; Codex continues when the session is opened", "{behind} 个会话的历史还没同步完，打开该会话时 Codex 会继续处理"))
        });
    }

    // Leftover temp files
    let tmps = tmp_leftovers();
    out.push(if tmps.is_empty() {
        item("tmp", l("Leftover temp files", "残留临时文件"), "ok", l("None", "没有"))
    } else {
        item("tmp", l("Leftover temp files", "残留临时文件"), "warn", tr!("{} file(s) left by interrupted writes, {} in total", "{} 个中断写入留下的文件，共 {}", tmps.len(), mb(tmps.iter().map(|t| t.1).sum())))
    });

    // Global state JSON
    let gs = home.join(".codex-global-state.json");
    if gs.exists() {
        let ok = fs::read_to_string(&gs).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).is_some();
        out.push(if ok {
            item("globalstate", l("Desktop UI state", "桌面端界面状态"), "ok", tr!("Parses fine, {}", "可以解析，{}", mb(file_len(&gs))))
        } else {
            item("globalstate", l("Desktop UI state", "桌面端界面状态"), "error", l("Can't be parsed; restore it from .bak", "无法解析，可从 .bak 恢复"))
        });
    }

    // Logs size
    let logs = home.join(LOGS_DB);
    if let Ok(db) = open_ro(&logs) {
        out.push(item("logs", l("Log database", "日志库"), if file_len(&logs) > 50 * 1_048_576 { "warn" } else { "ok" },
            tr!("{}, of which {} is reclaimable free space; clean it up below", "{}，其中可回收空闲 {}；可在下方清理", mb(file_len(&logs)), mb(free_bytes(&db)))));
    }
    Ok(out)
}

// ---------------------------------------------------------------- cleanup

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreview {
    pub tmp_count: usize,
    pub tmp_bytes: u64,
    pub logs_bytes: u64,
    pub logs_rows: i64,
    pub logs_old_rows: i64,
    pub logs_free_bytes: u64,
    pub wal_bytes: u64,
    pub codex_running: bool,
}

fn sqlite_files() -> Vec<PathBuf> {
    let h = codex_home();
    let mut v: Vec<PathBuf> = [STATE_DB, LOGS_DB, HISTORY_DB, "memories_1.sqlite", "queue_1.sqlite", "goals_1.sqlite"]
        .iter()
        .map(|n| h.join(n))
        .collect();
    v.push(h.join("sqlite").join("codex-dev.db"));
    v.into_iter().filter(|p| p.exists()).collect()
}

fn wal_of(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", p.display()))
}

/// Bytes on disk taken by Codex's databases, their WAL files and leftover temp files.
fn disk_usage() -> u64 {
    sqlite_files().iter().map(|p| file_len(p) + file_len(&wal_of(p))).sum::<u64>() + tmp_leftovers().iter().map(|t| t.1).sum::<u64>()
}

/// Free pages inside a database, in bytes (what VACUUM would give back).
fn free_bytes(c: &Connection) -> u64 {
    let page: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap_or(4096);
    let free: i64 = c.query_row("PRAGMA freelist_count", [], |r| r.get(0)).unwrap_or(0);
    (page * free) as u64
}

/// Unix time `days` days ago: log rows older than this count as old.
fn cutoff(days: u32) -> i64 {
    chrono::Utc::now().timestamp() - days as i64 * 86400
}

pub fn cleanup_preview(days: u32) -> Result<CleanupPreview> {
    let tmps = tmp_leftovers();
    let logs = codex_home().join(LOGS_DB);
    let (mut rows, mut old, mut free) = (0i64, 0i64, 0u64);
    if let Ok(l) = open_ro(&logs) {
        rows = l.query_row("SELECT count(*) FROM logs", [], |r| r.get(0)).unwrap_or(0);
        old = l.query_row("SELECT count(*) FROM logs WHERE ts < ?1", params![cutoff(days)], |r| r.get(0)).unwrap_or(0);
        free = free_bytes(&l);
    }
    Ok(CleanupPreview {
        tmp_count: tmps.len(),
        tmp_bytes: tmps.iter().map(|t| t.1).sum(),
        logs_bytes: file_len(&logs),
        logs_rows: rows,
        logs_old_rows: old,
        logs_free_bytes: free,
        wal_bytes: sqlite_files().iter().map(|p| file_len(&wal_of(p))).sum(),
        codex_running: codex_busy(),
    })
}

fn copy_db(p: &Path, dir: &Path) -> Result<()> {
    for f in [p.to_path_buf(), wal_of(p), PathBuf::from(format!("{}-shm", p.display()))] {
        if f.exists() {
            fs::copy(&f, dir.join(f.file_name().unwrap()))?;
        }
    }
    Ok(())
}

pub fn cleanup(tmp: bool, logs_days: Option<u32>, wal: bool) -> Result<String> {
    deny_wsl()?;
    if codex_busy() {
        return Err(anyhow!(l("Codex is running. Quit Codex (including the CLI) before cleaning up", "Codex 正在运行，请先退出 Codex（包括 CLI）再清理")));
    }
    let before = disk_usage();
    let dir = new_backup_dir("codex-cleanup")?;
    let mut done = vec![];
    if tmp {
        let tmps = tmp_leftovers();
        for (p, _) in &tmps {
            fs::rename(p, dir.join(p.file_name().unwrap())).or_else(|_| fs::copy(p, dir.join(p.file_name().unwrap())).and_then(|_| fs::remove_file(p)))?;
        }
        done.push(tr!("moved {} temp file(s)", "移走 {} 个临时文件", tmps.len()));
    }
    if let Some(days) = logs_days.filter(|_| codex_home().join(LOGS_DB).is_file()) {
        let logs = codex_home().join(LOGS_DB);
        copy_db(&logs, &dir)?;
        let c = Connection::open(&logs)?;
        let n = c.execute("DELETE FROM logs WHERE ts < ?1", params![cutoff(days)])?;
        c.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        done.push(tr!("deleted {n} log entries older than {days} days and compacted", "删除 {n} 条 {days} 天前的日志并压缩"));
    }
    if wal {
        let mut n = 0;
        for p in sqlite_files() {
            if file_len(&wal_of(&p)) > 0 {
                let c = Connection::open(&p)?;
                c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
                n += 1;
            }
        }
        done.push(tr!("truncated {n} WAL file(s)", "截断 {n} 个 WAL 文件"));
    }
    let after = disk_usage();
    Ok(tr!("{}; freed {} (backup in {})", "{}，释放 {}（备份在 {}）", done.join(l(", ", "，")), mb(before.saturating_sub(after)), display_path(&dir)))
}

// ---------------------------------------------------------------- repair

fn repairs_dir() -> PathBuf {
    agentplus_dir().join("repairs")
}

fn last_repair() -> Option<RepairSummary> {
    let mut logs: Vec<PathBuf> = fs::read_dir(repairs_dir()).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "json").unwrap_or(false)).collect();
    logs.sort();
    let p = logs.pop()?;
    let v: Value = serde_json::from_str(&fs::read_to_string(&p).ok()?).ok()?;
    Some(RepairSummary {
        stamp: p.file_stem()?.to_string_lossy().to_string(),
        target: v["target"].as_str().unwrap_or("").into(),
        count: v["entries"].as_array().map(|a| a.len()).unwrap_or(0),
        undone: v["undone"].as_bool().unwrap_or(false),
    })
}

/// All rollout files (all segments) per thread id, from sessions/ and archived_sessions/.
fn rollout_index() -> HashMap<String, Vec<PathBuf>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    // Codex nests sessions/YYYY/MM/DD; the depth cap and not following links keep a
    // junction loop from recursing forever.
    fn walk(d: &Path, map: &mut HashMap<String, Vec<PathBuf>>, depth: usize) {
        if depth > 8 {
            return;
        }
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    walk(&p, map, depth + 1);
                } else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    if n.starts_with("rollout-") && n.ends_with(".jsonl") {
                        // rollout-<time>-<uuid>[_<segment>].jsonl ; uuid = last 5 dash groups
                        let stem = n.trim_end_matches(".jsonl");
                        let base = stem.split('_').next().unwrap_or(stem);
                        let parts: Vec<&str> = base.split('-').collect();
                        if parts.len() >= 5 {
                            let id = parts[parts.len() - 5..].join("-");
                            map.entry(id).or_default().push(p);
                        }
                    }
                }
            }
        }
    }
    let h = codex_home();
    walk(&h.join("sessions"), &mut map, 0);
    walk(&h.join("archived_sessions"), &mut map, 0);
    map
}

/// Replaces the first line of `path` in place (streamed); returns the old line.
fn rewrite_first_line(path: &Path, edit: impl Fn(&str) -> Option<String>) -> Result<Option<String>> {
    let mut r = BufReader::new(File::open(path)?);
    let mut first = String::new();
    r.read_line(&mut first)?;
    let (body, eol) = if let Some(b) = first.strip_suffix("\r\n") {
        (b.to_string(), "\r\n")
    } else if let Some(b) = first.strip_suffix('\n') {
        (b.to_string(), "\n")
    } else {
        (first.clone(), "")
    };
    let Some(new) = edit(&body) else { return Ok(None) };
    let tmp = PathBuf::from(format!("{}.agentplus-tmp", path.display()));
    let written = (|| -> std::io::Result<()> {
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(new.as_bytes())?;
        w.write_all(eol.as_bytes())?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
        }
        w.flush()?;
        drop(w);
        drop(r);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| tr!("Failed to rewrite {}", "改写 {} 失败", path.display()));
    }
    Ok(Some(body))
}

/// Swaps `"model_provider":"<old>"` in a session_meta line, byte-preserving the rest
/// (including any spacing around the colon).
fn swap_provider(line: &str, target: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let old = v.pointer("/payload/model_provider")?.as_str()?;
    if old == target {
        return None;
    }
    let re = regex::Regex::new(&format!(r#"("model_provider"\s*:\s*){}"#, regex::escape(&serde_json::to_string(old).ok()?))).ok()?;
    let repl = serde_json::to_string(target).ok()?;
    re.is_match(line).then(|| re.replacen(line, 1, |c: &regex::Captures| format!("{}{repl}", &c[1])).into_owned())
}

/// Puts back the first lines of files already rewritten by a repair that then failed.
fn restore_lines(done: &[(PathBuf, String)]) {
    for (path, line) in done.iter().rev() {
        let _ = rewrite_first_line(path, |_| Some(line.clone()));
    }
}

/// Writes a repair log under a name no earlier repair uses (milliseconds, so they sort in
/// order). A log that can't be written completely is removed: an empty one would hide the
/// undo button of every repair (`last_repair` reads the newest).
fn write_repair_log(content: &str) -> Result<PathBuf> {
    fs::create_dir_all(repairs_dir())?;
    for _ in 0..50 {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f").to_string();
        let p = repairs_dir().join(format!("{stamp}.json"));
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(content.as_bytes()).and_then(|_| f.sync_all()) {
                    drop(f);
                    let _ = fs::remove_file(&p);
                    return Err(e.into());
                }
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::thread::sleep(std::time::Duration::from_millis(2)),
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow!(l("Couldn't create the repair log", "无法创建修复记录")))
}

/// Moves sessions to `target` provider (DB row + every rollout segment's session_meta).
pub fn repair(ids: &[String], target: &str) -> Result<String> {
    deny_wsl()?;
    if codex_busy() {
        return Err(anyhow!(l("Codex is running. Quit Codex (including the CLI) before repairing", "Codex 正在运行，请先退出 Codex（包括 CLI）再修复")));
    }
    let db = state_path();
    let conn = Connection::open(&db)?;
    if migration_version(&conn) != Some(STATE_VERSION) {
        return Err(anyhow!(l("state_5.sqlite version is unverified; not modifying it", "state_5.sqlite 版本未验证，不修改")));
    }
    let dir = new_backup_dir("codex-repair")?;
    copy_db(&db, &dir)?;
    let index = rollout_index();
    let mut entries = vec![];
    // Files rewritten so far: put back if anything fails before the commit.
    let mut done: Vec<(PathBuf, String)> = vec![];
    let tx = conn.unchecked_transaction()?;
    let result = (|| -> Result<()> {
        for id in ids {
            let old: Option<String> = tx.query_row("SELECT model_provider FROM threads WHERE id = ?1", params![id], |r| r.get(0)).ok();
            let Some(old) = old else { continue };
            if old == target {
                continue;
            }
            let mut files = vec![];
            for f in index.get(id).cloned().unwrap_or_default() {
                if let Some(old_line) = rewrite_first_line(&f, |l| swap_provider(l, target))? {
                    files.push(json!({ "path": f.to_string_lossy(), "line": old_line }));
                    done.push((f, old_line));
                }
            }
            tx.execute("UPDATE threads SET model_provider = ?1 WHERE id = ?2", params![target, id])?;
            entries.push(json!({ "id": id, "old": old, "files": files }));
        }
        Ok(())
    })();
    if let Err(e) = result.and_then(|_| tx.commit().map_err(Into::into)) {
        restore_lines(&done);
        return Err(e);
    }
    let n = entries.len();
    if n == 0 {
        return Ok(tr!("The selected sessions already belong to \"{target}\"; nothing to migrate", "选中的会话已经属于「{target}」，没有需要迁移的"));
    }
    write_repair_log(&serde_json::to_string(&json!({ "target": target, "backup": dir.to_string_lossy(), "entries": entries, "undone": false }))?)?;
    Ok(tr!("Migrated {n} session(s) to \"{target}\". Takes effect after restarting Codex (can be undone)", "已把 {n} 个会话迁移到「{target}」，重启 Codex 后生效（可撤销）"))
}

pub fn undo_repair(stamp: &str) -> Result<String> {
    deny_wsl()?;
    if !plain_name(stamp) {
        return Err(anyhow!(l("Invalid repair record", "无效的修复记录")));
    }
    if codex_busy() {
        return Err(anyhow!(l("Codex is running. Quit Codex before undoing", "Codex 正在运行，请先退出 Codex 再撤销")));
    }
    let p = repairs_dir().join(format!("{stamp}.json"));
    let mut log: Value = serde_json::from_str(&fs::read_to_string(&p)?)?;
    if log["undone"].as_bool().unwrap_or(false) {
        return Err(anyhow!(l("This repair has already been undone", "这次修复已经撤销过了")));
    }
    let conn = Connection::open(state_path())?;
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    // (path, the line it had before this undo), to put back on failure.
    let mut done: Vec<(PathBuf, String)> = vec![];
    let result = (|| -> Result<()> {
        for e in log["entries"].as_array().cloned().unwrap_or_default() {
            for f in e["files"].as_array().cloned().unwrap_or_default() {
                let path = PathBuf::from(f["path"].as_str().unwrap_or_default());
                let line = f["line"].as_str().unwrap_or_default().to_string();
                if path.is_file() {
                    if let Some(before) = rewrite_first_line(&path, |_| Some(line.clone()))? {
                        done.push((path, before));
                    }
                }
            }
            tx.execute("UPDATE threads SET model_provider = ?1 WHERE id = ?2", params![e["old"].as_str().unwrap_or_default(), e["id"].as_str().unwrap_or_default()])?;
            n += 1;
        }
        Ok(())
    })();
    if let Err(e) = result.and_then(|_| tx.commit().map_err(Into::into)) {
        restore_lines(&done);
        return Err(e);
    }
    log["undone"] = json!(true);
    fs::write(&p, serde_json::to_string(&log)?)?;
    Ok(tr!("Undone: {n} session(s) restored to their original provider", "已撤销，{n} 个会话恢复到原来的供应商"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_only_provider_field() {
        let line = r#"{"timestamp":"t","type":"session_meta","payload":{"id":"x","model_provider":"klpz","base_instructions":{"text":"a \"model_provider\":\"klpz\" b"}}}"#;
        let out = swap_provider(line, "agentplus").unwrap();
        assert!(out.contains(r#""model_provider":"agentplus","base_instructions""#));
        assert!(out.contains(r#"a \"model_provider\":\"klpz\" b"#));
        assert!(swap_provider(&out, "agentplus").is_none());
    }

    #[test]
    fn indexes_segments_by_thread_id() {
        let n = "rollout-2026-09-07T00-40-11-01a07797-7b0f-78d3-84b0-b7a65f7a6632_01a0ffff-0000-7000-8000-000000000000.jsonl";
        let stem = n.trim_end_matches(".jsonl");
        let base = stem.split('_').next().unwrap();
        let parts: Vec<&str> = base.split('-').collect();
        assert_eq!(parts[parts.len() - 5..].join("-"), "01a07797-7b0f-78d3-84b0-b7a65f7a6632");
    }

    /// Repair + undo on a COPY of ~/.codex (CODEX_HOME points at a temp dir).
    /// Checks the rollout files come back byte-identical and the DB rows restored.
    #[test]
    #[ignore]
    fn repair_roundtrip_on_copy() {
        let real = crate::util::home().join(".codex");
        let tmp = std::env::temp_dir().join("agentplus-repair-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sessions")).unwrap();
        for n in [STATE_DB, "state_5.sqlite-wal", "state_5.sqlite-shm", "config.toml"] {
            if real.join(n).exists() {
                fs::copy(real.join(n), tmp.join(n)).unwrap();
            }
        }
        std::env::set_var("CODEX_HOME", &tmp);
        // An empty store: a Codex folder picked in AgentPlus would win over CODEX_HOME.
        std::env::set_var("AGENTPLUS_HOME", tmp.join(".agentplus"));
        let before = list().unwrap();
        // two sessions not on the current provider, copy their rollout files
        let picks: Vec<&SessionRow> = before.sessions.iter().filter(|s| s.provider != before.current_provider && s.rollout_exists).take(2).collect();
        let real_index = {
            std::env::set_var("CODEX_HOME", &real);
            let i = rollout_index();
            std::env::set_var("CODEX_HOME", &tmp);
            i
        };
        let mut copies = vec![];
        for s in &picks {
            for f in real_index.get(&s.id).unwrap() {
                let dst = tmp.join("sessions").join(f.file_name().unwrap());
                fs::copy(f, &dst).unwrap();
                copies.push((dst.clone(), fs::read(&dst).unwrap()));
            }
        }
        let ids: Vec<String> = picks.iter().map(|s| s.id.clone()).collect();
        println!("{}", repair(&ids, "agentplus").unwrap());
        let mid = list().unwrap();
        for id in &ids {
            assert_eq!(mid.sessions.iter().find(|s| &s.id == id).unwrap().provider, "agentplus");
        }
        for (p, _) in &copies {
            let first = BufReader::new(File::open(p).unwrap()).lines().next().unwrap().unwrap();
            assert!(first.contains("\"model_provider\":\"agentplus\""));
        }
        let stamp = last_repair().unwrap().stamp;
        println!("{}", undo_repair(&stamp).unwrap());
        let after = list().unwrap();
        for s in &picks {
            assert_eq!(after.sessions.iter().find(|x| x.id == s.id).unwrap().provider, s.provider);
        }
        for (p, orig) in &copies {
            assert_eq!(&fs::read(p).unwrap(), orig, "{} not restored byte-identical", p.display());
        }
        // remove the test's undo log so the real UI doesn't show it
        let _ = fs::remove_file(repairs_dir().join(format!("{stamp}.json")));
        std::env::set_var("CODEX_HOME", &real);
        std::env::remove_var("AGENTPLUS_HOME");
        println!("roundtrip ok: {} sessions, {} files", ids.len(), copies.len());
    }

    /// Read-only against the real ~/.codex.
    #[test]
    #[ignore]
    fn dump_sessions_and_health() {
        let l = list().unwrap();
        println!("sessions={} current={} providers={:?} running={} writable={}", l.sessions.len(), l.current_provider, l.providers, l.codex_running, l.writable);
        let idx = rollout_index();
        let mapped = l.sessions.iter().filter(|s| idx.contains_key(&s.id)).count();
        println!("rollout index covers {mapped}/{} sessions", l.sessions.len());
        for h in health().unwrap() {
            println!("[{}] {}: {}", h.status, h.title, h.detail);
        }
        let c = cleanup_preview(3).unwrap();
        println!("cleanup: tmp {} ({}), logs {} rows {} old {} free {}, wal {}", c.tmp_count, c.tmp_bytes, c.logs_bytes, c.logs_rows, c.logs_old_rows, c.logs_free_bytes, c.wal_bytes);
    }
}
