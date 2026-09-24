//! Wire-protocol conversion between OpenAI Chat Completions (the pivot format),
//! OpenAI Responses and Anthropic Messages: requests, responses, errors, model
//! lists and SSE streams. Pure functions / state machines, no I/O.
#![allow(dead_code)]

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Chat,
    Responses,
    Anthropic,
}

impl Proto {
    pub fn from_api(s: &str) -> Option<Proto> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" => Some(Proto::Chat),
            "responses" => Some(Proto::Responses),
            "anthropic" => Some(Proto::Anthropic),
            _ => None,
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Proto::Chat => "/chat/completions",
            Proto::Responses => "/responses",
            Proto::Anthropic => "/messages",
        }
    }
}

/// Facts about the inbound request that responses need (model name, custom tool names...).
#[derive(Clone, Debug, Default)]
pub struct ReqCtx {
    pub model: String,
    /// Responses "custom" (freeform) tool names; their calls are mapped back to custom_tool_call.
    pub custom_tools: Vec<String>,
    pub stream: bool,
}

pub fn req_ctx(from: Proto, body: &Value) -> ReqCtx {
    let mut custom_tools = Vec::new();
    if from == Proto::Responses {
        for t in arr(body.get("tools")) {
            if sget(t, "type") == "custom" && !sget(t, "name").is_empty() {
                custom_tools.push(sget(t, "name").to_string());
            }
        }
    }
    ReqCtx {
        model: sget(body, "model").to_string(),
        custom_tools,
        stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// small helpers

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique-enough id: prefix + millis + process-wide counter.
fn gen_id(prefix: &str) -> String {
    let n = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}{:x}{:06x}",
        chrono::Utc::now().timestamp_millis(),
        n & 0xff_ffff
    )
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn sget<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn uget(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

fn nonnull(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| !x.is_null())
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Flatten any content shape (string, parts array, block object) into plain text.
fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(text_of)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => match o.get("text") {
            Some(Value::String(s)) => s.clone(),
            _ => o.get("content").map(text_of).unwrap_or_default(),
        },
        other => other.to_string(),
    }
}

/// Tool output: string as-is, parts joined, anything else JSON-encoded.
fn output_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(a @ Value::Array(_)) => text_of(a),
        Some(o) => o.to_string(),
    }
}

/// Parse tool-call arguments into a JSON object for Anthropic `input`.
fn parse_args(s: &str) -> Value {
    if s.trim().is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(s) {
        Ok(v @ Value::Object(_)) => v,
        _ => json!({ "raw": s }),
    }
}

/// Extract the freeform input of a custom tool from `{"input": ...}` arguments.
fn custom_input(args: &str) -> String {
    match serde_json::from_str::<Value>(args) {
        Ok(Value::Object(o)) => match o.get("input") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => args.to_string(),
        },
        _ => args.to_string(),
    }
}

/// Anthropic ids must match ^[a-zA-Z0-9_-]+$.
fn sanitize_id(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if t.is_empty() {
        "toolu_empty".into()
    } else {
        t
    }
}

fn truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

enum Part {
    Text(String),
    Image(String),
}

/// Normalize chat / responses content (string or parts) into text + image-url parts.
fn chat_parts(content: &Value) -> Vec<Part> {
    let mut out = Vec::new();
    match content {
        Value::Null => {}
        Value::String(s) => out.push(Part::Text(s.clone())),
        Value::Array(a) => {
            for p in a {
                if let Value::String(s) = p {
                    out.push(Part::Text(s.clone()));
                    continue;
                }
                match sget(p, "type") {
                    "text" | "input_text" | "output_text" => out.push(Part::Text(sget(p, "text").into())),
                    "refusal" => out.push(Part::Text(sget(p, "refusal").into())),
                    "image_url" | "input_image" => {
                        let url = match p.get("image_url") {
                            Some(Value::String(s)) => s.clone(),
                            Some(o) => sget(o, "url").to_string(),
                            None => sget(p, "file_id").to_string(),
                        };
                        if !url.is_empty() {
                            out.push(Part::Image(url));
                        }
                    }
                    _ => {}
                }
            }
        }
        other => out.push(Part::Text(text_of(other))),
    }
    out
}

fn parts_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) if !t.is_empty() => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Chat content: a plain string unless images are present.
fn parts_to_chat_content(parts: Vec<Part>) -> Value {
    if !parts.iter().any(|p| matches!(p, Part::Image(_))) {
        return Value::String(parts_text(&parts));
    }
    Value::Array(
        parts
            .into_iter()
            .map(|p| match p {
                Part::Text(t) => json!({"type": "text", "text": t}),
                Part::Image(u) => json!({"type": "image_url", "image_url": {"url": u}}),
            })
            .collect(),
    )
}

fn anthropic_image(url: &str) -> Value {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((meta, data)) = rest.split_once(',') {
            let media = meta.split(';').next().unwrap_or("image/png");
            return json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}});
        }
    }
    json!({"type": "image", "source": {"type": "url", "url": url}})
}

fn anthropic_image_url(block: &Value) -> Option<String> {
    let src = block.get("source")?;
    match sget(src, "type") {
        "base64" => Some(format!(
            "data:{};base64,{}",
            match sget(src, "media_type") {
                "" => "image/png",
                m => m,
            },
            sget(src, "data")
        )),
        "url" => Some(sget(src, "url").to_string()),
        _ => None,
    }
}

fn set_stream_opts(out: &mut Map<String, Value>, stream: bool) {
    if stream {
        out.insert("stream".into(), json!(true));
        out.insert("stream_options".into(), json!({"include_usage": true}));
    }
}

