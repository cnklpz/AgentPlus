//! `.env` files (`KEY=VALUE` lines) as the agents' dotenv loaders read them: `#` comment
//! lines, an optional `export `, quoted values, and ` #` inline comments after unquoted ones.
//! Edits touch only the line of the key; every other line stays as it was.

use crate::util::{read_text_or_new, TextMeta};
use std::path::Path;

/// The key a line assigns; None for comments, blank lines and anything without `=`.
pub fn line_key(l: &str) -> Option<&str> {
    let t = l.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let t = t.strip_prefix("export ").unwrap_or(t);
    let (k, _) = t.split_once('=')?;
    Some(k.trim())
}

/// The value part of an assignment (after `=`): a quoted value runs to its closing quote
/// (`\"` and `\\` are unescaped inside double quotes, whatever follows is ignored); an
/// unquoted one ends at an inline ` #` comment.
pub fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let mut out = String::new();
        let mut chars = v[1..].chars();
        while let Some(c) = chars.next() {
            if c == q {
                return out;
            }
            match (q, c, chars.clone().next()) {
                ('"', '\\', Some(n @ ('"' | '\\'))) => {
                    out.push(n);
                    chars.next();
                }
                _ => out.push(c),
            }
        }
        // No closing quote: read it like an unquoted value.
    }
    v.split(" #").next().unwrap_or(v).trim().to_string()
}

fn values<'a>(text: &'a str, key: &'a str) -> impl DoubleEndedIterator<Item = String> + 'a {
    text.lines().filter(move |l| line_key(l) == Some(key)).filter_map(|l| l.split_once('=')).map(|(_, v)| unquote(v))
}

/// The value of `key`; the last assignment wins (python-dotenv, node dotenv). An empty
/// value is None.
pub fn get(text: &str, key: &str) -> Option<String> {
    values(text, key).next_back().filter(|v| !v.is_empty())
}

/// The value of the first assignment of `key`, which may be empty. Codex keeps this
/// first-wins reading (see `adapters::codex::env_value`).
pub fn get_first(text: &str, key: &str) -> Option<String> {
    values(text, key).next()
}

/// Sets `key` (or removes it, value None), keeping every other line. The first assignment
/// is replaced and later duplicates are dropped; a new key goes at the end.
pub fn set(text: &str, key: &str, value: Option<&str>) -> String {
    let needs_q = |v: &str| v.chars().any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'');
    let line = value.map(|v| if needs_q(v) { format!("{key}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) } else { format!("{key}={v}") });
    let mut out: Vec<String> = vec![];
    let mut done = false;
    for l in text.split('\n') {
        if line_key(l) == Some(key) {
            if let (Some(n), false) = (&line, done) {
                out.push(n.clone());
            }
            done = true;
            continue;
        }
        out.push(l.to_string());
    }
    if let (Some(n), false) = (line, done) {
        while out.last().map(|l| l.is_empty()).unwrap_or(false) {
            out.pop();
        }
        out.push(n);
        out.push(String::new());
    }
    out.join("\n")
}

/// A .env file for reading: empty when it is missing or can't be read (writes use
/// `read_text_or_new`, which refuses a file it can't read).
pub fn load(path: &Path) -> (String, TextMeta) {
    read_text_or_new(path).unwrap_or((String::new(), TextMeta::NEW))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_like_dotenv() {
        let text = "# K=commented\nexport K = one\nOTHER=x\n  K=\"two\" # note\nE=\n";
        assert_eq!(get(text, "K").as_deref(), Some("two"), "last assignment wins");
        assert_eq!(get_first(text, "K").as_deref(), Some("one"));
        assert_eq!(get(text, "OTHER").as_deref(), Some("x"));
        assert_eq!(get(text, "E"), None, "empty is unset");
        assert_eq!(get_first(text, "E").as_deref(), Some(""));
        assert_eq!(get(text, "MISSING"), None);
        assert_eq!(line_key("# K=v"), None);
        assert_eq!(line_key("K"), None);
        assert_eq!(line_key(" export A_B= 1"), Some("A_B"));
    }

    #[test]
    fn unquotes_values() {
        assert_eq!(unquote(" plain # comment"), "plain");
        assert_eq!(unquote("x#y"), "x#y", "# without a space before it is part of the value");
        assert_eq!(unquote("\"v\" # c"), "v");
        assert_eq!(unquote("'a \\\" b'"), "a \\\" b", "no escapes in single quotes");
        assert_eq!(unquote(r#""a\"b\\c\n""#), "a\"b\\c\\n", "only \\\" and \\\\ are unescaped");
        assert_eq!(unquote("\"open"), "\"open", "an unclosed quote reads as plain text");
        assert_eq!(unquote("''"), "");
        assert_eq!(get("K=\"v\" # c", "K").as_deref(), Some("v"));
    }

    #[test]
    fn set_then_get_roundtrips() {
        for v in ["a\"b", "a b\\c", "x#y", "it's", "plain", "sk-abc_123", "\\"] {
            let text = set("", "K", Some(v));
            assert_eq!(get(&text, "K").as_deref(), Some(v), "{text:?}");
            let text = set("# head\nK=old\nZ=1\n", "K", Some(v));
            assert_eq!(get(&text, "K").as_deref(), Some(v), "{text:?}");
        }
    }

    #[test]
    fn set_keeps_other_lines() {
        assert_eq!(set("", "K", Some("v")), "K=v\n");
        assert_eq!(set("A=1\n\n\n", "K", Some("v")), "A=1\nK=v\n");
        assert_eq!(set("# c\nK=old\nA=1\nK=dup\n", "K", Some("new")), "# c\nK=new\nA=1\n");
        assert_eq!(set("# c\nexport K=old\nA=1\n", "K", None), "# c\nA=1\n");
        assert_eq!(set("A=1\n", "K", None), "A=1\n");
        assert_eq!(set("A=1", "K", Some("a b")), "A=1\nK=\"a b\"\n");
        assert_eq!(set("# K=keep\n", "K", Some("v")), "# K=keep\nK=v\n");
    }
}
