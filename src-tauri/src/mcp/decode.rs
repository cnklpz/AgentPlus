//! One agent's MCP server definition, in its own format, turned into the common shape.
//! The field names of every format are disjoint enough to read them all with one decoder:
//! Claude-style `type`/`env`/`headers`, OpenCode `environment` and a command array,
//! Gemini `url` (SSE) / `httpUrl`, Codex `http_headers`, Hermes/Kimi/OpenClaw `transport`.

use super::Family;
use serde_json::{Map, Value};

/// A definition before masking.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct Raw {
    pub transport: &'static str,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub env: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    /// Turned off inside the definition (`enabled: false`, `disabled: true`).
    pub off: bool,
    pub extra: Map<String, Value>,
}

/// Fields the common shape covers; everything else is kept in `extra`.
const KNOWN: [&str; 14] = ["type", "transport", "command", "args", "cwd", "url", "httpUrl", "env", "environment", "headers", "http_headers", "enabled", "disabled", "enable"];

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn texts(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(text).collect(),
        // Hermes and a few hand-written configs give a single argument as a string.
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    }
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| text(v).map(|v| (k.clone(), v))).collect()).unwrap_or_default()
}

pub fn transport(kind: &str) -> Option<&'static str> {
    Some(match kind.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "stdio" | "local" => "stdio",
        "http" | "streamablehttp" | "streamable" => "http",
        "sse" => "sse",
        "ws" | "websocket" => "ws",
        "remote" => "remote",
        _ => return None,
    })
}

pub fn decode(fam: Family, def: &Value) -> Raw {
    let Some(o) = def.as_object() else { return Raw { transport: "stdio", ..Default::default() } };
    let s = |k: &str| o.get(k).and_then(text).filter(|v| !v.trim().is_empty());
    // OpenCode (and ZCode's importer) put the program and its arguments in one array.
    let (command, mut args) = match o.get("command") {
        Some(Value::Array(a)) => {
            let mut parts = a.iter().filter_map(text);
            (parts.next(), parts.collect())
        }
        Some(v) => (text(v), vec![]),
        None => (None, vec![]),
    };
    args.extend(texts(o.get("args")));
    let url = s("httpUrl").or_else(|| s("url"));
    let declared = o.get("type").or_else(|| o.get("transport")).and_then(Value::as_str).and_then(transport);
    let transport = declared.unwrap_or(match (&command, &url) {
        (None, Some(_)) if o.contains_key("httpUrl") => "http",
        // Gemini and Qwen: `url` is SSE, `httpUrl` streamable HTTP.
        (None, Some(_)) if fam == Family::Gemini => "sse",
        // OpenCode negotiates HTTP or SSE by itself.
        (None, Some(_)) if fam == Family::OpenCode => "remote",
        (None, Some(_)) => "http",
        _ => "stdio",
    });
    let mut env = pairs(o.get("env"));
    env.extend(pairs(o.get("environment")));
    let mut headers = pairs(o.get("headers"));
    headers.extend(pairs(o.get("http_headers")));
    let flag = |k: &str| o.get(k).and_then(Value::as_bool);
    let off = flag("enabled") == Some(false) || flag("enable") == Some(false) || flag("disabled") == Some(true);
    let mut extra: Map<String, Value> = o.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    if fam == Family::Codex {
        codex_refs(&mut extra, &mut env, &mut headers);
    }
    Raw { transport, command, args, cwd: s("cwd"), url, env, headers, off, extra }
}