fn copy_keys(src: &Value, dst: &mut Map<String, Value>, keys: &[&str]) {
    for k in keys {
        if let Some(v) = nonnull(src.get(*k)) {
            dst.insert((*k).to_string(), v.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// usage

#[derive(Clone, Copy, Debug, Default)]
struct Usage {
    input: u64,
    output: u64,
    cached: u64,
    reasoning: u64,
}

fn usage_from_chat(u: &Value) -> Usage {
    Usage {
        input: uget(u, "prompt_tokens"),
        output: uget(u, "completion_tokens"),
        cached: u.get("prompt_tokens_details").map(|d| uget(d, "cached_tokens")).unwrap_or(0),
        reasoning: u.get("completion_tokens_details").map(|d| uget(d, "reasoning_tokens")).unwrap_or(0),
    }
}

fn usage_to_chat(u: Usage) -> Value {
    let mut v = json!({
        "prompt_tokens": u.input,
        "completion_tokens": u.output,
        "total_tokens": u.input + u.output,
    });
    if u.cached > 0 {
        v["prompt_tokens_details"] = json!({"cached_tokens": u.cached});
    }
    if u.reasoning > 0 {
        v["completion_tokens_details"] = json!({"reasoning_tokens": u.reasoning});
    }
    v
}

fn usage_from_responses(u: &Value) -> Usage {
    Usage {
        input: uget(u, "input_tokens"),
        output: uget(u, "output_tokens"),
        cached: u.get("input_tokens_details").map(|d| uget(d, "cached_tokens")).unwrap_or(0),
        reasoning: u.get("output_tokens_details").map(|d| uget(d, "reasoning_tokens")).unwrap_or(0),
    }
}

fn usage_to_responses(u: Usage) -> Value {
    json!({
        "input_tokens": u.input,
        "input_tokens_details": {"cached_tokens": u.cached},
        "output_tokens": u.output,
        "output_tokens_details": {"reasoning_tokens": u.reasoning},
        "total_tokens": u.input + u.output,
    })
}

/// Anthropic input_tokens excludes cache reads/writes; fold them into the total.
fn usage_from_anthropic(u: &Value) -> Usage {
    let cache_read = uget(u, "cache_read_input_tokens");
    Usage {
        input: uget(u, "input_tokens") + cache_read + uget(u, "cache_creation_input_tokens"),
        output: uget(u, "output_tokens"),
        cached: cache_read,
        reasoning: 0,
    }
}

/// (input, output) tokens from any protocol's body, event or chunk that carries usage:
/// top-level `usage`, or nested in `response` / `message` (Responses and Anthropic events).
pub fn usage_tokens(v: &Value) -> Option<(u64, u64)> {
    let u = [v.get("usage"), v.get("response").and_then(|r| r.get("usage")), v.get("message").and_then(|m| m.get("usage"))]
        .into_iter()
        .flatten()
        .find(|u| u.is_object())?;
    let x = if u.get("prompt_tokens").is_some() || u.get("completion_tokens").is_some() {
        usage_from_chat(u)
    } else if u.get("cache_read_input_tokens").is_some() || u.get("cache_creation_input_tokens").is_some() {
        usage_from_anthropic(u)
    } else {
        usage_from_responses(u)
    };
    (x.input > 0 || x.output > 0).then_some((x.input, x.output))
}

fn usage_to_anthropic(u: Usage) -> Value {
    let mut v = json!({"input_tokens": u.input.saturating_sub(u.cached), "output_tokens": u.output});
    if u.cached > 0 {
        v["cache_read_input_tokens"] = json!(u.cached);
    }
    v
}

// ---------------------------------------------------------------------------
// requests

pub fn request_to_chat(from: Proto, body: &Value) -> Result<Value> {
    if !body.is_object() {
        bail!("request body must be a JSON object");
    }
    match from {
        Proto::Chat => Ok(body.clone()),
        Proto::Responses => Ok(responses_req_to_chat(body)),
        Proto::Anthropic => Ok(anthropic_req_to_chat(body)),
    }
}

pub fn request_from_chat(to: Proto, chat: &Value) -> Result<Value> {
    if !chat.is_object() {
        bail!("request body must be a JSON object");
    }
    match to {
        Proto::Chat => Ok(chat.clone()),
        Proto::Responses => Ok(chat_req_to_responses(chat)),
        Proto::Anthropic => Ok(chat_req_to_anthropic(chat)),
    }
}

// ---- Responses -> Chat

fn responses_req_to_chat(b: &Value) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    if let Some(ins) = b.get("instructions") {
        let t = text_of(ins);
        if !t.is_empty() {
            msgs.push(json!({"role": "system", "content": t}));
        }
    }
    match b.get("input") {
        Some(Value::String(s)) => msgs.push(json!({"role": "user", "content": s})),
        Some(Value::Array(items)) => {
            for it in items {
                responses_item_to_chat(it, &mut msgs);
            }
        }
        _ => {}
    }

    let mut out = Map::new();
    out.insert("model".into(), json!(sget(b, "model")));
    out.insert("messages".into(), Value::Array(msgs));

    let mut tools = Vec::new();
    for t in arr(b.get("tools")) {
        match sget(t, "type") {
            "function" => {
                let mut f = json!({
                    "name": sget(t, "name"),
                    "parameters": nonnull(t.get("parameters")).cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                });
                if let Some(d) = t.get("description").and_then(Value::as_str) {
                    f["description"] = json!(d);
                }
                tools.push(json!({"type": "function", "function": f}));
            }
            "custom" => {
                let fmt = t.get("format").cloned().unwrap_or(Value::Null);
                let desc = [sget(&fmt, "definition"), sget(&fmt, "description")]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .unwrap_or("Raw freeform input for this tool.");
                tools.push(json!({"type": "function", "function": {
                    "name": sget(t, "name"),
                    "description": sget(t, "description"),
                    "parameters": {
                        "type": "object",
                        "properties": {"input": {"type": "string", "description": desc}},
                        "required": ["input"],
                    },
                }}));
            }
            _ => {} // built-in tools (web_search, local_shell, ...) are dropped
        }
    }
    let has_tools = !tools.is_empty();
    if has_tools {
        out.insert("tools".into(), Value::Array(tools));
        match b.get("tool_choice") {
            Some(Value::String(s)) if matches!(s.as_str(), "auto" | "none" | "required") => {
                out.insert("tool_choice".into(), json!(s));
            }
            Some(o @ Value::Object(_)) if matches!(sget(o, "type"), "function" | "custom") => {
                out.insert(
                    "tool_choice".into(),
                    json!({"type": "function", "function": {"name": sget(o, "name")}}),
                );
            }
            _ => {}
        }
        copy_keys(b, &mut out, &["parallel_tool_calls"]);
    }
    if let Some(m) = nonnull(b.get("max_output_tokens")) {
        out.insert("max_tokens".into(), m.clone());
    }
    if let Some(e) = b.get("reasoning").and_then(|r| r.get("effort")).and_then(Value::as_str) {
        out.insert("reasoning_effort".into(), json!(e));
    }
    copy_keys(b, &mut out, &["temperature", "top_p"]);
    set_stream_opts(&mut out, b.get("stream").and_then(Value::as_bool).unwrap_or(false));
    Value::Object(out)
}

fn responses_item_to_chat(it: &Value, msgs: &mut Vec<Value>) {
    let ty = match it.get("type").and_then(Value::as_str) {
        Some(t) => t,
        None if it.get("role").is_some() => "message",
        None => return,
    };
    match ty {
        "message" => {
            let role = match sget(it, "role") {
                "assistant" => "assistant",
                "system" | "developer" => "system",
                _ => "user",
            };
            let parts = chat_parts(it.get("content").unwrap_or(&Value::Null));
            if role == "assistant" {
                msgs.push(json!({"role": "assistant", "content": parts_text(&parts)}));
            } else {
                msgs.push(json!({"role": role, "content": parts_to_chat_content(parts)}));
            }
        }
        "function_call" | "custom_tool_call" => {
            let call_id = [sget(it, "call_id"), sget(it, "id")]
                .into_iter()
                .find(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| gen_id("call_"));
            let args = if ty == "custom_tool_call" {
                let input = match it.get("input") {
                    Some(Value::String(s)) => s.clone(),
                    other => output_text(other),
                };
                json!({ "input": input }).to_string()
            } else {
                match it.get("arguments") {
                    Some(Value::String(s)) => s.clone(),
                    None | Some(Value::Null) => "{}".to_string(),
                    Some(o) => o.to_string(),
                }
            };
            let tc = json!({"id": call_id, "type": "function",
                "function": {"name": sget(it, "name"), "arguments": args}});
            let last_is_assistant = msgs
                .last()
                .map(|m| sget(m, "role") == "assistant")
                .unwrap_or(false);
            if last_is_assistant {
                let obj = msgs.last_mut().unwrap().as_object_mut().unwrap();
                match obj.get_mut("tool_calls") {
                    Some(Value::Array(a)) => a.push(tc),
                    _ => {
                        obj.insert("tool_calls".into(), json!([tc]));
                    }
                }
            } else {
                msgs.push(json!({"role": "assistant", "content": null, "tool_calls": [tc]}));
            }
        }
        "function_call_output" | "custom_tool_call_output" => {
            msgs.push(json!({
                "role": "tool",
                "tool_call_id": sget(it, "call_id"),
                "content": output_text(it.get("output")),
            }));
        }
        _ => {} // reasoning, local_shell_call, web_search_call, ... are dropped
    }
}

// ---- Anthropic -> Chat

fn anthropic_req_to_chat(b: &Value) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    if let Some(sys) = b.get("system") {
        let t = text_of(sys);
        if !t.is_empty() {
            msgs.push(json!({"role": "system", "content": t}));
        }
    }
    for m in arr(b.get("messages")) {
        let content = m.get("content").unwrap_or(&Value::Null);
        if sget(m, "role") == "assistant" {
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut calls = Vec::new();
            match content {
                Value::String(s) => text.push_str(s),
                Value::Array(blocks) => {
                    for bl in blocks {
                        match sget(bl, "type") {
                            "text" => text.push_str(sget(bl, "text")),
                            "thinking" => reasoning.push_str(sget(bl, "thinking")),
                            "tool_use" => {
                                let input = nonnull(bl.get("input")).cloned().unwrap_or_else(|| json!({}));
                                calls.push(json!({"id": sget(bl, "id"), "type": "function",
                                    "function": {"name": sget(bl, "name"), "arguments": input.to_string()}}));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            let mut msg = json!({"role": "assistant",
                "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) }});
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            if !reasoning.is_empty() {
                msg["reasoning_content"] = json!(reasoning);
            }
            msgs.push(msg);
        } else {
            match content {
                Value::Array(blocks) => {
                    let mut tool_msgs = Vec::new();
                    let mut parts = Vec::new();
                    for bl in blocks {
                        match sget(bl, "type") {
                            "text" => parts.push(Part::Text(sget(bl, "text").into())),
                            "image" => {
                                if let Some(u) = anthropic_image_url(bl) {
                                    parts.push(Part::Image(u));
                                }
                            }
                            "tool_result" => {
                                let mut text = String::new();
                                match bl.get("content") {
                                    Some(Value::Array(inner)) => {
                                        let mut texts = Vec::new();
                                        for ib in inner {
                                            match sget(ib, "type") {
                                                "image" => {
                                                    if let Some(u) = anthropic_image_url(ib) {
                                                        parts.push(Part::Image(u));
                                                    }
                                                }
                                                _ => {
                                                    let t = text_of(ib);
                                                    if !t.is_empty() {
                                                        texts.push(t);
                                                    }
                                                }
                                            }
                                        }
                                        text = texts.join("\n");
                                    }
                                    other => text.push_str(&output_text(other)),
                                }
                                if bl.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                                    text = format!("[tool error] {text}");
                                }
                                tool_msgs.push(json!({"role": "tool",
                                    "tool_call_id": sget(bl, "tool_use_id"), "content": text}));
                            }
                            "document" => {
                                if let Some(src) = bl.get("source") {
                                    if sget(src, "type") == "text" {
                                        parts.push(Part::Text(sget(src, "data").into()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    msgs.extend(tool_msgs);
                    if !parts.is_empty() {
                        msgs.push(json!({"role": "user", "content": parts_to_chat_content(parts)}));
                    }
                }
                other => msgs.push(json!({"role": "user", "content": text_of(other)})),
            }
        }
    }

    let mut out = Map::new();
    out.insert("model".into(), json!(sget(b, "model")));
    out.insert("messages".into(), Value::Array(msgs));

    let tools: Vec<Value> = arr(b.get("tools"))
        .iter()
        .filter(|t| matches!(t.get("type").and_then(Value::as_str), None | Some("custom")))
        .map(|t| {
            let mut f = json!({"name": sget(t, "name"),
                "parameters": nonnull(t.get("input_schema")).cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}))});
            if let Some(d) = t.get("description").and_then(Value::as_str) {
                f["description"] = json!(d);
            }
            json!({"type": "function", "function": f})
        })
        .collect();
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
        if let Some(tc) = nonnull(b.get("tool_choice")) {
            let mapped = match sget(tc, "type") {
                "auto" => Some(json!("auto")),
                "any" => Some(json!("required")),
                "none" => Some(json!("none")),
                "tool" => Some(json!({"type": "function", "function": {"name": sget(tc, "name")}})),
                _ => None,
            };
            if let Some(m) = mapped {
                out.insert("tool_choice".into(), m);
            }
            if tc.get("disable_parallel_tool_use").and_then(Value::as_bool) == Some(true) {
                out.insert("parallel_tool_calls".into(), json!(false));
            }
        }
    }
    if let Some(m) = nonnull(b.get("max_tokens")) {
        out.insert("max_tokens".into(), m.clone());
    }
    if let Some(s) = nonnull(b.get("stop_sequences")) {
        out.insert("stop".into(), s.clone());
    }
    if let Some(th) = b.get("thinking") {
        if sget(th, "type") == "enabled" {
            let budget = uget(th, "budget_tokens");
            let effort = if budget < 4096 {
                "low"
            } else if budget < 16384 {
                "medium"
            } else {
                "high"
            };
            out.insert("reasoning_effort".into(), json!(effort));
        }
    }
    copy_keys(b, &mut out, &["temperature", "top_p"]);
    set_stream_opts(&mut out, b.get("stream").and_then(Value::as_bool).unwrap_or(false));
    Value::Object(out)
}

// ---- Chat -> Responses

fn chat_req_to_responses(c: &Value) -> Value {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for m in arr(c.get("messages")) {
        let content = m.get("content").unwrap_or(&Value::Null);
        match sget(m, "role") {
            "system" | "developer" => {
                let t = text_of(content);
                if !t.is_empty() {
                    instructions.push(t);
                }
            }
            "assistant" => {
                let t = parts_text(&chat_parts(content));
                if !t.is_empty() {
                    input.push(json!({"type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": t}]}));
                }
                for tc in arr(m.get("tool_calls")) {
                    let f = tc.get("function").unwrap_or(&Value::Null);
                    input.push(json!({"type": "function_call", "call_id": sget(tc, "id"),
                        "name": sget(f, "name"), "arguments": match f.get("arguments") {
                            Some(Value::String(s)) => s.clone(),
                            None | Some(Value::Null) => "{}".into(),
                            Some(o) => o.to_string(),
                        }}));
                }
            }
            "tool" | "function" => {
                input.push(json!({"type": "function_call_output",
                    "call_id": sget(m, "tool_call_id"), "output": text_of(content)}));
            }
            _ => {
                let parts: Vec<Value> = chat_parts(content)
                    .into_iter()
                    .map(|p| match p {
                        Part::Text(t) => json!({"type": "input_text", "text": t}),
                        Part::Image(u) => json!({"type": "input_image", "image_url": u}),
                    })
                    .collect();
                input.push(json!({"type": "message", "role": "user", "content": parts}));
            }
        }
    }
    let mut out = Map::new();
    out.insert("model".into(), json!(sget(c, "model")));
    if !instructions.is_empty() {
        out.insert("instructions".into(), json!(instructions.join("\n\n")));
    }
    out.insert("input".into(), Value::Array(input));
    let tools: Vec<Value> = arr(c.get("tools"))
        .iter()
        .filter_map(|t| {
            let f = t.get("function")?;
            let mut o = json!({"type": "function", "name": sget(f, "name"),
                "parameters": nonnull(f.get("parameters")).cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}))});
            if let Some(d) = f.get("description").and_then(Value::as_str) {
                o["description"] = json!(d);
            }
            Some(o)
        })
        .collect();
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
        match c.get("tool_choice") {
            Some(Value::String(s)) => {
                out.insert("tool_choice".into(), json!(s));
            }
            Some(o @ Value::Object(_)) => {
                let name = o.get("function").map(|f| sget(f, "name")).unwrap_or("");
                out.insert("tool_choice".into(), json!({"type": "function", "name": name}));
            }
            _ => {}
        }
        copy_keys(c, &mut out, &["parallel_tool_calls"]);
    }
    if let Some(m) = nonnull(c.get("max_completion_tokens")).or(nonnull(c.get("max_tokens"))) {
        out.insert("max_output_tokens".into(), m.clone());
    }
    if let Some(e) = c.get("reasoning_effort").and_then(Value::as_str) {
        out.insert("reasoning".into(), json!({"effort": e}));
    }
    copy_keys(c, &mut out, &["temperature", "top_p"]);
    out.insert("store".into(), json!(false));
    if c.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        out.insert("stream".into(), json!(true));
    }
    Value::Object(out)
}

// ---- Chat -> Anthropic

fn push_merge(msgs: &mut Vec<(String, Vec<Value>)>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    match msgs.last_mut() {
        Some((r, b)) if r == role => b.extend(blocks),
        _ => msgs.push((role.to_string(), blocks)),
    }
}

fn effort_budget(effort: &str) -> Option<u64> {
    match effort {
        "none" | "" => None,
        "minimal" | "low" => Some(2048),
        "medium" => Some(8192),
        _ => Some(16384), // high / xhigh / unknown
    }
}

fn chat_req_to_anthropic(c: &Value) -> Value {
    let mut system = Vec::new();
    let mut msgs: Vec<(String, Vec<Value>)> = Vec::new();
    for m in arr(c.get("messages")) {
        let content = m.get("content").unwrap_or(&Value::Null);
        match sget(m, "role") {
            "system" | "developer" => {
                let t = text_of(content);
                if !t.is_empty() {
                    system.push(t);
                }
            }
            "assistant" => {
                let mut blocks = Vec::new();
                let t = parts_text(&chat_parts(content));
                if !t.is_empty() {
                    blocks.push(json!({"type": "text", "text": t}));
                }
                for tc in arr(m.get("tool_calls")) {
                    let f = tc.get("function").unwrap_or(&Value::Null);
                    let input = match f.get("arguments") {
                        Some(Value::String(s)) => parse_args(s),
                        Some(o @ Value::Object(_)) => o.clone(),
                        _ => json!({}),
                    };
                    blocks.push(json!({"type": "tool_use", "id": sanitize_id(sget(tc, "id")),
                        "name": sget(f, "name"), "input": input}));
                }
                push_merge(&mut msgs, "assistant", blocks);
            }
            "tool" | "function" => {
                let t = text_of(content);
                let mut b = json!({"type": "tool_result", "tool_use_id": sanitize_id(sget(m, "tool_call_id"))});
                if !t.is_empty() {
                    b["content"] = json!(t);
                }
                push_merge(&mut msgs, "user", vec![b]);
            }
            _ => {
                let blocks: Vec<Value> = chat_parts(content)
                    .into_iter()
                    .filter_map(|p| match p {
                        Part::Text(t) if t.is_empty() => None,
                        Part::Text(t) => Some(json!({"type": "text", "text": t})),
                        Part::Image(u) => Some(anthropic_image(&u)),
                    })
                    .collect();
                push_merge(&mut msgs, "user", blocks);
            }
        }
    }
    // tool_result blocks must lead a user message
    for (role, blocks) in msgs.iter_mut() {
        if role == "user" {
            let (mut tr, rest): (Vec<Value>, Vec<Value>) =
                blocks.drain(..).partition(|b| sget(b, "type") == "tool_result");
            tr.extend(rest);
            *blocks = tr;
        }
    }
    if msgs.first().map(|(r, _)| r != "user").unwrap_or(true) {
        msgs.insert(0, ("user".into(), vec![json!({"type": "text", "text": "(continue)"})]));
    }

    let mut out = Map::new();
    out.insert("model".into(), json!(sget(c, "model")));
    if !system.is_empty() {
        out.insert("system".into(), json!(system.join("\n\n")));
    }
    out.insert(
        "messages".into(),
        Value::Array(
            msgs.into_iter()
                .map(|(r, b)| json!({"role": r, "content": b}))
                .collect(),
        ),
    );
    let mut max_tokens = nonnull(c.get("max_completion_tokens"))
        .or(nonnull(c.get("max_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or(8192);

    let tools: Vec<Value> = arr(c.get("tools"))
        .iter()
        .filter_map(|t| {
            let f = t.get("function")?;
            let mut o = json!({"name": sget(f, "name"),
                "input_schema": nonnull(f.get("parameters")).cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}))});
            if let Some(d) = f.get("description").and_then(Value::as_str) {
                o["description"] = json!(d);
            }
            Some(o)
        })
        .collect();
    let thinking = c
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(effort_budget);
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
        let mut tc = match c.get("tool_choice") {
            Some(Value::String(s)) => match s.as_str() {
                "required" => Some(json!({"type": "any"})),
                "none" => Some(json!({"type": "none"})),
                "auto" => Some(json!({"type": "auto"})),
                _ => None,
            },
            Some(o @ Value::Object(_)) => {
                let name = o.get("function").map(|f| sget(f, "name")).unwrap_or("");
                Some(json!({"type": "tool", "name": name}))
            }
            _ => None,
        };
        // Anthropic rejects forced tool use together with extended thinking.
        if thinking.is_some() && matches!(tc.as_ref().map(|t| sget(t, "type")), Some("any" | "tool")) {
            tc = Some(json!({"type": "auto"}));
        }
        if c.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false) {
            let t = tc.get_or_insert_with(|| json!({"type": "auto"}));
            if sget(t, "type") != "none" {
                t["disable_parallel_tool_use"] = json!(true);
            }
        }
        if let Some(t) = tc {
            out.insert("tool_choice".into(), t);
        }
    }
    match c.get("stop") {
        Some(Value::String(s)) => {
            out.insert("stop_sequences".into(), json!([s]));
        }
        Some(a @ Value::Array(_)) => {
            out.insert("stop_sequences".into(), a.clone());
        }
        _ => {}
    }
    if let Some(budget) = thinking {
        out.insert("thinking".into(), json!({"type": "enabled", "budget_tokens": budget}));
        if max_tokens <= budget {
            max_tokens = budget + 4096;
        }
    } else {
        copy_keys(c, &mut out, &["temperature", "top_p"]);
    }
    out.insert("max_tokens".into(), json!(max_tokens));
    if c.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        out.insert("stream".into(), json!(true));
    }
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// non-streaming responses

/// Assistant turn extracted from a chat.completion.
struct ChatTurn {
    id: String,
    model: String,
    text: String,
    reasoning: String,
    tools: Vec<(String, String, String)>, // (call id, name, arguments)
    finish: String,
    usage: Usage,
}

fn parse_chat_response(chat: &Value) -> ChatTurn {
    let ch = chat.get("choices").and_then(|c| c.get(0)).unwrap_or(&Value::Null);
    let msg = ch.get("message").unwrap_or(&Value::Null);
    let reasoning = [sget(msg, "reasoning_content"), sget(msg, "reasoning")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    let tools = arr(msg.get("tool_calls"))
        .iter()
        .map(|tc| {
            let f = tc.get("function").unwrap_or(&Value::Null);
            let args = match f.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                None | Some(Value::Null) => String::new(),
                Some(o) => o.to_string(),
            };
            let id = match sget(tc, "id") {
                "" => gen_id("call_"),
                s => s.to_string(),
            };
            (id, sget(f, "name").to_string(), args)
        })
        .collect();
    ChatTurn {
        id: sget(chat, "id").to_string(),
        model: sget(chat, "model").to_string(),
        text: parts_text(&chat_parts(msg.get("content").unwrap_or(&Value::Null))),
        reasoning,
        tools,
        finish: sget(ch, "finish_reason").to_string(),
        usage: chat.get("usage").map(usage_from_chat).unwrap_or_default(),
    }
}

fn chat_completion(
    id: &str,
    model: &str,
    text: &str,
    reasoning: &str,
    tools: &[(String, String, String)],
    finish: &str,
    usage: Usage,
) -> Value {
    let mut msg = json!({"role": "assistant",
        "content": if text.is_empty() && !tools.is_empty() { Value::Null } else { json!(text) }});
    if !reasoning.is_empty() {
        msg["reasoning_content"] = json!(reasoning);
    }
    if !tools.is_empty() {
        msg["tool_calls"] = Value::Array(
            tools
                .iter()
                .map(|(id, name, args)| {
                    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}})
                })
                .collect(),
        );
    }
    json!({
        "id": if id.is_empty() { gen_id("chatcmpl-") } else { id.to_string() },
        "object": "chat.completion",
        "created": now_secs(),
        "model": model,
        "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
        "usage": usage_to_chat(usage),
    })
}

