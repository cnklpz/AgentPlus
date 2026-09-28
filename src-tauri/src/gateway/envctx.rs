//! Codex describes the machine at the start of a conversation (and again when something
//! changes) in an `<environment_context>` block of a user message, including the time zone:
//!
//! ```text
//! <environment_context>
//!   <cwd>…</cwd>
//!   <current_date>2026-09-28</current_date>
//!   <timezone>Asia/Singapore</timezone>
//! </environment_context>
//! ```
//!
//! Codex reads the zone from the OS (on Windows it ignores `TZ`), so the gateway can put
//! another one there. The rewrite depends only on the text, so every request of a
//! conversation (its history included) carries the same prefix and prompt caching still works.

use serde_json::Value;

const OPEN: &str = "<environment_context>";
const CLOSE: &str = "</environment_context>";
const TZ_OPEN: &str = "<timezone>";
const TZ_CLOSE: &str = "</timezone>";

/// Whether `tz` looks like an IANA time zone name ("Asia/Shanghai", "UTC", "Etc/GMT+8").
/// Only the shape is checked: AgentPlus has no time zone database.
pub fn valid_timezone(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && tz.split('/').all(|part| {
            part.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        })
}

/// Puts `tz` in the `<timezone>` of every Codex environment context in `body` (any string,
/// at any depth, so every protocol and every history item is covered). Text outside those
/// blocks is left alone. Returns whether anything changed.
pub fn set_timezone(body: &mut Value, tz: &str) -> bool {
    match body {
        Value::String(s) => match rewrite(s, tz) {
            Some(new) => {
                *s = new;
                true
            }
            None => false,
        },
        Value::Array(items) => items.iter_mut().fold(false, |changed, v| set_timezone(v, tz) | changed),
        Value::Object(map) => map.values_mut().fold(false, |changed, v| set_timezone(v, tz) | changed),
        _ => false,
    }
}

/// `text` with the time zone of its environment contexts replaced, or None when there was
/// nothing to replace (or it already said `tz`).
fn rewrite(text: &str, tz: &str) -> Option<String> {
    if !text.contains(OPEN) {
        return None;
    }
    let (mut out, mut rest, mut changed) = (String::with_capacity(text.len()), text, false);
    while let Some(start) = rest.find(OPEN) {
        let Some(len) = rest[start..].find(CLOSE).map(|i| i + CLOSE.len()) else { break };
        out.push_str(&rest[..start]);
        let block = &rest[start..start + len];
        match zone_span(block) {
            Some((a, b)) if &block[a..b] != tz => {
                out.push_str(&block[..a]);
                out.push_str(tz);
                out.push_str(&block[b..]);
                changed = true;
            }
            _ => out.push_str(block),
        }
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Byte range of the zone name inside a context block's `<timezone>…</timezone>`. A value
/// that doesn't look like a name (say, an unavailable-marker element) is left as it is.
fn zone_span(block: &str) -> Option<(usize, usize)> {
    let a = block.find(TZ_OPEN)? + TZ_OPEN.len();
    let b = a + block[a..].find(TZ_CLOSE)?;
    let name = block[a..b].trim();
    valid_timezone(name).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CTX: &str = "<environment_context>\n  <cwd>D:\\w</cwd>\n  <shell>powershell</shell>\n  <current_date>2026-09-28</current_date>\n  <timezone>Asia/Singapore</timezone>\n</environment_context>";

    #[test]
    fn names_look_like_iana_zones() {
        for ok in ["UTC", "Asia/Shanghai", "America/Argentina/Buenos_Aires", "Etc/GMT+8", "Etc/GMT-14", "America/Port-au-Prince"] {
            assert!(valid_timezone(ok), "{ok}");
        }
        for bad in ["", "/Asia", "Asia/", "Asia//Tokyo", "Asia Shanghai", "+08:00", "Asia/<x>", "8/Tokyo"] {
            assert!(!valid_timezone(bad), "{bad}");
        }
        assert!(!valid_timezone(&"A".repeat(65)));
    }

    #[test]
    fn replaces_the_zone_in_every_context_of_a_responses_body() {
        let msg = |text: &str| json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] });
        let mut body = json!({
            "model": "gpt-5.5",
            "instructions": "Mention <timezone>Asia/Singapore</timezone> as written.",
            "input": [msg(CTX), msg("hi"), msg(&format!("before {CTX} after"))],
        });
        assert!(set_timezone(&mut body, "America/Los_Angeles"));
        let first = body["input"][0]["content"][0]["text"].as_str().unwrap();
        assert_eq!(first, CTX.replace("Asia/Singapore", "America/Los_Angeles"));
        assert!(first.contains("<current_date>2026-09-28</current_date>"), "the date is left alone");
        let third = body["input"][2]["content"][0]["text"].as_str().unwrap();
        assert_eq!(third, format!("before {} after", CTX.replace("Asia/Singapore", "America/Los_Angeles")));
        // Outside a context block the tag is just text.
        assert_eq!(body["instructions"], "Mention <timezone>Asia/Singapore</timezone> as written.");
        assert_eq!(body["input"][1]["content"][0]["text"], "hi");
        // The same zone again changes nothing (and the text is identical on every request).
        let again = body.clone();
        assert!(!set_timezone(&mut body, "America/Los_Angeles"));
        assert_eq!(body, again);
    }

    #[test]
    fn leaves_contexts_without_a_usable_zone_alone() {
        for text in [
            "<environment_context>\n  <cwd>x</cwd>\n</environment_context>",
            "<environment_context>\n  <timezone status=\"unavailable\" />\n</environment_context>",
            "<environment_context>\n  <timezone></timezone>\n</environment_context>",
            // Unclosed block (a cut-off paste) and a timezone after the block.
            "<environment_context>\n  <timezone>Asia/Singapore</timezone>",
            "<environment_context></environment_context><timezone>Asia/Singapore</timezone>",
        ] {
            let mut v = json!(text);
            assert!(!set_timezone(&mut v, "UTC"), "{text}");
            assert_eq!(v, json!(text));
        }
    }

    #[test]
    fn covers_chat_and_anthropic_shapes() {
        let mut chat = json!({ "messages": [{ "role": "user", "content": CTX }] });
        assert!(set_timezone(&mut chat, "Asia/Shanghai"));
        assert!(chat["messages"][0]["content"].as_str().unwrap().contains("<timezone>Asia/Shanghai</timezone>"));
        let mut anthropic = json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": CTX }] }] });
        assert!(set_timezone(&mut anthropic, "Asia/Shanghai"));
        assert!(anthropic["messages"][0]["content"][0]["text"].as_str().unwrap().contains("<timezone>Asia/Shanghai</timezone>"));
    }
}
