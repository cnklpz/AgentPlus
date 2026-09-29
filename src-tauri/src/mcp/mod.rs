//! MCP servers in each agent's global (user-level) config, read into one shape so the MCP
//! page can show them side by side. Project-level configs (`.mcp.json`, a project's
//! `opencode.json`) are out of scope.
//!
//! Where each agent keeps them:
//! - Claude Code: `~/.claude.json` → `mcpServers`; CodeBuddy `~/.codebuddy/.mcp.json` (older
//!   `mcp.json`); Droid `~/.factory/mcp.json`; Kimi Code `mcp.json` in its home. Same format.
//! - Gemini CLI, Qwen Code: `settings.json` → `mcpServers`, switched off through `mcp.excluded`.
//! - OpenCode, Kilo Code, MiMo: `mcp.<name>` in their OpenCode-format config.
//! - Codex: `config.toml` → `[mcp_servers.<name>]`. Hermes: `config.yaml` → `mcp_servers`.
//! - OpenClaw: `openclaw.json` (JSON5) → `mcp.servers`. ZCode: `~/.zcode/cli/config.json` →
//!   `mcp.servers` (its engine's config, not the `v2` provider files).
//! - pi has no MCP support.
//!
//! Secrets never reach the UI (see `mask`); servers are compared across agents by `sig`.

mod decode;
mod mask;

use crate::adapters::{self, claude, codebuddy, codex, droid, gemini, hermes, kilo, kimi, mimo, openclaw, opencode, qwen, zcode};
use crate::model::key_fingerprint;
use crate::store;
use crate::util::{display_path, home, read_text, strip_jsonc};
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// How an agent's config file is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    /// `mcpServers` with `type` (Claude Code, CodeBuddy, Droid, Kimi Code).
    Claude,
    /// `mcpServers` with `url` = SSE / `httpUrl`, plus `mcp.excluded` (Gemini CLI, Qwen Code).
    Gemini,
    /// `mcp.<name>` with `type` local / remote (OpenCode, Kilo Code, MiMo).
    OpenCode,
    Codex,
    Hermes,
    OpenClaw,
    ZCode,
}

/// Store key (per agent) of definitions AgentPlus turned off by taking them out of a config
/// that has no per-server switch.
pub const STASH_KEY: &str = "disabledMcp";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct McpKv {
    pub key: String,
    /// Masked when it is a secret.
    pub value: String,
    pub secret: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    /// "stdio" | "http" | "sse" | "ws" | "remote" (HTTP or SSE, whichever the server speaks).
    pub transport: String,
    pub command: Option<String>,
    /// Secrets masked.
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Secret query parameters masked.
    pub url: Option<String>,
    pub env: Vec<McpKv>,
    pub headers: Vec<McpKv>,
    pub enabled: bool,
    /// Turned off by AgentPlus: the definition waits in its store, not in the agent's config.
    pub stashed: bool,
    /// The agent's other fields (timeouts, tool filters, OAuth…), secrets masked.
    pub extra: Map<String, Value>,
    /// Fingerprint of what runs (transport kind, command, arguments, URL, env, headers), with
    /// variable references in one syntax: equal across agents means the same server.
    pub sig: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcp {
    pub agent: String,
    /// The agent can use MCP servers at all.
    pub supported: bool,
    /// Where they are read from; None when unsupported.
    pub file: Option<String>,
    pub exists: bool,
    pub servers: Vec<McpServer>,
    pub error: Option<String>,
}

/// `~/.claude.json` sits next to `~/.claude`; a config folder picked by hand keeps it inside.
fn claude_json() -> PathBuf {
    let d = claude::dir();
    if d == claude::default_dir() {
        home().join(".claude.json")
    } else {
        d.join(".claude.json")
    }
}

/// CodeBuddy reads `.mcp.json` first and the older `mcp.json` after it.
fn codebuddy_json() -> PathBuf {
    let d = codebuddy::dir();
    [".mcp.json", "mcp.json"].iter().map(|n| d.join(n)).find(|p| p.exists()).unwrap_or_else(|| d.join(".mcp.json"))
}

/// ZCode's engine config: `cli/config.json` under `~/.zcode` (the adapter's folder is `v2`).
fn zcode_json() -> PathBuf {
    let d = zcode::dir();
    let root = if d.file_name().is_some_and(|n| n == "v2") { d.parent().map(Path::to_path_buf).unwrap_or(d) } else { d };
    root.join("cli").join("config.json")
}

/// The file an agent keeps its global MCP servers in, and its layout.
pub fn source(agent: &str) -> Option<(Family, PathBuf)> {
    Some(match agent {
        claude::ID => (Family::Claude, claude_json()),
        codebuddy::ID => (Family::Claude, codebuddy_json()),
        droid::ID => (Family::Claude, droid::dir().join("mcp.json")),
        kimi::ID => (Family::Claude, kimi::dir().join("mcp.json")),
        gemini::ID => (Family::Gemini, gemini::settings_path()),
        qwen::ID => (Family::Gemini, qwen::settings_path()),
        opencode::ID => (Family::OpenCode, opencode::config_path()),
        kilo::ID => (Family::OpenCode, kilo::config_path()),
        mimo::ID => (Family::OpenCode, mimo::engine_path()),
        codex::ID => (Family::Codex, codex::config_path()),
        hermes::ID => (Family::Hermes, hermes::config_path()),
        openclaw::ID => (Family::OpenClaw, openclaw::config_path()),
        zcode::ID => (Family::ZCode, zcode_json()),
        _ => return None,
    })
}

fn toml_value(v: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match v {
        T::String(s) => json!(s.value()),
        T::Integer(i) => json!(i.value()),
        T::Float(f) => json!(f.value()),
        T::Boolean(b) => json!(b.value()),
        T::Datetime(d) => json!(d.value().to_string()),
        T::Array(a) => Value::Array(a.iter().map(toml_value).collect()),
        T::InlineTable(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), toml_value(v))).collect()),
    }
}

