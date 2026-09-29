//! Writing global MCP servers: the MCP page's ops for one agent, planned like the adapters'
//! own ops (a diff first; on apply a backup, then the file).
//!
//! Each agent gets the server in its own format, with variable references in its own
//! syntax (`{env:X}` for the OpenCode family, `${X}` elsewhere, Codex's `env_vars` /
//! `bearer_token_env_var` / `env_http_headers`). Editing a server keeps the fields the
//! page doesn't show (timeouts, tool filters, OAuth); a copy keeps them too when both
//! agents share a format.

use super::decode::{self, Raw};
use super::{load, mask, pointer, source, toml_item, Family, KvIn, McpInput, LIBRARY, STASH_KEY};
use crate::adapters::{claude, codebuddy, codex, display_name, droid, dsh, gemini, hermes, kilo, mimo, msg, opencode, qwen, zcode, Plan};
use crate::i18n::l;
use crate::model::{Diff, Op};
use crate::store;
use crate::util::{display_path, read_text, strip_jsonc, write_json, write_text_atomic, TextMeta};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

/// How an agent turns a server off without deleting it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Off {
    /// No per-server switch: the definition moves into the AgentPlus store.
    Stash,
    /// A field in the definition: (`disabled`, true) or (`enabled`, false).
    Field(&'static str, bool),
    /// Gemini / Qwen: the `mcp.excluded` list.
    Excluded,
}

fn off_style(agent: &str) -> Off {
    match agent {
        claude::ID | codebuddy::ID => Off::Stash,
        droid::ID => Off::Field("disabled", true),
        gemini::ID | qwen::ID => Off::Excluded,
        _ => Off::Field("enabled", false),
    }
}

/// The transports an agent's config can express.
fn transports(agent: &str) -> &'static [&'static str] {
    match agent {
        claude::ID => &["stdio", "http", "sse", "ws"],
        opencode::ID | kilo::ID | mimo::ID => &["stdio", "remote"],
        codex::ID | dsh::ID => &["stdio", "http"],
        _ => &["stdio", "http", "sse"],
    }
}

/// Agents whose stdio servers take a working folder.
fn has_cwd(agent: &str) -> bool {
    !matches!(agent, claude::ID | codebuddy::ID | droid::ID | hermes::ID)
}

/// Protocol names: not translated.
pub fn transport_label(t: &str) -> &'static str {
    match t {
        "stdio" => "stdio",
        "http" => "HTTP",
        "sse" => "SSE",
        "ws" => "WebSocket",
        _ => "HTTP / SSE",
    }
}

/// How transport `t` is written for `agent`: OpenCode's `remote` covers HTTP and SSE, and
/// is plain HTTP elsewhere.
fn transport_for(agent: &str, t: &str) -> Result<&'static str> {
    let ts = transports(agent);
    let want = match t {
        "remote" if !ts.contains(&"remote") => "http",
        "http" | "sse" if ts.contains(&"remote") => "remote",
        x => x,
    };
    ts.iter().find(|x| **x == want).copied().ok_or_else(|| {
        anyhow!(tr!("{} can't use {} MCP servers", "{} 不支持 {} 类型的 MCP 服务器", display_name(agent), transport_label(t)))
    })
}

fn re(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).unwrap())
}

/// `${X}` → `{env:X}` (a `${X:-default}` has no OpenCode form and stays).
fn to_oc(s: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").replace_all(s, "{env:$1}").into_owned()
}

/// `{env:X}` → `${X}`.
fn to_dollar(s: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\{env:([A-Za-z_][A-Za-z0-9_]*)\}").replace_all(s, "$${$1}").into_owned()
}

/// The variable a value is exactly a reference to (`${X}` → X).
fn only_ref(v: &str) -> Option<&str> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$").captures(v).map(|c| c.get(1).unwrap().as_str())
}

fn has_ref(v: &str) -> bool {
    v.contains("${")
}

fn codex_no_refs(what: &str) -> anyhow::Error {
    anyhow!(tr!(
        "Codex doesn't expand variables inside values ({what}). Use a variable of the same name (KEY = ${{KEY}}), \"Bearer ${{VAR}}\" for Authorization, or a literal value",
        "Codex 不会展开值里的变量（{what}）。可以用同名变量（KEY = ${{KEY}}）、Authorization 用「Bearer ${{VAR}}」，或者直接填值"
    ))
}

