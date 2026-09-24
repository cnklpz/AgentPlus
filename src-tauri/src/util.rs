//! File helpers: format-preserving reads, backups and atomic writes.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Home of the target environment (Windows user, or a WSL distro over \wsl.localhost).
pub fn home() -> PathBuf {
    crate::env::home()
}

/// AgentPlus data always stays on the Windows side.
pub fn agentplus_dir() -> PathBuf {
    // Tests that run real flows point this at a temp dir so they don't leave backups behind.
    #[cfg(test)]
    if let Some(d) = std::env::var_os("AGENTPLUS_HOME") {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".agentplus")
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
    let tmp = path.with_extension(format!(
        "{}.agentplus-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    fs::write(&tmp, out.as_bytes()).with_context(|| tr!("写入 {} 失败", "Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| tr!("替换 {} 失败", "Failed to replace {}", path.display()))?;
    Ok(())
}

/// Copies each file into `~/.agentplus/backups/<time>/<agent>/` before a write,
/// with a `manifest.json` recording where each file came from (for rollback).
pub fn backup(agent: &str, files: &[PathBuf]) -> Result<PathBuf> {
    backup_tagged(agent, files, crate::i18n::l("应用配置", "Apply config"))
}

pub fn backup_tagged(agent: &str, files: &[PathBuf], reason: &str) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut dir = agentplus_dir().join("backups").join(&stamp).join(agent);
    // Two backups in the same second: add a suffix instead of overwriting.
    let mut n = 2;
    while dir.join("manifest.json").exists() {
        dir = agentplus_dir().join("backups").join(format!("{stamp}-{n}")).join(agent);
        n += 1;
    }
    fs::create_dir_all(&dir)?;
    let mut entries = vec![];
    for f in files {
        if f.exists() {
            let name = f.file_name().map(|n| n.to_owned()).unwrap_or_default();
            fs::copy(f, dir.join(&name)).with_context(|| tr!("备份 {} 失败", "Failed to back up {}", f.display()))?;
            entries.push(serde_json::json!({ "name": name.to_string_lossy(), "path": f.to_string_lossy() }));
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
        let m = n as f64 / 1_048_576.0;
        let m2 = n as f64 / 1_000_000.0;
        let v = if (m - m.round()).abs() < (m2 - m2.round()).abs() { m } else { m2 };
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
    let re = regex::Regex::new(r",(\s*[}\]])").unwrap();
    (re.replace_all(&out, "$1").to_string(), had)
}

#[cfg(test)]
mod tests {
    use super::*;

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

pub fn read_json(path: &Path) -> Result<(serde_json::Value, TextMeta)> {
    let (text, meta) = read_text(path)?;
    let v = serde_json::from_str(&text).with_context(|| tr!("解析 {} 失败", "Failed to parse {}", path.display()))?;
    Ok((v, meta))
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
