//! Provider import links: `agentplus://v1/import?…`, and CC Switch's `ccswitch://v1/import?…`
//! that relay sites (Sub2API, New API…) put behind their "Import to CC Switch" buttons. Both
//! take the same query parameters (`resource=provider`, `app`, `name`, `endpoint`, `apiKey`,
//! `model`…), so a site that supports CC Switch only has to swap the scheme.
//!
//! A link never changes anything by itself: it is parsed into an `ImportRequest` that fills
//! in the add-provider dialog, and the user saves it from there.
//!
//! Links arrive while the app starts (its command line), from a second launch (the
//! single-instance plugin hands over the command line) or, on macOS, as an "open URL" event.
//! They wait in an inbox until the page takes them, so one that comes in before the page has
//! loaded isn't lost.

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

use crate::i18n::l;

/// Our own scheme (registered by the installer, see tauri.conf.json).
pub const SCHEME: &str = "agentplus";
/// CC Switch's scheme; AgentPlus only opens these when the user lets it (Settings).
pub const CCSWITCH: &str = "ccswitch";

/// Longer than any real link (a base64 config included); anything bigger is ignored.
const MAX_LINK: usize = 64 * 1024;

/// What a link asks to add: the add-provider dialog starts out with this.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub name: String,
    pub base_url: String,
    /// responses | chat | anthropic | gemini
    pub api: String,
    pub api_key: String,
    pub models: Vec<String>,
    /// The AgentPlus agent the link was made for (CC Switch's `app`), when AgentPlus has it.
    pub agent: Option<String>,
    pub homepage: Option<String>,
    /// The link's scheme: "agentplus" or "ccswitch".
    pub source: String,
}

/// A link taken from the inbox: what it asks for, or why it can't be imported.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub request: Option<ImportRequest>,
    pub error: Option<String>,
}

/// CC Switch's `app` values, and the AgentPlus agent and protocol each stands for.
/// Grok Build isn't an AgentPlus agent: its OpenAI-compatible address is still importable.
const APPS: [(&str, Option<&str>, &str); 7] = [
    ("claude", Some("claude"), "anthropic"),
    ("codex", Some("codex"), "responses"),
    ("gemini", Some("gemini"), "gemini"),
    ("opencode", Some("opencode"), "chat"),
    ("openclaw", Some("openclaw"), "chat"),
    ("hermes", Some("hermes"), "chat"),
    ("grokbuild", None, "chat"),
];

const APIS: [&str; 4] = ["responses", "chat", "anthropic", "gemini"];

/// Whether a command-line argument is an import link AgentPlus opens.
pub fn is_link(arg: &str) -> bool {
    let lower = arg.trim().to_ascii_lowercase();
    [SCHEME, CCSWITCH].iter().any(|s| lower.starts_with(&format!("{s}://")))
}

