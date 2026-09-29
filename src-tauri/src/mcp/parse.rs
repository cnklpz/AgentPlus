//! Pasted MCP configuration: a README snippet or a piece of any agent's config file
//! (`{"mcpServers": …}`, OpenCode `mcp`, Codex `[mcp_servers.x]`, Hermes YAML, a bare
//! `"name": { … }` fragment, or one server without a name).

use super::decode;
use super::{toml_item, Family, KvIn, McpInput};
use crate::i18n::l;
use crate::util::strip_jsonc;
use anyhow::Result;
use serde_json::{Map, Value};

fn any_value(text: &str) -> Option<(Value, bool)> {
    let t = text.trim();
    let json = |s: &str| serde_json::from_str::<Value>(&strip_jsonc(s).0).ok().or_else(|| json5::from_str::<Value>(s).ok());
    if let Some(v) = json(t).filter(Value::is_object) {
        return Some((v, false));
    }
    // `"name": { … }` copied without the braces around it.
    if let Some(v) = json(&format!("{{{}}}", t.trim_end_matches(','))).filter(Value::is_object) {
        return Some((v, false));
    }
    if let Ok(doc) = t.parse::<toml_edit::DocumentMut>() {
        let v = toml_item(doc.as_item());
        if v.as_object().is_some_and(|o| !o.is_empty()) {
            return Some((v, true));
        }
    }
    serde_yaml::from_str::<serde_yaml::Value>(t).ok().and_then(|y| serde_json::to_value(y).ok()).filter(Value::is_object).map(|v| (v, false))
}

fn is_def(v: &Value) -> bool {
    v.as_object().is_some_and(|o| ["command", "url", "httpUrl", "serverUrl"].iter().any(|k| o.contains_key(*k)))
}

/// The servers in a pasted text, with their values as pasted.
pub fn parse(text: &str) -> Result<Vec<McpInput>> {
    // A CC Switch / AgentPlus import link (`resource=mcp`).
    if crate::deeplink::is_link(text) {
        let item = crate::deeplink::parse_link(text)?;
        return item.mcp.map(|m| m.servers).ok_or_else(|| anyhow::anyhow!(l("This link imports a provider, not MCP servers", "这个链接导入的是供应商，不是 MCP 服务器")));
    }
    let none = || anyhow::anyhow!(l("No MCP server found in the pasted text", "粘贴的内容里没有找到 MCP 服务器"));
    let (v, toml) = any_value(text).ok_or_else(none)?;
    let map = |p: &str| v.pointer(p).and_then(Value::as_object).filter(|m| !m.is_empty() && m.values().all(is_def)).cloned();
    let gemini = |m: &Map<String, Value>| m.values().any(|d| d.get("httpUrl").is_some());
    let found: Option<(Family, Map<String, Value>)> = if let Some(m) = map("/mcpServers") {
        Some((if gemini(&m) { Family::Gemini } else { Family::Claude }, m))
    } else if let Some(m) = map("/mcp_servers") {
        Some((if toml { Family::Codex } else { Family::Hermes }, m))
    } else if let Some(m) = map("/mcp/servers") {
        Some((Family::ZCode, m))
    } else if let Some(m) = map("/mcp") {
        Some((Family::OpenCode, m))
    } else if let Some(m) = map("/servers") {
        Some((Family::Claude, m))
    } else if is_def(&v) {
        let fam = if v.get("command").is_some_and(Value::is_array) { Family::OpenCode } else { Family::Claude };
        Some((fam, Map::from_iter([(String::new(), v.clone())])))
    } else {
        map("").map(|m| (if gemini(&m) { Family::Gemini } else { Family::Claude }, m))
    };
    let (fam, m) = found.ok_or_else(none)?;
    let kv = |p: Vec<(String, String)>| p.into_iter().map(|(key, value)| KvIn { key, value }).collect();
    let out: Vec<McpInput> = m
        .iter()
        .map(|(name, def)| {
            // Windsurf writes remote servers as `serverUrl`.
            let def = match def.get("serverUrl") {
                Some(u) if def.get("url").is_none() => {
                    let mut d = def.clone();
                    d["url"] = u.clone();
                    d.as_object_mut().unwrap().remove("serverUrl");
                    d
                }
                _ => def.clone(),
            };
            let r = decode::decode(fam, &def);
            McpInput {
                name: name.clone(),
                transport: r.transport.into(),
                command: r.command,
                args: r.args,
                cwd: r.cwd,
                url: r.url,
                env: kv(r.env),
                headers: kv(r.headers),
                enabled: !r.off,
                extra: r.extra,
                extra_family: Some(fam),
                ..Default::default()
            }
        })
        .collect();
    if out.is_empty() {
        return Err(none());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<(String, String)> {
        parse(text).unwrap().into_iter().map(|i| (i.name, i.transport)).collect()
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    #[test]
    fn pasted_snippets() {
        assert_eq!(names(r#"{ "mcpServers": { "fs": { "command": "npx", "args": ["-y", "fs"] }, "gh": { "type": "http", "url": "https://h" } } }"#), [pair("fs", "stdio"), pair("gh", "http")]);
        assert_eq!(names(r#""fs": { "command": "npx", "args": ["-y", "fs"] },"#), [pair("fs", "stdio")]);
        assert_eq!(names(r#"{ "command": "uvx", "args": ["x"] }"#), [pair("", "stdio")]);
        assert_eq!(names("[mcp_servers.docs]\nurl = \"https://h/mcp\"\nbearer_token_env_var = \"T\"\n"), [pair("docs", "http")]);
        assert_eq!(names(r#"{ "mcp": { "o": { "type": "remote", "url": "https://h" } } }"#), [pair("o", "remote")]);
        assert_eq!(names(r#"{ "mcpServers": { "g": { "httpUrl": "https://h" }, "s": { "url": "https://h/sse" } } }"#), [pair("g", "http"), pair("s", "sse")]);
        assert_eq!(names("mcp_servers:\n  y:\n    url: https://h\n    transport: sse\n"), [pair("y", "sse")]);
        assert_eq!(names(r#"{ "servers": { "v": { "type": "stdio", "command": "x" } } }"#), [pair("v", "stdio")]);
        assert_eq!(names(r#"{ "mcpServers": { "w": { "serverUrl": "https://h/mcp" } } }"#), [pair("w", "http")]);
        let codex = parse("[mcp_servers.docs]\nurl = \"https://h/mcp\"\nbearer_token_env_var = \"T\"\n").unwrap();
        assert_eq!(codex[0].headers[0].value, "Bearer ${T}");
        assert_eq!(codex[0].extra_family, Some(Family::Codex));
        assert_eq!(parse("hello").unwrap_err().to_string(), "粘贴的内容里没有找到 MCP 服务器");
        assert!(parse("{}").is_err());
        let link = "ccswitch://v1/import?resource=mcp&apps=claude&config=eyJtY3BTZXJ2ZXJzIjp7Im1jcC1mZXRjaCI6eyJjb21tYW5kIjoidXZ4IiwiYXJncyI6WyJtY3Atc2VydmVyLWZldGNoIl19fX0%3D";
        assert_eq!(names(link), [pair("mcp-fetch", "stdio")]);
        assert_eq!(parse("ccswitch://v1/import?resource=provider&endpoint=https://e.example.com").unwrap_err().to_string(), "这个链接导入的是供应商，不是 MCP 服务器");
    }
}
