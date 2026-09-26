//! Conversation ids for upstreams that route by session. OpenCode Go (opencode.ai/zen/go)
//! answers 400 `MissingSessionID` since 2026-09-05 unless each request carries a
//! conversation id in `x-opencode-session`, stable across the turns of one conversation
//! (it pins the conversation to one upstream and keeps its prompt cache warm). Agents send
//! their own session header, but a forward converts the request and sends only what the
//! upstream protocol needs, so the gateway derives the id and adds the header itself.

use super::convert::Proto;
use serde_json::Value;
use std::hash::{Hash, Hasher};

pub const HEADER: &str = "x-opencode-session";

/// Whether `base` is OpenCode's gateway (opencode.ai or a subdomain): Zen and Go.
pub fn wants_session(base: &str) -> bool {
    url::Url::parse(base.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "opencode.ai" || h.ends_with(".opencode.ai"))
}

/// Session headers agents send, most specific first: OpenCode's own, OpenCode / Kilo
/// (`x-session-id`), Claude Code, Codex (`session_id`, `conversation_id`), then the
/// generic affinity header (which OpenCode Go itself does not accept).
const CLIENT_HEADERS: &[&str] = &[HEADER, "x-session-id", "x-claude-code-session-id", "session_id", "session-id", "conversation_id", "x-session-affinity"];

/// The conversation id for a request: the client's session header, else a session field of
/// the body (Claude Code's `metadata.user_id`, Codex's `prompt_cache_key`, …), else a hash
/// of how the conversation starts (system prompt and messages up to the first user turn),
/// which every later turn of the conversation resends unchanged.
pub fn session_id<'a>(header: impl Fn(&str) -> Option<&'a str>, inbound: Proto, body: &Value) -> String {
    CLIENT_HEADERS
        .iter()
        .filter_map(|h| header(h))
        .chain(body_session(body).as_deref())
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(clean)
        .unwrap_or_else(|| format!("agp-{:016x}", hash(&opening(inbound, body))))
}

/// A session id found in the body.
fn body_session(body: &Value) -> Option<String> {
    let s = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    claude_code_session(body["metadata"]["user_id"].as_str().unwrap_or_default())
        .or_else(|| s(body.get("prompt_cache_key")))
        .or_else(|| s(body.get("session_id")))
        .or_else(|| s(body.get("conversation")).or_else(|| s(body.pointer("/conversation/id"))))
}

/// Claude Code's session from `metadata.user_id`: a JSON object with `session_id`, or the
/// older `user_<hash>_account_<uuid>_session_<uuid>` form.
fn claude_code_session(user_id: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(user_id) {
        return v.get("session_id").and_then(Value::as_str).filter(|x| !x.is_empty()).map(String::from);
    }
    user_id.rsplit_once("_session_").map(|(_, id)| id.to_string()).filter(|x| !x.is_empty())
}

/// Header-safe: visible ASCII and not too long, as the client sent it; anything else is
/// replaced by a hash of it (still stable for the conversation).
fn clean(v: &str) -> String {
    if v.len() <= 128 && v.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        v.to_string()
    } else {
        format!("agp-{:016x}", hash(&Value::String(v.into())))
    }
}

/// The part of the request every turn of one conversation repeats: the system prompt and
/// the messages up to and including the first user message.
fn opening(inbound: Proto, body: &Value) -> Value {
    let until_user = |items: &[Value]| -> Vec<Value> {
        let n = items.iter().position(|m| m["role"] == "user").map_or(items.len(), |i| i + 1);
        items[..n].to_vec()
    };
    let list = |v: &Value| v.as_array().map(|a| until_user(a)).unwrap_or_default();
    match inbound {
        Proto::Chat => Value::Array(list(&body["messages"])),
        Proto::Anthropic => serde_json::json!([body["system"], list(&body["messages"])]),
        Proto::Responses => {
            let input = if body["input"].is_array() { Value::Array(list(&body["input"])) } else { body["input"].clone() };
            serde_json::json!([body["instructions"], input])
        }
    }
}