/// Parses an import link.
pub fn parse(link: &str) -> Result<ImportRequest> {
    let link = link.trim();
    if link.len() > MAX_LINK {
        bail!("{}", l("This import link is too long", "导入链接太长"));
    }
    let url = url::Url::parse(link).map_err(|_| anyhow!(l("This is not an import link", "这不是导入链接")))?;
    let source = url.scheme().to_ascii_lowercase();
    // agentplus://v1/import and ccswitch://v1/import (CC Switch accepts nothing else).
    if ![SCHEME, CCSWITCH].contains(&source.as_str()) || url.host_str() != Some("v1") || url.path().trim_end_matches('/') != "/import" {
        bail!("{}", l("This is not an import link", "这不是导入链接"));
    }
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
    let resource = q("resource").unwrap_or_else(|| "provider".into());
    if resource != "provider" {
        bail!("{}", tr!("AgentPlus imports providers only; this link carries a {resource}", "AgentPlus 只导入供应商，这个链接导入的是 {resource}"));
    }

    let app = q("app").map(|a| a.to_ascii_lowercase());
    let known = app.as_deref().and_then(|a| APPS.iter().find(|x| x.0 == a));
    // Our own links may name any AgentPlus agent.
    let agent = match known {
        Some(x) => x.1.map(str::to_string),
        None => app.clone().filter(|a| source == SCHEME && crate::adapters::ALL.contains(&a.as_str())),
    };
    let mut api = known.map(|x| x.2).unwrap_or("chat").to_string();
    if let Some(a) = q("api").filter(|_| source == SCHEME) {
        if !APIS.contains(&a.as_str()) {
            bail!("{}", tr!("Unknown API type {a}", "未知接口类型 {a}"));
        }
        api = a;
    }

    // Values in the URL win over the ones in `config`, as in CC Switch.
    let from_config = match q("config") {
        Some(c) => config_values(&c, q("configFormat").as_deref().unwrap_or("json"))?,
        None => Found::default(),
    };
    if q("configUrl").is_some() && q("endpoint").is_none() && from_config.base_url.is_none() {
        bail!("{}", l("Links that load their settings from a URL aren't supported", "不支持从网址加载配置的链接"));
    }
    // Several endpoints (comma-separated): the first one is the provider's address.
    let endpoint = q("endpoint").and_then(|e| e.split(',').map(str::trim).find(|s| !s.is_empty()).map(str::to_string)).or(from_config.base_url);
    let Some(endpoint) = endpoint else {
        bail!("{}", l("The link has no endpoint address", "链接里没有接口地址"));
    };
    let base_url = endpoint.trim_end_matches('/').to_string();
    let parsed = url::Url::parse(&base_url).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
    let Some(parsed) = parsed else {
        bail!("{}", tr!("The link's endpoint isn't an http(s) address: {base_url}", "链接里的接口地址不是 http(s) 地址：{base_url}"));
    };

    let name = q("name").or_else(|| parsed.host_str().map(str::to_string)).unwrap_or_default();
    let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
    let api_key = q("apiKey").or(from_config.api_key).unwrap_or_default();
    if api_key.len() > 4096 || api_key.chars().any(char::is_whitespace) {
        bail!("{}", l("The link's API key isn't valid", "链接里的 API Key 无效"));
    }

    let mut models: Vec<String> = vec![];
    let listed: Vec<String> = q("models").filter(|_| source == SCHEME).map(|m| m.split(',').map(str::to_string).collect()).unwrap_or_default();
    let named = ["model", "sonnetModel", "opusModel", "haikuModel"].iter().filter_map(|k| q(k));
    for m in listed.into_iter().chain(named).chain(from_config.models) {
        let m = m.trim();
        if !m.is_empty() && m.len() <= 200 && !m.chars().any(char::is_control) && !models.iter().any(|x| x == m) && models.len() < 100 {
            models.push(m.to_string());
        }
    }
    let homepage = q("homepage").filter(|h| h.starts_with("https://") || h.starts_with("http://"));
    Ok(ImportRequest { name, base_url, api, api_key, models, agent, homepage, source })
}

/// What a link's `config` says (CC Switch's own config format for the app).
#[derive(Default)]
struct Found {
    base_url: Option<String>,
    api_key: Option<String>,
    models: Vec<String>,
}

const URL_KEYS: [&str; 3] = ["ANTHROPIC_BASE_URL", "GOOGLE_GEMINI_BASE_URL", "OPENAI_BASE_URL"];
const KEY_KEYS: [&str; 4] = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "OPENAI_API_KEY"];
const MODEL_KEYS: [&str; 5] = ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "GEMINI_MODEL"];

/// `config`: base64 of JSON (Claude / Gemini: `{"env": {…}}`; Codex: `{"auth": {…}, "config": "<toml>"}`)
/// or of Codex's TOML.
fn config_values(b64: &str, format: &str) -> Result<Found> {
    let bad = || anyhow!(l("The link's config can't be read", "读不了链接里的配置"));
    let text = decode_base64(b64).ok_or_else(bad)?;
    let mut f = Found::default();
    if format.eq_ignore_ascii_case("toml") {
        toml_values(&text, &mut f);
        return Ok(f);
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| bad())?;
    let str_at = |obj: Option<&Value>, k: &str| obj.and_then(|o| o.get(k)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let env = v.get("env");
    f.base_url = URL_KEYS.iter().find_map(|k| str_at(env, k));
    f.api_key = KEY_KEYS.iter().find_map(|k| str_at(env, k).or_else(|| str_at(v.get("auth"), k)));
    f.models = MODEL_KEYS.iter().filter_map(|k| str_at(env, k)).collect();
    if let Some(toml) = v.get("config").and_then(Value::as_str) {
        toml_values(toml, &mut f);
    }
    Ok(f)
}

/// Codex's config.toml: the active provider's `base_url` (or any provider's) and `model`.
fn toml_values(text: &str, f: &mut Found) {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return };
    let providers = doc.get("model_providers").and_then(|p| p.as_table_like());
    let active = doc.get("model_provider").and_then(|p| p.as_str());
    let url_of = |id: &str| providers.and_then(|p| p.get(id)).and_then(|t| t.get("base_url")).and_then(|u| u.as_str()).map(str::to_string);
    let url = active.and_then(url_of).or_else(|| providers.and_then(|p| p.iter().find_map(|(id, _)| url_of(id))));
    if f.base_url.is_none() {
        f.base_url = url;
    }
    if let Some(m) = doc.get("model").and_then(|m| m.as_str()) {
        f.models.push(m.to_string());
    }
}

