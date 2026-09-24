//! File helpers: format-preserving reads, backups and atomic writes.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Home of the target environment (Windows user, or a WSL distro over \wsl.localhost).
pub fn home() -> PathBuf {
    if let Some(h) = test_home() {
        return h;
    }
    crate::env::home()
}

/// AgentPlus data always stays on the Windows side.
pub fn agentplus_dir() -> PathBuf {
    // Tests keep the store and backups in their temp home; tests that run real flows point
    // this at a temp dir so they don't leave backups behind.
    #[cfg(test)]
    {
        if let Some(h) = test_home() {
            return h.join(".agentplus");
        }
        if let Some(d) = std::env::var_os("AGENTPLUS_HOME") {
            return PathBuf::from(d);
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".agentplus")
}

#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests: the temp folder standing in for the home folder on this thread (agent configs,
/// the AgentPlus store and backups all go inside it). Always None outside tests.
pub fn test_home() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_HOME.with(|t| t.borrow().clone())
    }
    #[cfg(not(test))]
    {
        None
    }
}

/// Tests: sets a fresh, empty temp home for this thread; dropping it restores the real one
/// and deletes the folder. The agent environment is empty meanwhile (`env::set_test_vars`).
#[cfg(test)]
pub struct TestHome(pub PathBuf);

#[cfg(test)]
impl TestHome {
    pub fn new(tag: &str) -> TestHome {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("agentplus-test-{tag}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        TEST_HOME.with(|t| *t.borrow_mut() = Some(d.clone()));
        crate::env::set_test_vars(&[]);
        TestHome(d)
    }
}

#[cfg(test)]
impl Drop for TestHome {
    fn drop(&mut self) {
        TEST_HOME.with(|t| *t.borrow_mut() = None);
        crate::env::set_test_vars(&[]);
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Shows a path with the home directory folded back to `~`.
pub fn display_path(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.to_string_lossy().replace('\\', "/")),
        Err(_) => p.to_string_lossy().to_string(),
    }
}

/// How the original file was encoded, so a rewrite keeps the same shape.
#[derive(Clone, Copy, Debug)]
pub struct TextMeta {
    pub crlf: bool,
    pub trailing_newline: bool,
    /// Indentation of the first indented line: tab, or a number of spaces.
    pub indent_tab: bool,
    pub indent_width: usize,
}

impl TextMeta {
    /// For a file that doesn't exist yet.
    pub const NEW: TextMeta = TextMeta { crlf: false, trailing_newline: true, indent_tab: false, indent_width: 2 };
}

/// A text file that may not exist yet: empty when it's missing, an error when it exists but
/// can't be read (not UTF-8, locked). A rewrite must never replace content it couldn't see.
pub fn read_text_or_new(path: &Path) -> Result<(String, TextMeta)> {
    match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), TextMeta::NEW)),
        _ => read_text(path),
    }
}

pub fn read_text(path: &Path) -> Result<(String, TextMeta)> {
    let bytes = fs::read(path).with_context(|| tr!("读取 {} 失败", "Failed to read {}", path.display()))?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if bom { &bytes[3..] } else { &bytes[..] };
    let text = String::from_utf8(body.to_vec()).with_context(|| tr!("{} 不是 UTF-8", "{} is not UTF-8", path.display()))?;
    let first_indent = text
        .lines()
        .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
        .find(|ws| !ws.is_empty())
        .unwrap_or("  ");
    let meta = TextMeta {
        crlf: text.contains("\r\n"),
        trailing_newline: text.ends_with('\n'),
        indent_tab: first_indent.starts_with('\t'),
        indent_width: first_indent.len().max(1),
    };
    Ok((text.replace("\r\n", "\n"), meta))
}

