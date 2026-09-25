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

/// The escapes undone inside double quotes by most loaders: `\"` and `\\`.
const PLAIN_ESCAPES: &[char] = &['"', '\\'];
/// The escapes dotenvy (Codex's .env loader) undoes inside double quotes, `\n` aside.
const DOTENVY_ESCAPES: &[char] = &['"', '\\', '\'', '$', ' '];

/// The value part of an assignment (after `=`): a quoted value runs to its closing quote
/// (`\c` is unescaped inside double quotes for each `c` in `escapes`, whatever follows the
/// closing quote is ignored); an unquoted one ends at an inline ` #` comment.
fn unquote(v: &str, escapes: &[char]) -> String {
    let v = v.trim();
    if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let mut out = String::new();
        let mut chars = v[1..].chars();
        while let Some(c) = chars.next() {
            if c == q {
                return out;
            }
            match (q, c, chars.clone().next()) {
                ('"', '\\', Some(n)) if escapes.contains(&n) => {
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

fn values<'a>(text: &'a str, key: &'a str, escapes: &'a [char]) -> impl DoubleEndedIterator<Item = String> + 'a {
    text.lines().filter(move |l| line_key(l) == Some(key)).filter_map(|l| l.split_once('=')).map(move |(_, v)| unquote(v, escapes))
}

/// The value of `key`; the last assignment wins (python-dotenv, node dotenv). An empty
/// value is None.
pub fn get(text: &str, key: &str) -> Option<String> {
    values(text, key, PLAIN_ESCAPES).next_back().filter(|v| !v.is_empty())
}

/// The value of the last assignment of `key`, which may be empty: Codex sets every pair of
/// its .env in order, so the last one wins and an empty `KEY=` still shadows the process
/// environment (see `adapters::codex::env_value`). Double-quoted values are unescaped as
/// dotenvy (Codex's loader) does, so `\$` reads as `$` (see `set_dotenvy`).
pub fn get_last(text: &str, key: &str) -> Option<String> {
    values(text, key, DOTENVY_ESCAPES).next_back()
}

/// Sets `key` (or removes it, value None), keeping every other line. The first assignment
/// is replaced and later duplicates are dropped; a new key goes at the end. A value with
/// whitespace, `#` or a quote is double-quoted, with `\` and `"` escaped.
pub fn set(text: &str, key: &str, value: Option<&str>) -> String {
    let needs_q = |v: &str| v.chars().any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'');
    let line = value.map(|v| if needs_q(v) { format!("{key}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) } else { format!("{key}={v}") });
    replace_line(text, key, line)
}

/// `set` for a file read by dotenvy (Codex), which also substitutes `$VAR` / `${VAR}` in
/// unquoted and double-quoted values and reads `\` as an escape outside single quotes. A
/// value with `$` or `\` is single-quoted (read literally) when it has no `'`, else
/// double-quoted with `\`, `"` and `$` escaped; any other value is written as `set` does.
pub fn set_dotenvy(text: &str, key: &str, value: &str) -> String {
    if !value.contains(['$', '\\']) {
        return set(text, key, Some(value));
    }
    let line = if value.contains('\'') {
        format!("{key}=\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$"))
    } else {
        format!("{key}='{value}'")
    };
    replace_line(text, key, Some(line))
}

/// Puts `line` in place of the first assignment of `key` (None removes it) and drops the
/// later ones; a new line goes at the end.
fn replace_line(text: &str, key: &str, line: Option<String>) -> String {
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
        assert_eq!(get_last(text, "K").as_deref(), Some("two"));
        assert_eq!(get(text, "OTHER").as_deref(), Some("x"));
        assert_eq!(get(text, "E"), None, "empty is unset");
        assert_eq!(get_last(text, "E").as_deref(), Some(""));
        assert_eq!(get_last("E=x\nE=\n", "E").as_deref(), Some(""), "a later empty assignment wins");
        assert_eq!(get_last(text, "MISSING"), None);
        assert_eq!(get(text, "MISSING"), None);
        assert_eq!(line_key("# K=v"), None);
        assert_eq!(line_key("K"), None);
        assert_eq!(line_key(" export A_B= 1"), Some("A_B"));
    }

    #[test]
    fn unquotes_values() {
        assert_eq!(unquote(" plain # comment", PLAIN_ESCAPES), "plain");
        assert_eq!(unquote("x#y", PLAIN_ESCAPES), "x#y", "# without a space before it is part of the value");
        assert_eq!(unquote("\"v\" # c", PLAIN_ESCAPES), "v");
        assert_eq!(unquote("'a \\\" b'", PLAIN_ESCAPES), "a \\\" b", "no escapes in single quotes");
        assert_eq!(unquote(r#""a\"b\\c\n""#, PLAIN_ESCAPES), "a\"b\\c\\n", "only \\\" and \\\\ are unescaped");
        assert_eq!(unquote("\"open", PLAIN_ESCAPES), "\"open", "an unclosed quote reads as plain text");
        assert_eq!(unquote("''", PLAIN_ESCAPES), "");
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

    /// The value dotenvy 0.15.7 (Codex's loader) reads from the part after `=`, following its
    /// `parse_value`; None where it fails or would substitute a `$` variable.
    fn dotenvy_value(input: &str) -> Option<String> {
        let (mut strong, mut weak, mut escaped, mut end) = (false, false, false, false);
        let mut out = String::new();
        for c in input.trim().chars() {
            if end {
                match c {
                    ' ' | '\t' => continue,
                    '#' => break,
                    _ => return None,
                }
            } else if escaped {
                match c {
                    '\\' | '\'' | '"' | '$' | ' ' => out.push(c),
                    'n' => out.push('\n'),
                    _ => return None,
                }
                escaped = false;
            } else if strong {
                if c == '\'' {
                    strong = false;
                } else {
                    out.push(c);
                }
            } else if c == '$' {
                return None;
            } else if weak {
                match c {
                    '"' => weak = false,
                    '\\' => escaped = true,
                    _ => out.push(c),
                }
            } else {
                match c {
                    '\'' => strong = true,
                    '"' => weak = true,
                    '\\' => escaped = true,
                    ' ' | '\t' => end = true,
                    _ => out.push(c),
                }
            }
        }
        (!strong && !weak).then_some(out)
    }

    #[test]
    fn set_dotenvy_reads_back_in_dotenvy_and_here() {
        let cases = ["plain", "sk-abc_123", "a b", "x#y", "it's", "a\"b", "sk-$abc", "${HOME}x", "a\\b", "\\", "a\\nb", "it's $5", "o'k\\x", "a \"$b\" c"];
        for v in cases {
            for base in ["", "# head\nK=old\nZ=1\nK=dup\n"] {
                let text = set_dotenvy(base, "K", v);
                let line = text.lines().find(|l| line_key(l) == Some("K")).unwrap();
                assert_eq!(dotenvy_value(line.split_once('=').unwrap().1).as_deref(), Some(v), "dotenvy: {line:?}");
                assert_eq!(get_last(&text, "K").as_deref(), Some(v), "{text:?}");
                assert_eq!(text.lines().filter(|l| line_key(l) == Some("K")).count(), 1);
            }
        }
        // Values without `$` or `\` are written exactly as `set` writes them.
        for v in ["plain", "a b", "x#y", "it's", "a\"b"] {
            assert_eq!(set_dotenvy("A=1\n", "K", v), set("A=1\n", "K", Some(v)));
        }
        assert_eq!(set_dotenvy("", "K", "sk-$x\\y"), "K='sk-$x\\y'\n");
        assert_eq!(set_dotenvy("", "K", "it's $5"), "K=\"it's \\$5\"\n");
        // The plain writer's output is what dotenvy misreads.
        assert_eq!(dotenvy_value(set("", "K", Some("sk-$x")).split_once('=').unwrap().1.trim()), None);
        assert_eq!(dotenvy_value("a\\b"), None);
    }

    #[test]
    fn get_last_unescapes_like_dotenvy() {
        assert_eq!(get_last(r#"K="a\$b\'c\\d\"e""#, "K").as_deref(), Some("a$b'c\\d\"e"));
        assert_eq!(get(r#"K="a\$b""#, "K").as_deref(), Some("a\\$b"), "other loaders keep \\$");
        assert_eq!(get_last("K='a\\$b'", "K").as_deref(), Some("a\\$b"), "single quotes are literal");
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
