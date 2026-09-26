//! UI language for text the backend produces (labels, notes, diff lines, errors).
//! The frontend sets it with `set_locale` before its first other call, and again
//! whenever the user switches language. English is the source language and comes first
//! in every call; Chinese stays the default until the frontend says otherwise.
//!
//! - `l("English", "中文")` → `&'static str`, for text without placeholders.
//! - `tr!("Not found: {id}", "找不到 {id}")` → `String`, `format!` rules (escape `{{ }}`).
//! - Static tables store `(en, zh)` pairs and call `l(x.0, x.1)` when read.

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

/// "en…" selects English; anything else Chinese.
pub fn set(lang: &str) {
    EN.store(lang.starts_with("en"), Ordering::Relaxed);
}

pub fn is_en() -> bool {
    #[cfg(test)]
    if let Some(en) = TEST_EN.with(|t| t.get()) {
        return en;
    }
    EN.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// Tests: the language of this thread only (other tests keep the Chinese default).
    static TEST_EN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Pick the text for the current language.
pub fn l(en: &'static str, zh: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

/// A list in running text: "a, b" / "a、b".
pub fn join<S: AsRef<str>>(items: &[S]) -> String {
    items.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(l(", ", "、"))
}

/// A switch's state: "On" / "开".
pub fn on_off(b: bool) -> &'static str {
    if b { l("On", "开") } else { l("Off", "关") }
}

/// `format!` in the current language: `tr!("Not found: {id}", "找不到 {id}")`.
macro_rules! tr {
    ($en:literal, $zh:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::is_en() { format!($en $(, $($arg)*)?) } else { format!($zh $(, $($arg)*)?) }
    };
}

/// `tr!` with a count, an English singular / plural and then Chinese:
/// `trn!(n, "{n} model", "{n} models", "{n} 个模型")`. Chinese has one form; English takes
/// the singular when the count is 1. Every text writes the count as `{n}`; other
/// arguments follow as in `format!`.
macro_rules! trn {
    ($n:expr, $one:literal, $many:literal, $zh:literal $(, $($arg:tt)*)?) => {
        match $n {
            n if !$crate::i18n::is_en() => format!($zh $(, $($arg)*)?, n = n),
            1 => format!($one $(, $($arg)*)?, n = 1),
            n => format!($many $(, $($arg)*)?, n = n),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{join, on_off, TEST_EN};

    #[test]
    fn picks_language() {
        let id = "x";
        assert_eq!(tr!("Not found: {id}", "找不到 {id}"), "找不到 x");
        assert_eq!(tr!("{} in total", "共 {} 个", 3), "共 3 个");
        assert_eq!(super::l("yes", "是"), "是");
    }

    #[test]
    fn plurals_joins_and_switches() {
        let who = "R";
        let text = |n: usize| trn!(n, "\"{}\" {n} model ({who})", "\"{}\" {n} models ({who})", "「{}」{n} 个模型（{who}）", "p");
        assert_eq!((text(1), text(2), text(0)), ("「p」1 个模型（R）".into(), "「p」2 个模型（R）".into(), "「p」0 个模型（R）".into()));
        assert_eq!((join(&["a", "b"]), join::<String>(&[]), on_off(true), on_off(false)), ("a、b".into(), String::new(), "开", "关"));
        TEST_EN.with(|t| t.set(Some(true)));
        assert_eq!((text(1), text(2), text(0)), ("\"p\" 1 model (R)".into(), "\"p\" 2 models (R)".into(), "\"p\" 0 models (R)".into()));
        assert_eq!((join(&["a".to_string(), "b".to_string()]), on_off(true)), ("a, b".into(), "On"));
        TEST_EN.with(|t| t.set(None));
    }

    /// CJK ideographs plus CJK and full-width punctuation (corner brackets, full-width commas…).
    fn is_cjk(c: char) -> bool {
        matches!(c, '\u{3000}'..='\u{303f}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}')
    }

    /// Where the English slot holds Chinese: the first string literal of an `l(` / `tr!(`
    /// call, or the first two of a `trn!(` call. Skips comments, strings and char literals
    /// while looking for calls. Returns (line, literal).
    fn english_slot_violations(src: &str) -> Vec<(usize, String)> {
        let s: Vec<char> = src.chars().collect();
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        // A string literal ("…", r"…", r#"…"#, b"…") starting at i: its end.
        let literal_end = |i: usize| -> Option<usize> {
            let mut j = i;
            if s.get(j) == Some(&'b') {
                j += 1;
            }
            if s.get(j) == Some(&'r') {
                j += 1;
                let hashes = s[j..].iter().take_while(|&&c| c == '#').count();
                j += hashes;
                if s.get(j) != Some(&'"') {
                    return None;
                }
                j += 1;
                while j < s.len() && !(s[j] == '"' && s[j + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes) {
                    j += 1;
                }
                return Some(j + 1 + hashes);
            }
            if s.get(j) != Some(&'"') {
                return None;
            }
            j += 1;
            while j < s.len() && s[j] != '"' {
                j += if s[j] == '\\' { 2 } else { 1 };
            }
            Some(j + 1)
        };
        // Comment, string or char literal at i: where it ends.
        let skip = |i: usize| -> Option<usize> {
            match (s[i], s.get(i + 1)) {
                ('/', Some('/')) => Some(s[i..].iter().position(|&c| c == '\n').map_or(s.len(), |p| i + p)),
                ('/', Some('*')) => {
                    let (mut j, mut depth) = (i, 0);
                    while j + 1 < s.len() {
                        match (s[j], s[j + 1]) {
                            ('/', '*') => (depth, j) = (depth + 1, j + 2),
                            ('*', '/') => {
                                (depth, j) = (depth - 1, j + 2);
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => j += 1,
                        }
                    }
                    Some(j)
                }
                ('\'', Some('\\')) => Some(i + 2 + s[i + 2..].iter().position(|&c| c == '\'').unwrap_or(0) + 1),
                ('\'', _) if s.get(i + 2) == Some(&'\'') => Some(i + 3),
                ('"' | 'b' | 'r', _) if i == 0 || !ident(s[i - 1]) => literal_end(i),
                _ => None,
            }
        };
        let mut out = vec![];
        let mut i = 0;
        while i < s.len() {
            if let Some(end) = skip(i) {
                i = end;
                continue;
            }
            if !ident(s[i]) || (i > 0 && (ident(s[i - 1]) || s[i - 1] == '.')) {
                i += 1;
                continue;
            }
            let start = i;
            while i < s.len() && ident(s[i]) {
                i += 1;
            }
            let word: String = s[start..i].iter().collect();
            let after_fn = s[..start].iter().rev().skip_while(|c| c.is_whitespace()).take(2).eq(['n', 'f'].iter());
            let (wanted, open) = match word.as_str() {
                "l" if s.get(i) == Some(&'(') && !after_fn => (1, i),
                "tr" if s.get(i) == Some(&'!') && s.get(i + 1) == Some(&'(') => (1, i + 1),
                "trn" if s.get(i) == Some(&'!') && s.get(i + 1) == Some(&'(') => (2, i + 1),
                _ => continue,
            };
            // The first `wanted` string literals inside the call's parentheses.
            let (mut j, mut depth, mut found) = (open + 1, 0, 0);
            while j < s.len() && found < wanted {
                if let Some(end) = literal_end(j).filter(|_| !ident(s[j - 1])) {
                    let text: String = s[j..end].iter().collect();
                    if text.chars().any(is_cjk) {
                        out.push((s[..j].iter().filter(|&&c| c == '\n').count() + 1, text));
                    }
                    (found, j) = (found + 1, end);
                    continue;
                }
                if let Some(end) = skip(j) {
                    j = end;
                    continue;
                }
                match s[j] {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' if depth == 0 => break,
                    ')' | ']' | '}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
        }
        out
    }

    /// English is the source language: it always comes first, in every file under `src/`.
    #[test]
    fn english_comes_first_everywhere() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = vec![];
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
        assert!(files.len() > 10, "{files:?}");
        let bad: Vec<String> = files
            .iter()
            .flat_map(|f| english_slot_violations(&std::fs::read_to_string(f).unwrap()).into_iter().map(move |(line, text)| format!("{}:{line}: {text}", f.display())))
            .collect();
        assert!(bad.is_empty(), "Chinese in the English (first) slot:\n{}", bad.join("\n"));
    }

    #[test]
    fn guard_catches_chinese_first() {
        let old = r##"
            // l("注释", "comment") and "l(\"字符串\", \"string\")" are not calls.
            let a = l("中文", "English");
            let b = crate::i18n::l(
                "多行",
                "Multi-line",
            );
            let c = tr!("找不到 {id}", "Not found: {id}");
            let d = trn!(n, "{n} model", "{n} 个模型", "{n} models");
            let e = tr!(r#"原始 "{x}""#, r#"raw "{x}""#);
            let f = x.fill('中').call("中文", "zh");
            let g = l("English \"quoted\"", "中文");
        "##;
        let lines: Vec<usize> = english_slot_violations(old).into_iter().map(|(line, _)| line).collect();
        assert_eq!(lines, [3, 5, 8, 9, 10]);
    }
}
