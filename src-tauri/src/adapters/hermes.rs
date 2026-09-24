//! Hermes Agent (NousResearch): `config.yaml` in HERMES_HOME (Windows: the HERMES_HOME user
//! variable, else `%LOCALAPPDATA%\hermes`; Linux / WSL: `~/.hermes`).
//!
//! Providers are the entries of the legacy `custom_providers:` list (id = name) and of the
//! keyed `providers:` dict (id = key). One of them is active through `model.provider`
//! (`custom:<key or name>`), with `model.default` as the model id. A bare `custom` provider
//! (inline `model.base_url` / `model.api_key`) shows up as the synthetic "custom" provider;
//! a built-in Hermes provider (openrouter, anthropic, auto, …) shows up read-only.
//! Switching away from the inline or a built-in provider stashes it in the AgentPlus store
//! so it can be switched back to.
//!
//! Writing: serde_yaml drops comments and styling, so the file is split into top-level
//! blocks and only the changed `model` / `providers` / `custom_providers` blocks are
//! re-emitted (in Hermes' own PyYAML style); everything else stays byte-for-byte. A changed
//! block that contains comments or anchors is refused.


use super::{Plan, Endpoint};
use crate::dotenv;
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map as JMap, Value as J};
use serde_yaml::{Mapping, Value as Y};
use std::path::{Path, PathBuf};

pub const ID: &str = "hermes";
#[allow(dead_code)]
pub const NAME: &str = "Hermes";
/// Relative to the config dir.
pub const MARKER: &str = "config.yaml";
/// WSL: the launcher lives in ~/.local/bin, which is not on a non-login PATH.
#[allow(dead_code)]
pub const WSL_SCRIPT: &str = "(command -v hermes >/dev/null && hermes --version || $HOME/.local/bin/hermes --version) 2>/dev/null | head -n 1; pgrep -f '[b]in/hermes' >/dev/null && echo @running; true";
#[allow(dead_code)]
pub const WSL_MARKER: &str = ".hermes/config.yaml";

/// The synthetic provider for bare `model.provider: custom` (inline base_url / api_key).
const INLINE: &str = "custom";
/// Id prefix of built-in Hermes providers (read-only).
const BUILTIN: &str = "builtin:";
/// Top-level blocks this adapter may rewrite.
const BLOCKS: [&str; 3] = ["model", "providers", "custom_providers"];

// ---------------------------------------------------------------- paths

/// Default config dir: HERMES_HOME (process env, then the user environment in the registry,
/// since AgentPlus may have started before it was set), else `%LOCALAPPDATA%\hermes`.
/// In WSL mode (and in tests) `~/.hermes` of the target home.
pub fn default_dir() -> PathBuf {
    if crate::env::is_wsl() || test_home().is_some() {
        return home().join(".hermes");
    }
    native_home().unwrap_or_else(|| home().join(".hermes"))
}

#[cfg(windows)]
fn native_home() -> Option<PathBuf> {
    let from_env = crate::env::agent_var("HERMES_HOME").filter(|s| !s.trim().is_empty());
    let from_reg = || -> Option<String> {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment").ok()?.get_value::<String, _>("HERMES_HOME").ok().filter(|s| !s.trim().is_empty())
    };
    if let Some(raw) = from_env.or_else(from_reg) {
        return Some(PathBuf::from(expand_percent(raw.trim())));
    }
    Some(dirs::data_local_dir()?.join("hermes"))
}

#[cfg(not(windows))]
fn native_home() -> Option<PathBuf> {
    crate::env::agent_var("HERMES_HOME").filter(|s| !s.trim().is_empty()).map(PathBuf::from)
}

/// `%LOCALAPPDATA%\x` → the value of the variable (REG_EXPAND_SZ values come back raw).
#[allow(dead_code)]
fn expand_percent(s: &str) -> String {
    let re = regex::Regex::new(r"%([^%]+)%").unwrap();
    re.replace_all(s, |c: &regex::Captures| std::env::var(&c[1]).unwrap_or_else(|_| c[0].to_string())).to_string()
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn config_path() -> PathBuf {
    dir().join(MARKER)
}

fn env_path() -> PathBuf {
    dir().join(".env")
}

fn auth_path() -> PathBuf {
    dir().join("auth.json")
}

// ---------------------------------------------------------------- detection

fn pyproject_version(p: &Path) -> Option<String> {
    let text = std::fs::read_to_string(p).ok()?;
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("version").map(str::trim_start).and_then(|r| r.strip_prefix('=')).map(|r| r.trim().trim_matches('"').to_string()))
        .filter(|v| !v.is_empty())
}

/// Windows: `<HERMES_HOME>\hermes-agent\venv\Scripts\hermes.exe`; version from pyproject.toml
/// (no Python start-up). A CLI, so `exe` stays None.
#[allow(dead_code)]
pub fn detect() -> Install {
    let mut inst = Install::default();
    let mut roots = vec![default_dir()];
    if let Some(l) = dirs::data_local_dir() {
        roots.push(l.join("hermes"));
    }
    for root in roots {
        let agent = root.join("hermes-agent");
        let exe = agent.join("venv").join("Scripts").join("hermes.exe");
        if exe.exists() || agent.join("pyproject.toml").exists() {
            inst.installed = true;
            inst.version = pyproject_version(&agent.join("pyproject.toml"));
            inst.dir = Some(agent);
            break;
        }
    }
    if inst.installed {
        inst.running = crate::process::any_process(|name, path| {
            name.eq_ignore_ascii_case("hermes.exe") || path.to_lowercase().contains("\\hermes-agent\\venv\\")
        });
    }
    inst
}

// ---------------------------------------------------------------- YAML: reading helpers

fn ystr(v: &Y, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn key_str(k: &Y) -> Option<String> {
    match k {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn yk(s: &str) -> Y {
    Y::String(s.to_string())
}

fn y2j(v: &Y) -> J {
    serde_json::to_value(v).unwrap_or(J::Null)
}

fn j2y(v: &J) -> Y {
    serde_yaml::to_value(v).unwrap_or(Y::Null)
}

/// Returns (config, text with LF line ends, meta). A missing file is an empty mapping.
fn load() -> Result<(Y, String, TextMeta)> {
    let p = config_path();
    if !p.exists() {
        return Ok((Y::Mapping(Mapping::new()), String::new(), TextMeta::NEW));
    }
    let (text, meta) = read_text(&p)?;
    let v: Y = serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("config.yaml 解析失败：{e}", "Failed to parse config.yaml: {e}")))?;
    let v = match v {
        Y::Null => Y::Mapping(Mapping::new()),
        Y::Mapping(_) => v,
        _ => return Err(anyhow!(l("config.yaml 顶层不是映射", "The top level of config.yaml is not a mapping"))),
    };
    Ok((v, text, meta))
}

// ---------------------------------------------------------------- YAML: block rewrite

/// A top-level `key:` block: lines [start, end). Trailing blank lines and column-0
/// comments belong to the gap before the next block, not to this one.
#[derive(Debug)]
struct Block {
    key: String,
    start: usize,
    end: usize,
}

fn top_key(line: &str) -> Option<String> {
    let c = line.chars().next()?;
    if matches!(c, ' ' | '\t' | '#' | '-' | '%' | '?' | '[' | '{') || line.starts_with("...") {
        return None;
    }
    if c == '"' || c == '\'' {
        let end = line[1..].find(c)? + 1;
        let rest = &line[end + 1..];
        return rest.trim_start().starts_with(':').then(|| line[1..end].to_string());
    }
    let b = line.as_bytes();
    (0..b.len()).find(|&i| b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' ' || b[i + 1] == b'\t')).map(|i| line[..i].trim_end().to_string())
}

fn blocks(lines: &[&str]) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    let close = |out: &mut Vec<Block>, at: usize| {
        if let Some(b) = out.last_mut() {
            let mut end = at;
            while end > b.start + 1 && (lines[end - 1].trim().is_empty() || lines[end - 1].starts_with('#')) {
                end -= 1;
            }
            b.end = end;
        }
    };
    for (i, l) in lines.iter().enumerate() {
        if let Some(k) = top_key(l) {
            close(&mut out, i);
            out.push(Block { key: k, start: i, end: lines.len() });
        }
    }
    close(&mut out, lines.len());
    out
}

/// Comments, anchors / aliases or tags inside a block (re-emitting it would lose them).
fn has_extras(lines: &[&str]) -> bool {
    let anchor = regex::Regex::new(r"(^\s*|:\s+|-\s+)[&*!][^\s]").unwrap();
    lines.iter().any(|l| {
        if l.trim_start().starts_with('#') || anchor.is_match(l) {
            return true;
        }
        let (mut sq, mut dq, mut prev_ws) = (false, false, true);
        for c in l.chars() {
            match c {
                '\'' if !dq => sq = !sq,
                '"' if !sq => dq = !dq,
                '#' if !sq && !dq && prev_ws => return true,
                _ => {}
            }
            prev_ws = c == ' ' || c == '\t';
        }
        false
    })
}