fn toml_item(i: &toml_edit::Item) -> Value {
    use toml_edit::Item;
    let table = |t: &toml_edit::Table| Value::Object(t.iter().map(|(k, i)| (k.to_string(), toml_item(i))).collect());
    match i {
        Item::None => Value::Null,
        Item::Value(v) => toml_value(v),
        Item::Table(t) => table(t),
        Item::ArrayOfTables(a) => Value::Array(a.iter().map(table).collect()),
    }
}

fn parse_err(path: &Path, e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(tr!("Failed to parse {}: {e}", "{} 解析失败：{e}", display_path(path)))
}

/// The whole config file as JSON.
fn load(fam: Family, path: &Path) -> Result<Value> {
    let (text, _) = read_text(path)?;
    match fam {
        Family::Codex => {
            let doc: toml_edit::DocumentMut = text.parse().map_err(|e| parse_err(path, e))?;
            Ok(toml_item(doc.as_item()))
        }
        Family::Hermes => {
            let y: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| parse_err(path, e))?;
            serde_json::to_value(y).map_err(|e| parse_err(path, e))
        }
        Family::OpenClaw => serde_json::from_str(&text).or_else(|_| json5::from_str(&text)).map_err(|e| parse_err(path, e)),
        _ if text.trim().is_empty() => Ok(json!({})),
        _ => serde_json::from_str(&strip_jsonc(&text).0).map_err(|e| parse_err(path, e)),
    }
}

/// Where the servers sit inside the config.
fn pointer(fam: Family) -> &'static str {
    match fam {
        Family::Claude | Family::Gemini => "/mcpServers",
        Family::OpenCode => "/mcp",
        Family::Codex | Family::Hermes => "/mcp_servers",
        Family::OpenClaw | Family::ZCode => "/mcp/servers",
    }
}