/// Writes `text` (LF line endings) back with the original encoding details.
/// A BOM is never re-added: several agents fail to parse BOM-prefixed JSON/TOML.
pub fn write_text_atomic(path: &Path, text: &str, meta: TextMeta) -> Result<()> {
    let mut out = text.trim_end_matches('\n').to_string();
    if meta.trailing_newline {
        out.push('\n');
    }
    if meta.crlf {
        out = out.replace('\n', "\r\n");
    }
    // A symlinked config (dotfile managers) is written through, so the link survives.
    let target = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path).with_context(|| tr!("找不到 {} 指向的文件", "Can't resolve the link {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    let tmp = target.with_extension(format!(
        "{}.agentplus-tmp",
        target.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    fs::write(&tmp, out.as_bytes()).with_context(|| tr!("写入 {} 失败", "Failed to write {}", tmp.display()))?;
    // Replacing can fail for a moment while another program (antivirus, an editor) holds the file.
    let mut last = None;
    for _ in 0..10 {
        match fs::rename(&tmp, &target) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = fs::remove_file(&tmp);
    Err(anyhow::anyhow!(tr!("替换 {} 失败：{}", "Failed to replace {}: {}", path.display(), last.map(|e| e.to_string()).unwrap_or_default())))
}

/// Copies each file into `~/.agentplus/backups/<time>/<agent>/` before a write,
/// with a `manifest.json` recording where each file came from (for rollback).
pub fn backup(agent: &str, files: &[PathBuf]) -> Result<PathBuf> {
    backup_tagged(agent, files, crate::i18n::l("应用配置", "Apply config"))
}

pub fn backup_tagged(agent: &str, files: &[PathBuf], reason: &str) -> Result<PathBuf> {
    backup_in(&agentplus_dir().join("backups"), agent, files, reason)
}

/// A new, empty `~/.agentplus/backups/<stamp>/<agent>/` folder.
pub fn new_backup_dir(agent: &str) -> Result<PathBuf> {
    new_dir_in(&agentplus_dir().join("backups"), agent)
}

/// `<root>/<stamp>/<agent>`, created here. Two backups in the same second (or on two
/// threads at once) get `<stamp>-2`, `<stamp>-3`… instead of sharing a folder.
fn new_dir_in(root: &Path, agent: &str) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    for n in 1..1000 {
        let dir = root.join(if n == 1 { stamp.clone() } else { format!("{stamp}-{n}") }).join(agent);
        fs::create_dir_all(dir.parent().unwrap())?;
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| tr!("创建 {} 失败", "Failed to create {}", dir.display())),
        }
    }
    Err(anyhow::anyhow!(tr!("创建备份目录失败：{} 下同一秒的备份太多", "Failed to create a backup folder: too many backups in one second under {}", root.display())))
}

fn backup_in(root: &Path, agent: &str, files: &[PathBuf], reason: &str) -> Result<PathBuf> {
    let dir = new_dir_in(root, agent)?;
    let mut entries = vec![];
    let mut used: Vec<String> = vec!["manifest.json".into()];
    for f in files {
        if f.exists() {
            let base = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            // Several files can share a name (one models.json per OpenClaw agent): number the rest.
            let name = (1..).map(|n| if n == 1 { base.clone() } else { format!("{n}-{base}") }).find(|n| !used.contains(n)).unwrap();
            fs::copy(f, dir.join(&name)).with_context(|| tr!("备份 {} 失败", "Failed to back up {}", f.display()))?;
            entries.push(serde_json::json!({ "name": name, "path": f.to_string_lossy() }));
            used.push(name);
        }
    }
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "agent": agent, "reason": reason, "time": chrono::Local::now().to_rfc3339(), "files": entries }))?,
    )?;
    Ok(dir)
}

/// 272000 -> "272K", 1048576 -> "1M".
pub fn fmt_ctx(n: u64) -> String {
    if n >= 1_000_000 {
        // Binary megabytes only when they come out whole (1048576 → 1M); 1500000 is 1.5M, not 1.4M.
        let m = n as f64 / 1_048_576.0;
        let v = if (m - m.round()).abs() < 0.01 { m } else { n as f64 / 1_000_000.0 };
        if (v - v.round()).abs() < 0.05 { format!("{}M", v.round()) } else { format!("{:.1}M", v) }
    } else if n >= 1000 {
        format!("{}K", n / 1000)
    } else {
        n.to_string()
    }
}

/// "https://host:8080/v1" -> "host:8080/v1"
pub fn host_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let mut s = u.host_str().unwrap_or("").to_string();
            if let Some(p) = u.port() {
                s.push_str(&format!(":{p}"));
            }
            let path = u.path().trim_end_matches('/');
            if !path.is_empty() {
                s.push_str(path);
            }
            s
        }
        Err(_) => url.to_string(),
    }
}