/// The server as `agent` writes it. `base` is the definition being edited (its other fields
/// are kept); `on` = enabled.
pub(super) fn encode(agent: &str, fam: Family, i: &McpInput, base: Option<&Map<String, Value>>, on: bool) -> Result<Value> {
    let t = transport_for(agent, &i.transport)?;
    let stdio = t == "stdio";
    let command = i.command.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let url = i.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
    let cwd = i.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty());
    if stdio && command.is_none() {
        bail!("{}", l("A stdio server needs a command", "stdio 服务器需要填写命令"));
    }
    if !stdio && url.is_none() {
        bail!("{}", l("A remote server needs a URL", "远程服务器需要填写地址"));
    }
    if stdio && cwd.is_some() && !has_cwd(agent) {
        bail!("{}", tr!("{} can't set a working folder for MCP servers", "{} 的 MCP 服务器不支持设置工作目录", display_name(agent)));
    }
    let conv = |s: &str| if fam == Family::OpenCode { to_oc(s) } else { to_dollar(s) };
    let (command, url, cwd) = (command.map(conv), url.map(conv), cwd.map(conv));
    let args: Vec<String> = i.args.iter().map(|a| conv(a)).collect();
    let pairs = |kv: &[KvIn]| -> Map<String, Value> { kv.iter().filter(|p| !p.key.trim().is_empty()).map(|p| (p.key.trim().to_string(), json!(conv(&p.value)))).collect() };
    let (env, headers) = (pairs(&i.env), pairs(&i.headers));
    let mut o = Map::new();
    let mut put = |k: &str, v: Value| {
        o.insert(k.into(), v);
    };
    let obj = |m: &Map<String, Value>| Value::Object(m.clone());
    match fam {
        Family::OpenCode => {
            if stdio {
                put("type", json!("local"));
                put("command", json!(std::iter::once(command.clone().unwrap()).chain(args.clone()).collect::<Vec<_>>()));
                if !env.is_empty() {
                    put("environment", obj(&env));
                }
                if let Some(c) = &cwd {
                    put("cwd", json!(c));
                }
            } else {
                put("type", json!("remote"));
                put("url", json!(url));
                if !headers.is_empty() {
                    put("headers", obj(&headers));
                }
            }
        }
        Family::Dsh => {
            if stdio {
                put("transport", json!("stdio"));
                put("command", json!(command));
                if !args.is_empty() {
                    put("args", json!(args));
                }
                if !env.is_empty() {
                    put("env", obj(&env));
                }
                if let Some(c) = &cwd {
                    put("cwd", json!(c));
                }
            } else {
                put("transport", json!("streamable-http"));
                put("url", json!(url));
                if !headers.is_empty() {
                    put("headers", obj(&headers));
                }
            }
        }
        Family::Codex => {
            if stdio {
                put("command", json!(command));
                if !args.is_empty() {
                    put("args", json!(args));
                }
                let (mut lit, mut vars) = (Map::new(), vec![]);
                for (k, v) in &env {
                    let v = v.as_str().unwrap_or_default();
                    match only_ref(v) {
                        Some(r) if r == k => vars.push(json!(k)),
                        _ if has_ref(v) => return Err(codex_no_refs(&format!("{k}={v}"))),
                        _ => {
                            lit.insert(k.clone(), json!(v));
                        }
                    }
                }
                if let Some(c) = &cwd {
                    put("cwd", json!(c));
                }
                if !vars.is_empty() {
                    put("env_vars", Value::Array(vars));
                }
                if !lit.is_empty() {
                    put("env", Value::Object(lit));
                }
            } else {
                put("url", json!(url));
                let (mut lit, mut vars) = (Map::new(), Map::new());
                for (h, v) in &headers {
                    let v = v.as_str().unwrap_or_default();
                    let bearer = v.strip_prefix("Bearer ").and_then(only_ref);
                    if h.eq_ignore_ascii_case("authorization") && bearer.is_some() {
                        put("bearer_token_env_var", json!(bearer));
                    } else if let Some(r) = only_ref(v) {
                        vars.insert(h.clone(), json!(r));
                    } else if has_ref(v) {
                        return Err(codex_no_refs(&format!("{h}: {v}")));
                    } else {
                        lit.insert(h.clone(), json!(v));
                    }
                }
                if !lit.is_empty() {
                    put("http_headers", Value::Object(lit));
                }
                if !vars.is_empty() {
                    put("env_http_headers", Value::Object(vars));
                }
            }
        }
        _ => {
            // Claude Code, CodeBuddy, Droid and ZCode name the transport in `type`; Kimi,
            // Hermes and OpenClaw only mark the non-default one in `transport`.
            let typed = matches!(agent, claude::ID | codebuddy::ID | droid::ID | zcode::ID);
            if typed {
                put("type", json!(t));
            }
            if stdio {
                put("command", json!(command));
                put("args", json!(args));
                if !env.is_empty() {
                    put("env", obj(&env));
                }
                if let Some(c) = &cwd {
                    put("cwd", json!(c));
                }
            } else {
                // Gemini and Qwen: `url` is SSE, `httpUrl` streamable HTTP.
                let key = if fam == Family::Gemini && t == "http" { "httpUrl" } else { "url" };
                put(key, json!(url));
                if !headers.is_empty() {
                    put("headers", obj(&headers));
                }
                if !typed && fam != Family::Gemini {
                    match (fam, t) {
                        (Family::OpenClaw, "http") => put("transport", json!("streamable-http")),
                        (_, "sse") => put("transport", json!("sse")),
                        _ => {}
                    }
                }
            }
        }
    }
    // The fields the page doesn't cover: the edited definition's, else the source's when it
    // has the same format.
    let keep = match base {
        Some(b) => decode::decode(fam, &Value::Object(b.clone())).extra,
        None if i.extra_family == Some(fam) => i.extra.clone(),
        None => Map::new(),
    };
    for (k, v) in keep {
        match (o.get_mut(&k), v) {
            // Codex remote variables (`{ name, source }`) next to the ones written above.
            (Some(Value::Array(ours)), Value::Array(theirs)) if k == "env_vars" => ours.extend(theirs.into_iter().filter(|x| !x.is_string())),
            (Some(_), _) => {}
            (None, v) => {
                o.insert(k, v);
            }
        }
    }
    if let Off::Field(k, off) = off_style(agent) {
        if !on {
            o.insert(k.into(), json!(off));
        } else if base.is_some_and(|b| b.contains_key(k)) {
            o.insert(k.into(), json!(!off));
        }
    }
    Ok(Value::Object(o))
}