/// Names switched off outside their definitions: Gemini / Qwen `mcp.excluded` (and an
/// `mcp.allowed` list that leaves the others out), CodeBuddy's `disabledMcpServers`.
fn switched_off(fam: Family, cfg: &Value, name: &str) -> bool {
    let list = |p: &str| cfg.pointer(p).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect::<Vec<_>>());
    let hit = |l: &[String]| l.iter().any(|p| glob(p, name));
    match fam {
        Family::Gemini => list("/mcp/excluded").is_some_and(|l| hit(&l)) || list("/mcp/allowed").is_some_and(|l| !l.is_empty() && !hit(&l)),
        Family::Claude => list("/disabledMcpServers").is_some_and(|l| hit(&l)),
        _ => false,
    }
}

/// Qwen's lists take `*` wildcards.
fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((pre, rest)) => name.len() >= pre.len() && name.starts_with(pre) && (0..=name.len() - pre.len()).any(|i| name.is_char_boundary(pre.len() + i) && glob(rest, &name[pre.len() + i..])),
    }
}

/// `cmd /c npx …` (how Windows configs start npx) reads as `npx …`, and `npx.cmd` as `npx`.
fn program(command: Option<&str>, args: &[String]) -> (String, Vec<String>) {
    let stem = |c: &str| {
        let c = c.trim();
        let lower = c.to_ascii_lowercase();
        [".cmd", ".exe", ".bat"].iter().find_map(|e| lower.strip_suffix(e).map(|_| c[..c.len() - e.len()].to_string())).unwrap_or_else(|| c.to_string())
    };
    let cmd = command.map(stem).unwrap_or_default();
    if cmd.eq_ignore_ascii_case("cmd") && args.first().is_some_and(|a| a.eq_ignore_ascii_case("/c")) && args.len() > 1 {
        return (stem(&args[1]), args[2..].to_vec());
    }
    (cmd, args.to_vec())
}

fn sig(r: &decode::Raw) -> String {
    let n = |s: &str| mask::normalize_refs(s);
    let sorted = |kv: &[(String, String)], lower: bool| {
        let mut v: Vec<(String, String)> = kv.iter().map(|(k, x)| (if lower { k.to_ascii_lowercase() } else { k.clone() }, n(x))).collect();
        v.sort();
        v
    };
    let canon = if r.transport == "stdio" {
        let (cmd, args) = program(r.command.as_deref(), &r.args);
        json!(["stdio", cmd, args.iter().map(|a| n(a)).collect::<Vec<_>>(), r.cwd, sorted(&r.env, false)])
    } else {
        json!(["remote", r.url.as_deref().map(|u| n(u.trim_end_matches('/'))), sorted(&r.headers, true)])
    };
    key_fingerprint(&canon.to_string())
}

fn kvs(pairs: &[(String, String)]) -> Vec<McpKv> {
    pairs
        .iter()
        .map(|(k, v)| {
            let (value, secret) = mask::value(k, v);
            McpKv { key: k.clone(), value, secret }
        })
        .collect()
}

fn server(fam: Family, name: &str, def: &Value, enabled: bool, stashed: bool) -> McpServer {
    let r = decode::decode(fam, def);
    McpServer {
        name: name.to_string(),
        transport: r.transport.to_string(),
        command: r.command.clone(),
        args: mask::args(&r.args),
        cwd: r.cwd.clone(),
        url: r.url.as_deref().map(mask::url),
        env: kvs(&r.env),
        headers: kvs(&r.headers),
        enabled: enabled && !r.off,
        stashed,
        extra: mask::extra(&r.extra),
        sig: sig(&r),
    }
}

/// The servers in an already parsed config, plus the ones AgentPlus stashed for `agent`.
fn servers(agent: &str, fam: Family, cfg: &Value, root: &Value) -> Vec<McpServer> {
    let mut out: Vec<McpServer> = cfg
        .pointer(pointer(fam))
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(name, def)| server(fam, name, def, !switched_off(fam, cfg, name), false)).collect())
        .unwrap_or_default();
    for (name, def) in store::get_obj(root, agent, STASH_KEY) {
        if !out.iter().any(|s| s.name == name) {
            out.push(server(fam, &name, &def, false, true));
        }
    }
    out
}