pub fn response_to_chat(from: Proto, body: &Value) -> Result<Value> {
    if !body.is_object() {
        bail!("response body must be a JSON object");
    }
    match from {
        Proto::Chat => Ok(body.clone()),
        Proto::Responses => {
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut tools = Vec::new();
            for it in arr(body.get("output")) {
                match sget(it, "type") {
                    "message" => text.push_str(&parts_text(&chat_parts(it.get("content").unwrap_or(&Value::Null)))),
                    "reasoning" => {
                        let mut r = text_of(it.get("summary").unwrap_or(&Value::Null));
                        if r.is_empty() {
                            r = text_of(it.get("content").unwrap_or(&Value::Null));
                        }
                        reasoning.push_str(&r);
                    }
                    "function_call" => tools.push((
                        sget(it, "call_id").to_string(),
                        sget(it, "name").to_string(),
                        output_text(it.get("arguments")),
                    )),
                    "custom_tool_call" => tools.push((
                        sget(it, "call_id").to_string(),
                        sget(it, "name").to_string(),
                        json!({"input": output_text(it.get("input"))}).to_string(),
                    )),
                    _ => {}
                }
            }
            let finish = if sget(body, "status") == "incomplete" {
                match body.get("incomplete_details").map(|d| sget(d, "reason")) {
                    Some("content_filter") => "content_filter",
                    _ => "length",
                }
            } else if !tools.is_empty() {
                "tool_calls"
            } else {
                "stop"
            };
            let usage = body.get("usage").map(usage_from_responses).unwrap_or_default();
            Ok(chat_completion(sget(body, "id"), sget(body, "model"), &text, &reasoning, &tools, finish, usage))
        }
        Proto::Anthropic => {
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut tools = Vec::new();
            for bl in arr(body.get("content")) {
                match sget(bl, "type") {
                    "text" => text.push_str(sget(bl, "text")),
                    "thinking" => reasoning.push_str(sget(bl, "thinking")),
                    "tool_use" => tools.push((
                        sget(bl, "id").to_string(),
                        sget(bl, "name").to_string(),
                        nonnull(bl.get("input")).cloned().unwrap_or_else(|| json!({})).to_string(),
                    )),
                    _ => {}
                }
            }
            let mut finish = anthropic_stop_to_finish(sget(body, "stop_reason"));
            if finish == "stop" && !tools.is_empty() {
                finish = "tool_calls";
            }
            let usage = body.get("usage").map(usage_from_anthropic).unwrap_or_default();
            Ok(chat_completion(sget(body, "id"), sget(body, "model"), &text, &reasoning, &tools, finish, usage))
        }
    }
}

fn anthropic_stop_to_finish(s: &str) -> &'static str {
    match s {
        "max_tokens" | "model_context_window_exceeded" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        _ => "stop",
    }
}

fn finish_to_anthropic(f: &str, saw_tool: bool) -> &'static str {
    match f {
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        _ if saw_tool => "tool_use",
        _ => "end_turn",
    }
}

/// Responses output item for one tool call (custom_tool_call when the name is a custom tool).
fn responses_tool_item(ctx: &ReqCtx, item_id: &str, call_id: &str, name: &str, args: &str, status: &str) -> Value {
    if ctx.custom_tools.iter().any(|n| n == name) {
        json!({"type": "custom_tool_call", "id": item_id, "call_id": call_id, "name": name,
            "input": custom_input(args), "status": status})
    } else {
        json!({"type": "function_call", "id": item_id, "call_id": call_id, "name": name,
            "arguments": args, "status": status})
    }
}

fn is_custom(ctx: &ReqCtx, name: &str) -> bool {
    ctx.custom_tools.iter().any(|n| n == name)
}

fn reasoning_item(id: &str, text: &str) -> Value {
    json!({"type": "reasoning", "id": id, "summary": [{"type": "summary_text", "text": text}]})
}

fn message_item(id: &str, text: &str, status: &str) -> Value {
    json!({"type": "message", "id": id, "status": status, "role": "assistant",
        "content": [{"type": "output_text", "text": text, "annotations": []}]})
}

#[allow(clippy::too_many_arguments)]
fn responses_envelope(
    id: &str,
    created: i64,
    model: &str,
    status: &str,
    output: Vec<Value>,
    usage: Option<Usage>,
    incomplete: Option<&str>,
    error: Option<Value>,
) -> Value {
    json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": status,
        "background": false,
        "error": error.unwrap_or(Value::Null),
        "incomplete_details": incomplete.map(|r| json!({"reason": r})).unwrap_or(Value::Null),
        "instructions": null,
        "max_output_tokens": null,
        "model": model,
        "output": output,
        "parallel_tool_calls": true,
        "previous_response_id": null,
        "reasoning": {"effort": null, "summary": null},
        "store": false,
        "temperature": null,
        "text": {"format": {"type": "text"}},
        "tool_choice": "auto",
        "tools": [],
        "top_p": null,
        "truncation": "disabled",
        "usage": usage.map(usage_to_responses).unwrap_or(Value::Null),
        "user": null,
        "metadata": {},
    })
}

fn anthropic_message(
    id: &str,
    model: &str,
    reasoning: &str,
    text: &str,
    tools: &[(String, String, String)],
    stop_reason: &str,
    usage: Usage,
) -> Value {
    let mut content = Vec::new();
    if !reasoning.is_empty() {
        content.push(json!({"type": "thinking", "thinking": reasoning, "signature": ""}));
    }
    if !text.is_empty() || (tools.is_empty() && reasoning.is_empty()) {
        content.push(json!({"type": "text", "text": text}));
    }
    for (cid, name, args) in tools {
        content.push(json!({"type": "tool_use", "id": cid, "name": name, "input": parse_args(args)}));
    }
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_to_anthropic(usage),
    })
}

pub fn response_from_chat(to: Proto, chat: &Value, ctx: &ReqCtx) -> Result<Value> {
    if !chat.is_object() {
        bail!("response body must be a JSON object");
    }
    let t = parse_chat_response(chat);
    let model = if ctx.model.is_empty() { t.model.clone() } else { ctx.model.clone() };
    match to {
        Proto::Chat => Ok(chat.clone()),
        Proto::Responses => {
            let mut output = Vec::new();
            if !t.reasoning.is_empty() {
                output.push(reasoning_item(&gen_id("rs_"), &t.reasoning));
            }
            if !t.text.is_empty() {
                output.push(message_item(&gen_id("msg_"), &t.text, "completed"));
            }
            for (cid, name, args) in &t.tools {
                let prefix = if is_custom(ctx, name) { "ctc_" } else { "fc_" };
                output.push(responses_tool_item(ctx, &gen_id(prefix), cid, name, args, "completed"));
            }
            let incomplete = match t.finish.as_str() {
                "length" => Some("max_output_tokens"),
                "content_filter" => Some("content_filter"),
                _ => None,
            };
            Ok(responses_envelope(
                &gen_id("resp_"),
                now_secs(),
                &model,
                if incomplete.is_some() { "incomplete" } else { "completed" },
                output,
                Some(t.usage),
                incomplete,
                None,
            ))
        }
        Proto::Anthropic => Ok(anthropic_message(
            &gen_id("msg_"),
            &model,
            &t.reasoning,
            &t.text,
            &t.tools,
            finish_to_anthropic(&t.finish, !t.tools.is_empty()),
            t.usage,
        )),
    }
}

// ---------------------------------------------------------------------------
// errors & models

fn extract_error_message(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let candidates = [
            v.get("error").and_then(|e| e.get("message")),
            v.get("message"),
            v.get("error"),
            v.get("detail"),
        ];
        for c in candidates.into_iter().flatten() {
            match c {
                Value::String(s) if !s.is_empty() => return s.clone(),
                Value::Null | Value::String(_) => {}
                Value::Object(o) if o.contains_key("message") => {}
                other if !other.is_object() => return other.to_string(),
                _ => {}
            }
        }
    }
    let t = text.trim();
    if t.is_empty() {
        String::new()
    } else {
        truncate_chars(t, 500)
    }
}

fn error_type(status: u16) -> &'static str {
    match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        _ => "api_error",
    }
}

pub fn error_body(to: Proto, status: u16, upstream_text: &str) -> Value {
    let mut msg = extract_error_message(upstream_text);
    if msg.is_empty() {
        msg = format!("upstream error (HTTP {status})");
    }
    match to {
        Proto::Anthropic => json!({"type": "error", "error": {"type": error_type(status), "message": msg}}),
        _ => json!({"error": {"message": msg, "type": error_type(status), "code": status}}),
    }
}

pub fn models_body(to: Proto, upstream: &Value) -> Value {
    let list = if upstream.is_array() {
        upstream.as_array().map(Vec::as_slice).unwrap_or(&[])
    } else if upstream.get("data").map(Value::is_array).unwrap_or(false) {
        arr(upstream.get("data"))
    } else {
        arr(upstream.get("models"))
    };
    let mut seen = HashSet::new();
    let mut models: Vec<(String, String, String)> = Vec::new(); // (id, display, owner)
    for m in list {
        let (id, display, owner) = match m {
            Value::String(s) => (s.clone(), s.clone(), String::new()),
            Value::Object(_) => {
                let id = [sget(m, "id"), sget(m, "name"), sget(m, "model"), sget(m, "slug")]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .unwrap_or("");
                let id = id.strip_prefix("models/").unwrap_or(id).to_string();
                let display = [sget(m, "display_name"), sget(m, "displayName")]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| id.clone());
                (id, display, sget(m, "owned_by").to_string())
            }
            _ => continue,
        };
        if !id.is_empty() && seen.insert(id.clone()) {
            models.push((id, display, owner));
        }
    }
    match to {
        Proto::Anthropic => {
            let data: Vec<Value> = models
                .iter()
                .map(|(id, d, _)| json!({"type": "model", "id": id, "display_name": d,
                    "created_at": "1970-01-01T00:00:00Z"}))
                .collect();
            json!({
                "data": data,
                "has_more": false,
                "first_id": models.first().map(|m| json!(m.0)).unwrap_or(Value::Null),
                "last_id": models.last().map(|m| json!(m.0)).unwrap_or(Value::Null),
            })
        }
        _ => json!({
            "object": "list",
            "data": models.iter().map(|(id, _, o)| json!({"id": id, "object": "model", "created": 0,
                "owned_by": if o.is_empty() { "agentplus" } else { o.as_str() }})).collect::<Vec<_>>(),
        }),
    }
}

// ---------------------------------------------------------------------------
// streaming: upstream SSE -> chat chunks

struct RespTool {
    ti: usize,
    custom: bool,
    args_emitted: bool,
    buf: String,
    done: bool,
}

/// Parses upstream SSE (fed raw text chunks of any size, may split lines anywhere) into chat.completion.chunk Values.
pub struct UpstreamStream {
    from: Proto,
    buf: String,
    id: String,
    model: String,
    created: i64,
    started: bool,
    role_sent: bool,
    done: bool,
    tool_count: usize,
    finish: Option<String>,
    usage: Option<Usage>,
    // anthropic: content block index -> tool index
    block_tool: HashMap<u64, usize>,
    // responses: item id -> tool state; output_index -> item id; items that streamed text
    resp_tools: HashMap<String, RespTool>,
    oi_item: HashMap<u64, String>,
    text_items: HashSet<String>,
}

impl UpstreamStream {
    pub fn new(from: Proto) -> Self {
        UpstreamStream {
            from,
            buf: String::new(),
            id: gen_id("chatcmpl-"),
            model: String::new(),
            created: now_secs(),
            started: false,
            role_sent: false,
            done: false,
            tool_count: 0,
            finish: None,
            usage: None,
            block_tool: HashMap::new(),
            resp_tools: HashMap::new(),
            oi_item: HashMap::new(),
            text_items: HashSet::new(),
        }
    }

    pub fn feed(&mut self, bytes: &str) -> Vec<Value> {
        self.buf.push_str(bytes);
        if self.buf.contains('\r') {
            self.buf = self.buf.replace("\r\n", "\n");
        }
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find("\n\n") {
            let block: String = self.buf[..pos].to_string();
            self.buf.drain(..pos + 2);
            self.handle_block(&block, &mut out);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.buf);
        let rest = rest.replace('\r', "");
        if !rest.trim().is_empty() {
            self.handle_block(&rest, &mut out);
        }
        if self.from != Proto::Chat && self.started {
            self.emit_final(&mut out);
        }
        out
    }

    fn handle_block(&mut self, block: &str, out: &mut Vec<Value>) {
        let mut event = String::new();
        let mut data: Vec<&str> = Vec::new();
        for line in block.split('\n') {
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            match field {
                "event" => event = value.trim().to_string(),
                "data" => data.push(value),
                _ => {}
            }
        }
        let data = data.join("\n");
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return };
        let ty = match v.get("type").and_then(Value::as_str) {
            Some(t) => t.to_string(),
            None => event,
        };
        self.started = true;
        match self.from {
            Proto::Chat => {
                if let Some(e) = nonnull(v.get("error")) {
                    let msg = match e {
                        Value::String(s) => s.clone(),
                        o => sget(o, "message").to_string(),
                    };
                    out.push(self.error_chunk(&msg));
                } else if v.get("choices").is_some() {
                    out.push(v);
                }
            }
            Proto::Anthropic => self.anthropic_event(&ty, &v, out),
            Proto::Responses => self.responses_event(&ty, &v, out),
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    }

