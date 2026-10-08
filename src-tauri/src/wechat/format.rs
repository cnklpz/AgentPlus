//! Text the bridge sends to WeChat: session tags, long replies split to fit, turn summaries.

use super::ilink::TEXT_LIMIT;
use std::collections::BTreeSet;

/// Longest session title shown in a tag.
const TITLE_CHARS: usize = 16;

pub fn short_title(title: &str) -> String {
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= TITLE_CHARS {
        t
    } else {
        format!("{}…", t.chars().take(TITLE_CHARS - 1).collect::<String>())
    }
}

/// `【#3 title】`, which also lets a quoted reply find its session again.
pub fn tag(no: u32, title: &str) -> String {
    let t = short_title(title);
    if t.is_empty() { format!("【#{no}】") } else { format!("【#{no} {t}】") }
}

/// The last folder of a path, for lists.
pub fn folder_name(path: &str) -> String {
    let p = path.trim_end_matches(['/', '\\']);
    p.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(p).to_string()
}

/// Splits `text` into pieces of at most `limit` characters, at a line break when one is
/// in the second half of the piece.
pub fn split(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + limit).min(chars.len());
        if end < chars.len() {
            if let Some(nl) = chars[start + limit / 2..end].iter().rposition(|&c| c == '\n') {
                end = start + limit / 2 + nl + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim_end_matches('\n').to_string();
        if !piece.trim().is_empty() {
            out.push(piece);
        }
        start = end;
    }
    out
}

/// A message from a session, tagged and split to fit; parts are numbered `(1/3)`.
pub fn tagged(no: u32, title: &str, body: &str) -> Vec<String> {
    let head = tag(no, title);
    // Room for the tag, a line break and "(10/10) ".
    let room = TEXT_LIMIT - head.chars().count() - 12;
    let parts = split(body, room);
    let n = parts.len();
    match n {
        0 => vec![head],
        1 => vec![format!("{head}\n{}", parts[0])],
        _ => parts.into_iter().enumerate().map(|(i, p)| format!("{head} ({}/{n})\n{p}", i + 1)).collect(),
    }
}

/// Plain bot text (not from a session), split to fit.
pub fn plain(body: &str) -> Vec<String> {
    split(body, TEXT_LIMIT)
}

pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => tr!("{s}s", "{s} 秒"),
        60..=3599 => tr!("{}m {}s", "{} 分 {} 秒", s / 60, s % 60),
        _ => tr!("{}h {}m", "{} 小时 {} 分", s / 3600, s % 3600 / 60),
    }
}

/// "5 min ago" for a Unix time in seconds.
pub fn ago(then: i64, now: i64) -> String {
    let d = (now - then).max(0);
    match d {
        0..=59 => crate::i18n::l("just now", "刚刚").to_string(),
        60..=3599 => trn!(d / 60, "{n} min ago", "{n} min ago", "{n} 分钟前"),
        3600..=86399 => trn!(d / 3600, "{n} hour ago", "{n} hours ago", "{n} 小时前"),
        _ => trn!(d / 86400, "{n} day ago", "{n} days ago", "{n} 天前"),
    }
}

/// What a turn did besides answering.
#[derive(Default, Debug, Clone)]
pub struct TurnStats {
    pub commands: usize,
    pub files: BTreeSet<String>,
    pub tools: usize,
    pub duration_ms: Option<u64>,
}

/// "3 commands · 2 files changed · 1m 20s", or None when there's nothing to say.
pub fn summary(s: &TurnStats) -> Option<String> {
    let mut parts = vec![];
    if s.commands > 0 {
        parts.push(trn!(s.commands, "{n} command", "{n} commands", "执行 {n} 条命令"));
    }
    if !s.files.is_empty() {
        parts.push(trn!(s.files.len(), "{n} file changed", "{n} files changed", "改动 {n} 个文件"));
    }
    if s.tools > 0 {
        parts.push(trn!(s.tools, "{n} tool call", "{n} tool calls", "调用 {n} 次工具"));
    }
    if let Some(ms) = s.duration_ms.filter(|ms| *ms >= 1000) {
        parts.push(duration(ms));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_titles() {
        assert_eq!(tag(3, "Fix updater"), "【#3 Fix updater】");
        assert_eq!(tag(3, "  "), "【#3】");
        assert_eq!(short_title("a  b\nc"), "a b c");
        let long = "一二三四五六七八九十一二三四五六七八";
        assert_eq!(short_title(long).chars().count(), TITLE_CHARS);
        assert!(short_title(long).ends_with('…'));
        assert_eq!(super::super::command::tag_in(&tag(12, "x")), Some(12));
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name(r"D:\xm\AgentPlus"), "AgentPlus");
        assert_eq!(folder_name("/home/me/blog/"), "blog");
        assert_eq!(folder_name("/"), "");
        assert_eq!(folder_name(r"C:\"), "C:");
    }

    #[test]
    fn splits_at_line_breaks() {
        assert_eq!(split("", 10), Vec::<String>::new());
        assert_eq!(split("short", 10), vec!["short"]);
        assert_eq!(split("aaaaaa\nbbbbbb", 10), vec!["aaaaaa", "bbbbbb"]);
        // No line break in the second half: cut at the limit.
        assert_eq!(split("abcdefghijkl", 5), vec!["abcde", "fghij", "kl"]);
        // Counts characters, not bytes.
        assert_eq!(split("一二三四五六", 3), vec!["一二三", "四五六"]);
        let text = "line\n".repeat(2000);
        let parts = split(&text, 4000);
        assert!(parts.iter().all(|p| p.chars().count() <= 4000));
        assert_eq!(parts.concat().matches("line").count(), 2000);
    }

    #[test]
    fn tags_every_part() {
        assert_eq!(tagged(1, "t", "hi"), vec!["【#1 t】\nhi"]);
        assert_eq!(tagged(1, "t", ""), vec!["【#1 t】"]);
        let parts = tagged(2, "t", &"x".repeat(9000));
        assert_eq!(parts.len(), 3);
        assert!(parts[0].starts_with("【#2 t】 (1/3)\n"));
        assert!(parts.iter().all(|p| p.chars().count() <= TEXT_LIMIT));
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(&TurnStats::default()), None);
        let s = TurnStats { commands: 3, files: ["a".into(), "b".into()].into(), tools: 0, duration_ms: Some(80_000) };
        assert_eq!(summary(&s).unwrap(), "执行 3 条命令 · 改动 2 个文件 · 1 分 20 秒");
        assert_eq!(summary(&TurnStats { duration_ms: Some(500), ..Default::default() }), None);
        assert_eq!(ago(100, 100), "刚刚");
        assert_eq!(ago(0, 7200), "2 小时前");
    }
}