/// Replaces (Some) or removes (None) top-level blocks, leaving every other line as it was.
fn rewrite(text: &str, changes: &[(&str, Option<&Y>)]) -> Result<String> {
    let lines: Vec<&str> = if text.is_empty() { vec![] } else { text.split('\n').collect() };
    let all = blocks(&lines);
    let mut edits: Vec<(usize, usize, Vec<String>)> = vec![];
    let mut appended: Vec<String> = vec![];
    for (key, v) in changes {
        let found: Vec<&Block> = all.iter().filter(|b| b.key == *key).collect();
        if found.len() > 1 {
            return Err(anyhow!(tr!("config.yaml 里有重复的 {key}:，不写入", "config.yaml has more than one {key}: block; not writing it")));
        }
        let new_lines = match v {
            Some(v) => emit_top(key, v)?,
            None => vec![],
        };
        match found.first() {
            Some(b) => {
                if has_extras(&lines[b.start..b.end]) {
                    return Err(anyhow!(tr!(
                        "config.yaml 的 {key}: 段里有注释（或锚点），为避免丢失不写入；请先手动删掉这些注释",
                        "The {key}: block in config.yaml has comments (or anchors); not writing it so they aren't lost. Remove them by hand first"
                    )));
                }
                edits.push((b.start, b.end, new_lines));
            }
            None => appended.extend(new_lines),
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    for (s, e, new) in edits {
        out.splice(s..e, new);
    }
    if !appended.is_empty() {
        while out.last().map(|l| l.is_empty()).unwrap_or(false) {
            out.pop();
        }
        out.extend(appended);
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

// ---------------------------------------------------------------- YAML: emitter (PyYAML style)

fn needs_quotes(s: &str) -> bool {
    if s.is_empty() || s != s.trim() {
        return true;
    }
    // Indicator characters (conservative: PyYAML would leave "-x" plain, quoting is still correct).
    if "-?:,[]{}#&*!|>'\"%@`=<~".contains(s.chars().next().unwrap()) {
        return true;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') || s.contains('\t') {
        return true;
    }
    // YAML 1.1 (PyYAML) implicit types: booleans, null, numbers, timestamps.
    const WORDS: [&str; 10] = ["yes", "no", "on", "off", "true", "false", "null", "~", ".inf", ".nan"];
    if WORDS.contains(&s.to_lowercase().as_str()) {
        return true;
    }
    let num = regex::Regex::new(r"^[-+]?(\.?[0-9][0-9_]*(\.[0-9_]*)?([eE][-+]?[0-9]+)?|0[xXoObB][0-9a-fA-F_]+|[0-9][0-9_]*(:[0-5]?[0-9])+(\.[0-9_]*)?|\.(inf|Inf|INF|nan|NaN|NAN))$").unwrap();
    let date = regex::Regex::new(r"^[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}").unwrap();
    if num.is_match(s) || date.is_match(s) {
        return true;
    }
    // Whatever YAML 1.2 would read as something other than this very string.
    !matches!(serde_yaml::from_str::<Y>(s), Ok(Y::String(ref x)) if x == s)
}

fn quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        // A JSON string is a valid YAML double-quoted scalar.
        return serde_json::to_string(s).unwrap();
    }
    if needs_quotes(s) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

fn scalar(v: &Y) -> Result<String> {
    Ok(match v {
        Y::Null => "null".into(),
        Y::Bool(b) => b.to_string(),
        Y::Number(n) => n.to_string(),
        Y::String(s) => quote(s),
        Y::Mapping(m) if m.is_empty() => "{}".into(),
        Y::Sequence(s) if s.is_empty() => "[]".into(),
        _ => return Err(anyhow!(l("无法写出这个 YAML 值", "Can't write this YAML value"))),
    })
}

fn emit_entry(out: &mut Vec<String>, indent: usize, key: &Y, v: &Y) -> Result<()> {
    let pad = " ".repeat(indent);
    let k = match key {
        Y::String(s) => quote(s),
        other => scalar(other)?,
    };
    match v {
        Y::Mapping(m) if !m.is_empty() => {
            out.push(format!("{pad}{k}:"));
            emit_map(out, indent + 2, m)?;
        }
        Y::Sequence(s) if !s.is_empty() => {
            out.push(format!("{pad}{k}:"));
            emit_seq(out, indent + 2, s)?;
        }
        Y::Tagged(_) => return Err(anyhow!(tr!("{k} 带 YAML 标签，不写入", "{k} has a YAML tag; not writing it"))),
        _ => out.push(format!("{pad}{k}: {}", scalar(v)?)),
    }
    Ok(())
}

fn emit_map(out: &mut Vec<String>, indent: usize, m: &Mapping) -> Result<()> {
    for (k, v) in m {
        emit_entry(out, indent, k, v)?;
    }
    Ok(())
}

fn emit_seq(out: &mut Vec<String>, indent: usize, s: &[Y]) -> Result<()> {
    let pad = " ".repeat(indent);
    for item in s {
        let mut sub = vec![];
        match item {
            Y::Mapping(m) if !m.is_empty() => emit_map(&mut sub, indent + 2, m)?,
            Y::Sequence(ss) if !ss.is_empty() => emit_seq(&mut sub, indent + 2, ss)?,
            Y::Tagged(_) => return Err(anyhow!(l("列表项带 YAML 标签，不写入", "A list item has a YAML tag; not writing it"))),
            _ => {
                out.push(format!("{pad}- {}", scalar(item)?));
                continue;
            }
        }
        sub[0] = format!("{pad}- {}", &sub[0][indent + 2..]);
        out.extend(sub);
    }
    Ok(())
}

/// `key:` block lines; checked by parsing them back.
fn emit_top(key: &str, v: &Y) -> Result<Vec<String>> {
    let mut out = vec![];
    emit_entry(&mut out, 0, &yk(key), v)?;
    let back: Y = serde_yaml::from_str(&out.join("\n")).map_err(|e| anyhow!(tr!("生成的 YAML 无法解析：{e}", "The generated YAML can't be parsed: {e}")))?;
    if back.get(key) != Some(v) {
        return Err(anyhow!(tr!("生成的 {key}: 段校验失败，不写入", "The generated {key}: block failed verification; not writing it")));
    }
    Ok(out)
}

// ---------------------------------------------------------------- .env

fn env_key(env: &str, var: &str) -> Option<String> {
    dotenv::get(env, var).or_else(|| crate::env::agent_var(var).filter(|v| !v.trim().is_empty()))
}

// ---------------------------------------------------------------- providers

#[derive(Clone, Debug, PartialEq)]
enum Src {
    /// `providers.<key>`
    Dict(String),
    /// `custom_providers[i]`
    List(usize),
    Inline,
    Builtin(String),
}

/// Named entries: (id, source). Dict keys first (Hermes resolves them first).
fn entries(cfg: &Y) -> Vec<(String, Src)> {
    let mut out: Vec<(String, Src)> = vec![];
    if let Some(m) = cfg.get("providers").and_then(|p| p.as_mapping()) {
        for (k, v) in m {
            if let (Some(k), true) = (key_str(k), v.is_mapping()) {
                out.push((k.clone(), Src::Dict(k)));
            }
        }
    }
    if let Some(s) = cfg.get("custom_providers").and_then(|p| p.as_sequence()) {
        for (i, e) in s.iter().enumerate() {
            let Some(name) = ystr(e, "name") else { continue };
            let id = if out.iter().any(|(x, _)| x == &name) { format!("{name}@{i}") } else { name };
            out.push((id, Src::List(i)));
        }
    }
    out
}

fn def_of<'a>(cfg: &'a Y, src: &Src) -> Option<&'a Y> {
    match src {
        Src::Dict(k) => cfg.get("providers")?.get(k.as_str()),
        Src::List(i) => cfg.get("custom_providers")?.as_sequence()?.get(*i),
        _ => None,
    }
}

fn def_mut<'a>(cfg: &'a mut Y, src: &Src) -> Option<&'a mut Y> {
    match src {
        Src::Dict(k) => cfg.get_mut("providers")?.get_mut(k.as_str()),
        Src::List(i) => cfg.get_mut("custom_providers")?.as_sequence_mut()?.get_mut(*i),
        _ => None,
    }
}

fn norm_name(s: &str) -> String {
    s.trim().to_lowercase().replace(' ', "-")
}

/// Every identity Hermes accepts for an entry (see hermes_cli/providers.py custom_provider_aliases).
fn aliases(name: &str, key: &str) -> Vec<String> {
    let mut out = vec![];
    for v in [name, key] {
        let raw = v.trim().to_lowercase();
        if raw.is_empty() {
            continue;
        }
        let n = raw.replace(' ', "-");
        out.push(raw);
        if let Some(suffix) = n.strip_prefix("custom:") {
            out.push(suffix.to_string());
        } else {
            out.push(format!("custom:{n}"));
        }
        out.push(n);
    }
    out
}

/// The `model.provider` value that selects an entry.
fn slug_of(cfg: &Y, src: &Src) -> String {
    let ident = match src {
        Src::Dict(k) => k.clone(),
        Src::List(_) => def_of(cfg, src).and_then(|d| ystr(d, "name")).unwrap_or_default(),
        Src::Inline => return "custom".into(),
        Src::Builtin(b) => return b.clone(),
    };
    let n = norm_name(&ident);
    if n.starts_with("custom:") { n } else { format!("custom:{n}") }
}

fn entry_aliases(cfg: &Y, src: &Src) -> Vec<String> {
    let d = def_of(cfg, src);
    let name = d.and_then(|d| ystr(d, "name")).unwrap_or_default();
    match src {
        Src::Dict(k) => aliases(if name.is_empty() { k } else { &name }, k),
        _ => aliases(&name, ""),
    }
}

fn model_provider(cfg: &Y) -> String {
    match cfg.get("model") {
        Some(Y::Mapping(m)) => m.get("provider").and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "auto".into()),
        _ => "auto".into(),
    }
}

fn model_default(cfg: &Y) -> Option<String> {
    match cfg.get("model") {
        Some(Y::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(m @ Y::Mapping(_)) => ystr(m, "default").or_else(|| ystr(m, "model")),
        _ => None,
    }
}

/// Which provider `model.provider` selects: (id, source).
fn current(cfg: &Y) -> (String, Src) {
    let p = model_provider(cfg);
    let n = norm_name(&p);
    let ents = entries(cfg);
    if let Some((id, src)) = ents.iter().find(|(_, s)| entry_aliases(cfg, s).contains(&n)) {
        // A named entry literally called "custom" wins over the inline trust path, like in Hermes.
        return (id.clone(), src.clone());
    }
    if n == "custom" {
        return (INLINE.into(), Src::Inline);
    }
    (format!("{BUILTIN}{p}"), Src::Builtin(p))
}

fn find(cfg: &Y, id: &str) -> Option<Src> {
    if id == INLINE {
        if let Some((_, s)) = entries(cfg).into_iter().find(|(x, _)| x == INLINE) {
            return Some(s);
        }
        return Some(Src::Inline);
    }
    if let Some(b) = id.strip_prefix(BUILTIN) {
        return Some(Src::Builtin(b.to_string()));
    }
    entries(cfg).into_iter().find(|(x, _)| x == id).map(|(_, s)| s)
}

fn url_key(def: &Y) -> &'static str {
    ["base_url", "url", "api"].into_iter().find(|k| ystr(def, k).is_some()).unwrap_or("base_url")
}

fn mode_key(def: &Y) -> &'static str {
    if ystr(def, "api_mode").is_none() && ystr(def, "transport").is_some() { "transport" } else { "api_mode" }
}

fn mode_of(def: &Y) -> Option<String> {
    ystr(def, "api_mode").or_else(|| ystr(def, "transport"))
}

/// Hermes api_mode → AgentPlus api (None = not something AgentPlus speaks).
fn api_of(mode: Option<&str>) -> Option<&'static str> {
    match mode.unwrap_or("chat_completions") {
        "chat_completions" => Some("chat"),
        "codex_responses" => Some("responses"),
        "anthropic_messages" => Some("anthropic"),
        _ => None,
    }
}

fn mode_for(api: &str) -> Result<&'static str> {
    match api {
        "chat" => Ok("chat_completions"),
        "responses" => Ok("codex_responses"),
        "anthropic" => Ok("anthropic_messages"),
        other => Err(anyhow!(tr!("Hermes 不支持 {other} 协议（可选 Chat / Responses / Anthropic）", "Hermes doesn't support the {other} protocol (choose Chat / Responses / Anthropic)"))),
    }
}

fn key_env_of(def: &Y) -> Option<String> {
    ystr(def, "key_env").or_else(|| ystr(def, "api_key_env")).or_else(|| ystr(def, "keyEnv")).or_else(|| ystr(def, "apiKeyEnv"))
}

/// The entry's key: `key_env` (from .env / the environment) first, then inline `api_key`.
fn key_of(def: &Y, env: &str) -> Option<String> {
    key_env_of(def).and_then(|v| env_key(env, &v)).or_else(|| ystr(def, "api_key")).or_else(|| ystr(def, "apiKey"))
}

/// Where an entry keeps its default model: the key already used, else `model` for legacy
/// list entries (what `hermes model` writes) and `default_model` for keyed ones.
fn default_key(def: &Y, legacy: bool) -> &'static str {
    if ystr(def, "default_model").is_some() {
        "default_model"
    } else if ystr(def, "model").is_some() || legacy {
        "model"
    } else {
        "default_model"
    }
}

fn default_of(def: &Y) -> Option<String> {
    ystr(def, "default_model").or_else(|| ystr(def, "model"))
}

// ------------------------------------------------ models of an entry

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Map,
    List,
}