/// A base URL in comparable form: trimmed, without trailing slashes, lowercase.
pub fn norm_url(u: &str) -> String {
    u.trim().trim_end_matches('/').to_lowercase()
}

/// `v[k]` as an owned string; empty when it is missing or not a string.
pub fn str_field(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

/// The strings of a JSON array (other items skipped); None when `v` is not an array.
pub fn str_list(v: Option<&serde_json::Value>) -> Option<Vec<String>> {
    v.and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
}

/// Strips `//` and `/* */` comments outside strings, plus trailing commas.
/// Returns (clean json, had_comments).
pub fn strip_jsonc(src: &str) -> (String, bool) {
    let mut out = String::with_capacity(src.len());
    let chars: Vec<char> = src.chars().collect();
    let (mut i, mut in_str, mut had) = (0, false, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            had = true;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            had = true;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    (drop_trailing_commas(&out), had)
}

/// `[1, 2,]` → `[1, 2]`, leaving string contents (`"a,]"`) alone. Runs after comments are gone.
fn drop_trailing_commas(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            in_str = c != '"';
        } else if c == ',' && matches!(chars[i + 1..].iter().find(|x| !x.is_whitespace()), Some('}' | ']')) {
            // dropped
        } else {
            in_str = c == '"';
            out.push(c);
        }
        i += 1;
    }
    out
}

/// One folder or file name coming from the UI (a backup stamp, an agent id): no separators,
/// drive prefix or `..`, so joining it onto a folder can't leave that folder.
pub fn plain_name(s: &str) -> bool {
    !s.trim().is_empty() && s != "." && s != ".." && !s.contains(['/', '\\', ':', '\0'])
}

/// JSON Pointer from path segments, escaped per RFC 6901: ids like "anthropic/claude-sonnet-4"
/// stay one segment.
pub fn jptr(parts: &[&str]) -> String {
    parts.iter().map(|p| format!("/{}", p.replace('~', "~0").replace('/', "~1"))).collect()
}

pub fn read_json(path: &Path) -> Result<(serde_json::Value, TextMeta)> {
    let (text, meta) = read_text(path)?;
    let v = serde_json::from_str(&text).with_context(|| tr!("解析 {} 失败", "Failed to parse {}", path.display()))?;
    Ok((v, meta))
}

/// `read_json` for a config that gets edited by key: the top level must be an object
/// (indexing into `[]` or a string would panic, and replacing it would lose the file).
pub fn read_json_object(path: &Path) -> Result<(serde_json::Value, TextMeta)> {
    let (v, meta) = read_json(path)?;
    if !v.is_object() {
        anyhow::bail!("{}", tr!("{} 的顶层不是 JSON 对象，不修改它", "The top level of {} is not a JSON object; leaving it alone", display_path(path)));
    }
    Ok((v, meta))
}

/// The object at `path` inside `v`, creating missing (or null) levels. Anything else in the
/// way is an error rather than being overwritten.
pub fn obj_at<'a>(v: &'a mut serde_json::Value, path: &[&str]) -> Result<&'a mut serde_json::Map<String, serde_json::Value>> {
    let mut cur = v;
    for (i, key) in std::iter::once(&"").chain(path).enumerate() {
        if i > 0 {
            cur = cur.as_object_mut().unwrap().entry(key.to_string()).or_insert(serde_json::Value::Null);
        }
        if cur.is_null() {
            *cur = serde_json::json!({});
        }
        if !cur.is_object() {
            let at = if i == 0 { crate::i18n::l("（顶层）", "(top level)").to_string() } else { path[..i].join(".") };
            anyhow::bail!("{}", tr!("{at} 不是对象", "{at} is not an object"));
        }
    }
    Ok(cur.as_object_mut().unwrap())
}