/// Standard or URL-safe base64, padded or not; a `+` that became a space in the URL is put back.
fn decode_base64(s: &str) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let s = s.trim().replace(' ', "+");
    let bytes = [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD].iter().find_map(|e| e.decode(&s).ok())?;
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------- Inbox

static INBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keeps a link for the page (which takes it once it has loaded).
pub fn queue(link: &str) {
    let link = link.trim();
    let scheme = link.split("://").next().unwrap_or_default().to_ascii_lowercase();
    crate::applog::info("import", format!("{scheme} link received"));
    if let Ok(mut inbox) = INBOX.lock() {
        // macOS may report the link that started the app twice (its launch and the open event).
        if inbox.last().map(String::as_str) != Some(link) {
            inbox.push(link.to_string());
        }
    }
}

/// A link came in while the app runs: keep it, tell the page, and bring the window up.
pub fn receive(app: &AppHandle, link: &str) {
    if !is_link(link) {
        return;
    }
    queue(link);
    let _ = app.emit("import-link", ());
    crate::tray::show_main(app);
}

/// A second launch's command line, handed over: its import links.
pub fn receive_args<S: AsRef<str>>(app: &AppHandle, args: impl IntoIterator<Item = S>) {
    for a in args.into_iter().skip(1) {
        receive(app, a.as_ref());
    }
}

/// The links waiting, each parsed now (errors in the current UI language).
pub fn take() -> Vec<ImportItem> {
    let links = INBOX.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
    links
        .iter()
        .map(|link| match parse(link) {
            Ok(r) => ImportItem { request: Some(r), error: None },
            Err(e) => ImportItem { request: None, error: Some(format!("{e:#}")) },
        })
        .collect()
}

// ---------------------------------------------------------------- Opening ccswitch:// links

/// Whether `ccswitch://` links open in AgentPlus (Windows only: elsewhere the system decides).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LinkHandler {
    pub supported: bool,
    pub on: bool,
    /// The program that opens them now, when it isn't AgentPlus (e.g. "CC Switch.exe").
    pub other: Option<String>,
}

#[cfg(windows)]
mod win {
    use super::LinkHandler;
    use anyhow::Result;
    use winreg::enums::*;
    use winreg::RegKey;

    const KEY: &str = r"Software\Classes\ccswitch";
    const CMD: &str = r"shell\open\command";
    const ICON: &str = "DefaultIcon";
    /// Values AgentPlus leaves on the key while it opens the links: what opened them before
    /// ("" = nothing in the user's registry), so turning it off hands them back.
    const PREV_CMD: &str = "AgentPlusPreviousCommand";
    const PREV_ICON: &str = "AgentPlusPreviousIcon";

    fn exe() -> Result<std::path::PathBuf> {
        Ok(std::env::current_exe()?)
    }

    fn command(exe: &std::path::Path) -> String {
        format!("\"{}\" \"%1\"", exe.display())
    }