/// What runs, for diff lines (secrets masked).
pub(super) fn summary(fam: Family, def: &Value) -> String {
    let r = decode::decode(fam, def);
    if r.transport == "stdio" {
        std::iter::once(r.command.unwrap_or_default()).chain(mask::args(&r.args)).collect::<Vec<_>>().join(" ")
    } else {
        format!("{} {}", transport_label(r.transport), r.url.as_deref().map(mask::url).unwrap_or_default())
    }
}

/// An agent's config file, open for editing its MCP servers.
struct Doc {
    agent: String,
    fam: Family,
    path: PathBuf,
    /// The whole config as JSON (Codex and Hermes: a read-only copy; they are written by block).
    cfg: Value,
    text: String,
    meta: TextMeta,
    /// Why the file can't be written (comments, JSON5 syntax).
    readonly: Option<String>,
    servers: Map<String, Value>,
    stash: Map<String, Value>,
    /// Names whose definition (or presence) in the config changed.
    changed: BTreeSet<String>,
    lists_changed: bool,
    stash_changed: bool,
}

impl Doc {
    fn open(agent: &str) -> Result<Doc> {
        if crate::adapters::ocproject::is_project(agent) {
            bail!("{}", l("Project configs have no global MCP servers", "项目配置没有全局 MCP 服务器"));
        }
        let (fam, path) = source(agent).ok_or_else(|| anyhow!(tr!("{} doesn't support MCP servers", "{} 不支持 MCP 服务器", display_name(agent))))?;
        let (text, meta) = if path.exists() { read_text(&path)? } else { (String::new(), TextMeta::NEW) };
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let readonly = match fam {
            Family::Codex | Family::Hermes => None,
            Family::OpenClaw if serde_json::from_str::<Value>(&text).is_err() && !text.trim().is_empty() => Some(tr!(
                "{file} uses JSON5 syntax that AgentPlus would lose on write, so it's read-only.",
                "{file} 用了 JSON5 写法，AgentPlus 写回会丢掉这些格式，已切换为只读。"
            )),
            _ => strip_jsonc(&text).1.then(|| msg::comments_readonly(&file)),
        };
        let cfg = if path.exists() { load(fam, &path)? } else { json!({}) };
        if !cfg.is_object() {
            bail!("{}", tr!("The top level of {} is not an object; leaving it alone", "{} 的顶层不是对象，不修改它", display_path(&path)));
        }
        let servers = cfg.pointer(pointer(fam)).and_then(Value::as_object).cloned().unwrap_or_default();
        let stash = store::get_obj(&store::load(), agent, STASH_KEY);
        Ok(Doc { agent: agent.into(), fam, path, cfg, text, meta, readonly, servers, stash, changed: BTreeSet::new(), lists_changed: false, stash_changed: false })
    }

    fn file(&self) -> String {
        display_path(&self.path)
    }