    fn error_chunk(&mut self, msg: &str) -> Value {
        self.done = true;
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "error": {"message": if msg.is_empty() { "upstream stream error" } else { msg }, "type": "upstream_error"},
        })
    }

    fn role(&mut self, out: &mut Vec<Value>) {
        if !self.role_sent {
            self.role_sent = true;
            out.push(self.chunk(json!({"role": "assistant", "content": ""}), None));
        }
    }

    fn content(&mut self, s: &str, out: &mut Vec<Value>) {
        if !s.is_empty() {
            self.role(out);
            out.push(self.chunk(json!({"content": s}), None));
        }
    }

    fn reasoning(&mut self, s: &str, out: &mut Vec<Value>) {
        if !s.is_empty() {
            self.role(out);
            out.push(self.chunk(json!({"reasoning_content": s}), None));
        }
    }

    fn tool_start(&mut self, id: &str, name: &str, out: &mut Vec<Value>) -> usize {
        self.role(out);
        let ti = self.tool_count;
        self.tool_count += 1;
        out.push(self.chunk(
            json!({"tool_calls": [{"index": ti, "id": id, "type": "function",
                "function": {"name": name, "arguments": ""}}]}),
            None,
        ));
        ti
    }

    fn tool_args(&self, ti: usize, args: &str, out: &mut Vec<Value>) {
        if !args.is_empty() {
            out.push(self.chunk(
                json!({"tool_calls": [{"index": ti, "function": {"arguments": args}}]}),
                None,
            ));
        }
    }

    fn emit_final(&mut self, out: &mut Vec<Value>) {
        if self.done {
            return;
        }
        self.done = true;
        self.role(out);
        let finish = self.finish.clone().unwrap_or_else(|| {
            if self.tool_count > 0 { "tool_calls" } else { "stop" }.to_string()
        });
        out.push(self.chunk(json!({}), Some(&finish)));
        if let Some(u) = self.usage {
            out.push(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": usage_to_chat(u),
            }));
        }
    }

    fn anthropic_event(&mut self, ty: &str, v: &Value, out: &mut Vec<Value>) {
        if self.done && ty != "error" {
            return;
        }
        match ty {
            "message_start" => {
                let m = v.get("message").unwrap_or(&Value::Null);
                if !sget(m, "id").is_empty() {
                    self.id = sget(m, "id").to_string();
                }
                if !sget(m, "model").is_empty() {
                    self.model = sget(m, "model").to_string();
                }
                if let Some(u) = m.get("usage") {
                    self.usage = Some(usage_from_anthropic(u));
                }
                self.role(out);
            }
            "content_block_start" => {
                let b = v.get("content_block").unwrap_or(&Value::Null);
                match sget(b, "type") {
                    "tool_use" => {
                        let ti = self.tool_start(sget(b, "id"), sget(b, "name"), out);
                        self.block_tool.insert(uget(v, "index"), ti);
                    }
                    "text" => self.content(sget(b, "text"), out),
                    "thinking" => self.reasoning(sget(b, "thinking"), out),
                    _ => {}
                }
            }
            "content_block_delta" => {
                let d = v.get("delta").unwrap_or(&Value::Null);
                match sget(d, "type") {
                    "text_delta" => self.content(sget(d, "text"), out),
                    "thinking_delta" => self.reasoning(sget(d, "thinking"), out),
                    "input_json_delta" => {
                        if let Some(&ti) = self.block_tool.get(&uget(v, "index")) {
                            self.tool_args(ti, sget(d, "partial_json"), out);
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(sr) = v.get("delta").and_then(|d| d.get("stop_reason")).and_then(Value::as_str) {
                    self.finish = Some(anthropic_stop_to_finish(sr).to_string());
                }
                if let Some(u) = v.get("usage") {
                    let mut cur = self.usage.unwrap_or_default();
                    if u.get("output_tokens").is_some() {
                        cur.output = uget(u, "output_tokens");
                    }
                    if uget(u, "input_tokens") > 0 {
                        let nu = usage_from_anthropic(u);
                        cur.input = nu.input;
                        cur.cached = nu.cached;
                    }
                    self.usage = Some(cur);
                }
            }
            "message_stop" => self.emit_final(out),
            "error" => {
                let e = v.get("error").unwrap_or(v);
                let msg = sget(e, "message").to_string();
                let c = self.error_chunk(&msg);
                out.push(c);
            }
            _ => {} // ping, unknown
        }
    }

    fn resp_tool_key(&self, v: &Value) -> Option<String> {
        let id = sget(v, "item_id");
        if !id.is_empty() && self.resp_tools.contains_key(id) {
            return Some(id.to_string());
        }
        v.get("output_index")
            .and_then(Value::as_u64)
            .and_then(|oi| self.oi_item.get(&oi).cloned())
    }

    /// Register a Responses tool item (idempotent); returns its item key.
    fn resp_tool_open(&mut self, item: &Value, oi: Option<u64>, out: &mut Vec<Value>) -> String {
        let key = match sget(item, "id") {
            "" => format!("oi_{}", oi.unwrap_or(self.tool_count as u64 + 1000)),
            s => s.to_string(),
        };
        if !self.resp_tools.contains_key(&key) {
            let call_id = match sget(item, "call_id") {
                "" => key.clone(),
                s => s.to_string(),
            };
            let ti = self.tool_start(&call_id, sget(item, "name"), out);
            self.resp_tools.insert(
                key.clone(),
                RespTool {
                    ti,
                    custom: sget(item, "type") == "custom_tool_call",
                    args_emitted: false,
                    buf: String::new(),
                    done: false,
                },
            );
        }
        if let Some(oi) = oi {
            self.oi_item.insert(oi, key.clone());
        }
        key
    }

    fn responses_event(&mut self, ty: &str, v: &Value, out: &mut Vec<Value>) {
        if self.done && ty != "error" && ty != "response.failed" {
            return;
        }
        match ty {
            "response.created" | "response.in_progress" => {
                if let Some(r) = v.get("response") {
                    if !sget(r, "id").is_empty() {
                        self.id = sget(r, "id").to_string();
                    }
                    if !sget(r, "model").is_empty() {
                        self.model = sget(r, "model").to_string();
                    }
                }
                self.role(out);
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.text_items.insert(sget(v, "item_id").to_string());
                self.content(sget(v, "delta"), out);
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.reasoning(sget(v, "delta"), out);
            }
            "response.reasoning_summary_part.added" => {
                if uget(v, "summary_index") > 0 {
                    self.reasoning("\n\n", out);
                }
            }
            "response.output_item.added" => {
                let item = v.get("item").unwrap_or(&Value::Null);
                if matches!(sget(item, "type"), "function_call" | "custom_tool_call") {
                    self.resp_tool_open(item, v.get("output_index").and_then(Value::as_u64), out);
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(key) = self.resp_tool_key(v) {
                    let t = self.resp_tools.get_mut(&key).unwrap();
                    let delta = sget(v, "delta");
                    if t.custom {
                        t.buf.push_str(delta);
                    } else if !delta.is_empty() {
                        t.args_emitted = true;
                        let ti = t.ti;
                        self.tool_args(ti, delta, out);
                    }
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(key) = self.resp_tool_key(v) {
                    let t = self.resp_tools.get_mut(&key).unwrap();
                    let args = sget(v, "arguments");
                    if !t.custom && !t.args_emitted && !args.is_empty() {
                        t.args_emitted = true;
                        let ti = t.ti;
                        self.tool_args(ti, args, out);
                    }
                }
            }
            "response.custom_tool_call_input.delta" => {
                if let Some(key) = self.resp_tool_key(v) {
                    self.resp_tools.get_mut(&key).unwrap().buf.push_str(sget(v, "delta"));
                }
            }
            "response.custom_tool_call_input.done" => {
                if let Some(key) = self.resp_tool_key(v) {
                    if let Some(input) = v.get("input").and_then(Value::as_str) {
                        self.resp_tools.get_mut(&key).unwrap().buf = input.to_string();
                    }
                }
            }
            "response.output_item.done" => {
                let item = v.get("item").unwrap_or(&Value::Null);
                let ity = sget(item, "type");
                match ity {
                    "function_call" | "custom_tool_call" => {
                        let key = self.resp_tool_open(item, v.get("output_index").and_then(Value::as_u64), out);
                        let t = self.resp_tools.get_mut(&key).unwrap();
                        if t.done {
                            return;
                        }
                        t.done = true;
                        let ti = t.ti;
                        if ity == "custom_tool_call" || t.custom {
                            let input = if t.buf.is_empty() {
                                output_text(item.get("input"))
                            } else {
                                t.buf.clone()
                            };
                            self.tool_args(ti, &json!({"input": input}).to_string(), out);
                        } else if !t.args_emitted {
                            t.args_emitted = true;
                            let args = output_text(item.get("arguments"));
                            self.tool_args(ti, &args, out);
                        }
                    }
                    // Text that arrived only in the final item (no deltas streamed).
                    "message" if !self.text_items.contains(sget(item, "id")) => {
                        let t = parts_text(&chat_parts(item.get("content").unwrap_or(&Value::Null)));
                        self.text_items.insert(sget(item, "id").to_string());
                        self.content(&t, out);
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" | "response.done" => {
                let r = v.get("response").unwrap_or(&Value::Null);
                if let Some(u) = nonnull(r.get("usage")) {
                    self.usage = Some(usage_from_responses(u));
                }
                if sget(r, "status") == "incomplete" || ty == "response.incomplete" {
                    let reason = r.get("incomplete_details").map(|d| sget(d, "reason")).unwrap_or("");
                    self.finish = Some(if reason == "content_filter" { "content_filter" } else { "length" }.into());
                }
                self.emit_final(out);
            }
            "response.failed" => {
                let r = v.get("response").unwrap_or(&Value::Null);
                let msg = r.get("error").map(|e| sget(e, "message")).unwrap_or("").to_string();
                let msg = if msg.is_empty() { "response failed".to_string() } else { msg };
                let c = self.error_chunk(&msg);
                out.push(c);
            }
            "error" => {
                let msg = match sget(v, "message") {
                    "" => v.get("error").map(|e| sget(e, "message")).unwrap_or("").to_string(),
                    s => s.to_string(),
                };
                let c = self.error_chunk(&msg);
                out.push(c);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// streaming: chat chunks -> client SSE frames

enum Cur {
    None,
    Text { item_id: String, oi: u64, text: String },
    Reasoning { item_id: String, oi: u64, text: String },
    Tool(u64),
}

struct DTool {
    item_id: String,
    oi: u64, // Responses output_index / Anthropic block index
    call_id: String,
    name: String,
    args: String,
    custom: bool,
}

/// Turns chat chunks into the client's SSE frames. Each returned String is a complete SSE frame.
pub struct DownstreamStream {
    to: Proto,
    ctx: ReqCtx,
    id: String,
    created: i64,
    model: String,
    started: bool,
    finished: bool,
    usage: Option<Usage>,
    usage_sent: bool,
    finish_reason: Option<String>,
    seq: u64,
    next_index: u64,
    cur: Cur,
    tools: BTreeMap<u64, DTool>,
    output: Vec<Value>,
}

impl DownstreamStream {
    pub fn new(to: Proto, ctx: ReqCtx) -> Self {
        let id = match to {
            Proto::Chat => gen_id("chatcmpl-"),
            Proto::Responses => gen_id("resp_"),
            Proto::Anthropic => gen_id("msg_"),
        };
        DownstreamStream {
            to,
            model: ctx.model.clone(),
            ctx,
            id,
            created: now_secs(),
            started: false,
            finished: false,
            usage: None,
            usage_sent: false,
            finish_reason: None,
            seq: 0,
            next_index: 0,
            cur: Cur::None,
            tools: BTreeMap::new(),
            output: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &Value) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        if let Some(e) = nonnull(chunk.get("error")) {
            let msg = match e {
                Value::String(s) => s.clone(),
                o => sget(o, "message").to_string(),
            };
            let status = e.get("code").and_then(Value::as_u64).unwrap_or(502) as u16;
            return self.error(status, &msg);
        }
        if self.model.is_empty() {
            self.model = sget(chunk, "model").to_string();
        }
        if let Some(u) = nonnull(chunk.get("usage")) {
            self.usage = Some(usage_from_chat(u));
        }
        if self.to == Proto::Chat {
            return self.push_chat(chunk);
        }
        let mut out = Vec::new();
        self.start(&mut out);
        let Some(ch) = chunk.get("choices").and_then(|c| c.get(0)) else {
            return out;
        };
        let d = ch.get("delta").unwrap_or(&Value::Null);
        let reasoning = [sget(d, "reasoning_content"), sget(d, "reasoning")]
            .into_iter()
            .find(|s| !s.is_empty());
        if let Some(r) = reasoning {
            self.on_reasoning(r, &mut out);
        }
        if let Some(t) = d.get("content").and_then(Value::as_str) {
            if !t.is_empty() {
                self.on_text(t, &mut out);
            }
        }
        for (pos, tc) in arr(d.get("tool_calls")).iter().enumerate() {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(pos as u64);
            let f = tc.get("function").unwrap_or(&Value::Null);
            let args = match f.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                None | Some(Value::Null) => String::new(),
                Some(o) => o.to_string(),
            };
            self.on_tool(idx, sget(tc, "id"), sget(f, "name"), &args, &mut out);
        }
        if let Some(f) = ch.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(f.to_string());
        }
        out
    }

    pub fn finish(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        match self.to {
            Proto::Chat => {
                if let (Some(u), false) = (self.usage, self.usage_sent) {
                    let c = json!({"id": self.id, "object": "chat.completion.chunk", "created": self.created,
                        "model": self.model, "choices": [], "usage": usage_to_chat(u)});
                    out.push(format!("data: {c}\n\n"));
                    self.usage_sent = true;
                }
                out.push("data: [DONE]\n\n".into());
            }
            Proto::Responses => {
                self.start(&mut out);
                self.close_current(&mut out);
                let incomplete = match self.finish_reason.as_deref() {
                    Some("length") => Some("max_output_tokens"),
                    Some("content_filter") => Some("content_filter"),
                    _ => None,
                };
                let resp = responses_envelope(
                    &self.id,
                    self.created,
                    &self.model,
                    if incomplete.is_some() { "incomplete" } else { "completed" },
                    self.output.clone(),
                    Some(self.usage.unwrap_or_default()),
                    incomplete,
                    None,
                );
                out.push(self.rev("response.completed", json!({"response": resp})));
            }
            Proto::Anthropic => {
                self.start(&mut out);
                self.close_current(&mut out);
                let stop = finish_to_anthropic(self.finish_reason.as_deref().unwrap_or(""), !self.tools.is_empty());
                let u = self.usage.unwrap_or_default();
                let mut usage = usage_to_anthropic(u);
                usage["output_tokens"] = json!(u.output);
                out.push(self.aev("message_delta", json!({
                    "delta": {"stop_reason": stop, "stop_sequence": null},
                    "usage": usage,
                })));
                out.push(self.aev("message_stop", json!({})));
            }
        }
        self.finished = true;
        out
    }

    pub fn error(&mut self, status: u16, msg: &str) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        let msg = if msg.is_empty() { "upstream error" } else { msg };
        let mut out = Vec::new();
        match self.to {
            Proto::Chat => {
                let e = json!({"error": {"message": msg, "type": "upstream_error", "code": status}});
                out.push(format!("data: {e}\n\n"));
                out.push("data: [DONE]\n\n".into());
            }
            Proto::Responses => {
                self.start(&mut out);
                let code = match status {
                    429 => "rate_limit_exceeded",
                    400 | 422 => "invalid_request",
                    _ => "upstream_error",
                };
                let resp = responses_envelope(
                    &self.id,
                    self.created,
                    &self.model,
                    "failed",
                    self.output.clone(),
                    self.usage,
                    None,
                    Some(json!({"code": code, "message": msg})),
                );
                out.push(self.rev("response.failed", json!({"response": resp})));
            }
            Proto::Anthropic => {
                let e = json!({"type": "error", "error": {"type": error_type(status), "message": msg}});
                out.push(format!("event: error\ndata: {e}\n\n"));
            }
        }
        self.finished = true;
        out
    }

    fn push_chat(&mut self, chunk: &Value) -> Vec<String> {
        let mut c = chunk.clone();
        let usage_only = arr(chunk.get("choices")).is_empty();
        if usage_only {
            return Vec::new(); // usage-only chunk is emitted by finish()
        }
        if nonnull(chunk.get("usage")).is_some() {
            self.usage_sent = true;
        }
        if let Some(o) = c.as_object_mut() {
            o.insert("id".into(), json!(self.id));
            o.insert("object".into(), json!("chat.completion.chunk"));
            o.insert("created".into(), json!(self.created));
            o.insert("model".into(), json!(self.model));
        }
        vec![format!("data: {c}\n\n")]
    }

    /// Responses event frame with type + sequence_number first.
    fn rev(&mut self, ty: &str, fields: Value) -> String {
        let mut m = Map::new();
        m.insert("type".into(), json!(ty));
        m.insert("sequence_number".into(), json!(self.seq));
        self.seq += 1;
        if let Value::Object(f) = fields {
            m.extend(f);
        }
        format!("event: {ty}\ndata: {}\n\n", Value::Object(m))
    }

    /// Anthropic event frame with type first.
    fn aev(&self, ty: &str, fields: Value) -> String {
        let mut m = Map::new();
        m.insert("type".into(), json!(ty));
        if let Value::Object(f) = fields {
            m.extend(f);
        }
        format!("event: {ty}\ndata: {}\n\n", Value::Object(m))
    }

    fn start(&mut self, out: &mut Vec<String>) {
        if self.started {
            return;
        }
        self.started = true;
        match self.to {
            Proto::Chat => {}
            Proto::Responses => {
                let r = responses_envelope(&self.id, self.created, &self.model, "in_progress", vec![], None, None, None);
                out.push(self.rev("response.created", json!({"response": r.clone()})));
                out.push(self.rev("response.in_progress", json!({"response": r})));
            }
            Proto::Anthropic => {
                let m = json!({"message": {"id": self.id, "type": "message", "role": "assistant",
                    "model": self.model, "content": [], "stop_reason": null, "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}}});
                out.push(self.aev("message_start", m));
            }
        }
    }

    fn next_idx(&mut self) -> u64 {
        let i = self.next_index;
        self.next_index += 1;
        i
    }

    fn close_current(&mut self, out: &mut Vec<String>) {
        match std::mem::replace(&mut self.cur, Cur::None) {
            Cur::None => {}
            Cur::Text { item_id, oi, text } => {
                if self.to == Proto::Anthropic {
                    out.push(self.aev("content_block_stop", json!({"index": oi})));
                    return;
                }
                let base = json!({"item_id": item_id, "output_index": oi, "content_index": 0});
                let mut f = base.clone();
                f["text"] = json!(text);
                f["logprobs"] = json!([]);
                out.push(self.rev("response.output_text.done", f));
                let mut f = base;
                f["part"] = json!({"type": "output_text", "text": text, "annotations": []});
                out.push(self.rev("response.content_part.done", f));
                let item = message_item(&item_id, &text, "completed");
                out.push(self.rev("response.output_item.done", json!({"output_index": oi, "item": item.clone()})));
                self.output.push(item);
            }
            Cur::Reasoning { item_id, oi, text } => {
                if self.to == Proto::Anthropic {
                    out.push(self.aev("content_block_stop", json!({"index": oi})));
                    return;
                }
                let base = json!({"item_id": item_id, "output_index": oi, "summary_index": 0});
                let mut f = base.clone();
                f["text"] = json!(text);
                out.push(self.rev("response.reasoning_summary_text.done", f));
                let mut f = base;
                f["part"] = json!({"type": "summary_text", "text": text});
                out.push(self.rev("response.reasoning_summary_part.done", f));
                let item = reasoning_item(&item_id, &text);
                out.push(self.rev("response.output_item.done", json!({"output_index": oi, "item": item.clone()})));
                self.output.push(item);
            }
            Cur::Tool(idx) => {
                let Some(t) = self.tools.get(&idx) else { return };
                let (item_id, oi, call_id, name, args, custom) =
                    (t.item_id.clone(), t.oi, t.call_id.clone(), t.name.clone(), t.args.clone(), t.custom);
                if self.to == Proto::Anthropic {
                    out.push(self.aev("content_block_stop", json!({"index": oi})));
                    return;
                }
                if custom {
                    let input = custom_input(&args);
                    out.push(self.rev("response.custom_tool_call_input.delta",
                        json!({"item_id": item_id, "output_index": oi, "delta": input})));
                    out.push(self.rev("response.custom_tool_call_input.done",
                        json!({"item_id": item_id, "output_index": oi, "input": input})));
                } else {
                    out.push(self.rev("response.function_call_arguments.done",
                        json!({"item_id": item_id, "output_index": oi, "arguments": args})));
                }
                let item = responses_tool_item(&self.ctx, &item_id, &call_id, &name, &args, "completed");
                out.push(self.rev("response.output_item.done", json!({"output_index": oi, "item": item.clone()})));
                self.output.push(item);
            }
        }
    }

    fn on_text(&mut self, s: &str, out: &mut Vec<String>) {
        if !matches!(self.cur, Cur::Text { .. }) {
            self.close_current(out);
            let oi = self.next_idx();
            let item_id = gen_id("msg_");
            match self.to {
                Proto::Anthropic => out.push(self.aev("content_block_start",
                    json!({"index": oi, "content_block": {"type": "text", "text": ""}}))),
                _ => {
                    let item = json!({"type": "message", "id": item_id, "status": "in_progress",
                        "role": "assistant", "content": []});
                    out.push(self.rev("response.output_item.added", json!({"output_index": oi, "item": item})));
                    out.push(self.rev("response.content_part.added", json!({"item_id": item_id,
                        "output_index": oi, "content_index": 0,
                        "part": {"type": "output_text", "text": "", "annotations": []}})));
                }
            }
            self.cur = Cur::Text { item_id, oi, text: String::new() };
        }
        let (item_id, oi) = match &mut self.cur {
            Cur::Text { item_id, oi, text } => {
                text.push_str(s);
                (item_id.clone(), *oi)
            }
            _ => unreachable!(),
        };
        match self.to {
            Proto::Anthropic => out.push(self.aev("content_block_delta",
                json!({"index": oi, "delta": {"type": "text_delta", "text": s}}))),
            _ => out.push(self.rev("response.output_text.delta", json!({"item_id": item_id,
                "output_index": oi, "content_index": 0, "delta": s, "logprobs": []}))),
        }
    }

    fn on_reasoning(&mut self, s: &str, out: &mut Vec<String>) {
        if !matches!(self.cur, Cur::Reasoning { .. }) {
            self.close_current(out);
            let oi = self.next_idx();
            let item_id = gen_id("rs_");
            match self.to {
                Proto::Anthropic => out.push(self.aev("content_block_start",
                    json!({"index": oi, "content_block": {"type": "thinking", "thinking": ""}}))),
                _ => {
                    let item = json!({"type": "reasoning", "id": item_id, "summary": []});
                    out.push(self.rev("response.output_item.added", json!({"output_index": oi, "item": item})));
                    out.push(self.rev("response.reasoning_summary_part.added", json!({"item_id": item_id,
                        "output_index": oi, "summary_index": 0,
                        "part": {"type": "summary_text", "text": ""}})));
                }
            }
            self.cur = Cur::Reasoning { item_id, oi, text: String::new() };
        }
        let (item_id, oi) = match &mut self.cur {
            Cur::Reasoning { item_id, oi, text } => {
                text.push_str(s);
                (item_id.clone(), *oi)
            }
            _ => unreachable!(),
        };
        match self.to {
            Proto::Anthropic => out.push(self.aev("content_block_delta",
                json!({"index": oi, "delta": {"type": "thinking_delta", "thinking": s}}))),
            _ => out.push(self.rev("response.reasoning_summary_text.delta", json!({"item_id": item_id,
                "output_index": oi, "summary_index": 0, "delta": s}))),
        }
    }

    fn on_tool(&mut self, idx: u64, id: &str, name: &str, args: &str, out: &mut Vec<String>) {
        let is_cur = matches!(self.cur, Cur::Tool(i) if i == idx);
        if !is_cur {
            if let Some(t) = self.tools.get_mut(&idx) {
                // Late delta for an already-closed call: patch the stored item only.
                t.args.push_str(args);
                let (item_id, full) = (t.item_id.clone(), t.args.clone());
                if let Some(item) = self.output.iter_mut().find(|i| sget(i, "id") == item_id) {
                    if item.get("input").is_some() {
                        item["input"] = json!(custom_input(&full));
                    } else {
                        item["arguments"] = json!(full);
                    }
                }
                return;
            }
            self.close_current(out);
            let oi = self.next_idx();
            let custom = is_custom(&self.ctx, name);
            let call_id = if id.is_empty() { gen_id("call_") } else { id.to_string() };
            let item_id = gen_id(if custom { "ctc_" } else { "fc_" });
            match self.to {
                Proto::Anthropic => out.push(self.aev("content_block_start", json!({"index": oi,
                    "content_block": {"type": "tool_use", "id": call_id, "name": name, "input": {}}}))),
                _ => {
                    let mut item = responses_tool_item(&self.ctx, &item_id, &call_id, name, "", "in_progress");
                    if custom {
                        item["input"] = json!("");
                    }
                    out.push(self.rev("response.output_item.added", json!({"output_index": oi, "item": item})));
                }
            }
            self.tools.insert(idx, DTool {
                item_id,
                oi,
                call_id,
                name: name.to_string(),
                args: String::new(),
                custom,
            });
            self.cur = Cur::Tool(idx);
        }
        let t = self.tools.get_mut(&idx).unwrap();
        if t.name.is_empty() && !name.is_empty() {
            t.name = name.to_string();
        }
        if args.is_empty() {
            return;
        }
        t.args.push_str(args);
        let (item_id, oi, custom) = (t.item_id.clone(), t.oi, t.custom);
        match self.to {
            Proto::Anthropic => out.push(self.aev("content_block_delta",
                json!({"index": oi, "delta": {"type": "input_json_delta", "partial_json": args}}))),
            _ if custom => {} // custom tool input is emitted on close
            _ => out.push(self.rev("response.function_call_arguments.delta",
                json!({"item_id": item_id, "output_index": oi, "delta": args}))),
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse SSE frames into (event, data) pairs.
    fn frames(v: &[String]) -> Vec<(String, Value)> {
        v.iter()
            .map(|f| {
                assert!(f.ends_with("\n\n"), "frame must end with blank line: {f:?}");
                let mut ev = String::new();
                let mut data = String::new();
                for line in f.trim_end().lines() {
                    if let Some(e) = line.strip_prefix("event: ") {
                        ev = e.to_string();
                    } else if let Some(d) = line.strip_prefix("data: ") {
                        data = d.to_string();
                    }
                }
                let v = if data == "[DONE]" { json!("[DONE]") } else { serde_json::from_str(&data).unwrap() };
                (ev, v)
            })
            .collect()
    }

    fn feed_in_pieces(up: &mut UpstreamStream, text: &str, n: usize) -> Vec<Value> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        for c in chars.chunks(n) {
            out.extend(up.feed(&c.iter().collect::<String>()));
        }
        out.extend(up.finish());
        out
    }

    fn sse(event: &str, data: Value) -> String {
        format!("event: {event}\ndata: {data}\n\n")
    }

    #[test]
    fn proto_basics() {
        assert_eq!(Proto::from_api("responses"), Some(Proto::Responses));
        assert_eq!(Proto::from_api("Anthropic"), Some(Proto::Anthropic));
        assert_eq!(Proto::from_api("chat"), Some(Proto::Chat));
        assert_eq!(Proto::from_api("gemini"), None);
        assert_eq!(Proto::Chat.path(), "/chat/completions");
        assert_eq!(Proto::Responses.path(), "/responses");
        assert_eq!(Proto::Anthropic.path(), "/messages");
    }

    #[test]
    fn responses_request_to_chat_full() {
        let body = json!({
            "model": "gpt-5-codex",
            "instructions": "You are Codex.",
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "abc",
            "max_output_tokens": 1000,
            "reasoning": {"effort": "high", "summary": "auto"},
            "parallel_tool_calls": false,
            "tool_choice": "auto",
            "input": [
                {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "dev note"}]},
                {"role": "user", "content": [
                    {"type": "input_text", "text": "look"},
                    {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}
                ]},
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "thinking"}], "encrypted_content": "xx"},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Let me check."}]},
                {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"ls\"}"},
                {"type": "function_call", "call_id": "call_2", "name": "shell", "arguments": "{\"cmd\":\"pwd\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"},
                {"type": "function_call_output", "call_id": "call_2", "output": [{"type": "input_text", "text": "/home"}]},
                {"type": "custom_tool_call", "call_id": "call_3", "name": "apply_patch", "input": "*** Begin Patch"},
                {"type": "custom_tool_call_output", "call_id": "call_3", "output": "ok"},
                {"type": "local_shell_call", "call_id": "x", "action": {}},
                {"type": "totally_unknown", "foo": 1},
                {"no_type_no_role": true},
                {"type": "message", "role": "user", "content": "thanks"}
            ],
            "tools": [
                {"type": "function", "name": "shell", "description": "run", "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}, "strict": false},
                {"type": "custom", "name": "apply_patch", "description": "patch files", "format": {"type": "grammar", "syntax": "lark", "definition": "start: x"}},
                {"type": "web_search"},
                {"type": "local_shell"}
            ]
        });
        let chat = request_to_chat(Proto::Responses, &body).unwrap();
        let m = chat["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "You are Codex."}));
        assert_eq!(m[1], json!({"role": "system", "content": "dev note"}));
        assert_eq!(m[2]["role"], "user");
        assert_eq!(m[2]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        // assistant text + two function calls merged into one message
        assert_eq!(m[3]["role"], "assistant");
        assert_eq!(m[3]["content"], "Let me check.");
        assert_eq!(m[3]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(m[3]["tool_calls"][1]["function"]["arguments"], "{\"cmd\":\"pwd\"}");
        assert_eq!(m[4], json!({"role": "tool", "tool_call_id": "call_1", "content": "a.txt"}));
        assert_eq!(m[5]["content"], "/home");
        assert_eq!(m[6]["role"], "assistant");
        assert_eq!(m[6]["tool_calls"][0]["function"]["name"], "apply_patch");
        let args: Value = serde_json::from_str(m[6]["tool_calls"][0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args, json!({"input": "*** Begin Patch"}));
        assert_eq!(m[7], json!({"role": "tool", "tool_call_id": "call_3", "content": "ok"}));
        assert_eq!(m[8], json!({"role": "user", "content": "thanks"}));
        assert_eq!(m.len(), 9);

        let tools = chat["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["function"]["name"], "shell");
        assert_eq!(tools[1]["function"]["parameters"]["required"], json!(["input"]));
        assert_eq!(tools[1]["function"]["parameters"]["properties"]["input"]["description"], "start: x");
        assert_eq!(chat["max_tokens"], 1000);
        assert_eq!(chat["reasoning_effort"], "high");
        assert_eq!(chat["parallel_tool_calls"], false);
        assert_eq!(chat["tool_choice"], "auto");
        assert_eq!(chat["stream_options"]["include_usage"], true);
        assert!(chat.get("store").is_none() && chat.get("include").is_none());

        let ctx = req_ctx(Proto::Responses, &body);
        assert_eq!(ctx.model, "gpt-5-codex");
        assert!(ctx.stream);
        assert_eq!(ctx.custom_tools, vec!["apply_patch".to_string()]);

        // string input + object tool_choice
        let chat = request_to_chat(Proto::Responses, &json!({"model": "m", "input": "hi",
            "tools": [{"type": "function", "name": "f"}], "tool_choice": {"type": "function", "name": "f"}})).unwrap();
        assert_eq!(chat["messages"][0], json!({"role": "user", "content": "hi"}));
        assert_eq!(chat["tool_choice"], json!({"type": "function", "function": {"name": "f"}}));
        assert!(chat.get("stream_options").is_none());
    }

    #[test]
    fn chat_to_anthropic_request() {
        let chat = json!({
            "model": "claude-x",
            "messages": [
                {"role": "system", "content": "sys A"},
                {"role": "developer", "content": [{"type": "text", "text": "sys B"}]},
                {"role": "assistant", "content": "I start"},
                {"role": "user", "content": "q1"},
                {"role": "user", "content": [
                    {"type": "text", "text": "q2"},
                    {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,QUJD"}},
                    {"type": "image_url", "image_url": {"url": "https://x/y.png"}}
                ]},
                {"role": "assistant", "content": "calling", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "a", "arguments": "{\"x\":1}"}},
                    {"id": "call.2", "type": "function", "function": {"name": "b", "arguments": "not json"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "r1"},
                {"role": "tool", "tool_call_id": "call.2", "content": "r2"},
                {"role": "user", "content": "next"}
            ],
            "tools": [{"type": "function", "function": {"name": "a", "description": "d", "parameters": {"type": "object"}}}],
            "tool_choice": "required",
            "max_tokens": 1000,
            "temperature": 0.5,
            "stop": "END",
            "reasoning_effort": "high",
            "stream": true
        });
        let a = request_from_chat(Proto::Anthropic, &chat).unwrap();
        assert_eq!(a["system"], "sys A\n\nsys B");
        let m = a["messages"].as_array().unwrap();
        // first message must be user
        assert_eq!(m[0]["role"], "user");
        assert_eq!(m[0]["content"][0]["text"], "(continue)");
        assert_eq!(m[1]["role"], "assistant");
        // two user messages merged
        assert_eq!(m[2]["role"], "user");
        let u = m[2]["content"].as_array().unwrap();
        assert_eq!(u.len(), 4);
        assert_eq!(u[2]["source"], json!({"type": "base64", "media_type": "image/jpeg", "data": "QUJD"}));
        assert_eq!(u[3]["source"], json!({"type": "url", "url": "https://x/y.png"}));
        // assistant tool_use
        let asst = m[3]["content"].as_array().unwrap();
        assert_eq!(asst[0], json!({"type": "text", "text": "calling"}));
        assert_eq!(asst[1]["input"], json!({"x": 1}));
        assert_eq!(asst[2]["id"], "call_2");
        assert_eq!(asst[2]["input"], json!({"raw": "not json"}));
        // tool results merged with following user text, results first
        assert_eq!(m[4]["role"], "user");
        let tr = m[4]["content"].as_array().unwrap();
        assert_eq!(tr[0], json!({"type": "tool_result", "tool_use_id": "call_1", "content": "r1"}));
        assert_eq!(tr[1]["tool_use_id"], "call_2");
        assert_eq!(tr[2], json!({"type": "text", "text": "next"}));
        assert_eq!(m.len(), 5);
        // thinking budget rules
        assert_eq!(a["thinking"], json!({"type": "enabled", "budget_tokens": 16384}));
        assert_eq!(a["max_tokens"], 16384 + 4096);
        assert!(a.get("temperature").is_none());
        // forced tool choice is incompatible with thinking
        assert_eq!(a["tool_choice"], json!({"type": "auto"}));
        assert_eq!(a["stop_sequences"], json!(["END"]));
        assert_eq!(a["tools"][0]["input_schema"], json!({"type": "object"}));
        assert_eq!(a["stream"], true);

        // without thinking: tool_choice maps directly, default max_tokens, temperature kept
        let a = request_from_chat(Proto::Anthropic, &json!({"model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "a"}}],
            "tool_choice": {"type": "function", "function": {"name": "a"}},
            "parallel_tool_calls": false, "temperature": 0.2})).unwrap();
        assert_eq!(a["max_tokens"], 8192);
        assert_eq!(a["temperature"], 0.2);
        assert_eq!(a["tool_choice"], json!({"type": "tool", "name": "a", "disable_parallel_tool_use": true}));
        assert!(a.get("system").is_none());
        assert_eq!(a["messages"][0]["content"][0]["text"], "hi");

        // large max_tokens stays
        let a = request_from_chat(Proto::Anthropic, &json!({"model": "m", "max_tokens": 32000,
            "reasoning_effort": "low", "messages": []})).unwrap();
        assert_eq!(a["thinking"]["budget_tokens"], 2048);
        assert_eq!(a["max_tokens"], 32000);
        assert_eq!(a["messages"][0]["role"], "user");
    }

    #[test]
    fn anthropic_request_to_chat() {
        let body = json!({
            "model": "claude-sonnet",
            "system": [{"type": "text", "text": "S1"}, {"type": "text", "text": "S2", "cache_control": {"type": "ephemeral"}}],
            "max_tokens": 4096,
            "top_k": 5,
            "stop_sequences": ["X"],
            "metadata": {"user_id": "u"},
            "thinking": {"type": "enabled", "budget_tokens": 10000},
            "stream": true,
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "text", "text": "using tool"},
                    {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a"}},
                    {"type": "tool_use", "id": "toolu_2", "name": "read", "input": {"path": "b"}}
                ]},
                {"role": "user", "content": [
                    {"type": "text", "text": "after results"},
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "AAA"},
                    {"type": "tool_result", "tool_use_id": "toolu_2", "is_error": true, "content": [
                        {"type": "text", "text": "boom"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "Zm9v"}}
                    ]},
                    {"type": "image", "source": {"type": "url", "url": "https://img"}}
                ]}
            ],
            "tools": [
                {"name": "read", "description": "Read", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}},
                {"type": "web_search_20250305", "name": "web_search", "max_uses": 3},
                {"type": "custom", "name": "custom_one", "input_schema": {"type": "object"}}
            ],
            "tool_choice": {"type": "any"}
        });
        let c = request_to_chat(Proto::Anthropic, &body).unwrap();
        let m = c["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "S1\nS2"}));
        assert_eq!(m[1], json!({"role": "user", "content": "hello"}));
        assert_eq!(m[2]["content"], "using tool");
        assert_eq!(m[2]["reasoning_content"], "hmm");
        assert_eq!(m[2]["tool_calls"][1]["id"], "toolu_2");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"path\":\"a\"}");
        // tool results come before remaining user text
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "toolu_1", "content": "AAA"}));
        assert_eq!(m[4]["role"], "tool");
        assert_eq!(m[4]["content"], "[tool error] boom");
        assert_eq!(m[5]["role"], "user");
        let parts = m[5]["content"].as_array().unwrap();
        assert_eq!(parts[0], json!({"type": "text", "text": "after results"}));
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,Zm9v");
        assert_eq!(parts[2]["image_url"]["url"], "https://img");
        assert_eq!(m.len(), 6);
        let tools = c["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1]["function"]["name"], "custom_one");
        assert_eq!(c["tool_choice"], "required");
        assert_eq!(c["reasoning_effort"], "medium");
        assert_eq!(c["stop"], json!(["X"]));
        assert_eq!(c["max_tokens"], 4096);
        assert_eq!(c["stream_options"]["include_usage"], true);
        assert!(c.get("top_k").is_none() && c.get("metadata").is_none());

        let ctx = req_ctx(Proto::Anthropic, &body);
        assert_eq!(ctx.model, "claude-sonnet");
        assert!(ctx.custom_tools.is_empty());
    }

    #[test]
    fn chat_to_responses_request() {
        let chat = json!({"model": "m", "max_tokens": 50, "reasoning_effort": "low", "stream": true,
            "messages": [
                {"role": "system", "content": "S"},
                {"role": "user", "content": [{"type": "text", "text": "hi"}, {"type": "image_url", "image_url": {"url": "http://i"}}]},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "c1", "content": "out"}
            ],
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {"type": "object"}}}],
            "tool_choice": {"type": "function", "function": {"name": "f"}}});
        let r = request_from_chat(Proto::Responses, &chat).unwrap();
        assert_eq!(r["instructions"], "S");
        let input = r["input"].as_array().unwrap();
        assert_eq!(input[0]["content"][1], json!({"type": "input_image", "image_url": "http://i"}));
        assert_eq!(input[1], json!({"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"}));
        assert_eq!(input[2], json!({"type": "function_call_output", "call_id": "c1", "output": "out"}));
        assert_eq!(r["tools"][0]["name"], "f");
        assert_eq!(r["tool_choice"], json!({"type": "function", "name": "f"}));
        assert_eq!(r["max_output_tokens"], 50);
        assert_eq!(r["reasoning"]["effort"], "low");
        assert_eq!(r["store"], false);
        assert_eq!(r["stream"], true);
        // chat -> chat is a clone
        assert_eq!(request_from_chat(Proto::Chat, &chat).unwrap(), chat);
        assert_eq!(request_to_chat(Proto::Chat, &chat).unwrap(), chat);
        assert!(request_to_chat(Proto::Responses, &json!("nope")).is_err());
    }

    fn sample_chat_response() -> Value {
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1, "model": "up-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello",
                "reasoning_content": "think",
                "tool_calls": [
                    {"id": "call_a", "type": "function", "function": {"name": "shell", "arguments": "{\"cmd\":\"ls\"}"}},
                    {"id": "call_b", "type": "function", "function": {"name": "apply_patch", "arguments": "{\"input\":\"*** Begin Patch\"}"}}
                ]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })
    }

    #[test]
    fn nonstream_roundtrip_responses() {
        let ctx = ReqCtx { model: "client-model".into(), custom_tools: vec!["apply_patch".into()], stream: false };
        let r = response_from_chat(Proto::Responses, &sample_chat_response(), &ctx).unwrap();
        assert_eq!(r["object"], "response");
        assert_eq!(r["status"], "completed");
        assert_eq!(r["model"], "client-model");
        assert!(r["id"].as_str().unwrap().starts_with("resp_"));
        let out = r["output"].as_array().unwrap();
        assert_eq!(out[0]["type"], "reasoning");
        assert_eq!(out[0]["summary"][0]["text"], "think");
        assert_eq!(out[1]["type"], "message");
        assert_eq!(out[1]["content"][0], json!({"type": "output_text", "text": "Hello", "annotations": []}));
        assert_eq!(out[2]["type"], "function_call");
        assert_eq!(out[2]["call_id"], "call_a");
        assert_eq!(out[3]["type"], "custom_tool_call");
        assert_eq!(out[3]["input"], "*** Begin Patch");
        assert_eq!(r["usage"]["input_tokens"], 10);
        assert_eq!(r["usage"]["total_tokens"], 15);
        assert_eq!(r["text"]["format"]["type"], "text");

        let back = response_to_chat(Proto::Responses, &r).unwrap();
        let msg = &back["choices"][0]["message"];
        assert_eq!(msg["content"], "Hello");
        assert_eq!(msg["reasoning_content"], "think");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"cmd\":\"ls\"}");
        assert_eq!(msg["tool_calls"][1]["function"]["arguments"], "{\"input\":\"*** Begin Patch\"}");
        assert_eq!(back["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(back["usage"], json!({"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}));

        // length -> incomplete and back
        let mut c = sample_chat_response();
        c["choices"][0]["finish_reason"] = json!("length");
        c["choices"][0]["message"] = json!({"role": "assistant", "content": "cut"});
        let r = response_from_chat(Proto::Responses, &c, &ReqCtx::default()).unwrap();
        assert_eq!(r["status"], "incomplete");
        assert_eq!(r["incomplete_details"]["reason"], "max_output_tokens");
        assert_eq!(r["model"], "up-model");
        let back = response_to_chat(Proto::Responses, &r).unwrap();
        assert_eq!(back["choices"][0]["finish_reason"], "length");
        assert_eq!(back["choices"][0]["message"]["content"], "cut");
    }

    #[test]
    fn nonstream_roundtrip_anthropic() {
        let ctx = ReqCtx { model: "claude-client".into(), ..Default::default() };
        let a = response_from_chat(Proto::Anthropic, &sample_chat_response(), &ctx).unwrap();
        assert_eq!(a["type"], "message");
        assert_eq!(a["stop_reason"], "tool_use");
        assert_eq!(a["model"], "claude-client");
        let content = a["content"].as_array().unwrap();
        assert_eq!(content[0], json!({"type": "thinking", "thinking": "think", "signature": ""}));
        assert_eq!(content[1], json!({"type": "text", "text": "Hello"}));
        assert_eq!(content[2]["input"], json!({"cmd": "ls"}));
        assert_eq!(a["usage"], json!({"input_tokens": 10, "output_tokens": 5}));

        let back = response_to_chat(Proto::Anthropic, &a).unwrap();
        let msg = &back["choices"][0]["message"];
        assert_eq!(msg["content"], "Hello");
        assert_eq!(msg["reasoning_content"], "think");
        assert_eq!(msg["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(back["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(back["usage"]["prompt_tokens"], 10);

        // native anthropic response with cache usage
        let native = json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "claude",
            "content": [{"type": "text", "text": "Hi"}], "stop_reason": "max_tokens", "stop_sequence": null,
            "usage": {"input_tokens": 3, "cache_read_input_tokens": 7, "output_tokens": 2}});
        let c = response_to_chat(Proto::Anthropic, &native).unwrap();
        assert_eq!(c["choices"][0]["finish_reason"], "length");
        assert_eq!(c["usage"]["prompt_tokens"], 10);
        assert_eq!(c["usage"]["prompt_tokens_details"]["cached_tokens"], 7);
        assert_eq!(c["id"], "msg_1");
    }

    #[test]
    fn nonstream_chat_passthrough_and_responses_custom() {
        let c = sample_chat_response();
        assert_eq!(response_to_chat(Proto::Chat, &c).unwrap(), c);
        assert_eq!(response_from_chat(Proto::Chat, &c, &ReqCtx::default()).unwrap(), c);
        let r = json!({"id": "resp_x", "object": "response", "status": "completed", "model": "gpt",
            "output": [
                {"type": "reasoning", "id": "rs", "summary": []},
                {"type": "custom_tool_call", "id": "ctc", "call_id": "c9", "name": "apply_patch", "input": "PATCH"},
                {"type": "web_search_call", "id": "ws"}
            ],
            "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}});
        let c = response_to_chat(Proto::Responses, &r).unwrap();
        let tc = &c["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["id"], "c9");
        assert_eq!(serde_json::from_str::<Value>(tc["function"]["arguments"].as_str().unwrap()).unwrap(),
            json!({"input": "PATCH"}));
        assert_eq!(c["choices"][0]["message"]["content"], Value::Null);
        assert_eq!(c["choices"][0]["finish_reason"], "tool_calls");
    }

    fn anthropic_transcript() -> String {
        let mut s = String::new();
        s += &sse("message_start", json!({"type": "message_start", "message": {"id": "msg_up", "type": "message",
            "role": "assistant", "model": "claude-up", "content": [], "stop_reason": null,
            "usage": {"input_tokens": 25, "output_tokens": 1}}}));
        s += ": keepalive comment\n\n";
        s += &sse("ping", json!({"type": "ping"}));
        s += &sse("content_block_start", json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "thinking", "thinking": ""}}));
        s += &sse("content_block_delta", json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Let me think"}}));
        s += &sse("content_block_delta", json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "signature_delta", "signature": "abc"}}));
        s += &sse("content_block_stop", json!({"type": "content_block_stop", "index": 0}));
        s += &sse("content_block_start", json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "text", "text": ""}}));
        s += &sse("content_block_delta", json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "text_delta", "text": "Hello, "}}));
        s += &sse("content_block_delta", json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "text_delta", "text": "wörld 🌍!"}}));
        s += &sse("content_block_stop", json!({"type": "content_block_stop", "index": 1}));
        s += &sse("content_block_start", json!({"type": "content_block_start", "index": 2,
            "content_block": {"type": "tool_use", "id": "toolu_9", "name": "shell", "input": {}}}));
        for part in ["{\"cmd\"", ": \"ls ", "-la\"}"] {
            s += &sse("content_block_delta", json!({"type": "content_block_delta", "index": 2,
                "delta": {"type": "input_json_delta", "partial_json": part}}));
        }
        s += &sse("content_block_stop", json!({"type": "content_block_stop", "index": 2}));
        s += &sse("message_delta", json!({"type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 42}}));
        s += &sse("message_stop", json!({"type": "message_stop"}));
        s
    }

    #[test]
    fn stream_anthropic_upstream_to_responses_downstream() {
        let mut up = UpstreamStream::new(Proto::Anthropic);
        let chunks = feed_in_pieces(&mut up, &anthropic_transcript(), 7);
        assert!(chunks.iter().all(|c| c["object"] == "chat.completion.chunk"));
        let text: String = chunks.iter()
            .filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(text, "Hello, wörld 🌍!");
        let args: String = chunks.iter()
            .filter_map(|c| c["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str()).collect();
        assert_eq!(args, "{\"cmd\": \"ls -la\"}");
        let fin: Vec<&Value> = chunks.iter().filter(|c| !c["choices"][0]["finish_reason"].is_null()).collect();
        assert_eq!(fin.len(), 1);
        assert_eq!(fin[0]["choices"][0]["finish_reason"], "tool_calls");
        let usage = chunks.iter().find(|c| c.get("usage").is_some()).unwrap();
        assert_eq!(usage["usage"]["prompt_tokens"], 25);
        assert_eq!(usage["usage"]["completion_tokens"], 42);
        assert_eq!(chunks[0]["id"], "msg_up");

        let ctx = ReqCtx { model: "gpt-client".into(), custom_tools: vec![], stream: true };
        let mut down = DownstreamStream::new(Proto::Responses, ctx);
        let mut out = Vec::new();
        for c in &chunks {
            out.extend(down.push(c));
        }
        out.extend(down.finish());
        assert!(down.finish().is_empty());
        let fr = frames(&out);
        let types: Vec<&str> = fr.iter().map(|(e, _)| e.as_str()).collect();
        for (e, d) in &fr {
            assert_eq!(d["type"], e.as_str());
        }
        let seqs: Vec<u64> = fr.iter().map(|(_, d)| d["sequence_number"].as_u64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
        let mut dedup = types.clone();
        dedup.dedup();
        assert_eq!(dedup, vec![
            "response.created", "response.in_progress",
            "response.output_item.added", "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta", "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done", "response.output_item.done",
            "response.output_item.added", "response.content_part.added", "response.output_text.delta",
            "response.output_text.done", "response.content_part.done", "response.output_item.done",
            "response.output_item.added", "response.function_call_arguments.delta",
            "response.function_call_arguments.done", "response.output_item.done",
            "response.completed",
        ]);
        let (_, done) = fr.last().unwrap();
        let resp = &done["response"];
        assert_eq!(resp["status"], "completed");
        assert_eq!(resp["model"], "gpt-client");
        let output = resp["output"].as_array().unwrap();
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["summary"][0]["text"], "Let me think");
        assert_eq!(output[1]["content"][0]["text"], "Hello, wörld 🌍!");
        assert_eq!(output[2]["type"], "function_call");
        assert_eq!(output[2]["call_id"], "toolu_9");
        assert_eq!(output[2]["name"], "shell");
        assert_eq!(output[2]["arguments"], "{\"cmd\": \"ls -la\"}");
        assert_eq!(resp["usage"]["input_tokens"], 25);
        assert_eq!(resp["usage"]["output_tokens"], 42);
        assert_eq!(resp["usage"]["total_tokens"], 67);
        // output_index increments per item and matches added/done
        let added: Vec<u64> = fr.iter().filter(|(e, _)| e == "response.output_item.added")
            .map(|(_, d)| d["output_index"].as_u64().unwrap()).collect();
        assert_eq!(added, vec![0, 1, 2]);
        let fc_added = fr.iter().find(|(e, d)| e == "response.output_item.added" && d["item"]["type"] == "function_call").unwrap();
        assert_eq!(fc_added.1["item"]["arguments"], "");
        assert_eq!(fc_added.1["item"]["id"], output[2]["id"]);
    }

    #[test]
    fn stream_crlf_and_multiline_data() {
        let mut up = UpstreamStream::new(Proto::Anthropic);
        let t = anthropic_transcript().replace('\n', "\r\n");
        let chunks = feed_in_pieces(&mut up, &t, 3);
        let text: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(text, "Hello, wörld 🌍!");

        // multi-line data, [DONE], junk
        let mut up = UpstreamStream::new(Proto::Chat);
        let src = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\ndata: \"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: not json\n\ndata: [DONE]\n\n";
        let chunks = feed_in_pieces(&mut up, src, 5);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "hi");

        // Responses upstream that only sends the finished message item (no deltas)
        let mut up = UpstreamStream::new(Proto::Responses);
        let src = sse("response.output_item.done", json!({"type": "response.output_item.done", "output_index": 0,
            "item": {"type": "message", "id": "m1", "content": [{"type": "output_text", "text": "whole"}]}}))
            + &sse("response.completed", json!({"type": "response.completed", "response": {"status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}}}));
        let chunks = feed_in_pieces(&mut up, &src, 9);
        let text: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(text, "whole");
        assert_eq!(chunks.last().unwrap()["choices"][0]["finish_reason"], "length");
    }

    fn responses_transcript() -> String {
        let mut s = String::new();
        let ev = |t: &str, mut v: Value| {
            v["type"] = json!(t);
            sse(t, v)
        };
        s += &ev("response.created", json!({"response": {"id": "resp_up", "model": "gpt-up", "status": "in_progress", "output": []}}));
        s += &ev("response.output_item.added", json!({"output_index": 0, "item": {"type": "reasoning", "id": "rs_1", "summary": []}}));
        s += &ev("response.reasoning_summary_text.delta", json!({"item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "plan"}));
        s += &ev("response.output_item.done", json!({"output_index": 0, "item": {"type": "reasoning", "id": "rs_1"}}));
        s += &ev("response.output_item.added", json!({"output_index": 1, "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}}));
        s += &ev("response.output_text.delta", json!({"item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": "Sure"}));
        s += &ev("response.output_text.delta", json!({"item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": " thing"}));
        s += &ev("response.output_item.done", json!({"output_index": 1, "item": {"type": "message", "id": "msg_1",
            "content": [{"type": "output_text", "text": "Sure thing"}]}}));
        s += &ev("response.output_item.added", json!({"output_index": 2, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "shell", "arguments": ""}}));
        s += &ev("response.function_call_arguments.delta", json!({"item_id": "fc_1", "output_index": 2, "delta": "{\"cmd\":"}));
        s += &ev("response.function_call_arguments.delta", json!({"item_id": "fc_1", "output_index": 2, "delta": "\"ls\"}"}));
        s += &ev("response.function_call_arguments.done", json!({"item_id": "fc_1", "output_index": 2, "arguments": "{\"cmd\":\"ls\"}"}));
        s += &ev("response.output_item.done", json!({"output_index": 2, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "shell", "arguments": "{\"cmd\":\"ls\"}"}}));
        s += &ev("response.output_item.added", json!({"output_index": 3, "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_2", "name": "apply_patch", "input": ""}}));
        s += &ev("response.custom_tool_call_input.delta", json!({"item_id": "ctc_1", "output_index": 3, "delta": "*** Begin"}));
        s += &ev("response.custom_tool_call_input.delta", json!({"item_id": "ctc_1", "output_index": 3, "delta": " Patch\n\"q\""}));
        s += &ev("response.custom_tool_call_input.done", json!({"item_id": "ctc_1", "output_index": 3, "input": "*** Begin Patch\n\"q\""}));
        s += &ev("response.output_item.done", json!({"output_index": 3, "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_2", "name": "apply_patch", "input": "*** Begin Patch\n\"q\""}}));
        s += &ev("response.completed", json!({"response": {"id": "resp_up", "status": "completed",
            "usage": {"input_tokens": 100, "output_tokens": 20, "total_tokens": 120, "input_tokens_details": {"cached_tokens": 60}}}}));
        s
    }

    #[test]
    fn stream_responses_upstream_to_anthropic_downstream() {
        let mut up = UpstreamStream::new(Proto::Responses);
        let chunks = feed_in_pieces(&mut up, &responses_transcript(), 11);
        let reasoning: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["reasoning_content"].as_str()).collect();
        assert_eq!(reasoning, "plan");
        let text: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(text, "Sure thing");
        let mut args = [String::new(), String::new()];
        for c in &chunks {
            for tc in arr(c["choices"][0]["delta"].get("tool_calls")) {
                let i = tc["index"].as_u64().unwrap() as usize;
                args[i].push_str(tc["function"]["arguments"].as_str().unwrap_or(""));
            }
        }
        assert_eq!(args[0], "{\"cmd\":\"ls\"}");
        assert_eq!(serde_json::from_str::<Value>(&args[1]).unwrap(), json!({"input": "*** Begin Patch\n\"q\""}));
        let fin = chunks.iter().find(|c| !c["choices"][0]["finish_reason"].is_null()).unwrap();
        assert_eq!(fin["choices"][0]["finish_reason"], "tool_calls");
        let u = chunks.iter().find(|c| c.get("usage").is_some()).unwrap();
        assert_eq!(u["usage"]["prompt_tokens"], 100);
        assert_eq!(u["usage"]["prompt_tokens_details"]["cached_tokens"], 60);

        let mut down = DownstreamStream::new(Proto::Anthropic, ReqCtx { model: "claude-client".into(), ..Default::default() });
        let mut out = Vec::new();
        for c in &chunks {
            out.extend(down.push(c));
        }
        out.extend(down.finish());
        assert!(down.finish().is_empty());
        let fr = frames(&out);
        for (e, d) in &fr {
            assert_eq!(d["type"], e.as_str());
        }
        let types: Vec<&str> = fr.iter().map(|(e, _)| e.as_str()).collect();
        let mut dedup = types.clone();
        dedup.dedup();
        assert_eq!(dedup, vec![
            "message_start",
            "content_block_start", "content_block_delta", "content_block_stop", // thinking
            "content_block_start", "content_block_delta", "content_block_stop", // text
            "content_block_start", "content_block_delta", "content_block_stop", // tool 1
            "content_block_start", "content_block_delta", "content_block_stop", // tool 2
            "message_delta", "message_stop",
        ]);
        assert_eq!(fr[0].1["message"]["model"], "claude-client");
        let starts: Vec<&Value> = fr.iter().filter(|(e, _)| e == "content_block_start").map(|(_, d)| d).collect();
        assert_eq!(starts[0]["content_block"]["type"], "thinking");
        assert_eq!(starts[1]["content_block"]["type"], "text");
        assert_eq!(starts[2]["content_block"], json!({"type": "tool_use", "id": "call_1", "name": "shell", "input": {}}));
        assert_eq!(starts[3]["content_block"]["name"], "apply_patch");
        assert_eq!(starts.iter().map(|s| s["index"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        let json_for = |idx: u64| -> String {
            fr.iter().filter(|(e, d)| e == "content_block_delta" && d["index"] == idx)
                .map(|(_, d)| d["delta"]["partial_json"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(json_for(2), "{\"cmd\":\"ls\"}");
        assert_eq!(serde_json::from_str::<Value>(&json_for(3)).unwrap()["input"], "*** Begin Patch\n\"q\"");
        let md = fr.iter().find(|(e, _)| e == "message_delta").unwrap();
        assert_eq!(md.1["delta"]["stop_reason"], "tool_use");
        assert_eq!(md.1["usage"]["output_tokens"], 20);
        assert_eq!(md.1["usage"]["input_tokens"], 40);
        assert_eq!(md.1["usage"]["cache_read_input_tokens"], 60);
    }

    #[test]
    fn stream_custom_tool_to_responses_downstream() {
        let ctx = ReqCtx { model: "gpt".into(), custom_tools: vec!["apply_patch".into()], stream: true };
        let mut down = DownstreamStream::new(Proto::Responses, ctx);
        let mk = |delta: Value, fin: Value| json!({"id": "x", "object": "chat.completion.chunk", "model": "m",
            "choices": [{"index": 0, "delta": delta, "finish_reason": fin}]});
        let mut out = Vec::new();
        out.extend(down.push(&mk(json!({"role": "assistant", "content": "Patching"}), Value::Null)));
        out.extend(down.push(&mk(json!({"tool_calls": [{"index": 0, "id": "call_p", "type": "function",
            "function": {"name": "apply_patch", "arguments": "{\"inp"}}]}), Value::Null)));
        out.extend(down.push(&mk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "ut\":\"*** Begin Patch\"}"}}]}), Value::Null)));
        out.extend(down.push(&mk(json!({}), json!("tool_calls"))));
        out.extend(down.finish());
        let fr = frames(&out);
        assert!(!fr.iter().any(|(e, _)| e == "response.function_call_arguments.delta"));
        let added = fr.iter().find(|(e, d)| e == "response.output_item.added" && d["output_index"] == 1).unwrap();
        assert_eq!(added.1["item"]["type"], "custom_tool_call");
        let done = fr.iter().find(|(e, _)| e == "response.custom_tool_call_input.done").unwrap();
        assert_eq!(done.1["input"], "*** Begin Patch");
        let resp = &fr.last().unwrap().1["response"];
        assert_eq!(resp["output"][0]["content"][0]["text"], "Patching");
        assert_eq!(resp["output"][1], json!({"type": "custom_tool_call", "id": resp["output"][1]["id"],
            "call_id": "call_p", "name": "apply_patch", "input": "*** Begin Patch", "status": "completed"}));
        // message item closed before tool item opened
        let pos = |name: &str| fr.iter().position(|(e, _)| e == name).unwrap();
        assert!(pos("response.output_text.done") < pos("response.custom_tool_call_input.delta"));
    }

    #[test]
    fn downstream_chat_frames() {
        let mut up = UpstreamStream::new(Proto::Anthropic);
        let chunks = feed_in_pieces(&mut up, &anthropic_transcript(), 64);
        let mut down = DownstreamStream::new(Proto::Chat, ReqCtx { model: "client".into(), ..Default::default() });
        let mut out = Vec::new();
        for c in &chunks {
            out.extend(down.push(c));
        }
        out.extend(down.finish());
        assert_eq!(out.last().unwrap(), "data: [DONE]\n\n");
        assert!(down.finish().is_empty());
        let fr = frames(&out);
        let ids: HashSet<String> = fr.iter().filter_map(|(_, d)| d["id"].as_str().map(String::from)).collect();
        assert_eq!(ids.len(), 1);
        assert!(fr.iter().filter(|(_, d)| d.is_object()).all(|(_, d)| d["model"] == "client"));
        // usage chunk right before [DONE]
        let (_, u) = &fr[fr.len() - 2];
        assert_eq!(u["usage"]["total_tokens"], 67);
        assert_eq!(u["choices"], json!([]));
        assert_eq!(fr.iter().filter(|(_, d)| d.get("usage").is_some()).count(), 1);

        let mut down = DownstreamStream::new(Proto::Chat, ReqCtx::default());
        let e = down.error(500, "boom");
        assert_eq!(e.len(), 2);
        let fr = frames(&e);
        assert_eq!(fr[0].1["error"]["message"], "boom");
        assert_eq!(fr[0].1["error"]["type"], "upstream_error");
        assert_eq!(e[1], "data: [DONE]\n\n");
        assert!(down.finish().is_empty());
        assert!(down.push(&json!({"choices": []})).is_empty());
    }

    #[test]
    fn stream_errors() {
        // upstream anthropic error event -> responses failed
        let mut up = UpstreamStream::new(Proto::Anthropic);
        let mut src = anthropic_transcript();
        src.truncate(src.find("event: content_block_start").unwrap());
        src += &sse("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}));
        let chunks = feed_in_pieces(&mut up, &src, 13);
        assert!(chunks.iter().any(|c| c["error"]["message"] == "Overloaded"));
        let mut down = DownstreamStream::new(Proto::Responses, ReqCtx::default());
        let mut out = Vec::new();
        for c in &chunks {
            out.extend(down.push(c));
        }
        out.extend(down.finish());
        let fr = frames(&out);
        let (e, d) = fr.last().unwrap();
        assert_eq!(e, "response.failed");
        assert_eq!(d["response"]["status"], "failed");
        assert_eq!(d["response"]["error"]["message"], "Overloaded");
        assert!(!fr.iter().any(|(e, _)| e == "response.completed"));

        // responses failed upstream -> anthropic error event
        let mut up = UpstreamStream::new(Proto::Responses);
        let chunks = up.feed(&sse("response.failed", json!({"type": "response.failed",
            "response": {"status": "failed", "error": {"code": "server_error", "message": "bad things"}}})));
        let mut down = DownstreamStream::new(Proto::Anthropic, ReqCtx::default());
        let mut out = Vec::new();
        for c in &chunks {
            out.extend(down.push(c));
        }
        out.extend(down.finish());
        let fr = frames(&out);
        assert_eq!(fr.last().unwrap().0, "error");
        assert_eq!(fr.last().unwrap().1["error"]["message"], "bad things");
        assert_eq!(fr.last().unwrap().1["error"]["type"], "api_error");

        let mut down = DownstreamStream::new(Proto::Anthropic, ReqCtx::default());
        let e = frames(&down.error(429, "slow down"));
        assert_eq!(e[0].1["error"]["type"], "rate_limit_error");
        assert!(down.error(500, "again").is_empty());
    }

    #[test]
    fn finish_idempotent_and_empty_streams() {
        for p in [Proto::Chat, Proto::Responses, Proto::Anthropic] {
            let mut d = DownstreamStream::new(p, ReqCtx { model: "m".into(), ..Default::default() });
            let first = d.finish();
            assert!(!first.is_empty());
            assert!(d.finish().is_empty());
            assert!(d.error(500, "x").is_empty());
            let fr = frames(&first);
            match p {
                Proto::Chat => assert_eq!(first, vec!["data: [DONE]\n\n".to_string()]),
                Proto::Responses => {
                    assert_eq!(fr[0].0, "response.created");
                    assert_eq!(fr.last().unwrap().0, "response.completed");
                    assert_eq!(fr.last().unwrap().1["response"]["usage"]["total_tokens"], 0);
                }
                Proto::Anthropic => {
                    assert_eq!(fr[0].0, "message_start");
                    assert_eq!(fr.last().unwrap().0, "message_stop");
                }
            }
        }
        // upstream without final event: finish() synthesizes finish + usage
        let mut up = UpstreamStream::new(Proto::Responses);
        let mut chunks = up.feed(&sse("response.output_text.delta", json!({"type": "response.output_text.delta", "item_id": "m", "delta": "partial"})));
        chunks.extend(up.finish());
        assert_eq!(chunks.last().unwrap()["choices"][0]["finish_reason"], "stop");
        assert!(up.finish().is_empty());
        let mut up = UpstreamStream::new(Proto::Anthropic);
        assert!(up.finish().is_empty());
    }

    #[test]
    fn unknown_items_do_not_panic() {
        let weird = json!({"model": "m", "input": [1, "str", null, {"type": 5}, {"type": "message"},
            {"type": "function_call"}, {"type": "function_call_output"}, {"type": "custom_tool_call"},
            {"type": "message", "role": "user", "content": [{"type": "input_file", "file_id": "f"}, 3]}],
            "tools": [{"type": "custom"}, {"type": "function"}, 7], "tool_choice": {"type": "allowed_tools"}});
        let c = request_to_chat(Proto::Responses, &weird).unwrap();
        assert!(c["messages"].is_array());
        let a = request_to_chat(Proto::Anthropic, &json!({"messages": [{"role": "user", "content": [{"type": "mystery"}, 4]},
            {"role": "assistant"}, {"role": "user", "content": null}], "system": 5, "tools": [{}]})).unwrap();
        assert!(a["messages"].is_array());
        let _ = request_from_chat(Proto::Anthropic, &json!({"messages": [{"role": "tool"}, {"role": "assistant", "tool_calls": [{}]}, {}]})).unwrap();
        let _ = request_from_chat(Proto::Responses, &json!({"messages": [{"role": "tool"}, {"role": "assistant", "tool_calls": [{}]}, 5]})).unwrap();
        let _ = response_to_chat(Proto::Responses, &json!({"output": [{"type": "zzz"}, 1, {"type": "message", "content": 5}]})).unwrap();
        let _ = response_to_chat(Proto::Anthropic, &json!({"content": [{"type": "server_tool_use"}, null]})).unwrap();
        let _ = response_from_chat(Proto::Responses, &json!({"choices": []}), &ReqCtx::default()).unwrap();
        let _ = response_from_chat(Proto::Anthropic, &json!({}), &ReqCtx::default()).unwrap();
        // upstream stream with odd events
        for p in [Proto::Responses, Proto::Anthropic, Proto::Chat] {
            let mut up = UpstreamStream::new(p);
            let mut chunks = up.feed("event: weird\ndata: {\"type\":\"x.y\"}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"nope\"}\n\ndata: {\"type\":\"content_block_delta\",\"index\":9,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"x\"}}\n\nretry: 5\nid: 1\n\n");
            chunks.extend(up.finish());
            let mut d = DownstreamStream::new(Proto::Responses, ReqCtx::default());
            for c in &chunks {
                d.push(c);
            }
            d.push(&json!({"choices": [{"delta": {"tool_calls": [{"function": {"arguments": "late"}}]}}]}));
            d.push(&json!({"choices": [{"delta": {"content": "t"}}]}));
            d.push(&json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "more"}}]}}]}));
            let f = frames(&d.finish());
            assert_eq!(f.last().unwrap().1["response"]["output"][0]["arguments"], "latemore");
        }
    }

    #[test]
    fn error_body_shapes() {
        let e = error_body(Proto::Chat, 401, r#"{"error":{"message":"bad key","type":"invalid"}}"#);
        assert_eq!(e, json!({"error": {"message": "bad key", "type": "authentication_error", "code": 401}}));
        let e = error_body(Proto::Responses, 429, r#"{"message":"slow"}"#);
        assert_eq!(e["error"]["message"], "slow");
        assert_eq!(e["error"]["type"], "rate_limit_error");
        let e = error_body(Proto::Anthropic, 400, r#"{"error":"plain string"}"#);
        assert_eq!(e, json!({"type": "error", "error": {"type": "invalid_request_error", "message": "plain string"}}));
        let e = error_body(Proto::Anthropic, 422, r#"{"detail":[{"loc":["x"]}]}"#);
        assert_eq!(e["error"]["type"], "invalid_request_error");
        assert!(e["error"]["message"].as_str().unwrap().contains("loc"));
        assert_eq!(error_body(Proto::Anthropic, 403, "x")["error"]["type"], "permission_error");
        assert_eq!(error_body(Proto::Anthropic, 404, "x")["error"]["type"], "not_found_error");
        assert_eq!(error_body(Proto::Anthropic, 503, "<html>down</html>")["error"],
            json!({"type": "api_error", "message": "<html>down</html>"}));
        let long = "é".repeat(800);
        let m = error_body(Proto::Chat, 500, &long)["error"]["message"].as_str().unwrap().to_string();
        assert_eq!(m.chars().count(), 503);
        assert_eq!(error_body(Proto::Chat, 502, "")["error"]["message"], "upstream error (HTTP 502)");
        // anthropic-shaped upstream error
        let e = error_body(Proto::Chat, 529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        assert_eq!(e["error"]["message"], "Overloaded");
    }

    #[test]
    fn models_body_shapes() {
        let openai = json!({"object": "list", "data": [{"id": "gpt-5", "owned_by": "openai"}, {"id": "gpt-5"}, {"id": "o3"}]});
        let a = models_body(Proto::Anthropic, &openai);
        assert_eq!(a["data"].as_array().unwrap().len(), 2);
        assert_eq!(a["data"][0], json!({"type": "model", "id": "gpt-5", "display_name": "gpt-5", "created_at": "1970-01-01T00:00:00Z"}));
        assert_eq!(a["first_id"], "gpt-5");
        assert_eq!(a["last_id"], "o3");
        assert_eq!(a["has_more"], false);
        let anth = json!({"data": [{"type": "model", "id": "claude-x", "display_name": "Claude X"}], "has_more": false});
        let c = models_body(Proto::Responses, &anth);
        assert_eq!(c, json!({"object": "list", "data": [{"id": "claude-x", "object": "model", "created": 0, "owned_by": "agentplus"}]}));
        let g = models_body(Proto::Chat, &json!({"models": [{"name": "models/gemini-pro", "displayName": "Gemini"}, "plain"]}));
        assert_eq!(g["data"][0]["id"], "gemini-pro");
        assert_eq!(g["data"][1]["id"], "plain");
        assert_eq!(models_body(Proto::Anthropic, &json!({"models": [{"name": "models/g", "displayName": "G"}]}))["data"][0]["display_name"], "G");
        assert_eq!(models_body(Proto::Chat, &json!("junk"))["data"], json!([]));
    }
}