    /// The program a `shell\open\command` value runs.
    pub(super) fn program(cmd: &str) -> String {
        let cmd = cmd.trim();
        match cmd.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or_default().to_string(),
            None => cmd.split_whitespace().next().unwrap_or_default().to_string(),
        }
    }

    fn file_name(path: &str) -> String {
        std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }

    fn read(hive: winreg::HKEY, sub: &str) -> Option<String> {
        RegKey::predef(hive).open_subkey(format!(r"{KEY}\{sub}")).ok()?.get_value::<String, _>("").ok()
    }

    /// What runs for a `ccswitch://` link: the user's registration wins over the machine's.
    fn current() -> Option<String> {
        read(HKEY_CURRENT_USER, CMD).or_else(|| read(HKEY_LOCAL_MACHINE, CMD))
    }

    fn is_ours(cmd: &str) -> bool {
        exe().map(|e| program(cmd).eq_ignore_ascii_case(&e.to_string_lossy())).unwrap_or(false)
    }

    pub fn status() -> LinkHandler {
        let cmd = current();
        let on = cmd.as_deref().is_some_and(is_ours);
        let other = if on { None } else { cmd.map(|c| file_name(&program(&c))).filter(|n| !n.is_empty()) };
        LinkHandler { supported: true, on, other }
    }

    pub fn set(on: bool) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if on {
            let exe = exe()?;
            let (key, _) = hkcu.create_subkey(KEY)?;
            // The first takeover's backup stays: taking over again must not back up AgentPlus itself.
            if key.get_value::<String, _>(PREV_CMD).is_err() {
                key.set_value(PREV_CMD, &read(HKEY_CURRENT_USER, CMD).unwrap_or_default())?;
                key.set_value(PREV_ICON, &read(HKEY_CURRENT_USER, ICON).unwrap_or_default())?;
            }
            if key.get_value::<String, _>("").is_err() {
                key.set_value("", &"URL:ccswitch")?;
            }
            key.set_value("URL Protocol", &"")?;
            hkcu.create_subkey(format!(r"{KEY}\{ICON}"))?.0.set_value("", &format!("{},0", exe.display()))?;
            hkcu.create_subkey(format!(r"{KEY}\{CMD}"))?.0.set_value("", &command(&exe))?;
            return Ok(());
        }
        let Ok(key) = hkcu.open_subkey_with_flags(KEY, KEY_ALL_ACCESS) else { return Ok(()) };
        match key.get_value::<String, _>(PREV_CMD) {
            // Nothing was registered for the user before: remove the whole key (a machine-wide
            // registration, if any, takes over again).
            Ok(prev) if prev.is_empty() => hkcu.delete_subkey_all(KEY)?,
            Ok(prev) => {
                hkcu.create_subkey(format!(r"{KEY}\{CMD}"))?.0.set_value("", &prev)?;
                let icon: String = key.get_value(PREV_ICON).unwrap_or_default();
                if !icon.is_empty() {
                    hkcu.create_subkey(format!(r"{KEY}\{ICON}"))?.0.set_value("", &icon)?;
                }
                let _ = key.delete_value(PREV_CMD);
                let _ = key.delete_value(PREV_ICON);
            }
            // No backup: only undo a registration that is AgentPlus's.
            Err(_) => {
                if read(HKEY_CURRENT_USER, CMD).as_deref().is_some_and(is_ours) {
                    hkcu.delete_subkey_all(KEY)?;
                }
            }
        }
        Ok(())
    }

    /// At start: AgentPlus took the links over, and has since moved (an update to another
    /// folder, a portable copy): point them at this copy. If another program took them back
    /// since (its own registration), it keeps them.
    pub fn refresh() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) = hkcu.open_subkey(KEY) else { return };
        if key.get_value::<String, _>(PREV_CMD).is_err() {
            return;
        }
        let (Some(cmd), Ok(exe)) = (read(HKEY_CURRENT_USER, CMD), exe()) else { return };
        let ran = program(&cmd);
        let same_app = file_name(&ran).eq_ignore_ascii_case(&file_name(&exe.to_string_lossy()));
        if same_app && !ran.eq_ignore_ascii_case(&exe.to_string_lossy()) {
            let _ = set(true);
        }
    }
}

pub fn ccswitch_status() -> LinkHandler {
    #[cfg(windows)]
    return win::status();
    #[cfg(not(windows))]
    LinkHandler { supported: false, on: false, other: None }
}

pub fn set_ccswitch(on: bool) -> Result<LinkHandler> {
    #[cfg(windows)]
    {
        win::set(on)?;
        crate::applog::info("import", format!("ccswitch links {}", if on { "taken over" } else { "handed back" }));
        Ok(win::status())
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        bail!("{}", l("Only available on Windows", "仅 Windows 可用"))
    }
}