/// Codex has no `${VAR}` expansion: it passes variables by name instead. Those read as
/// references here, so a server reads the same as in the agents that expand them.
/// `env_vars = ["K"]` is `K = ${K}`; `bearer_token_env_var = "T"` is
/// `Authorization: Bearer ${T}`; `env_http_headers = { H = "V" }` is `H: ${V}`.
fn codex_refs(extra: &mut Map<String, Value>, env: &mut Vec<(String, String)>, headers: &mut Vec<(String, String)>) {
    // `{ name, source }` entries (remote variables) have no reference form: left as they are.
    if let Some(Value::Array(a)) = extra.get("env_vars") {
        if a.iter().all(Value::is_string) {
            env.extend(a.iter().filter_map(Value::as_str).map(|k| (k.to_string(), format!("${{{k}}}"))));
            extra.remove("env_vars");
        }
    }
    if let Some(Value::String(t)) = extra.get("bearer_token_env_var") {
        headers.push(("Authorization".into(), format!("Bearer ${{{t}}}")));
        extra.remove("bearer_token_env_var");
    }
    if let Some(Value::Object(m)) = extra.get("env_http_headers") {
        if m.values().all(Value::is_string) {
            headers.extend(m.iter().filter_map(|(h, v)| v.as_str().map(|v| (h.clone(), format!("${{{v}}}")))));
            extra.remove("env_http_headers");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_style_entries() {
        let r = decode(Family::Claude, &json!({ "type": "stdio", "command": "npx", "args": ["-y", "x"], "env": { "A": "1", "N": 2 } }));
        assert_eq!((r.transport, r.command.as_deref(), r.args.len(), r.env.len(), r.off), ("stdio", Some("npx"), 2, 2, false));
        let r = decode(Family::Claude, &json!({ "type": "http", "url": "https://h/mcp", "headers": { "Authorization": "Bearer ${T}" }, "timeout": 5000 }));
        assert_eq!((r.transport, r.url.as_deref(), r.headers.len()), ("http", Some("https://h/mcp"), 1));
        assert_eq!(r.extra, json!({ "timeout": 5000 }).as_object().unwrap().clone());
        // Droid turns an entry off with `disabled`.
        assert!(decode(Family::Claude, &json!({ "command": "x", "disabled": true })).off);
        // No type and only a URL: streamable HTTP.
        assert_eq!(decode(Family::Claude, &json!({ "url": "https://h" })).transport, "http");
        // Kimi marks legacy servers with `transport`.
        assert_eq!(decode(Family::Claude, &json!({ "url": "https://h", "transport": "sse" })).transport, "sse");
    }

    #[test]
    fn opencode_command_arrays_and_remote() {
        let r = decode(Family::OpenCode, &json!({ "type": "local", "command": ["npx", "-y", "x"], "environment": { "K": "{env:K}" }, "enabled": false }));
        assert_eq!((r.transport, r.command.as_deref(), r.args, r.off), ("stdio", Some("npx"), vec!["-y".to_string(), "x".into()], true));
        assert_eq!(r.env, vec![("K".to_string(), "{env:K}".to_string())]);
        assert_eq!(decode(Family::OpenCode, &json!({ "type": "remote", "url": "https://h" })).transport, "remote");
    }

    #[test]
    fn gemini_urls() {
        assert_eq!(decode(Family::Gemini, &json!({ "url": "https://h/sse" })).transport, "sse");
        let r = decode(Family::Gemini, &json!({ "httpUrl": "https://h/mcp", "trust": true }));
        assert_eq!((r.transport, r.url.as_deref()), ("http", Some("https://h/mcp")));
        assert!(r.extra.contains_key("trust"));
    }

    #[test]
    fn codex_hermes_openclaw_zcode() {
        let r = decode(Family::Codex, &json!({ "url": "https://h", "http_headers": { "X": "y" }, "bearer_token_env_var": "T", "env_http_headers": { "K": "KV" }, "enabled": false }));
        assert_eq!((r.transport, r.off), ("http", true));
        let h = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(r.headers, [h("X", "y"), h("Authorization", "Bearer ${T}"), h("K", "${KV}")]);
        assert!(r.extra.is_empty());
        let r = decode(Family::Codex, &json!({ "command": "x", "env": { "A": "1" }, "env_vars": ["B"] }));
        assert_eq!(r.env, [h("A", "1"), h("B", "${B}")]);
        // Remote variables stay as they are.
        let r = decode(Family::Codex, &json!({ "command": "x", "env_vars": ["B", { "name": "C", "source": "remote" }] }));
        assert!(r.env.is_empty() && r.extra.contains_key("env_vars"));
        // Hermes: a single string argument.
        assert_eq!(decode(Family::Hermes, &json!({ "command": "python", "args": "server.py" })).args, vec!["server.py"]);
        assert_eq!(decode(Family::OpenClaw, &json!({ "url": "https://h", "transport": "streamable-http" })).transport, "http");
        // ZCode's alias for `enabled`.
        assert!(decode(Family::ZCode, &json!({ "type": "stdio", "command": "x", "enable": false })).off);
    }
}