/// One agent's global MCP servers.
pub fn read(agent: &str) -> AgentMcp {
    let Some((fam, path)) = source(agent) else {
        return AgentMcp { agent: agent.into(), supported: false, file: None, exists: false, servers: vec![], error: None };
    };
    let exists = path.exists();
    let (servers, error) = match exists.then(|| load(fam, &path)) {
        None => (servers(agent, fam, &json!({}), &store::load()), None),
        Some(Ok(cfg)) => (servers(agent, fam, &cfg, &store::load()), None),
        Some(Err(e)) => (vec![], Some(e.to_string())),
    };
    AgentMcp { agent: agent.into(), supported: true, file: Some(display_path(&path)), exists, servers, error }
}

/// `read` for each of `agents` (ids the UI lists; OpenCode project configs are left out).
pub fn list(agents: &[String]) -> Vec<AgentMcp> {
    agents.iter().filter(|a| adapters::ext(a).is_some()).map(|a| read(a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use std::fs;

    fn write(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn names(a: &AgentMcp) -> Vec<(&str, &str, bool)> {
        a.servers.iter().map(|s| (s.name.as_str(), s.transport.as_str(), s.enabled)).collect()
    }

    #[test]
    fn every_family_reads_its_file() {
        let h = TestHome::new("mcp-read");
        let home = &h.0;
        write(&home.join(".claude.json"), r#"{"numStartups":3,"mcpServers":{"gh":{"type":"http","url":"https://api.githubcopilot.com/mcp/","headers":{"Authorization":"Bearer ghp_0123456789abcdefghijklmnop"}}}}"#);
        write(&home.join(".codex/config.toml"), "model = \"x\"\n[mcp_servers.node_repl]\ncommand = 'C:\\bin\\node_repl.exe'\nenv_vars = [\"A\"]\nstartup_timeout_sec = 120\n[mcp_servers.node_repl.env]\nNODE = \"1\"\n[mcp_servers.off]\nurl = \"https://h/mcp\"\nenabled = false\n");
        write(&home.join(".config/opencode/opencode.json"), r#"{ // c
          "mcp": { "fs": { "type": "local", "command": ["npx", "-y", "fs"] }, "r": { "type": "remote", "url": "https://h", "enabled": false } } }"#);
        write(&home.join(".gemini/settings.json"), r#"{"mcpServers":{"vk":{"command":"npx","args":["vibe"],"timeout":60000},"s":{"url":"https://h/sse"}},"mcp":{"excluded":["s"]}}"#);
        write(&home.join(".hermes/config.yaml"), "model:\n  default: x\nmcp_servers:\n  llm-review:\n    command: python\n    args:\n      - server.py\n    connect_timeout: 60\n    enabled: true\n");
        write(&home.join(".openclaw/openclaw.json"), "{ mcp: { servers: { a: { url: 'https://h', transport: 'sse' } } }, }");
        write(&home.join(".zcode/cli/config.json"), r#"{"mcp":{"servers":{"z":{"type":"stdio","command":"uvx","args":["z"],"enabled":false}}}}"#);
        write(&home.join(".factory/mcp.json"), r#"{"mcpServers":{"d":{"command":"x","disabled":true}}}"#);
        write(&home.join(".codebuddy/mcp.json"), r#"{"mcpServers":{"old":{"command":"x"}}}"#);
        write(&home.join(".codebuddy/.mcp.json"), r#"{"mcpServers":{"cb":{"command":"x"},"no":{"command":"y"}},"disabledMcpServers":["no"]}"#);

        let claude = read(claude::ID);
        assert_eq!(names(&claude), [("gh", "http", true)]);
        assert_eq!(claude.servers[0].headers[0].value, "Bearer ••••mnop");
        assert!(claude.servers[0].headers[0].secret);

        let codex = read(codex::ID);
        assert_eq!(names(&codex), [("node_repl", "stdio", true), ("off", "http", false)]);
        assert_eq!(codex.servers[0].env[0].key, "NODE");
        assert_eq!(codex.servers[0].extra["startup_timeout_sec"], 120);

        assert_eq!(names(&read(opencode::ID)), [("fs", "stdio", true), ("r", "remote", false)]);
        assert_eq!(names(&read(gemini::ID)), [("vk", "stdio", true), ("s", "sse", false)]);
        let hermes = read(hermes::ID);
        assert_eq!(names(&hermes), [("llm-review", "stdio", true)]);
        assert_eq!(hermes.servers[0].args, ["server.py"]);
        assert_eq!(names(&read(openclaw::ID)), [("a", "sse", true)]);
        assert_eq!(names(&read(zcode::ID)), [("z", "stdio", false)]);
        assert_eq!(names(&read(droid::ID)), [("d", "stdio", false)]);
        assert_eq!(names(&read(codebuddy::ID)), [("cb", "stdio", true), ("no", "stdio", false)]);

        // Nothing configured yet: an empty, existing-or-not list rather than an error.
        let kimi = read(kimi::ID);
        assert!(kimi.supported && !kimi.exists && kimi.servers.is_empty() && kimi.error.is_none());
        let pi = read(adapters::pi::ID);
        assert!(!pi.supported && pi.file.is_none());
    }

    #[test]
    fn stashed_servers_show_as_off() {
        let h = TestHome::new("mcp-stash");
        write(&h.0.join(".claude.json"), r#"{"mcpServers":{"a":{"command":"x"}}}"#);
        let mut root = store::load();
        store::set_value(&mut root, claude::ID, STASH_KEY, json!({ "b": { "type": "sse", "url": "https://h" }, "a": { "command": "old" } }));
        store::save(&root).unwrap();
        let c = read(claude::ID);
        // A stashed copy of a server that is in the config again doesn't show twice.
        assert_eq!(names(&c), [("a", "stdio", true), ("b", "sse", false)]);
        assert!(c.servers[1].stashed && !c.servers[0].stashed);
    }

    #[test]
    fn broken_files_are_reported_not_fatal() {
        let h = TestHome::new("mcp-broken");
        write(&h.0.join(".gemini/settings.json"), "{ nope");
        let g = read(gemini::ID);
        assert!(g.error.as_deref().is_some_and(|e| e.contains("settings.json")), "{:?}", g.error);
        assert!(g.servers.is_empty());
    }

    #[test]
    fn the_same_server_matches_across_formats() {
        let a = server(Family::Claude, "x", &json!({ "command": "cmd", "args": ["/c", "npx", "-y", "@m/s"], "env": { "T": "${T}" } }), true, false);
        let b = server(Family::OpenCode, "x", &json!({ "type": "local", "command": ["npx.cmd", "-y", "@m/s"], "environment": { "T": "{env:T}" } }), true, false);
        let c = server(Family::Codex, "x", &json!({ "command": "npx", "args": ["-y", "@m/s"], "env": { "T": "other" } }), true, false);
        assert_eq!(a.sig, b.sig);
        assert_ne!(a.sig, c.sig);
        let r1 = server(Family::Gemini, "y", &json!({ "url": "https://h/sse/" }), true, false);
        let r2 = server(Family::Claude, "y", &json!({ "type": "sse", "url": "https://h/sse" }), true, false);
        assert_eq!(r1.sig, r2.sig);
    }

    #[test]
    fn globs() {
        assert!(glob("a*", "abc") && glob("*c", "abc") && glob("*", "") && glob("a*c", "abbc") && glob("abc", "abc"));
        assert!(!glob("a*d", "abc") && !glob("ab", "abc"));
    }
}

#[cfg(test)]
mod dump {
    /// Read-only summary of every agent's MCP servers on this machine (secrets masked).
    /// `cargo test --lib mcp::dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_mcp() {
        let ids: Vec<String> = crate::adapters::ALL.iter().map(|s| s.to_string()).collect();
        for a in super::list(&ids) {
            println!("{:<10} supported={} exists={} file={:?} error={:?}", a.agent, a.supported, a.exists, a.file, a.error);
            for s in a.servers {
                println!("    {} [{}] on={} stashed={} sig={} {:?} {:?} {:?} env={:?} extra={}", s.name, s.transport, s.enabled, s.stashed, s.sig, s.command, s.args, s.url, s.env, serde_json::Value::Object(s.extra));
            }
        }
    }
}