/// At start: keep the `ccswitch://` takeover pointing at this copy of AgentPlus.
#[cfg(windows)]
pub fn refresh_ccswitch() {
    win::refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    /// Sub2API's link for an Anthropic key (see its frontend/src/utils/ccswitchImport.ts).
    #[test]
    fn sub2api_claude() {
        let link = format!(
            "ccswitch://v1/import?resource=provider&app=claude&name=My+Relay&homepage=https%3A%2F%2Frelay.example.com&endpoint=https%3A%2F%2Frelay.example.com&apiKey=sk-abc123&configFormat=json&usageEnabled=true&usageScript={}&usageAutoInterval=30",
            b64("({ request: {} })")
        );
        let r = parse(&link).unwrap();
        assert_eq!(r.name, "My Relay");
        assert_eq!(r.base_url, "https://relay.example.com");
        assert_eq!((r.api.as_str(), r.agent.as_deref()), ("anthropic", Some("claude")));
        assert_eq!(r.api_key, "sk-abc123");
        assert_eq!(r.homepage.as_deref(), Some("https://relay.example.com"));
        assert_eq!(r.source, "ccswitch");
        assert!(r.models.is_empty());
    }

    #[test]
    fn codex_and_other_apps() {
        let r = parse("ccswitch://v1/import?resource=provider&app=codex&model=gpt-5.5&name=R&endpoint=https://r.example.com/v1/&apiKey=k").unwrap();
        assert_eq!((r.api.as_str(), r.agent.as_deref(), r.base_url.as_str()), ("responses", Some("codex"), "https://r.example.com/v1"));
        assert_eq!(r.models, ["gpt-5.5"]);
        let g = parse("ccswitch://v1/import?resource=provider&app=gemini&name=G&endpoint=https://g.example.com&apiKey=k").unwrap();
        assert_eq!((g.api.as_str(), g.agent.as_deref()), ("gemini", Some("gemini")));
        let x = parse("ccswitch://v1/import?resource=provider&app=grokbuild&name=X&endpoint=https://x.example.com/v1&apiKey=k&model=grok-4.5").unwrap();
        assert_eq!((x.api.as_str(), x.agent), ("chat", None));
        // An app CC Switch may add later: still importable, as an OpenAI-compatible address.
        let n = parse("ccswitch://v1/import?resource=provider&app=newthing&name=N&endpoint=https://n.example.com&apiKey=k").unwrap();
        assert_eq!((n.api.as_str(), n.agent), ("chat", None));
    }

    /// New API's link: several models for Claude's roles, `enabled=true`.
    #[test]
    fn claude_roles_and_several_endpoints() {
        let r = parse("ccswitch://v1/import?resource=provider&app=claude&name=N&endpoint=https://a.example.com/,https://b.example.com&apiKey=k&model=claude-sonnet-5&haikuModel=claude-haiku-4-5&sonnetModel=claude-sonnet-5&opusModel=claude-opus-5-5&enabled=true").unwrap();
        assert_eq!(r.base_url, "https://a.example.com");
        assert_eq!(r.models, ["claude-sonnet-5", "claude-opus-5-5", "claude-haiku-4-5"]);
    }

    #[test]
    fn own_scheme_extras() {
        let r = parse("AgentPlus://v1/import?name=Mine&endpoint=https://m.example.com/v1&apiKey=k&api=chat&models=a,b,,a&app=kimi").unwrap();
        assert_eq!((r.source.as_str(), r.api.as_str(), r.agent.as_deref()), ("agentplus", "chat", Some("kimi")));
        assert_eq!(r.models, ["a", "b"]);
        // CC Switch links can't name agents or protocols AgentPlus has and CC Switch doesn't.
        let c = parse("ccswitch://v1/import?resource=provider&name=C&endpoint=https://c.example.com&app=kimi&api=anthropic").unwrap();
        assert_eq!((c.api.as_str(), c.agent), ("chat", None));
        assert!(parse("agentplus://v1/import?name=M&endpoint=https://m.example.com&api=grpc").is_err());
    }

    #[test]
    fn config_fills_what_the_url_leaves_out() {
        let claude = b64(r#"{"env":{"ANTHROPIC_BASE_URL":"https://cfg.example.com","ANTHROPIC_AUTH_TOKEN":"sk-cfg","ANTHROPIC_MODEL":"m1"}}"#);
        let r = parse(&format!("ccswitch://v1/import?resource=provider&app=claude&name=C&config={claude}")).unwrap();
        assert_eq!((r.base_url.as_str(), r.api_key.as_str(), r.models.as_slice()), ("https://cfg.example.com", "sk-cfg", &["m1".to_string()][..]));
        // The URL's own values win.
        let r = parse(&format!("ccswitch://v1/import?resource=provider&app=claude&name=C&endpoint=https://url.example.com&apiKey=sk-url&config={claude}")).unwrap();
        assert_eq!((r.base_url.as_str(), r.api_key.as_str()), ("https://url.example.com", "sk-url"));
        // Codex: auth + TOML in JSON, URL-safe base64 without padding.
        let codex = r#"{"auth":{"OPENAI_API_KEY":"sk-x"},"config":"model_provider = \"custom\"\nmodel = \"gpt-5.5\"\n[model_providers.custom]\nbase_url = \"https://cx.example.com/v1\"\n"}"#;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(codex);
        let r = parse(&format!("ccswitch://v1/import?resource=provider&app=codex&name=X&config={enc}")).unwrap();
        assert_eq!((r.base_url.as_str(), r.api_key.as_str(), r.models.as_slice()), ("https://cx.example.com/v1", "sk-x", &["gpt-5.5".to_string()][..]));
        let toml = b64("[model_providers.a]\nbase_url = \"https://t.example.com/v1\"\n");
        let r = parse(&format!("ccswitch://v1/import?resource=provider&app=codex&name=T&apiKey=k&config={toml}&configFormat=toml")).unwrap();
        assert_eq!(r.base_url, "https://t.example.com/v1");
    }

    #[test]
    fn rejects_what_it_cant_import() {
        let err = |l: &str| format!("{:#}", parse(l).unwrap_err());
        assert_eq!(err("https://example.com/v1/import"), "这不是导入链接");
        assert_eq!(err("ccswitch://v2/import?name=a"), "这不是导入链接");
        assert_eq!(err("ccswitch://v1/import?resource=mcp&app=claude&name=a"), "AgentPlus 只导入供应商，这个链接导入的是 mcp");
        assert_eq!(err("ccswitch://v1/import?resource=provider&app=claude&name=a&apiKey=k"), "链接里没有接口地址");
        assert_eq!(err("ccswitch://v1/import?resource=provider&app=claude&name=a&configUrl=https://x.example.com/c.json"), "不支持从网址加载配置的链接");
        assert!(err("ccswitch://v1/import?resource=provider&app=claude&name=a&endpoint=file:///etc/passwd").starts_with("链接里的接口地址不是 http(s) 地址"));
        assert_eq!(err("ccswitch://v1/import?resource=provider&app=claude&name=a&endpoint=https://e.example.com&apiKey=a%20b"), "链接里的 API Key 无效");
        assert_eq!(err("ccswitch://v1/import?resource=provider&app=claude&name=a&endpoint=https://e.example.com&config=%%%"), "读不了链接里的配置");
    }

    #[test]
    fn name_falls_back_to_host_and_is_cleaned() {
        let r = parse("ccswitch://v1/import?resource=provider&app=claude&endpoint=https://relay.example.com:8443/api").unwrap();
        assert_eq!(r.name, "relay.example.com");
        let r = parse("ccswitch://v1/import?resource=provider&app=claude&name=a%0Ab&endpoint=https://r.example.com&homepage=javascript:alert(1)").unwrap();
        assert_eq!((r.name.as_str(), r.homepage), ("ab", None));
    }

    #[test]
    fn spots_links_on_a_command_line() {
        assert!(is_link("agentplus://v1/import?x"));
        assert!(is_link(" CCSwitch://v1/import"));
        assert!(!is_link("--minimized"));
        assert!(!is_link("https://example.com"));
    }

    #[cfg(windows)]
    #[test]
    fn program_of_a_command() {
        assert_eq!(win::program(r#""C:\Program Files\CC Switch\cc-switch.exe" "%1""#), r"C:\Program Files\CC Switch\cc-switch.exe");
        assert_eq!(win::program(r"C:\a\b.exe %1"), r"C:\a\b.exe");
    }
}