fn hash(v: &Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_headers(_: &str) -> Option<&'static str> {
        None
    }

    #[test]
    fn only_opencode_hosts() {
        assert!(wants_session("https://opencode.ai/zen/go/v1"));
        assert!(wants_session("https://opencode.ai/zen/v1/"));
        assert!(wants_session("https://api.OpenCode.ai/v1"));
        assert!(!wants_session("https://opencode.ai.evil.example/v1"));
        assert!(!wants_session("https://notopencode.ai/v1"));
        assert!(!wants_session("https://api.openai.com/v1"));
        assert!(!wants_session("not a url"));
    }

    #[test]
    fn client_header_wins_in_order() {
        let hs = [("session_id", "codex-1"), ("x-session-affinity", "aff"), ("x-claude-code-session-id", "cc-1")];
        let get = |n: &str| hs.iter().find(|(k, _)| *k == n).map(|(_, v)| *v);
        let body = json!({ "prompt_cache_key": "pck" });
        assert_eq!(session_id(get, Proto::Chat, &body), "cc-1");
        let only_aff = |n: &str| (n == "x-session-affinity").then_some("aff");
        assert_eq!(session_id(only_aff, Proto::Chat, &json!({})), "aff", "better than a hash");
        let blank = |n: &str| (n == HEADER).then_some("  ");
        assert_eq!(session_id(blank, Proto::Chat, &body), "pck", "an empty header is skipped");
    }

    #[test]
    fn body_fields() {
        let json_uid = json!({ "metadata": { "user_id": "{\"device_id\":\"d\",\"account_uuid\":\"a\",\"session_id\":\"s-123\"}" } });
        assert_eq!(session_id(no_headers, Proto::Anthropic, &json_uid), "s-123");
        let legacy = json!({ "metadata": { "user_id": "user_abc_account_1111_session_2222-3333" } });
        assert_eq!(session_id(no_headers, Proto::Anthropic, &legacy), "2222-3333");
        assert_eq!(session_id(no_headers, Proto::Responses, &json!({ "prompt_cache_key": "019a-conv" })), "019a-conv");
        assert_eq!(session_id(no_headers, Proto::Responses, &json!({ "conversation": { "id": "conv_1" } })), "conv_1");
        assert_eq!(session_id(no_headers, Proto::Chat, &json!({ "session_id": "or-1" })), "or-1");
    }

    #[test]
    fn unsafe_values_are_hashed() {
        let long = "x".repeat(200);
        for bad in ["有中文", "a b", long.as_str(), "a\r\nx-evil: 1"] {
            let get = move |n: &str| (n == HEADER).then_some(bad);
            let id = session_id(get, Proto::Chat, &json!({}));
            assert!(id.starts_with("agp-") && id.len() == 20, "{id}");
            assert_eq!(id, session_id(get, Proto::Chat, &json!({})), "stable");
        }
    }

    #[test]
    fn opening_hash_is_stable_across_turns() {
        let turn1 = json!({ "model": "m", "messages": [{ "role": "system", "content": "sys" }, { "role": "user", "content": "hi" }] });
        let turn2 = json!({ "model": "m2", "messages": [
            { "role": "system", "content": "sys" }, { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "hello" }, { "role": "user", "content": "more" }] });
        let other = json!({ "messages": [{ "role": "system", "content": "sys" }, { "role": "user", "content": "bye" }] });
        let id = session_id(no_headers, Proto::Chat, &turn1);
        assert!(id.starts_with("agp-"));
        assert_eq!(id, session_id(no_headers, Proto::Chat, &turn2));
        assert_ne!(id, session_id(no_headers, Proto::Chat, &other));

        let a1 = json!({ "system": "s", "messages": [{ "role": "user", "content": "q" }] });
        let a2 = json!({ "system": "s", "messages": [{ "role": "user", "content": "q" }, { "role": "assistant", "content": "a" }, { "role": "user", "content": "q2" }] });
        let a3 = json!({ "system": "other", "messages": [{ "role": "user", "content": "q" }] });
        assert_eq!(session_id(no_headers, Proto::Anthropic, &a1), session_id(no_headers, Proto::Anthropic, &a2));
        assert_ne!(session_id(no_headers, Proto::Anthropic, &a1), session_id(no_headers, Proto::Anthropic, &a3));

        let r1 = json!({ "instructions": "i", "input": [{ "type": "message", "role": "user", "content": "q" }] });
        let r2 = json!({ "instructions": "i", "input": [{ "type": "message", "role": "user", "content": "q" }, { "type": "function_call_output", "call_id": "c", "output": "x" }] });
        assert_eq!(session_id(no_headers, Proto::Responses, &r1), session_id(no_headers, Proto::Responses, &r2));
        assert_ne!(session_id(no_headers, Proto::Responses, &r1), session_id(no_headers, Proto::Responses, &json!({ "instructions": "i", "input": "q" })));
    }
}