/// `models` as ordered (id, definition) rows; a list of plain ids has Null definitions.
fn model_rows(def: &Y) -> (Shape, Vec<(String, Y)>) {
    match def.get("models") {
        Some(Y::Sequence(s)) => (
            Shape::List,
            s.iter()
                .filter_map(|it| match it {
                    Y::String(x) if !x.trim().is_empty() => Some((x.trim().to_string(), Y::Null)),
                    Y::Mapping(_) => ystr(it, "id").or_else(|| ystr(it, "name")).map(|id| (id, it.clone())),
                    _ => None,
                })
                .collect(),
        ),
        Some(Y::Mapping(m)) => (Shape::Map, m.iter().filter_map(|(k, v)| Some((key_str(k)?, v.clone()))).collect()),
        _ => (Shape::Map, vec![]),
    }
}

fn set_model_rows(def: &mut Y, shape: Shape, rows: &[(String, Y)]) {
    let v = match shape {
        Shape::List if rows.iter().all(|(_, d)| d.is_null() || d.is_mapping()) => Y::Sequence(
            rows.iter()
                .map(|(id, d)| match d {
                    Y::Mapping(_) => d.clone(),
                    _ => yk(id),
                })
                .collect(),
        ),
        _ => {
            let mut m = Mapping::new();
            for (id, d) in rows {
                m.insert(yk(id), d.clone());
            }
            Y::Mapping(m)
        }
    };
    if let Some(m) = def.as_mapping_mut() {
        m.insert(yk("models"), v);
    }
}

/// Turns a list-shaped `models` into the dict shape (needed to store a name / context).
fn to_map_rows(rows: Vec<(String, Y)>) -> Vec<(String, Y)> {
    rows.into_iter()
        .map(|(id, d)| {
            let mut m = Mapping::new();
            if let Y::Mapping(src) = &d {
                let had_id = src.get("id").is_some();
                for (k, v) in src {
                    let ks = key_str(k).unwrap_or_default();
                    if ks == "id" || (ks == "name" && !had_id) {
                        continue;
                    }
                    m.insert(k.clone(), v.clone());
                }
            }
            (id, Y::Mapping(m))
        })
        .collect()
}