    /// A name list in the config (Gemini `mcp.excluded`, CodeBuddy `disabledMcpServers`).
    fn list_ptr(&self) -> Option<&'static str> {
        match (off_style(&self.agent), self.agent.as_str()) {
            (Off::Excluded, _) => Some("/mcp/excluded"),
            (_, codebuddy::ID) => Some("/disabledMcpServers"),
            _ => None,
        }
    }

    fn listed(&self, name: &str) -> bool {
        self.list_ptr().and_then(|p| self.cfg.pointer(p)).and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_str() == Some(name)))
    }

    fn set_listed(&mut self, name: &str, on_list: bool) -> Result<()> {
        let Some(p) = self.list_ptr() else { return Ok(()) };
        if self.listed(name) == on_list {
            return Ok(());
        }
        let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
        let (last, parent) = parts.split_last().unwrap();
        let obj = crate::util::obj_at(&mut self.cfg, parent)?;
        let list = obj.entry(last.to_string()).or_insert_with(|| json!([]));
        let Some(a) = list.as_array_mut() else { bail!("{}", tr!("{} is not a list", "{} 不是列表", parts.join("."))) };
        if on_list {
            a.push(json!(name));
        } else {
            a.retain(|x| x.as_str() != Some(name));
        }
        self.lists_changed = true;
        Ok(())
    }

    fn exists(&self, name: &str) -> bool {
        self.servers.contains_key(name) || self.stash.contains_key(name)
    }

    /// In the config, not switched off in its definition, and not on an off list.
    fn enabled(&self, name: &str) -> bool {
        self.servers.get(name).is_some_and(|d| !decode::decode(self.fam, d).off) && !self.listed(name)
    }

    fn remove(&mut self, name: &str) -> Result<bool> {
        let mut found = false;
        if self.servers.shift_remove(name).is_some() {
            self.changed.insert(name.into());
            found = true;
        }
        if self.stash.shift_remove(name).is_some() {
            self.stash_changed = true;
            found = true;
        }
        if found {
            self.set_listed(name, false)?;
        }
        Ok(found)
    }

    /// Puts a definition in place: in the config, or in the stash when it is off and the agent
    /// has no switch of its own.
    fn place(&mut self, name: &str, def: Value, on: bool) -> Result<()> {
        let style = off_style(&self.agent);
        if !on && style == Off::Stash {
            if self.servers.shift_remove(name).is_some() {
                self.changed.insert(name.into());
            }
            self.stash.insert(name.into(), def);
            self.stash_changed = true;
            return self.set_listed(name, false);
        }
        if self.stash.shift_remove(name).is_some() {
            self.stash_changed = true;
        }
        if self.servers.get(name) != Some(&def) {
            self.servers.insert(name.into(), def);
            self.changed.insert(name.into());
        }
        self.set_listed(name, style == Off::Excluded && !on)
    }

    fn apply(&mut self, op: &Op, diff: &mut Diff) -> Result<()> {
        let file = self.file();
        match op {
            Op::UpsertMcp { server: i } => {
                let name = i.name.trim();
                if name.is_empty() {
                    return Err(msg::name_required());
                }
                if name.contains(['.', '/']) && self.fam == Family::Codex {
                    bail!("{}", l("Codex server names can't contain \".\" or \"/\"", "Codex 的服务器名称不能包含「.」或「/」"));
                }
                let old = i.replaces.as_deref().map(str::trim).filter(|r| *r != name && self.exists(r));
                if old.is_some() && self.exists(name) {
                    bail!("{}", tr!("{} already has an MCP server named \"{name}\"", "{} 已经有名为「{name}」的 MCP 服务器", display_name(&self.agent)));
                }
                let key = old.unwrap_or(name);
                let base = self.servers.get(key).or_else(|| self.stash.get(key)).cloned();
                let def = encode(&self.agent, self.fam, i, base.as_ref().and_then(Value::as_object), i.enabled)?;
                let was_on = self.enabled(key);
                let same = base.as_ref() == Some(&def) && was_on == i.enabled && old.is_none();
                if same {
                    return Ok(());
                }
                if let Some(o) = old {
                    self.remove(o)?;
                    diff.push(&file, tr!("MCP \"{o}\" renamed to \"{name}\"", "MCP「{o}」改名为「{name}」"), true);
                }
                let what = summary(self.fam, &def);
                match &base {
                    None => diff.push(&file, tr!("+ MCP \"{name}\": {what}", "+ MCP「{name}」：{what}"), true),
                    Some(b) if *b != def => diff.push(&file, tr!("MCP \"{name}\" updated: {what}", "MCP「{name}」修改为：{what}"), true),
                    _ => {}
                }
                if base.is_some() && was_on != i.enabled {
                    diff.push(&file, onoff(name, i.enabled), i.enabled);
                } else if base.is_none() && !i.enabled {
                    diff.push(&file, onoff(name, false), false);
                }
                self.place(name, def, i.enabled)
            }
            Op::DeleteMcp { name } => {
                if !self.remove(name)? {
                    bail!("{}", tr!("No MCP server named \"{name}\"", "没有名为「{name}」的 MCP 服务器"));
                }
                diff.push(&file, tr!("− MCP \"{name}\"", "− MCP「{name}」"), false);
                Ok(())
            }
            Op::SetMcpEnabled { name, enabled } => {
                let def = self.servers.get(name).or_else(|| self.stash.get(name)).cloned().ok_or_else(|| anyhow!(tr!("No MCP server named \"{name}\"", "没有名为「{name}」的 MCP 服务器")))?;
                if self.enabled(name) == *enabled {
                    return Ok(());
                }
                let mut def = def;
                if let (Off::Field(k, off), Some(o)) = (off_style(&self.agent), def.as_object_mut()) {
                    if *enabled {
                        // `enabled: true` written out stays written out; `disabled: true` goes away.
                        if o.get(k) == Some(&json!(off)) {
                            if k == "enabled" { o.insert(k.into(), json!(true)); } else { o.remove(k); }
                        }
                        o.remove("enable");
                    } else {
                        o.insert(k.into(), json!(off));
                    }
                }
                diff.push(&file, onoff(name, *enabled), *enabled);
                self.place(name, def, *enabled)
            }
            _ => Ok(()),
        }
    }

    fn save_stash(&self, stash: &Map<String, Value>) -> Result<()> {
        let mut root = store::load();
        store::set_value(&mut root, &self.agent, STASH_KEY, if stash.is_empty() { Value::Null } else { Value::Object(stash.clone()) });
        store::save(&root)
    }

    fn write(&mut self) -> Result<()> {
        let config_changed = !self.changed.is_empty() || self.lists_changed;
        // A server moves between the config and the stash. Whichever write fails, it must stay
        // in one of them: the stash first gains what it takes in, and only loses what went
        // back to the config once the config is written.
        if self.stash_changed && config_changed && self.readonly.is_none() {
            let mut both = store::get_obj(&store::load(), &self.agent, STASH_KEY);
            let before = both.clone();
            both.extend(self.stash.iter().map(|(k, v)| (k.clone(), v.clone())));
            if both != before {
                self.save_stash(&both)?;
            }
        }
        if config_changed {
            if let Some(why) = &self.readonly {
                bail!("{why}");
            }
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            match self.fam {
                Family::Codex => self.write_codex()?,
                Family::Hermes => {
                    let block = (!self.servers.is_empty()).then(|| serde_yaml::to_value(Value::Object(self.servers.clone()))).transpose()?;
                    let out = hermes::rewrite(&self.text, &[("mcp_servers", block.as_ref())])?;
                    write_text_atomic(&self.path, &out, self.meta)?;
                }
                _ => {
                    let parts: Vec<&str> = pointer(self.fam).trim_start_matches('/').split('/').collect();
                    let (last, parent) = parts.split_last().unwrap();
                    crate::util::obj_at(&mut self.cfg, parent)?.insert(last.to_string(), Value::Object(self.servers.clone()));
                    write_json(&self.path, &self.cfg, self.meta)?;
                }
            }
        }
        if self.stash_changed {
            self.save_stash(&self.stash)?;
        }
        Ok(())
    }

    /// Codex: only the changed `[mcp_servers.<name>]` tables are touched, key by key, so the
    /// rest of config.toml keeps its layout.
    fn write_codex(&self) -> Result<()> {
        let mut doc: toml_edit::DocumentMut = self.text.parse().map_err(|e| anyhow!(tr!("Failed to parse config.toml: {e}", "config.toml 解析失败：{e}")))?;
        let root = doc.as_table_mut();
        if !root.contains_key("mcp_servers") {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            root.insert("mcp_servers", toml_edit::Item::Table(t));
        }
        let all = root["mcp_servers"].as_table_mut().ok_or_else(|| anyhow!(l("mcp_servers in config.toml is not a table", "config.toml 里的 mcp_servers 不是表")))?;
        for name in &self.changed {
            let Some(Value::Object(def)) = self.servers.get(name) else {
                all.remove(name);
                continue;
            };
            let t = all.entry(name).or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
            let Some(t) = t.as_table_mut() else {
                *t = toml_edit::Item::Table(table_of(def));
                continue;
            };
            let gone: Vec<String> = t.iter().map(|(k, _)| k.to_string()).filter(|k| !def.contains_key(k)).collect();
            for k in gone {
                t.remove(&k);
            }
            for (k, v) in def {
                if t.get(k).map(toml_item).as_ref() != Some(v) {
                    t[k.as_str()] = item_of(v);
                }
            }
        }
        write_text_atomic(&self.path, &doc.to_string(), self.meta)
    }
}