/// Serializes with the original file's indentation so unchanged parts stay byte-identical.
pub fn write_json(path: &Path, v: &serde_json::Value, meta: TextMeta) -> Result<()> {
    use serde::Serialize;
    let indent = if meta.indent_tab { "\t".to_string() } else { " ".repeat(meta.indent_width) };
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    v.serialize(&mut ser)?;
    write_text_atomic(path, &String::from_utf8(buf)?, meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentplus-util-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn jptr_escapes_segments() {
        assert_eq!(jptr(&["provider", "anthropic/claude-sonnet-4", "models"]), "/provider/anthropic~1claude-sonnet-4/models");
        assert_eq!(jptr(&["a~b", "c/d~1"]), "/a~0b/c~1d~01");
        assert_eq!(jptr(&[""]), "/");
        assert_eq!(jptr(&[]), "");
        let v = serde_json::json!({ "p": { "a/b~c": 1 } });
        assert_eq!(v.pointer(&jptr(&["p", "a/b~c"])), Some(&serde_json::json!(1)));
    }

    #[test]
    fn obj_at_creates_or_refuses() {
        let mut v = serde_json::json!({ "a": null, "s": "x" });
        obj_at(&mut v, &["a", "b"]).unwrap().insert("c".into(), serde_json::json!(1));
        assert_eq!(v["a"]["b"]["c"], 1);
        assert!(obj_at(&mut v, &["s"]).unwrap_err().to_string().contains("s 不是对象"));
        assert!(obj_at(&mut v, &["s", "deeper"]).is_err());
        assert_eq!(v["s"], "x");
        assert!(obj_at(&mut serde_json::json!([1]), &[]).is_err());
        assert!(obj_at(&mut serde_json::json!(null), &[]).is_ok());
    }

    #[test]
    fn json_object_roots_only() {
        let d = tmp("root");
        for (name, body, ok) in [("o.json", "{}", true), ("a.json", "[]", false), ("s.json", "\"x\"", false), ("n.json", "null", false), ("bad.json", "{", false)] {
            fs::write(d.join(name), body).unwrap();
            assert_eq!(read_json_object(&d.join(name)).is_ok(), ok, "{name}");
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn plain_names() {
        for ok in ["20250101-120000", "codex", "codex-repair", "a b", "设置", "x.y"] {
            assert!(plain_name(ok), "{ok}");
        }
        for bad in ["", " ", ".", "..", "a/b", r"a\b", "C:", r"C:\x", r"\srv\s", "/etc", "a\0b"] {
            assert!(!plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn strip_jsonc_keeps_strings() {
        let (clean, had) = strip_jsonc(r#"{"a":"x,]","b":[1,],"c":"// not a comment","d":"/* nor this */",}"#);
        assert!(!had);
        let v: serde_json::Value = serde_json::from_str(&clean).unwrap();
        assert_eq!(v["a"], "x,]");
        assert_eq!(v["b"], serde_json::json!([1]));
        assert_eq!(v["c"], "// not a comment");
        assert_eq!(v["d"], "/* nor this */");

        let (clean, had) = strip_jsonc("{\n  // note\n  \"a\": \"q\\\"//x\", /* c */\n  \"b\": [1, 2 ,\n  ],\n}\n");
        assert!(had);
        let v: serde_json::Value = serde_json::from_str(&clean).unwrap();
        assert_eq!((v["a"].as_str(), v["b"].clone()), (Some("q\"//x"), serde_json::json!([1, 2])));
        // Unterminated block comment and a lone slash at the end don't panic.
        assert!(strip_jsonc("{} /* open").1);
        assert_eq!(strip_jsonc("/").0, "/");
        assert_eq!(strip_jsonc("").0, "");
    }

    #[test]
    fn text_roundtrip_keeps_shape() {
        let d = tmp("text");
        for (name, src) in [
            ("crlf.json", "{\r\n\t\"a\": 1\r\n}\r\n"),
            ("lf.json", "{\n    \"a\": 1\n}"),
            ("bom.json", "\u{feff}{\n  \"a\": 1\n}\n"),
            ("empty.txt", ""),
        ] {
            let p = d.join(name);
            fs::write(&p, src).unwrap();
            let (text, meta) = read_text(&p).unwrap();
            assert!(!text.contains('\r') && !text.starts_with('\u{feff}'));
            write_text_atomic(&p, &text, meta).unwrap();
            assert_eq!(fs::read_to_string(&p).unwrap(), src.trim_start_matches('\u{feff}'), "{name}");
        }
        let (_, meta) = read_text(&d.join("crlf.json")).unwrap();
        assert!(meta.indent_tab && meta.crlf && meta.trailing_newline);
        let (_, meta) = read_text(&d.join("lf.json")).unwrap();
        assert_eq!((meta.indent_tab, meta.indent_width, meta.trailing_newline), (false, 4, false));
        // Not UTF-8 (UTF-16 from PowerShell 5, GBK from Notepad): an error, never a rewrite.
        fs::write(d.join("utf16.env"), [0xFF, 0xFE, b'A', 0, b'=', 0, b'1', 0]).unwrap();
        assert!(read_text(&d.join("utf16.env")).is_err());
        assert!(read_text(&d.join("missing.json")).is_err());
        // No temp file is left behind.
        assert!(fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("agentplus-tmp")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn write_json_keeps_indent_and_order() {
        let d = tmp("json");
        let p = d.join("a.json");
        fs::write(&p, "{\n\t\"z\": 1,\n\t\"a\": [1, 2]\n}\n").unwrap();
        let (mut v, meta) = read_json(&p).unwrap();
        v["m"] = serde_json::json!("x");
        write_json(&p, &v, meta).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\n\t\"z\": 1,\n\t\"a\": [\n\t\t1,\n\t\t2\n\t],\n\t\"m\": \"x\"\n}\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn backups_keep_files_that_share_a_name() {
        let d = tmp("backup");
        let (a, b) = (d.join("a").join("models.json"), d.join("b").join("models.json"));
        for (p, body) in [(&a, "A"), (&b, "B")] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        let root = d.join("backups");
        let gone = d.join("gone.json");
        let first = backup_in(&root, "openclaw", &[a.clone(), b.clone(), gone], "t").unwrap();
        let m: serde_json::Value = serde_json::from_str(&fs::read_to_string(first.join("manifest.json")).unwrap()).unwrap();
        let files = m["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "missing files are skipped");
        for (f, body, from) in [(&files[0], "A", &a), (&files[1], "B", &b)] {
            assert_eq!(fs::read_to_string(first.join(f["name"].as_str().unwrap())).unwrap(), body);
            assert_eq!(f["path"].as_str().unwrap(), from.to_string_lossy());
        }
        assert_ne!(files[0]["name"], files[1]["name"]);
        // A second backup in the same second gets its own folder.
        let second = backup_in(&root, "openclaw", &[a], "t").unwrap();
        assert_ne!(first, second);
        assert!(first.join("manifest.json").exists() && second.join("manifest.json").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn host_and_ctx_formatting() {
        assert_eq!(host_of("https://host:8080/v1/"), "host:8080/v1");
        assert_eq!(host_of("https://host"), "host");
        assert_eq!(host_of("not a url"), "not a url");
        assert_eq!(fmt_ctx(999), "999");
        assert_eq!(fmt_ctx(999_999), "999K");
        assert_eq!(fmt_ctx(1_048_576), "1M");
        assert_eq!(fmt_ctx(1_500_000), "1.5M");
        assert!(fmt_ctx(u64::MAX).ends_with('M'));
    }

    /// Reads real agent configs and rewrites them unchanged into a temp dir;
    /// the output must be byte-identical. The originals are only read.
    #[test]
    #[ignore]
    fn roundtrip_real_files() {
        let tmp = std::env::temp_dir().join("agentplus-roundtrip");
        fs::create_dir_all(&tmp).unwrap();
        let files = [
            ".zcode/v2/provider_config.json",
            ".zcode/v2/setting.json",
            ".config/mimocode/mimocode.jsonc",
            "AppData/Roaming/Xiaomi MiMo/preferences.json",
            ".codex/models.json",
        ];
        for f in files {
            let src = home().join(f);
            if !src.exists() {
                continue;
            }
            let (v, meta) = read_json(&src).unwrap();
            let dst = tmp.join(src.file_name().unwrap());
            write_json(&dst, &v, meta).unwrap();
            assert_eq!(fs::read(&src).unwrap(), fs::read(&dst).unwrap(), "{f} changed on rewrite");
            println!("identical: {f}");
        }
        let (text, meta) = read_text(&home().join(".codex/config.toml")).unwrap();
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        let dst = tmp.join("config.toml");
        write_text_atomic(&dst, &doc.to_string(), meta).unwrap();
        assert_eq!(fs::read(home().join(".codex/config.toml")).unwrap(), fs::read(&dst).unwrap());
        println!("identical: .codex/config.toml");
    }
}