fn to_model(id: &str, d: &Y, shape: Shape, visible: bool) -> Model {
    let context = d.get("context_length").and_then(|x| x.as_u64());
    Model {
        id: id.into(),
        visible,
        ctx: context.map(fmt_ctx),
        context,
        name: if shape == Shape::Map { ystr(d, "name").filter(|n| n != id) } else { None },
        deletable: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------- state

fn entry_provider(cfg: &Y, id: &str, src: &Src, hidden: &JMap<String, J>, env: &str, cur: Option<&str>) -> Provider {
    let def = def_of(cfg, src).cloned().unwrap_or(Y::Null);
    let base = ystr(&def, url_key(&def));
    let mode = mode_of(&def);
    let api = api_of(mode.as_deref());
    let (shape, rows) = model_rows(&def);
    let dflt = default_of(&def);
    let mut models: Vec<Model> = rows.iter().map(|(mid, d)| to_model(mid, d, shape, true)).collect();
    let prefix = format!("{id}|");
    for (k, d) in hidden {
        if let Some(mid) = k.strip_prefix(&prefix) {
            if !models.iter().any(|m| m.id == mid) {
                models.push(to_model(mid, &j2y(d), shape, false));
            }
        }
    }
    for extra in [dflt.clone(), cur.map(String::from)].into_iter().flatten() {
        if !models.iter().any(|m| m.id == extra) {
            models.insert(0, Model { id: extra, visible: true, readonly: true, deletable: false, ..Default::default() });
        }
    }
    for m in models.iter_mut() {
        if dflt.as_deref() == Some(m.id.as_str()) {
            m.tags.push(Tag::default_model());
        }
        if cur == Some(m.id.as_str()) {
            m.tags.push(Tag::current());
        }
    }
    let key_env = key_env_of(&def);
    let key = key_of(&def, env);
    let key_note = match (&key_env, &key, ystr(&def, "api_key").is_some()) {
        (Some(v), Some(k), _) if env_key(env, v).is_some() => tr!("{v}（.env）· {}", "{v} (.env) · {}", mask_key(k)),
        (_, Some(k), true) => tr!("api_key · 明文保存在 config.yaml · {}", "api_key · stored in plain text in config.yaml · {}", mask_key(k)),
        (Some(v), _, _) => tr!("{v}（.env 里没有设置）", "{v} (not set in .env)"),
        _ => l("未填写", "Not set").into(),
    };
    let place = match src {
        Src::Dict(k) => format!("providers.{k}"),
        Src::List(i) => format!("custom_providers[{i}]"),
        _ => String::new(),
    };
    let enabled = def.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
    let mut details = vec![
        Kv::mono(l("配置位置", "Config location"), place),
        Kv::mono(l("地址", "Base URL"), base.clone().unwrap_or_else(|| "-".into())),
        Kv::mono("api_mode", mode.clone().unwrap_or_else(|| l("（自动，按地址判断）", "(auto, based on the URL)").into())),
        Kv::text(l("密钥", "API key"), key_note),
    ];
    if let Some(d) = &dflt {
        details.push(Kv::mono(l("默认模型", "Default model"), d.clone()));
    }
    if !enabled {
        details.push(Kv::text(
            l("状态", "Status"),
            l("enabled: false（Hermes 忽略它；设为当前时会重新启用）", "enabled: false (Hermes ignores it; making it current re-enables it)"),
        ));
    }
    let reason = if base.is_none() {
        Some(l("没有 base_url，Hermes 会忽略这一项", "No base_url; Hermes ignores this entry").to_string())
    } else if api.is_none() {
        Some(tr!("api_mode = {}，AgentPlus 只能查看", "api_mode = {}; AgentPlus can only view it", mode.clone().unwrap_or_default()))
    } else {
        None
    };
    Provider {
        id: id.into(),
        name: ystr(&def, "name").unwrap_or_else(|| id.to_string()),
        host: base.as_deref().map(host_of).unwrap_or_default(),
        base_url: base,
        apis: vec![api.map(api_label).unwrap_or(l("其他", "Other")).into()],
        builtin: false,
        enabled,
        compatible: reason.is_none(),
        reason,
        models,
        details,
        editable: true,
        api: api.unwrap_or("chat").into(),
        has_key: key.is_some(),
        key_fp: None,
        key_hint: None,
        official_auth: false,
    }
}

/// The inline (bare `custom`) provider: from `model` when active, else from the stash.
#[allow(clippy::type_complexity)]
fn inline_values(cfg: &Y, root: &J, active: bool) -> Option<(String, Option<String>, Option<String>, Option<String>)> {
    if active {
        let m = cfg.get("model")?;
        return Some((ystr(m, "base_url").unwrap_or_default(), ystr(m, "api_key"), ystr(m, "api_mode"), model_default(cfg)));
    }
    let s = store::agent_get(root, ID, "inline")?;
    let base = str_field(s, "baseUrl");
    if base.is_empty() {
        return None;
    }
    let opt = |k: &str| Some(str_field(s, k)).filter(|x| !x.is_empty());
    Some((base, opt("apiKey"), opt("apiMode"), opt("default")))
}

fn inline_provider(vals: &(String, Option<String>, Option<String>, Option<String>), same_as: Option<String>) -> Provider {
    let (base, key, mode, dflt) = vals;
    let api = api_of(mode.as_deref());
    let mut details = vec![
        Kv::mono(
            l("配置位置", "Config location"),
            l("model.provider: custom（model.base_url / model.api_key）", "model.provider: custom (model.base_url / model.api_key)"),
        ),
        Kv::mono(l("地址", "Base URL"), base.clone()),
        Kv::mono("api_mode", mode.clone().unwrap_or_else(|| l("（自动，按地址判断）", "(auto, based on the URL)").into())),
        Kv::text(l("密钥", "API key"), key.as_deref().map(|k| format!("model.api_key · {}", mask_key(k))).unwrap_or_else(|| l("未填写", "Not set").into())),
    ];
    if let Some(n) = same_as {
        details.push(Kv::text(l("说明", "Note"), tr!("地址和「{n}」相同；密钥写在 model 里", "Same base URL as \"{n}\"; the API key is in model")));
    }
    Provider {
        id: INLINE.into(),
        name: l("直连（model 里的 custom）", "Direct (custom in model)").into(),
        host: host_of(base),
        base_url: Some(base.clone()),
        apis: vec![api.map(api_label).unwrap_or(l("其他", "Other")).into()],
        builtin: false,
        enabled: true,
        compatible: api.is_some(),
        reason: api.is_none().then(|| tr!("api_mode = {}，AgentPlus 只能查看", "api_mode = {}; AgentPlus can only view it", mode.clone().unwrap_or_default())),
        models: dflt.iter().map(|m| Model { id: m.clone(), visible: true, readonly: true, tags: vec![Tag::default_model()], ..Default::default() }).collect(),
        details,
        editable: true,
        api: api.unwrap_or("chat").into(),
        has_key: key.is_some(),
        key_fp: None,
        key_hint: None,
        official_auth: false,
    }
}

fn builtin_provider(name: &str, dflt: Option<String>, known: bool) -> Provider {
    Provider {
        id: format!("{BUILTIN}{name}"),
        name: tr!("内置 · {name}", "Built-in · {name}"),
        base_url: None,
        host: l("Hermes 内置供应商", "Hermes built-in provider").into(),
        apis: vec![l("内置", "Built-in").into()],
        builtin: true,
        enabled: true,
        compatible: known,
        reason: (!known).then(|| l("config.yaml 里找不到这个自定义供应商", "This custom provider isn't in config.yaml").to_string()),
        models: dflt.iter().map(|m| Model { id: m.clone(), visible: true, readonly: true, tags: vec![Tag::default_model()], ..Default::default() }).collect(),
        details: vec![
            Kv::mono("model.provider", name.to_string()),
            Kv::text(
                l("说明", "Note"),
                l(
                    "Hermes 内置的供应商（凭据在 .env / auth.json，用 hermes model / hermes auth 管理）",
                    "A provider built into Hermes (credentials live in .env / auth.json; manage them with hermes model / hermes auth)",
                ),
            ),
        ],
        editable: false,
        api: "chat".into(),
        has_key: true,
        key_fp: None,
        key_hint: None,
        official_auth: false,
    }
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = AgentState {
        id: ID.into(),
        name: NAME.into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "single".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![display_path(&config_path()), display_path(&env_path())],
        current_provider: None,
        providers: vec![],
        catalog: None,
        catalog_file: None,
        settings: vec![],
        current: vec![],
        notes: vec![l("Hermes 的改动对新会话生效；gateway（消息平台）需要重启。", "Hermes changes apply to new sessions; the gateway (messaging platforms) needs a restart.").into()],
        readonly: false,
        fixed_pending: false,
        fixed_prompt: false,
        restartable: false,
        model_fields: vec![],
    };
    let (cfg, text, _) = match load() {
        Ok(x) => x,
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    let root = store::load();
    let (env, _) = dotenv::load(&env_path());
    let hidden = store::get_obj(&root, ID, "hiddenModels");
    let (cur_id, cur_src) = current(&cfg);
    let cur_model = model_default(&cfg);
    st.current_provider = Some(cur_id.clone());

    let ents = entries(&cfg);
    for (id, src) in &ents {
        let cm = (id == &cur_id).then(|| cur_model.clone()).flatten();
        st.providers.push(entry_provider(&cfg, id, src, &hidden, &env, cm.as_deref()));
    }
    let inline_active = cur_src == Src::Inline;
    if let Some(vals) = inline_values(&cfg, &root, inline_active) {
        let same = ents
            .iter()
            .filter_map(|(_, s)| def_of(&cfg, s))
            .find(|d| ystr(d, url_key(d)).map(|u| u.trim_end_matches('/').eq_ignore_ascii_case(vals.0.trim_end_matches('/'))).unwrap_or(false))
            .and_then(|d| ystr(d, "name"));
        st.providers.push(inline_provider(&vals, same));
    }
    let builtins = store::get_obj(&root, ID, "builtins");
    if let Src::Builtin(b) = &cur_src {
        let known = !b.to_lowercase().starts_with("custom:");
        st.providers.push(builtin_provider(b, cur_model.clone(), known));
    }
    for (b, v) in &builtins {
        if !matches!(&cur_src, Src::Builtin(x) if x == b) {
            st.providers.push(builtin_provider(b, Some(str_field(v, "default")).filter(|x| !x.is_empty()), true));
        }
    }

    // Comments inside the blocks AgentPlus rewrites make it read-only (elsewhere they are kept).
    let lines: Vec<&str> = text.split('\n').collect();
    let commented: Vec<String> = blocks(&lines).iter().filter(|b| BLOCKS.contains(&b.key.as_str()) && has_extras(&lines[b.start..b.end])).map(|b| format!("{}:", b.key)).collect();
    if !commented.is_empty() {
        st.readonly = true;
        st.notes.push(tr!(
            "config.yaml 的 {} 段里有注释或锚点，写回会丢失，已切换为只读。",
            "The {} block(s) in config.yaml have comments or anchors, which would be lost on write, so it's read-only.",
            commented.join(l("、", ", "))
        ));
    }
    if std::fs::read_to_string(auth_path()).map(|t| t.contains("\"custom:")).unwrap_or(false) {
        st.notes.push(
            l(
                "改了自定义供应商的密钥后，如果 Hermes 仍报认证失败，运行 hermes doctor 刷新 auth.json 里的凭据池。",
                "If Hermes still reports an auth failure after you change a custom provider's API key, run hermes doctor to refresh the credential pool in auth.json.",
            )
            .into(),
        );
    }

    let cur_name = st.providers.iter().find(|p| p.id == cur_id).map(|p| p.name.clone()).unwrap_or_default();
    let m = cfg.get("model");
    st.current = vec![
        Kv::text(l("供应商", "Provider"), cur_name),
        Kv::mono("model.provider", model_provider(&cfg)),
        Kv::mono(l("模型", "Model"), cur_model.unwrap_or_else(|| "-".into())),
    ];
    let base = match &cur_src {
        Src::Inline => m.and_then(|m| ystr(m, "base_url")),
        Src::Dict(_) | Src::List(_) => def_of(&cfg, &cur_src).and_then(|d| ystr(d, url_key(d))),
        Src::Builtin(_) => None,
    };
    if let Some(b) = base {
        st.current.push(Kv::mono(l("地址", "Base URL"), b));
    }
    if let Some(mode) = m.and_then(|m| ystr(m, "api_mode")) {
        st.current.push(Kv::mono("api_mode", mode));
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _, _) = load()?;
    let (env, _) = dotenv::load(&env_path());
    let src = find(&cfg, id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
    match &src {
        Src::Builtin(_) => Err(anyhow!(l("Hermes 内置供应商没有可用的地址", "Hermes built-in providers have no usable base URL"))),
        Src::Inline => {
            let active = current(&cfg).1 == Src::Inline;
            let (base, key, mode, _) = inline_values(&cfg, &store::load(), active).ok_or_else(|| anyhow!(l("找不到直连配置", "Direct config not found")))?;
            let api = api_of(mode.as_deref()).ok_or_else(|| anyhow!(l("这个供应商的 api_mode AgentPlus 不支持", "AgentPlus doesn't support this provider's api_mode")))?;
            Ok((base, key, api.into()))
        }
        _ => {
            let def = def_of(&cfg, &src).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
            let base = ystr(def, url_key(def)).ok_or_else(|| anyhow!(tr!("供应商 {id} 没有 base_url", "Provider {id} has no base_url")))?;
            let api = api_of(mode_of(def).as_deref()).ok_or_else(|| anyhow!(l("这个供应商的 api_mode AgentPlus 不支持", "AgentPlus doesn't support this provider's api_mode")))?;
            Ok((base, key_of(def, &env), api.into()))
        }
    }
}

// ---------------------------------------------------------------- plan

fn model_map(cfg: &mut Y) -> &mut Mapping {
    let root = cfg.as_mapping_mut().expect("config is a mapping");
    let m = root.entry(yk("model")).or_insert(Y::Mapping(Mapping::new()));
    if let Y::String(s) = m {
        let mut n = Mapping::new();
        n.insert(yk("default"), yk(s));
        *m = Y::Mapping(n);
    } else if !m.is_mapping() {
        *m = Y::Mapping(Mapping::new());
    }
    m.as_mapping_mut().unwrap()
}

fn set_str(m: &mut Mapping, k: &str, v: Option<&str>) -> bool {
    let old = m.get(k).and_then(|x| x.as_str()).map(String::from);
    if old.as_deref() == v {
        return false;
    }
    match v {
        Some(v) => {
            m.insert(yk(k), yk(v));
        }
        None => {
            if m.shift_remove(k).is_none() {
                return false;
            }
        }
    }
    true
}

struct Ctx<'a> {
    diff: &'a mut Diff,
    file: String,
    envfile: String,
    store_label: &'static str,
}

fn plan_err_readonly(src: &Src) -> Result<()> {
    match src {
        Src::Builtin(_) => Err(anyhow!(l("Hermes 内置供应商不能在这里编辑，用 hermes model 管理", "Hermes built-in providers can't be edited here; manage them with hermes model"))),
        _ => Ok(()),
    }
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let (cfg0, text, meta) = load()?;
    let mut cfg = cfg0.clone();
    let mut root = store::load();
    // Unlike the read-only views, a write refuses an .env it can't read (UTF-16, GBK…).
    let (env0, env_meta) = read_text_or_new(&env_path())?;
    let mut env = env0.clone();
    let mut diff = Diff::default();
    let mut store_dirty = false;
    let cx = Ctx { diff: &mut diff, file: display_path(&config_path()), envfile: display_path(&env_path()), store_label: l("AgentPlus · Hermes 暂存", "AgentPlus · Hermes stash") };

    for op in ops {
        match op {
            Op::UpsertProvider { provider: p } => {
                let mode = mode_for(&p.api)?;
                let base = p.base_url.trim();
                let name = p.name.trim();
                if base.is_empty() {
                    return Err(anyhow!(l("地址不能为空", "Base URL is required")));
                }
                let key = p.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty());
                match p.id.as_deref() {
                    None => {
                        if name.is_empty() {
                            return Err(anyhow!(l("名称不能为空", "Name is required")));
                        }
                        let taken: Vec<String> = entries(&cfg).iter().flat_map(|(id, s)| {
                            let mut a = entry_aliases(&cfg, s);
                            a.push(id.to_lowercase());
                            a
                        }).collect();
                        let id = unique_id(&slug(name), |c| c == INLINE || taken.iter().any(|t| t == c) || taken.contains(&format!("custom:{c}")));
                        let mut e = Mapping::new();
                        e.insert(yk("name"), yk(name));
                        e.insert(yk("base_url"), yk(base));
                        if let Some(k) = key {
                            e.insert(yk("api_key"), yk(k));
                        }
                        e.insert(yk("api_mode"), yk(mode));
                        let ids = clean_ids(&p.models);
                        if let Some(first) = ids.first() {
                            e.insert(yk("default_model"), yk(first));
                        }
                        let mut ms = Mapping::new();
                        for m in &ids {
                            ms.insert(yk(m), Y::Mapping(Mapping::new()));
                        }
                        e.insert(yk("models"), Y::Mapping(ms));
                        let rootm = cfg.as_mapping_mut().unwrap();
                        let provs = rootm.entry(yk("providers")).or_insert(Y::Mapping(Mapping::new()));
                        if !provs.is_mapping() {
                            return Err(anyhow!(l("config.yaml 的 providers 不是映射", "providers in config.yaml is not a mapping")));
                        }
                        provs.as_mapping_mut().unwrap().insert(yk(&id), Y::Mapping(e));
                        let key_part = key.map(|k| tr!(" · 密钥 {}", " · API key {}", mask_key(k))).unwrap_or_default();
                        cx.diff.push(
                            &cx.file,
                            tr!("+ providers.{id}（{base} · {} · {} 个模型{}）", "+ providers.{id} ({base} · {} · {} model(s){})", api_label(&p.api), ids.len(), key_part),
                            true,
                        );
                    }
                    Some(id) => {
                        let src = find(&cfg, id).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
                        plan_err_readonly(&src)?;
                        if src == Src::Inline {
                            let active = current(&cfg).1 == Src::Inline;
                            if active {
                                let m = model_map(&mut cfg);
                                if set_str(m, "base_url", Some(base)) {
                                    cx.diff.push(&cx.file, format!("model.base_url = {base}"), true);
                                }
                                let want_mode = if mode == "chat_completions" && m.get("api_mode").is_none() { None } else { Some(mode) };
                                if set_str(m, "api_mode", want_mode) {
                                    cx.diff.push(&cx.file, format!("model.api_mode = {mode}"), true);
                                }
                                if let Some(k) = key {
                                    if set_str(m, "api_key", Some(k)) {
                                        cx.diff.push(&cx.file, format!("model.api_key = {}", mask_key(k)), true);
                                    }
                                }
                            } else {
                                let s = store::section(&mut root, ID, "inline");
                                if s.get("baseUrl").is_none() {
                                    return Err(anyhow!(l("找不到直连配置", "Direct config not found")));
                                }
                                s.insert("baseUrl".into(), json!(base));
                                s.insert("apiMode".into(), json!(mode));
                                if let Some(k) = key {
                                    s.insert("apiKey".into(), json!(k));
                                }
                                let key_part = key.map(|k| tr!(" · 密钥 {}", " · API key {}", mask_key(k))).unwrap_or_default();
                                cx.diff.push(cx.store_label, tr!("直连配置：{base} · {}{}", "Direct config: {base} · {}{}", api_label(&p.api), key_part), true);
                                store_dirty = true;
                            }
                            continue;
                        }
                        let place = match &src {
                            Src::Dict(k) => format!("providers.{k}"),
                            Src::List(i) => format!("custom_providers[{i}]"),
                            _ => unreachable!(),
                        };
                        let was_current = current(&cfg).0 == *id;
                        let def = def_mut(&mut cfg, &src).and_then(|d| d.as_mapping_mut()).ok_or_else(|| anyhow!(tr!("找不到供应商 {id}", "Provider not found: {id}")))?;
                        let dv = Y::Mapping(def.clone());
                        let (uk, mk) = (url_key(&dv), mode_key(&dv));
                        if !name.is_empty() && set_str(def, "name", Some(name)) {
                            cx.diff.push(&cx.file, format!("{place}.name = {name}"), true);
                        }
                        if set_str(def, uk, Some(base)) {
                            cx.diff.push(&cx.file, format!("{place}.{uk} = {base}"), true);
                        }
                        let want_mode = if mode == "chat_completions" && mode_of(&dv).is_none() { None } else { Some(mode) };
                        if want_mode.is_some() && set_str(def, mk, want_mode) {
                            cx.diff.push(&cx.file, format!("{place}.{mk} = {mode}"), true);
                        }
                        if let Some(k) = key {
                            match key_env_of(&dv) {
                                Some(var) => {
                                    if dotenv::get(&env, &var).as_deref() != Some(k) {
                                        env = dotenv::set(&env, &var, Some(k));
                                        cx.diff.push(&cx.envfile, format!("{var} = {}", mask_key(k)), true);
                                    }
                                }
                                None => {
                                    if set_str(def, "api_key", Some(k)) {
                                        cx.diff.push(&cx.file, format!("{place}.api_key = {}", mask_key(k)), true);
                                    }
                                }
                            }
                        }
                        // A list entry's identity is its name: keep model.provider and stashes pointing at it.
                        if let Src::List(_) = src {
                            let new_id = entries(&cfg).into_iter().find(|(_, s)| s == &src).map(|(x, _)| x).unwrap_or_default();
                            if new_id != *id {
                                if was_current {
                                    let slug = slug_of(&cfg, &src);
                                    model_map(&mut cfg).insert(yk("provider"), yk(&slug));
                                    cx.diff.push(&cx.file, format!("model.provider = {slug}"), true);
                                }
                                let h = store::section(&mut root, ID, "hiddenModels");
                                let old: Vec<String> = h.keys().filter(|k| k.starts_with(&format!("{id}|"))).cloned().collect();
                                for k in old {
                                    let v = h.remove(&k).unwrap();
                                    h.insert(format!("{new_id}|{}", &k[id.len() + 1..]), v);
                                    store_dirty = true;
                                }
                            }
                        }
                    }
                }
            }
            Op::DeleteProvider { provider } => {
                let src = find(&cfg, provider).ok_or_else(|| anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")))?;
                if current(&cfg).0 == *provider {
                    return Err(anyhow!(tr!("「{provider}」正在使用，先切换到其他供应商", "\"{provider}\" is in use; switch to another provider first")));
                }
                match &src {
                    Src::Dict(k) => {
                        cfg.get_mut("providers").and_then(|p| p.as_mapping_mut()).map(|m| m.shift_remove(k.as_str()));
                        cx.diff.push(&cx.file, format!("- providers.{k}"), false);
                    }
                    Src::List(i) => {
                        if let Some(s) = cfg.get_mut("custom_providers").and_then(|p| p.as_sequence_mut()) {
                            s.remove(*i);
                        }
                        cx.diff.push(&cx.file, tr!("- custom_providers[{i}]（{provider}）", "- custom_providers[{i}] ({provider})"), false);
                    }
                    Src::Inline => {
                        store::set_value(&mut root, ID, "inline", J::Null);
                        cx.diff.push(cx.store_label, l("- 直连配置", "- Direct config"), false);
                        store_dirty = true;
                    }
                    Src::Builtin(b) => {
                        if store::section(&mut root, ID, "builtins").remove(b).is_none() {
                            return Err(anyhow!(l("这一项不能删除", "This entry can't be deleted")));
                        }
                        cx.diff.push(cx.store_label, tr!("- 内置 · {b}（只是从列表移除）", "- Built-in · {b} (only removed from the list)"), false);
                        store_dirty = true;
                    }
                }
                let prefix = format!("{provider}|");
                let h = store::section(&mut root, ID, "hiddenModels");
                let n = h.len();
                h.retain(|k, _| !k.starts_with(&prefix));
                store_dirty |= h.len() != n;
            }
            Op::SetCurrentProvider { provider } => {
                let src = find(&cfg, provider).ok_or_else(|| anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")))?;
                let (cur_id, cur_src) = current(&cfg);
                if cur_id == *provider {
                    continue;
                }
                // Target values first (fail before touching anything).
                let (slug, dflt, mode, inline) = match &src {
                    Src::Dict(_) | Src::List(_) => {
                        let def = def_of(&cfg, &src).unwrap();
                        let (_, rows) = model_rows(def);
                        let dflt = default_of(def).or_else(|| rows.first().map(|r| r.0.clone())).ok_or_else(|| anyhow!(tr!("「{provider}」还没有模型，先添加一个", "\"{provider}\" has no models yet; add one first")))?;
                        (slug_of(&cfg, &src), Some(dflt), mode_of(def), None)
                    }
                    Src::Inline => {
                        let vals = inline_values(&cfg, &root, false).ok_or_else(|| anyhow!(l("找不到直连配置", "Direct config not found")))?;
                        ("custom".to_string(), vals.3.clone(), vals.2.clone(), Some(vals))
                    }
                    Src::Builtin(b) => {
                        let d = store::get_obj(&root, ID, "builtins").get(b).map(|v| str_field(v, "default")).filter(|x| !x.is_empty());
                        (b.clone(), d, None, None)
                    }
                };
                // Stash what is being switched away from, so it can come back.
                match &cur_src {
                    Src::Inline => {
                        let m = cfg.get("model").cloned().unwrap_or(Y::Null);
                        let s = store::section(&mut root, ID, "inline");
                        s.clear();
                        s.insert("baseUrl".into(), json!(ystr(&m, "base_url").unwrap_or_default()));
                        for (k, yk_) in [("apiKey", "api_key"), ("apiMode", "api_mode")] {
                            if let Some(v) = ystr(&m, yk_) {
                                s.insert(k.into(), json!(v));
                            }
                        }
                        if let Some(d) = model_default(&cfg) {
                            s.insert("default".into(), json!(d));
                        }
                        cx.diff.push(
                            cx.store_label,
                            l("暂存直连配置（model.base_url / api_key），可切换回来", "Stash the direct config (model.base_url / api_key) so you can switch back"),
                            true,
                        );
                        store_dirty = true;
                    }
                    Src::Builtin(b) if !b.to_lowercase().starts_with("custom:") => {
                        store::section(&mut root, ID, "builtins").insert(b.clone(), json!({ "default": model_default(&cfg).unwrap_or_default() }));
                        store_dirty = true;
                    }
                    _ => {}
                }
                if let Src::Dict(k) = &src {
                    if let Some(d) = def_mut(&mut cfg, &src).and_then(|d| d.as_mapping_mut()) {
                        if d.get("enabled").and_then(|x| x.as_bool()) == Some(false) {
                            d.shift_remove("enabled");
                            cx.diff.push(&cx.file, tr!("providers.{k}.enabled（删除，重新启用）", "providers.{k}.enabled (removed; re-enabled)"), true);
                        }
                    }
                }
                let m = model_map(&mut cfg);
                if set_str(m, "provider", Some(&slug)) {
                    cx.diff.push(&cx.file, format!("model.provider = {slug}"), true);
                }
                if let Some(d) = &dflt {
                    if set_str(m, "default", Some(d)) {
                        cx.diff.push(&cx.file, format!("model.default = {d}"), true);
                    }
                }
                match inline {
                    Some((base, key, _, _)) => {
                        if set_str(m, "base_url", Some(&base)) {
                            cx.diff.push(&cx.file, format!("model.base_url = {base}"), true);
                        }
                        if set_str(m, "api_key", key.as_deref()) {
                            cx.diff.push(&cx.file, format!("model.api_key = {}", key.as_deref().map(mask_key).unwrap_or_default()), true);
                        }
                    }
                    None => {
                        for k in ["base_url", "api_key"] {
                            if set_str(m, k, None) {
                                cx.diff.push(&cx.file, format!("- model.{k}"), false);
                            }
                        }
                    }
                }
                // Like `hermes model`: carry the entry's api_mode, or let Hermes detect it.
                match mode {
                    Some(md) => {
                        if set_str(m, "api_mode", Some(&md)) {
                            cx.diff.push(&cx.file, format!("model.api_mode = {md}"), true);
                        }
                    }
                    None => {
                        if set_str(m, "api_mode", None) {
                            cx.diff.push(&cx.file, "- model.api_mode", false);
                        }
                    }
                }
                if src == Src::Inline {
                    store::set_value(&mut root, ID, "inline", J::Null);
                    store_dirty = true;
                }
                if let Src::Builtin(b) = &src {
                    store::section(&mut root, ID, "builtins").remove(b);
                    store_dirty = true;
                }
            }
            Op::SetModelVisible { provider, model, visible } => {
                let src = entry_src(&cfg, provider)?;
                let key = format!("{provider}|{model}");
                let is_default = def_of(&cfg, &src).and_then(default_of).as_deref() == Some(model.as_str());
                let is_current = current(&cfg).0 == *provider && model_default(&cfg).as_deref() == Some(model.as_str());
                let def = def_mut(&mut cfg, &src).unwrap();
                let (shape, mut rows) = model_rows(def);
                let hidden = store::section(&mut root, ID, "hiddenModels");
                if *visible {
                    if !rows.iter().any(|r| &r.0 == model) {
                        let d = hidden.remove(&key).map(|j| j2y(&j)).unwrap_or(Y::Null);
                        let d = if shape == Shape::Map && d.is_null() { Y::Mapping(Mapping::new()) } else { d };
                        rows.push((model.clone(), d));
                        set_model_rows(def, shape, &rows);
                        cx.diff.push(&cx.file, format!("{provider}.models + {model}"), true);
                        store_dirty = true;
                    }
                } else if let Some(i) = rows.iter().position(|r| &r.0 == model) {
                    if is_default || is_current {
                        return Err(anyhow!(tr!(
                            "{model} 是默认 / 当前模型，不能隐藏；先换一个默认模型",
                            "{model} is the default / current model and can't be hidden; pick another default model first"
                        )));
                    }
                    let (_, d) = rows.remove(i);
                    hidden.insert(key, y2j(&d));
                    set_model_rows(def, shape, &rows);
                    cx.diff.push(&cx.file, tr!("{provider}.models - {model}（定义暂存在 AgentPlus）", "{provider}.models - {model} (definition kept in AgentPlus)"), false);
                    store_dirty = true;
                }
            }
            Op::UpsertModel { provider, model: mi } => {
                let src = entry_src(&cfg, provider)?;
                let mid = mi.id.trim().to_string();
                if mid.is_empty() {
                    return Err(anyhow!(l("模型 ID 不能为空", "Model ID is required")));
                }
                let key = format!("{provider}|{mid}");
                // A hidden model is edited in the stash.
                if let Some(h) = store::section(&mut root, ID, "hiddenModels").get_mut(&key) {
                    let mut d = j2y(h);
                    if !d.is_mapping() {
                        d = Y::Mapping(Mapping::new());
                    }
                    if apply_model_meta(&mut d, mi) {
                        *h = y2j(&d);
                        cx.diff.push(cx.store_label, tr!("{provider}.models.{mid}（隐藏中）已修改", "{provider}.models.{mid} (hidden) changed"), true);
                        store_dirty = true;
                    }
                    continue;
                }
                let def = def_mut(&mut cfg, &src).unwrap();
                let (mut shape, mut rows) = model_rows(def);
                let wants_meta = mi.name.as_deref().map(|n| !n.trim().is_empty()).unwrap_or(false) || mi.context.is_some();
                if shape == Shape::List && wants_meta {
                    rows = to_map_rows(rows);
                    shape = Shape::Map;
                }
                let before = rows.clone();
                match rows.iter().position(|r| r.0 == mid) {
                    Some(i) => {
                        if !rows[i].1.is_mapping() {
                            rows[i].1 = Y::Mapping(Mapping::new());
                        }
                        if !apply_model_meta(&mut rows[i].1, mi) && before == rows {
                            continue;
                        }
                        cx.diff.push(&cx.file, tr!("{provider}.models.{mid} 已修改", "{provider}.models.{mid} changed"), true);
                    }
                    None => {
                        let mut d = if shape == Shape::Map || wants_meta { Y::Mapping(Mapping::new()) } else { Y::Null };
                        apply_model_meta(&mut d, mi);
                        rows.push((mid.clone(), d));
                        cx.diff.push(&cx.file, format!("{provider}.models + {mid}"), true);
                    }
                }
                set_model_rows(def, shape, &rows);
            }
            Op::DeleteModel { provider, model } => {
                let src = entry_src(&cfg, provider)?;
                if def_of(&cfg, &src).and_then(default_of).as_deref() == Some(model.as_str()) || (current(&cfg).0 == *provider && model_default(&cfg).as_deref() == Some(model.as_str())) {
                    return Err(anyhow!(tr!(
                        "{model} 是默认 / 当前模型，不能删除；先换一个默认模型",
                        "{model} is the default / current model and can't be deleted; pick another default model first"
                    )));
                }
                let def = def_mut(&mut cfg, &src).unwrap();
                let (shape, mut rows) = model_rows(def);
                let n = rows.len();
                rows.retain(|r| &r.0 != model);
                if rows.len() != n {
                    set_model_rows(def, shape, &rows);
                    cx.diff.push(&cx.file, tr!("{provider}.models - {model}（删除）", "{provider}.models - {model} (deleted)"), false);
                }
                if store::section(&mut root, ID, "hiddenModels").remove(&format!("{provider}|{model}")).is_some() {
                    store_dirty = true;
                    if rows.len() == n {
                        cx.diff.push(cx.store_label, tr!("{provider}.models - {model}（删除）", "{provider}.models - {model} (deleted)"), false);
                    }
                }
            }
            Op::SetProviderModels { provider, models } => {
                let src = entry_src(&cfg, provider)?;
                let def = def_mut(&mut cfg, &src).unwrap();
                let (shape, rows) = model_rows(def);
                let new: Vec<(String, Y)> = clean_ids(models)
                    .into_iter()
                    .map(|m| {
                        let d = rows.iter().find(|r| r.0 == m).map(|r| r.1.clone()).unwrap_or(if shape == Shape::Map { Y::Mapping(Mapping::new()) } else { Y::Null });
                        (m, d)
                    })
                    .collect();
                if new != rows {
                    set_model_rows(def, shape, &new);
                    cx.diff.push(&cx.file, tr!("{provider}.models：{} 个模型", "{provider}.models: {} model(s)", new.len()), true);
                }
                let prefix = format!("{provider}|");
                let h = store::section(&mut root, ID, "hiddenModels");
                let n = h.len();
                h.retain(|k, _| !k.starts_with(&prefix));
                store_dirty |= h.len() != n;
            }
            Op::SetModelRoles { provider, roles } => {
                // Hermes has one role: the default model (the entry's default_model / model,
                // and model.default when the provider is active).
                if roles.keys().any(|k| k != "default") {
                    return Err(anyhow!(l("Hermes 只有「默认模型」一个角色", "Hermes has only one role: \"Default model\"")));
                }
                let Some(m) = roles.get("default").map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) else { continue };
                let src = find(&cfg, provider).ok_or_else(|| anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}")))?;
                let is_current = current(&cfg).0 == *provider;
                match &src {
                    Src::Dict(_) | Src::List(_) => {
                        let place = if let Src::Dict(k) = &src { format!("providers.{k}") } else { tr!("custom_providers（{provider}）", "custom_providers ({provider})") };
                        let def = def_mut(&mut cfg, &src).and_then(|d| d.as_mapping_mut()).unwrap();
                        let dk = default_key(&Y::Mapping(def.clone()), matches!(src, Src::List(_)));
                        if set_str(def, dk, Some(&m)) {
                            cx.diff.push(&cx.file, format!("{place}.{dk} = {m}"), true);
                        }
                    }
                    Src::Inline if !is_current => {
                        store::section(&mut root, ID, "inline").insert("default".into(), json!(m));
                        cx.diff.push(cx.store_label, tr!("直连配置默认模型 = {m}", "Direct config default model = {m}"), true);
                        store_dirty = true;
                    }
                    Src::Builtin(b) if !is_current => {
                        store::section(&mut root, ID, "builtins").insert(b.clone(), json!({ "default": m }));
                        cx.diff.push(cx.store_label, tr!("内置 · {b} 默认模型 = {m}", "Built-in · {b} default model = {m}"), true);
                        store_dirty = true;
                    }
                    _ => {}
                }
                if is_current && set_str(model_map(&mut cfg), "default", Some(&m)) {
                    cx.diff.push(&cx.file, format!("model.default = {m}"), true);
                }
            }
            Op::SetProviderEnabled { .. } => return Err(anyhow!(l("Hermes 同时只用一个供应商，请用「设为当前」", "Hermes uses one provider at a time; use \"Set as current\""))),
            Op::SetSetting { key, .. } => return Err(anyhow!(tr!("未知设置 {key}", "Unknown setting: {key}"))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
        }
    }

    // Drop list / dict blocks that became empty.
    if let Some(rm) = cfg.as_mapping_mut() {
        for k in ["providers", "custom_providers"] {
            let empty = match rm.get(k) {
                Some(Y::Mapping(m)) => m.is_empty(),
                Some(Y::Sequence(s)) => s.is_empty(),
                _ => false,
            };
            let was_nonempty = match cfg0.get(k) {
                Some(Y::Mapping(m)) => !m.is_empty(),
                Some(Y::Sequence(s)) => !s.is_empty(),
                _ => false,
            };
            if empty && was_nonempty {
                rm.shift_remove(k);
            }
        }
    }
    let changes: Vec<(&str, Option<&Y>)> = BLOCKS.iter().filter(|k| cfg0.get(**k) != cfg.get(**k)).map(|k| (*k, cfg.get(*k))).collect();
    let new_text = if changes.is_empty() { None } else { Some(rewrite(&text, &changes)?) };
    if let Some(t) = &new_text {
        // The whole file must still read back as exactly what we meant.
        let back: Y = serde_yaml::from_str(t).map_err(|e| anyhow!(tr!("写回的 config.yaml 无法解析：{e}", "The rewritten config.yaml can't be parsed: {e}")))?;
        let back = if back.is_null() { Y::Mapping(Mapping::new()) } else { back };
        if back != cfg {
            return Err(anyhow!(l("写回的 config.yaml 校验失败，不写入", "The rewritten config.yaml failed verification; not writing it")));
        }
    }
    let env_dirty = env != env0;

    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run {
        let mut targets = vec![];
        if new_text.is_some() {
            targets.push(config_path());
        }
        if env_dirty {
            targets.push(env_path());
        }
        if !targets.is_empty() {
            backup_dir = Some(backup(ID, &targets)?);
            std::fs::create_dir_all(dir())?;
        }
        if let Some(t) = new_text {
            write_text_atomic(&config_path(), &t, meta)?;
            written.push(config_path());
        }
        if env_dirty {
            write_text_atomic(&env_path(), &env, env_meta)?;
            written.push(env_path());
        }
        if store_dirty {
            store::save(&root)?;
        }
    }
    Ok((diff, written, backup_dir))
}

/// A named entry (models live only there).
fn entry_src(cfg: &Y, provider: &str) -> Result<Src> {
    match find(cfg, provider) {
        Some(s @ (Src::Dict(_) | Src::List(_))) => Ok(s),
        Some(Src::Inline) => Err(anyhow!(l(
            "直连配置（model 里的 custom）没有模型列表；可以在 config.yaml 里改 model.default，或新建一个供应商",
            "The direct config (custom in model) has no model list; change model.default in config.yaml, or add a new provider"
        ))),
        Some(Src::Builtin(_)) => Err(anyhow!(l("Hermes 内置供应商的模型用 hermes model 选择", "Pick models for Hermes built-in providers with hermes model"))),
        None => Err(anyhow!(tr!("找不到供应商 {provider}", "Provider not found: {provider}"))),
    }
}

/// Sets name / context_length on a model definition; true if anything changed.
fn apply_model_meta(d: &mut Y, mi: &ModelInput) -> bool {
    let Some(m) = d.as_mapping_mut() else { return false };
    let mut changed = false;
    if let Some(n) = mi.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        changed |= set_str(m, "name", Some(n));
    }
    if let Some(c) = mi.context {
        if m.get("context_length").and_then(|x| x.as_u64()) != Some(c) {
            m.insert(yk("context_length"), Y::Number(c.into()));
            changed = true;
        }
    }
    changed
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    const SAMPLE: &str = "\
# Hermes config (hand-written header comment)
model:
  default: deepseek-v4-flash
  provider: custom
  base_url: http://relay.example:8080/v1
  api_mode: chat_completions
  api_key: sk-inline-secret-1111
  extra_body:
    thinking:
      type: disabled
database:
  journal_mode: wal   # keep me
agent:
  max_turns: 500
  personalities: {}
skills:
  external_dirs:
    - D:\\klpz\\skills
mcp_servers:
  llm-review:
    args:
      - >-
        D:\\x\\server.py
custom_providers:
  - name: opencode
    base_url: https://opencode.ai/zen/go/v1
    api_key: sk-opencode-secret-2222
    models:
      deepseek-v4-flash:
        context_length: 1000000
        name: deepseek-v4-flash
    model: deepseek-v4-flash
  - name: relay-103
    base_url: http://relay.example:8080/v1
    extra_body:
      thinking:
        type: disabled

# trailing comment before hooks
hooks:
  pre_tool_call:
    - command: >-
        \"D:/nodejs/node.exe\" x.mjs
      timeout: 15
";

    /// (config dir, the temp home it lives in)
    struct Tmp(PathBuf, #[allow(dead_code)] TestHome);

    fn setup(yaml: &str) -> Tmp {
        crate::env::force(crate::env::Target::Windows);
        let home = TestHome::new("hermes");
        let d = dir();
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("config.yaml"), yaml).unwrap();
        Tmp(d, home)
    }

    fn apply(ops: Vec<Op>) -> Result<Diff> {
        plan(&ops, false).map(|x| x.0)
    }

    fn read_cfg(t: &Tmp) -> (String, Y) {
        let text = fs::read_to_string(t.0.join("config.yaml")).unwrap();
        let v = serde_yaml::from_str(&text).unwrap();
        (text, v)
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect::<Vec<_>>().join("\n")
    }

    fn pi(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> ProviderInput {
        ProviderInput { id: id.map(String::from), name: name.into(), base_url: base.into(), api: api.into(), api_key: key.map(String::from), models: models.iter().map(|s| s.to_string()).collect(), key_from_library: None, official_auth: None }
    }

    /// The file minus the blocks AgentPlus may rewrite: must never change.
    fn untouched(text: &str) -> String {
        let t = text.replace("\r\n", "\n");
        let lines: Vec<&str> = t.split('\n').collect();
        let bl = blocks(&lines);
        let mut keep = vec![true; lines.len()];
        for b in bl.iter().filter(|b| BLOCKS.contains(&b.key.as_str())) {
            for k in keep.iter_mut().take(b.end).skip(b.start) {
                *k = false;
            }
        }
        lines.iter().zip(keep).filter(|(_, k)| *k).map(|(l, _)| *l).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn reads_state() {
        let _t = setup(SAMPLE);
        let st = state(&Install::default());
        assert!(!st.readonly, "{:?}", st.notes);
        assert_eq!(st.mode, "single");
        assert_eq!(st.current_provider.as_deref(), Some(INLINE));
        let ids: Vec<&str> = st.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["opencode", "relay-103", "custom"]);
        let oc = &st.providers[0];
        assert!(oc.has_key);
        assert_eq!(oc.api, "chat");
        assert_eq!(oc.models.len(), 1);
        assert_eq!(oc.models[0].context, Some(1_000_000));
        assert!(oc.models[0].tags.contains(&Tag::default_model()));
        assert!(!st.providers[1].has_key);
        let inline = &st.providers[2];
        assert!(inline.has_key && inline.editable);
        assert!(inline.details.iter().any(|d| d.v.contains("relay-103")));
        let dump = format!("{:?}", st);
        assert!(!dump.contains("secret"), "a key leaked into state");
        let (b, k, api) = provider_endpoint("opencode").unwrap();
        assert_eq!((b.as_str(), k.as_deref(), api.as_str()), ("https://opencode.ai/zen/go/v1", Some("sk-opencode-secret-2222"), "chat"));
    }

    #[test]
    fn keyed_schema_and_builtin() {
        let _t = setup("model:\n  provider: custom:myproxy\n  default: glm-5\nproviders:\n  myproxy:\n    name: My Proxy\n    base_url: https://x/v1\n    api_key: sk-aaaa\n    api_mode: anthropic_messages\n    default_model: glm-5\n    models:\n      glm-5:\n        context_length: 200000\n");
        let st = state(&Install::default());
        assert_eq!(st.current_provider.as_deref(), Some("myproxy"));
        assert_eq!(st.providers[0].api, "anthropic");
        assert_eq!(st.providers[0].name, "My Proxy");
        drop(_t);
        let _t = setup("model:\n  provider: openrouter\n  default: anthropic/claude-sonnet-4\n");
        let st = state(&Install::default());
        assert_eq!(st.current_provider.as_deref(), Some("builtin:openrouter"));
        assert!(st.providers[0].builtin && !st.providers[0].editable);
        assert!(apply(vec![Op::UpsertProvider { provider: pi(Some("builtin:openrouter"), "x", "https://x", "chat", None, &[]) }]).is_err());
    }

    #[test]
    fn create_provider_keeps_rest_of_file() {
        let t = setup(SAMPLE);
        let d = apply(vec![Op::UpsertProvider { provider: pi(None, "My Proxy", "https://proxy.example/v1", "responses", Some("sk-new-secret-9999"), &["glm-5", "kimi-k2"]) }]).unwrap();
        let dt = diff_text(&d);
        assert!(dt.contains("+ providers.my-proxy"), "{dt}");
        assert!(!dt.contains("sk-new-secret") && dt.contains("••••9999"));
        let (text, v) = read_cfg(&t);
        assert_eq!(untouched(&text), untouched(SAMPLE));
        assert!(text.starts_with(&SAMPLE[..SAMPLE.find("custom_providers:").unwrap()]), "unrelated prefix changed:\n{text}");
        assert!(text.contains("# trailing comment before hooks\nhooks:"));
        assert!(text.contains("providers:\n  my-proxy:\n    name: My Proxy\n    base_url: https://proxy.example/v1\n    api_key: sk-new-secret-9999\n    api_mode: codex_responses\n    default_model: glm-5\n    models:\n      glm-5: {}\n      kimi-k2: {}\n"), "{text}");
        assert_eq!(v["providers"]["my-proxy"]["api_key"].as_str(), Some("sk-new-secret-9999"));
        // Second one with the same name gets a fresh id.
        apply(vec![Op::UpsertProvider { provider: pi(None, "My Proxy", "https://p2/v1", "chat", None, &[]) }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert!(v["providers"].get("my-proxy-2").is_some());
        // Name that collides with a legacy list entry.
        apply(vec![Op::UpsertProvider { provider: pi(None, "opencode", "https://p3/v1", "chat", None, &[]) }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert!(v["providers"].get("opencode-2").is_some());
    }

    #[test]
    fn edit_provider_list_and_rename_current() {
        let t = setup(SAMPLE);
        // Edit a list entry (not current): name, url, key.
        apply(vec![Op::UpsertProvider { provider: pi(Some("opencode"), "opencode-zen", "https://opencode.ai/zen/v1", "anthropic", Some("sk-rotated-3333"), &[]) }]).unwrap();
        let (text, v) = read_cfg(&t);
        let e = &v["custom_providers"][0];
        assert_eq!(e["name"].as_str(), Some("opencode-zen"));
        assert_eq!(e["base_url"].as_str(), Some("https://opencode.ai/zen/v1"));
        assert_eq!(e["api_mode"].as_str(), Some("anthropic_messages"));
        assert_eq!(e["api_key"].as_str(), Some("sk-rotated-3333"));
        assert!(text.contains("custom_providers:\n  - name: opencode-zen\n"), "{text}");
        assert_eq!(untouched(&text), untouched(SAMPLE));
        // Make it current, then rename: model.provider follows.
        apply(vec![Op::SetCurrentProvider { provider: "opencode-zen".into() }]).unwrap();
        apply(vec![Op::UpsertProvider { provider: pi(Some("opencode-zen"), "Zen Go", "https://opencode.ai/zen/v1", "anthropic", None, &[]) }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["model"]["provider"].as_str(), Some("custom:zen-go"));
        assert_eq!(v["custom_providers"][0]["api_key"].as_str(), Some("sk-rotated-3333"), "key kept when None");
        assert_eq!(state(&Install::default()).current_provider.as_deref(), Some("Zen Go"));
    }

    #[test]
    fn key_env_goes_to_dotenv() {
        let t = setup("model:\n  provider: custom:kp\n  default: m1\nproviders:\n  kp:\n    name: KP\n    base_url: https://kp/v1\n    key_env: KP_API_KEY\n    models: [m1, m2]\n");
        fs::write(t.0.join(".env"), "# my env\r\nOTHER=1\r\nKP_API_KEY=old\r\n").unwrap();
        let st = state(&Install::default());
        assert!(st.providers[0].has_key);
        assert_eq!(st.providers[0].models.len(), 2);
        let d = apply(vec![Op::UpsertProvider { provider: pi(Some("kp"), "KP", "https://kp/v1", "chat", Some("sk-env-secret-4444"), &[]) }]).unwrap();
        assert!(!diff_text(&d).contains("sk-env-secret"));
        assert_eq!(fs::read_to_string(t.0.join(".env")).unwrap(), "# my env\r\nOTHER=1\r\nKP_API_KEY=sk-env-secret-4444\r\n");
        let (_, v) = read_cfg(&t);
        assert!(v["providers"]["kp"].get("api_key").is_none());
        // List-shaped models stay a list.
        apply(vec![Op::UpsertModel { provider: "kp".into(), model: ModelInput { id: "m3".into(), name: None, context: None, ..Default::default() } }]).unwrap();
        let (text, _) = read_cfg(&t);
        assert!(text.contains("    models:\n      - m1\n      - m2\n      - m3\n"), "{text}");
        // Adding a context converts to the dict shape.
        apply(vec![Op::UpsertModel { provider: "kp".into(), model: ModelInput { id: "m2".into(), name: None, context: Some(128000), ..Default::default() } }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["providers"]["kp"]["models"]["m2"]["context_length"].as_u64(), Some(128000));
        assert!(v["providers"]["kp"]["models"]["m1"].is_mapping());
    }

    #[test]
    fn unreadable_dotenv_is_never_rewritten() {
        let t = setup("model:\n  provider: custom:kp\n  default: m1\nproviders:\n  kp:\n    name: KP\n    base_url: https://kp/v1\n    key_env: KP_API_KEY\n    models: [m1]\n");
        // UTF-16LE, as PowerShell 5's `echo X=1 > .env` writes it.
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("OTHER=1\r\nKP_API_KEY=old\r\n".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        fs::write(t.0.join(".env"), &utf16).unwrap();
        let err = apply(vec![Op::UpsertProvider { provider: pi(Some("kp"), "KP", "https://kp/v1", "chat", Some("sk-new-1111"), &[]) }]).err().expect("must refuse");
        assert!(err.to_string().contains("UTF-8"), "{err}");
        assert_eq!(fs::read(t.0.join(".env")).unwrap(), utf16);
        // Reading state still works and just doesn't see the key.
        assert_eq!(state(&Install::default()).providers.len(), 1);
    }

    #[test]
    fn delete_provider() {
        let t = setup(SAMPLE);
        assert!(apply(vec![Op::DeleteProvider { provider: "custom".into() }]).is_err(), "current can't be deleted");
        apply(vec![Op::DeleteProvider { provider: "relay-103".into() }]).unwrap();
        let (text, v) = read_cfg(&t);
        assert_eq!(v["custom_providers"].as_sequence().unwrap().len(), 1);
        assert_eq!(untouched(&text), untouched(SAMPLE));
        apply(vec![Op::DeleteProvider { provider: "opencode".into() }]).unwrap();
        let (text, v) = read_cfg(&t);
        assert!(v.get("custom_providers").is_none());
        assert_eq!(untouched(&text), untouched(SAMPLE));
    }

    #[test]
    fn models_hide_show_add_delete_replace() {
        let t = setup(SAMPLE);
        apply(vec![Op::UpsertModel { provider: "opencode".into(), model: ModelInput { id: "glm-5".into(), name: Some("GLM 5".into()), context: Some(200000), ..Default::default() } }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["custom_providers"][0]["models"]["glm-5"]["name"].as_str(), Some("GLM 5"));
        assert!(apply(vec![Op::SetModelVisible { provider: "opencode".into(), model: "deepseek-v4-flash".into(), visible: false }]).is_err(), "default model");
        apply(vec![Op::SetModelVisible { provider: "opencode".into(), model: "glm-5".into(), visible: false }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert!(v["custom_providers"][0]["models"].get("glm-5").is_none());
        let st = state(&Install::default());
        let m = st.providers[0].models.iter().find(|m| m.id == "glm-5").unwrap();
        assert!(!m.visible && m.context == Some(200000));
        apply(vec![Op::SetModelVisible { provider: "opencode".into(), model: "glm-5".into(), visible: true }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["custom_providers"][0]["models"]["glm-5"]["context_length"].as_u64(), Some(200000));
        apply(vec![Op::DeleteModel { provider: "opencode".into(), model: "glm-5".into() }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert!(v["custom_providers"][0]["models"].get("glm-5").is_none());
        apply(vec![Op::SetProviderModels { provider: "relay-103".into(), models: vec!["a".into(), "b".into(), "a".into()] }]).unwrap();
        let (text, v) = read_cfg(&t);
        assert_eq!(v["custom_providers"][1]["models"].as_mapping().unwrap().len(), 2);
        assert_eq!(untouched(&text), untouched(SAMPLE));
        // Default role.
        apply(vec![Op::SetModelRoles { provider: "relay-103".into(), roles: BTreeMap::from([("default".to_string(), "b".to_string())]) }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["custom_providers"][1]["model"].as_str(), Some("b"));
        assert!(apply(vec![Op::SetModelRoles { provider: "relay-103".into(), roles: BTreeMap::from([("opus".to_string(), "b".to_string())]) }]).is_err());
        assert!(apply(vec![Op::UpsertModel { provider: "custom".into(), model: ModelInput { id: "x".into(), name: None, context: None, ..Default::default() } }]).is_err());
    }

    #[test]
    fn switch_current_and_back() {
        let t = setup(SAMPLE);
        let d = apply(vec![Op::SetCurrentProvider { provider: "opencode".into() }]).unwrap();
        let dt = diff_text(&d);
        assert!(dt.contains("model.provider = custom:opencode") && dt.contains("- model.api_key"), "{dt}");
        assert!(!dt.contains("secret"));
        let (text, v) = read_cfg(&t);
        let m = &v["model"];
        assert_eq!(m["provider"].as_str(), Some("custom:opencode"));
        assert_eq!(m["default"].as_str(), Some("deepseek-v4-flash"));
        assert!(m.get("base_url").is_none() && m.get("api_key").is_none() && m.get("api_mode").is_none());
        assert!(m.get("extra_body").is_some(), "other model keys kept");
        assert_eq!(untouched(&text), untouched(SAMPLE));
        let st = state(&Install::default());
        assert_eq!(st.current_provider.as_deref(), Some("opencode"));
        let inline = st.providers.iter().find(|p| p.id == INLINE).expect("inline stays listed from the stash");
        assert!(inline.has_key);
        assert_eq!(provider_endpoint(INLINE).unwrap().1.as_deref(), Some("sk-inline-secret-1111"));
        // And back.
        apply(vec![Op::SetCurrentProvider { provider: INLINE.into() }]).unwrap();
        let (_, v) = read_cfg(&t);
        let m = &v["model"];
        assert_eq!(m["provider"].as_str(), Some("custom"));
        assert_eq!(m["base_url"].as_str(), Some("http://relay.example:8080/v1"));
        assert_eq!(m["api_key"].as_str(), Some("sk-inline-secret-1111"));
        assert_eq!(m["api_mode"].as_str(), Some("chat_completions"));
        assert_eq!(m["default"].as_str(), Some("deepseek-v4-flash"));
        // A provider without models can't be made current.
        assert!(apply(vec![Op::SetCurrentProvider { provider: "relay-103".into() }]).is_err());
        assert!(apply(vec![Op::SetProviderEnabled { provider: "opencode".into(), enabled: false }]).is_err());
        assert!(apply(vec![Op::SetSetting { key: "x".into(), value: json!(true) }]).is_err());
    }

    #[test]
    fn builtin_is_stashed_when_leaving() {
        let t = setup("model:\n  provider: openrouter\n  default: anthropic/claude-sonnet-4\nproviders:\n  p:\n    base_url: https://p/v1\n    models: {m: {}}\n");
        apply(vec![Op::SetCurrentProvider { provider: "p".into() }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["model"]["provider"].as_str(), Some("custom:p"));
        assert_eq!(v["model"]["default"].as_str(), Some("m"));
        let st = state(&Install::default());
        assert!(st.providers.iter().any(|p| p.id == "builtin:openrouter"));
        apply(vec![Op::SetCurrentProvider { provider: "builtin:openrouter".into() }]).unwrap();
        let (_, v) = read_cfg(&t);
        assert_eq!(v["model"]["provider"].as_str(), Some("openrouter"));
        assert_eq!(v["model"]["default"].as_str(), Some("anthropic/claude-sonnet-4"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let t = setup(SAMPLE);
        let (d, written, backup) = plan(&[Op::SetCurrentProvider { provider: "opencode".into() }, Op::UpsertProvider { provider: pi(None, "X", "https://x/v1", "chat", Some("sk-x-5555"), &["m"]) }], true).unwrap();
        assert!(!d.groups.is_empty());
        assert!(written.is_empty() && backup.is_none());
        assert_eq!(fs::read_to_string(t.0.join("config.yaml")).unwrap(), SAMPLE);
        assert!(!agentplus_dir().join("store.json").exists());
    }

    #[test]
    fn comments_in_changed_block_refuse_write() {
        let yaml = SAMPLE.replace("  default: deepseek-v4-flash\n", "  default: deepseek-v4-flash  # the model\n");
        let t = setup(&yaml);
        let st = state(&Install::default());
        assert!(st.readonly);
        assert!(apply(vec![Op::SetCurrentProvider { provider: "opencode".into() }]).is_err());
        assert_eq!(fs::read_to_string(t.0.join("config.yaml")).unwrap(), yaml);
        // Comments in other blocks are fine (database has one), and in untouched target blocks too.
        let yaml = SAMPLE.replace("  - name: relay-103\n", "  # the relay\n  - name: relay-103\n");
        let t2 = {
            drop(t);
            setup(&yaml)
        };
        assert!(apply(vec![Op::UpsertProvider { provider: pi(None, "N", "https://n/v1", "chat", None, &[]) }]).is_ok());
        assert!(fs::read_to_string(t2.0.join("config.yaml")).unwrap().contains("  # the relay\n"));
    }

    #[test]
    fn crlf_kept_and_missing_file() {
        let t = setup(&SAMPLE.replace('\n', "\r\n"));
        apply(vec![Op::SetCurrentProvider { provider: "opencode".into() }]).unwrap();
        let raw = fs::read_to_string(t.0.join("config.yaml")).unwrap();
        assert!(!raw.replace("\r\n", "").contains('\n'), "mixed line endings");
        assert_eq!(untouched(&raw), untouched(SAMPLE));
        drop(t);
        let t = setup("");
        fs::remove_file(t.0.join("config.yaml")).unwrap();
        let st = state(&Install::default());
        assert!(!st.readonly && st.providers.iter().all(|p| p.builtin));
        apply(vec![Op::UpsertProvider { provider: pi(None, "First", "https://f/v1", "chat", Some("sk-f-6666"), &["m1"]) }, Op::SetCurrentProvider { provider: "first".into() }]).unwrap();
        let (text, v) = read_cfg(&t);
        assert_eq!(v["model"]["provider"].as_str(), Some("custom:first"));
        assert!(text.starts_with("providers:\n") || text.starts_with("model:\n"), "{text}");
    }

    #[test]
    fn emitter_quotes_and_roundtrips() {
        let src = "k:\n  a: ''\n  b: 'yes'\n  c: '1.5'\n  d: 'a: b'\n  e: '#x'\n  f: 'null'\n  g: \"line\\nbreak\"\n  h: http://h:80/v1\n  i: 中文 名字\n  j: '0x1F'\n  k: '2024-01-01'\n  l: [1, 2]\n  m: {}\n  n: - x\n";
        let src = src.replace("  n: - x\n", "  n:\n    - x\n    - {id: y, context_length: 3}\n    - [p, q]\n");
        let v: Y = serde_yaml::from_str(&src).unwrap();
        let out = emit_top("k", &v["k"]).unwrap().join("\n");
        let back: Y = serde_yaml::from_str(&out).unwrap();
        assert_eq!(back, v, "{out}");
        assert!(out.contains("  b: 'yes'") && out.contains("  h: http://h:80/v1") && out.contains("  i: 中文 名字"), "{out}");
        assert!(out.contains("  n:\n    - x\n    - id: y\n      context_length: 3\n    - - p\n      - q"), "{out}");
    }

    #[test]
    fn block_split() {
        let lines: Vec<&str> = SAMPLE.split('\n').collect();
        let b = blocks(&lines);
        let keys: Vec<&str> = b.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(keys, vec!["model", "database", "agent", "skills", "mcp_servers", "custom_providers", "hooks"]);
        let cp = b.iter().find(|b| b.key == "custom_providers").unwrap();
        assert!(!lines[cp.end - 1].is_empty() && !lines[cp.end - 1].starts_with('#'));
        assert!(!has_extras(&lines[cp.start..cp.end]));
        assert!(has_extras(&["  a: 1 # c"]) && has_extras(&["  a: &x 1"]) && !has_extras(&["  a: 'x # y'", "  u: http://h/#f"]));
    }

    /// Read-only look at the real Hermes config on this machine (keys masked).
    #[test]
    #[ignore]
    fn dump_hermes() {
        println!("dir: {}", dir().display());
        let inst = detect();
        println!("detect: installed={} version={:?} running={} dir={:?}", inst.installed, inst.version, inst.running, inst.dir);
        let st = state(&inst);
        println!("mode={} current={:?} readonly={}", st.mode, st.current_provider, st.readonly);
        for p in &st.providers {
            println!(
                "- {} [{}] {:?} api={} has_key={} builtin={} compatible={} models={:?}",
                p.id,
                p.name,
                p.base_url,
                p.api,
                p.has_key,
                p.builtin,
                p.compatible,
                p.models.iter().map(|m| format!("{}{}{}", m.id, if m.visible { "" } else { "(hidden)" }, if m.tags.is_empty() { String::new() } else { format!("{:?}", m.tags.iter().map(|t| &t.label).collect::<Vec<_>>()) })).collect::<Vec<_>>()
            );
            for d in &p.details {
                println!("    {}: {}", d.k, d.v);
            }
        }
        for kv in &st.current {
            println!("current {}: {}", kv.k, kv.v);
        }
        for n in &st.notes {
            println!("note: {n}");
        }
        let target = st.providers.iter().find(|p| Some(&p.id) != st.current_provider.as_ref() && !p.builtin && p.compatible && !p.models.is_empty()).map(|p| p.id.clone());
        if let Some(t) = target {
            let (d, written, backup) = plan(&[Op::SetCurrentProvider { provider: t.clone() }], true).unwrap();
            println!("dry-run switch to {t}:");
            for g in &d.groups {
                for l in &g.lines {
                    println!("  {} {} {}", g.file, if l.add { "+" } else { "-" }, l.text);
                }
            }
            assert!(written.is_empty() && backup.is_none());

            // Real write on a temp copy of the real file (the original is only read).
            let real = fs::read_to_string(config_path()).unwrap();
            let home = TestHome::new("hermes-real");
            let tmp = dir();
            fs::create_dir_all(&tmp).unwrap();
            fs::write(tmp.join("config.yaml"), &real).unwrap();
            let back_to = st.current_provider.clone().unwrap();
            plan(&[Op::SetCurrentProvider { provider: t.clone() }], false).unwrap();
            plan(&[Op::SetCurrentProvider { provider: back_to }], false).unwrap();
            let after = fs::read_to_string(tmp.join("config.yaml")).unwrap();
            drop(home);
            assert_eq!(untouched(&after), untouched(&real));
            let (a, b): (Y, Y) = (serde_yaml::from_str(&after).unwrap(), serde_yaml::from_str(&real).unwrap());
            assert_eq!(a, b, "switch and back must give the same config");
            println!("temp-copy round trip: other blocks byte-identical, config equal after switching back");
        }
    }
}