pub(super) fn onoff(name: &str, on: bool) -> String {
    if on {
        tr!("MCP \"{name}\" → on", "MCP「{name}」→ 启用")
    } else {
        tr!("MCP \"{name}\" → off", "MCP「{name}」→ 停用")
    }
}

fn value_of(v: &Value) -> toml_edit::Value {
    match v {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => n.as_i64().map(toml_edit::Value::from).unwrap_or_else(|| n.as_f64().unwrap_or_default().into()),
        Value::Array(a) => toml_edit::Value::Array(a.iter().map(value_of).collect()),
        Value::Object(o) => toml_edit::Value::InlineTable(o.iter().map(|(k, v)| (k.as_str(), value_of(v))).collect()),
        Value::Null => "".into(),
    }
}

/// Objects become sub-tables (`[mcp_servers.x.env]`, as Codex writes them), the rest values.
fn item_of(v: &Value) -> toml_edit::Item {
    match v {
        Value::Object(o) => toml_edit::Item::Table(table_of(o)),
        other => toml_edit::Item::Value(value_of(other)),
    }
}

fn table_of(o: &Map<String, Value>) -> toml_edit::Table {
    let mut t = toml_edit::Table::new();
    for (k, v) in o {
        t.insert(k, item_of(v));
    }
    t
}

/// The MCP ops of one apply (see `adapters::plan_resolved`).
pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    if agent == dsh::ID {
        return super::dsh::plan(ops, dry_run);
    }
    let mut doc = Doc::open(agent)?;
    let mut diff = Diff::default();
    for op in ops {
        doc.apply(op, &mut diff)?;
    }
    let config_changed = !doc.changed.is_empty() || doc.lists_changed;
    if !config_changed && !doc.stash_changed {
        return Ok((diff, vec![], None));
    }
    if let (true, Some(why)) = (config_changed, &doc.readonly) {
        bail!("{why}");
    }
    // (Not in tests: they run next to a real Claude Code often enough.)
    if agent == claude::ID && !crate::env::is_wsl() && crate::util::test_home().is_none() && crate::process::any_process(|n, _| n.eq_ignore_ascii_case("claude.exe") || n == "claude") {
        diff.push(
            l("Note", "注意"),
            l(
                "Claude Code is running, and it rewrites ~/.claude.json itself. Quit it before applying so this change isn't overwritten.",
                "Claude Code 正在运行，它自己也会改写 ~/.claude.json。应用前先退出它，免得这次的改动被覆盖。",
            ),
            false,
        );
    }
    if dry_run {
        return Ok((diff, vec![doc.path.clone()], None));
    }
    let backup = if config_changed && doc.path.exists() { Some(crate::util::backup(agent, std::slice::from_ref(&doc.path))?) } else { None };
    doc.write()?;
    Ok((diff, if config_changed { vec![doc.path] } else { vec![] }, backup))
}

// ---------------------------------------------------------------- sources of masked values

/// A server as an agent or the library has it, with its real values.
pub(super) fn raw_from(source: &str, name: &str) -> Result<Raw> {
    if source == LIBRARY {
        return super::library::raw(name).map(|(r, _)| r);
    }
    raw_in(source, name).map(|(r, _)| r)
}

/// A server as `agent` has it (the config or the stash), read into the common shape.
fn raw_in(agent: &str, name: &str) -> Result<(Raw, Family)> {
    if agent == dsh::ID {
        return super::dsh::raw(name).map(|r| (r, Family::Dsh));
    }
    let doc = Doc::open(agent)?;
    let def = doc.servers.get(name).or_else(|| doc.stash.get(name)).ok_or_else(|| anyhow!(tr!("{} has no MCP server named \"{name}\"", "{} 没有名为「{name}」的 MCP 服务器", display_name(agent))))?;
    Ok((decode::decode(doc.fam, def), doc.fam))
}

/// Puts the real values back where the UI sent the masked ones it was shown (by key for
/// env and headers, by position for arguments), and brings the source's other fields along.
pub fn resolve(mut i: McpInput) -> Result<McpInput> {
    if let Some((src, name)) = i.from.take() {
        let (raw, fam) = if src == LIBRARY { super::library::raw(&name)? } else { raw_in(&src, &name).map(|(r, f)| (r, Some(f)))? };
        let unmask = |kv: &mut Vec<KvIn>, from: &[(String, String)]| {
            for p in kv.iter_mut() {
                if let Some((k, v)) = from.iter().find(|(k, _)| *k == p.key) {
                    if mask::value(k, v).0 == p.value {
                        p.value = v.clone();
                    }
                }
            }
        };
        unmask(&mut i.env, &raw.env);
        unmask(&mut i.headers, &raw.headers);
        let shown = mask::args(&raw.args);
        for (n, a) in i.args.iter_mut().enumerate() {
            if shown.get(n) == Some(a) {
                *a = raw.args[n].clone();
            }
        }
        if let (Some(u), Some(real)) = (i.url.as_mut(), raw.url.as_ref()) {
            if *u == mask::url(real) {
                *u = real.clone();
            }
        }
        if i.extra.is_empty() {
            i.extra = raw.extra;
            i.extra_family = fam;
        }
    }
    let masked = |s: &str| s.contains('•');
    let bad = i.args.iter().map(String::as_str).chain(i.url.as_deref()).chain(i.env.iter().chain(&i.headers).map(|p| p.value.as_str())).any(masked);
    if bad {
        bail!("{}", l("A hidden value couldn't be restored; enter it again", "有被隐藏的值无法还原，请重新填写"));
    }
    Ok(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use std::fs;
    use crate::adapters::{kimi, openclaw};
    use std::path::Path;

    fn write(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// Applies `ops` through the same path as the app; returns the diff lines.
    fn run(agent: &str, ops: Vec<Value>) -> Result<Vec<String>> {
        let ops: Vec<Op> = ops.into_iter().map(|v| serde_json::from_value(v).unwrap()).collect();
        let ops = crate::adapters::resolve(agent, &ops)?;
        let (d, _, _) = store::transaction(|| crate::adapters::plan_resolved(agent, &ops, false))?;
        Ok(d.groups.into_iter().flat_map(|g| g.lines).map(|l| l.text).collect())
    }

    fn upsert(v: Value) -> Value {
        json!({ "op": "upsert_mcp", "server": v })
    }

    #[test]
    fn stdio_server_in_every_format() {
        let h = TestHome::new("mcp-write-formats");
        let i = json!({ "name": "fs", "transport": "stdio", "command": "npx", "args": ["-y", "@m/fs"], "env": [{ "key": "TOKEN", "value": "${TOKEN}" }] });
        for a in [claude::ID, codebuddy::ID, droid::ID, kimi::ID, gemini::ID, qwen::ID, opencode::ID, kilo::ID, mimo::ID, codex::ID, hermes::ID, openclaw::ID, zcode::ID] {
            run(a, vec![upsert(i.clone())]).unwrap_or_else(|e| panic!("{a}: {e}"));
            let got = super::super::read(a);
            let s = got.servers.iter().find(|s| s.name == "fs").unwrap_or_else(|| panic!("{a}: {:?}", got));
            assert_eq!((s.transport.as_str(), s.command.as_deref(), s.enabled), ("stdio", Some("npx"), true), "{a}");
            assert_eq!(s.args, ["-y", "@m/fs"], "{a}");
            assert_eq!(s.env[0].key, "TOKEN", "{a}");
        }
        let text = |p: &str| fs::read_to_string(h.0.join(p)).unwrap();
        assert!(text(".config/opencode/opencode.json").contains("\"TOKEN\": \"{env:TOKEN}\""));
        assert!(text(".codex/config.toml").contains("[mcp_servers.fs]\ncommand = \"npx\"\nargs = [\"-y\", \"@m/fs\"]\nenv_vars = [\"TOKEN\"]"), "{}", text(".codex/config.toml"));
        assert!(text(".claude.json").contains("\"type\": \"stdio\""));
        assert!(!text(".kimi-code/mcp.json").contains("\"type\""));
        assert!(text(".hermes/config.yaml").contains("mcp_servers:\n  fs:\n"));
        // The same server everywhere.
        let sigs: BTreeSet<String> = super::super::list(&crate::adapters::ALL.map(String::from)).into_iter().flat_map(|a| a.servers).map(|s| s.sig).collect();
        assert_eq!(sigs.len(), 1, "{sigs:?}");
    }

    #[test]
    fn remote_servers_follow_each_format() {
        let h = TestHome::new("mcp-write-remote");
        let sse = json!({ "name": "r", "transport": "sse", "url": "https://h/sse", "headers": [{ "key": "Authorization", "value": "Bearer ${T}" }] });
        run(gemini::ID, vec![upsert(sse.clone())]).unwrap();
        run(opencode::ID, vec![upsert(sse.clone())]).unwrap();
        run(hermes::ID, vec![upsert(sse.clone())]).unwrap();
        let err = run(codex::ID, vec![upsert(sse.clone())]).unwrap_err().to_string();
        assert_eq!(err, "Codex 不支持 SSE 类型的 MCP 服务器");
        let http = json!({ "name": "h", "transport": "http", "url": "https://h/mcp", "headers": [{ "key": "Authorization", "value": "Bearer ${T}" }, { "key": "X-Team", "value": "${TEAM}" }] });
        run(gemini::ID, vec![upsert(http.clone())]).unwrap();
        run(codex::ID, vec![upsert(http.clone())]).unwrap();
        let g: Value = serde_json::from_str(&fs::read_to_string(h.0.join(".gemini/settings.json")).unwrap()).unwrap();
        assert_eq!(g["mcpServers"]["r"]["url"], "https://h/sse");
        assert_eq!(g["mcpServers"]["h"]["httpUrl"], "https://h/mcp");
        let oc: Value = serde_json::from_str(&fs::read_to_string(h.0.join(".config/opencode/opencode.json")).unwrap()).unwrap();
        assert_eq!(oc["mcp"]["r"], json!({ "type": "remote", "url": "https://h/sse", "headers": { "Authorization": "Bearer {env:T}" } }));
        let codex = fs::read_to_string(h.0.join(".codex/config.toml")).unwrap();
        assert!(codex.contains("bearer_token_env_var = \"T\"") && codex.contains("X-Team = \"TEAM\""), "{codex}");
        assert!(fs::read_to_string(h.0.join(".hermes/config.yaml")).unwrap().contains("transport: sse"));
        // Codex can't expand a variable that isn't the whole value.
        let bad = json!({ "name": "b", "transport": "stdio", "command": "x", "env": [{ "key": "A", "value": "pre-${B}" }] });
        assert!(run(codex::ID, vec![upsert(bad)]).unwrap_err().to_string().starts_with("Codex 不会展开"));
    }

    #[test]
    fn switching_off_uses_each_agents_mechanism() {
        let h = TestHome::new("mcp-write-off");
        let i = json!({ "name": "s", "transport": "stdio", "command": "x" });
        for a in [claude::ID, droid::ID, gemini::ID, codex::ID] {
            run(a, vec![upsert(i.clone())]).unwrap();
            let d = run(a, vec![json!({ "op": "set_mcp_enabled", "name": "s", "enabled": false })]).unwrap();
            assert_eq!(d, ["MCP「s」→ 停用"], "{a}");
            let s = super::super::read(a).servers.into_iter().find(|s| s.name == "s").unwrap();
            assert!(!s.enabled, "{a}");
            assert_eq!(s.stashed, a == claude::ID, "{a}");
        }
        let claude_json = fs::read_to_string(h.0.join(".claude.json")).unwrap();
        assert!(!claude_json.contains("\"s\""), "stashed out of the config: {claude_json}");
        assert!(fs::read_to_string(h.0.join(".factory/mcp.json")).unwrap().contains("\"disabled\": true"));
        assert!(fs::read_to_string(h.0.join(".gemini/settings.json")).unwrap().contains("\"excluded\""));
        assert!(fs::read_to_string(h.0.join(".codex/config.toml")).unwrap().contains("enabled = false"));
        // And back on.
        for a in [claude::ID, droid::ID, gemini::ID, codex::ID] {
            run(a, vec![json!({ "op": "set_mcp_enabled", "name": "s", "enabled": true })]).unwrap();
            assert!(super::super::read(a).servers.iter().any(|s| s.name == "s" && s.enabled && !s.stashed), "{a}");
        }
        assert!(store::get_obj(&store::load(), claude::ID, STASH_KEY).is_empty());
        assert!(!fs::read_to_string(h.0.join(".factory/mcp.json")).unwrap().contains("disabled"));
        // Nothing to do: no diff, no write.
        assert!(run(codex::ID, vec![json!({ "op": "set_mcp_enabled", "name": "s", "enabled": true })]).unwrap().is_empty());
    }

    /// A config that can't be replaced (read-only on Windows) never costs a stashed server.
    #[cfg(windows)]
    #[test]
    fn a_failed_config_write_keeps_the_server_somewhere() {
        let h = TestHome::new("mcp-write-stash-order");
        let cfg = h.0.join(".claude.json");
        let set_ro = |on: bool| {
            let mut p = fs::metadata(&cfg).unwrap().permissions();
            p.set_readonly(on);
            fs::set_permissions(&cfg, p).unwrap();
        };
        let stashed = || store::get_obj(&store::load(), claude::ID, STASH_KEY).contains_key("s");
        let in_config = || fs::read_to_string(&cfg).unwrap().contains("\"s\"");
        run(claude::ID, vec![upsert(json!({ "name": "s", "transport": "stdio", "command": "x" }))]).unwrap();
        let off = || run(claude::ID, vec![json!({ "op": "set_mcp_enabled", "name": "s", "enabled": false })]);
        let on = || run(claude::ID, vec![json!({ "op": "set_mcp_enabled", "name": "s", "enabled": true })]);
        // Off: the stash takes it before the config lets go.
        set_ro(true);
        assert!(off().is_err());
        set_ro(false);
        assert!(in_config() && stashed());
        off().unwrap();
        assert!(!in_config() && stashed());
        // On: the stash lets go only once the config has it.
        set_ro(true);
        assert!(on().is_err());
        set_ro(false);
        assert!(!in_config() && stashed());
        on().unwrap();
        assert!(in_config() && !stashed());
    }

    #[test]
    fn editing_keeps_other_fields_and_masked_values() {
        let h = TestHome::new("mcp-write-edit");
        write(&h.0.join(".codex/config.toml"), "model = \"m\" # keep\n\n[mcp_servers.gh]\nurl = \"https://h/mcp\"\nhttp_headers = { \"X-Key\" = \"abcdefgh12345678\" }\nstartup_timeout_sec = 30\n\n[mcp_servers.other]\ncommand = \"o\"\n");
        let shown = super::super::read(codex::ID).servers.into_iter().find(|s| s.name == "gh").unwrap();
        assert_eq!(shown.headers[0].value, "••••5678");
        // The page sends back what it showed, with a new URL and a rename.
        run(codex::ID, vec![upsert(json!({
            "name": "github", "replaces": "gh", "transport": "http", "url": "https://h/v2",
            "headers": [{ "key": "X-Key", "value": "••••5678" }], "from": ["codex", "gh"],
        }))]).unwrap();
        let text = fs::read_to_string(h.0.join(".codex/config.toml")).unwrap();
        assert!(text.starts_with("model = \"m\" # keep\n"), "{text}");
        assert!(text.contains("[mcp_servers.other]\ncommand = \"o\""), "{text}");
        assert!(text.contains("abcdefgh12345678") && text.contains("startup_timeout_sec = 30") && text.contains("https://h/v2") && !text.contains("[mcp_servers.gh]"), "{text}");
        // A masked value from nowhere can't be written.
        let err = run(codex::ID, vec![upsert(json!({ "name": "x", "transport": "http", "url": "https://h", "headers": [{ "key": "K", "value": "••••1234" }] }))]).unwrap_err();
        assert_eq!(err.to_string(), "有被隐藏的值无法还原，请重新填写");
    }

    #[test]
    fn copies_bring_secrets_and_same_format_fields() {
        let h = TestHome::new("mcp-write-copy");
        write(&h.0.join(".claude.json"), r#"{"mcpServers":{"k":{"type":"stdio","command":"npx","args":["srv","--api-key","sk-secret-0123456789"],"env":{"API_KEY":"abcdefgh99999999"},"alwaysLoad":true}}}"#);
        let s = super::super::read(claude::ID).servers.remove(0);
        let copy = json!({
            "name": "k", "transport": "stdio", "command": "npx", "args": s.args, "from": ["claude", "k"],
            "env": s.env.iter().map(|p| json!({ "key": p.key, "value": p.value })).collect::<Vec<_>>(),
        });
        run(codebuddy::ID, vec![upsert(copy.clone())]).unwrap();
        run(opencode::ID, vec![upsert(copy)]).unwrap();
        let cb = fs::read_to_string(h.0.join(".codebuddy/.mcp.json")).unwrap();
        assert!(cb.contains("sk-secret-0123456789") && cb.contains("abcdefgh99999999") && cb.contains("alwaysLoad"), "{cb}");
        // A different format doesn't get Claude's own fields.
        let oc = fs::read_to_string(h.0.join(".config/opencode/opencode.json")).unwrap();
        assert!(oc.contains("abcdefgh99999999") && !oc.contains("alwaysLoad"), "{oc}");
    }

    #[test]
    fn deleting_and_readonly_files() {
        let h = TestHome::new("mcp-write-delete");
        write(&h.0.join(".config/opencode/opencode.json"), "{\n  // mine\n  \"mcp\": { \"a\": { \"type\": \"local\", \"command\": [\"a\"] } }\n}\n");
        let err = run(opencode::ID, vec![json!({ "op": "delete_mcp", "name": "a" })]).unwrap_err().to_string();
        assert!(err.contains("含注释"), "{err}");
        write(&h.0.join(".gemini/settings.json"), r#"{"mcpServers":{"a":{"command":"a"}},"mcp":{"excluded":["a"]}}"#);
        let d = run(gemini::ID, vec![json!({ "op": "delete_mcp", "name": "a" })]).unwrap();
        assert_eq!(d, ["− MCP「a」"]);
        let g: Value = serde_json::from_str(&fs::read_to_string(h.0.join(".gemini/settings.json")).unwrap()).unwrap();
        assert_eq!(g, json!({ "mcpServers": {}, "mcp": { "excluded": [] } }));
        assert_eq!(run(gemini::ID, vec![json!({ "op": "delete_mcp", "name": "a" })]).unwrap_err().to_string(), "没有名为「a」的 MCP 服务器");
        assert!(run(crate::adapters::pi::ID, vec![json!({ "op": "delete_mcp", "name": "a" })]).unwrap_err().to_string().contains("不支持 MCP"));
    }

    #[test]
    fn cwd_and_transport_limits() {
        let _h = TestHome::new("mcp-write-limits");
        let cwd = json!({ "name": "c", "transport": "stdio", "command": "x", "cwd": "D:/w" });
        assert_eq!(run(claude::ID, vec![upsert(cwd.clone())]).unwrap_err().to_string(), "Claude Code 的 MCP 服务器不支持设置工作目录");
        run(codex::ID, vec![upsert(cwd)]).unwrap();
        let ws = json!({ "name": "w", "transport": "ws", "url": "wss://h" });
        run(claude::ID, vec![upsert(ws.clone())]).unwrap();
        assert!(run(gemini::ID, vec![upsert(ws)]).is_err());
    }
}
