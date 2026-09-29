//! The AgentPlus MCP library: server definitions kept by AgentPlus itself
//! (`~/.agentplus/store.json` → "mcp"), independent of any agent and of the Windows / WSL
//! target, to copy into agents. Values are stored as given; the UI only sees them masked.

use super::decode::{self, Raw};
use super::{from_raw, mask, Family, McpInput, McpServer};
use crate::adapters::msg;
use crate::i18n::l;
use crate::store;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

const KEY: &str = "mcp";

fn entries(root: &Value) -> Vec<McpInput> {
    root.get(KEY).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()).unwrap_or_default()
}

/// An entry in the common shape.
pub(super) fn raw_of(i: &McpInput) -> Raw {
    let pairs = |kv: &[super::KvIn]| kv.iter().map(|p| (p.key.clone(), p.value.clone())).collect();
    Raw {
        transport: decode::transport(&i.transport).unwrap_or("stdio"),
        command: i.command.clone(),
        args: i.args.clone(),
        cwd: i.cwd.clone(),
        url: i.url.clone(),
        env: pairs(&i.env),
        headers: pairs(&i.headers),
        off: false,
        extra: i.extra.clone(),
    }
}

/// The library, secrets masked.
pub fn list() -> Vec<McpServer> {
    entries(&store::load()).iter().map(|i| from_raw(&i.name, raw_of(i), true, false)).collect()
}

/// An entry with its real values, and the format its extra fields belong to.
pub fn raw(name: &str) -> Result<(Raw, Option<Family>)> {
    let i = entries(&store::load()).into_iter().find(|i| i.name == name).ok_or_else(|| anyhow!(tr!("No MCP server named \"{name}\" in the library", "MCP 库里没有「{name}」")))?;
    Ok((raw_of(&i), i.extra_family))
}

/// Adds or replaces an entry (`replaces` renames one); masked values are taken from `from`.
pub fn save(input: McpInput) -> Result<()> {
    // Resolved before taking the store lock: it reads agent configs and the store.
    let mut i = super::write::resolve(input)?;
    i.name = i.name.trim().to_string();
    if i.name.is_empty() {
        return Err(msg::name_required());
    }
    let t = decode::transport(&i.transport).ok_or_else(|| anyhow!(tr!("Unknown transport {}", "未知传输方式 {}", i.transport)))?;
    let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    if t == "stdio" && blank(&i.command) {
        bail!("{}", l("A stdio server needs a command", "stdio 服务器需要填写命令"));
    }
    if t != "stdio" && blank(&i.url) {
        bail!("{}", l("A remote server needs a URL", "远程服务器需要填写地址"));
    }
    i.transport = t.into();
    i.enabled = true;
    let old = i.replaces.take().filter(|r| *r != i.name);
    store::update(|root| {
        let mut list = entries(root);
        if old.is_some() && list.iter().any(|e| e.name == i.name) {
            bail!("{}", tr!("The library already has an MCP server named \"{}\"", "MCP 库里已经有名为「{}」的服务器", i.name));
        }
        let at = list.iter().position(|e| Some(&e.name) == old.as_ref() || e.name == i.name);
        match at {
            Some(n) => list[n] = i.clone(),
            None => list.push(i.clone()),
        }
        root[KEY] = serde_json::to_value(list)?;
        Ok(())
    })?;
    crate::sync::changed();
    Ok(())
}

pub fn delete(name: &str) -> Result<()> {
    store::update(|root| {
        let list: Vec<McpInput> = entries(root).into_iter().filter(|e| e.name != name).collect();
        root[KEY] = serde_json::to_value(list)?;
        Ok(())
    })?;
    crate::sync::changed();
    Ok(())
}

// ---------------------------------------------------------------- sync

/// The library for the sync file. Secrets (the values the page masks, and `extra` when it
/// holds one) are replaced by `fp(value)`, which returns the value's fingerprint and keeps the
/// value in the file's keys when those are exported.
pub fn export(root: &Value, fp: &mut dyn FnMut(&str) -> String) -> Vec<Value> {
    entries(root)
        .iter()
        .map(|i| {
            let kv = |list: &[super::KvIn], fp: &mut dyn FnMut(&str) -> String| -> Vec<Value> {
                list.iter().map(|p| if mask::value(&p.key, &p.value).1 { json!({ "key": p.key, "fp": fp(&p.value) }) } else { json!({ "key": p.key, "value": p.value }) }).collect()
            };
            let shown = mask::args(&i.args);
            let args: Vec<Value> = i.args.iter().zip(&shown).map(|(a, s)| if a != s { json!({ "fp": fp(a) }) } else { json!(a) }).collect();
            let mut v = json!({ "name": i.name, "transport": i.transport, "command": i.command, "args": args, "cwd": i.cwd, "env": kv(&i.env, fp), "headers": kv(&i.headers, fp) });
            if let Some(u) = &i.url {
                let masked = mask::url(u);
                // Not even the last characters the page shows: the file may be read by others,
                // and part of a short password narrows it down.
                static HINT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                v["url"] = json!(HINT.get_or_init(|| regex::Regex::new("••••[^&@/#:]*").unwrap()).replace_all(&masked, "••••"));
                if masked != *u {
                    v["urlFp"] = json!(fp(u));
                }
            }
            if !i.extra.is_empty() {
                if mask::extra(&i.extra) == i.extra {
                    v["extra"] = Value::Object(i.extra.clone());
                } else {
                    v["extraFp"] = json!(fp(&Value::Object(i.extra.clone()).to_string()));
                }
                v["extraFamily"] = json!(i.extra_family);
            }
            v
        })
        .collect()
}

/// An exported entry read back: values looked up by `key(fp)`. Returns the entry and how
/// many values the file doesn't hold (left empty).
pub fn import(v: &Value, key: &dyn Fn(&str) -> Option<String>) -> (McpInput, usize) {
    let mut missing = 0;
    let mut get = |fp: Option<&str>| match fp.and_then(key) {
        Some(k) => k,
        None => {
            missing += 1;
            String::new()
        }
    };
    let s = |x: &Value| x.as_str().map(String::from);
    let mut kv = |list: &Value| -> Vec<super::KvIn> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|p| super::KvIn { key: s(&p["key"]).unwrap_or_default(), value: if p.get("fp").is_some() { get(p["fp"].as_str()) } else { s(&p["value"]).unwrap_or_default() } })
            .collect()
    };
    let (env, headers) = (kv(&v["env"]), kv(&v["headers"]));
    let args = v["args"].as_array().into_iter().flatten().map(|a| if a.is_object() { get(a["fp"].as_str()) } else { s(a).unwrap_or_default() }).collect();
    let url = if v.get("urlFp").is_some() { Some(get(v["urlFp"].as_str())) } else { s(&v["url"]) };
    let extra = match v.get("extraFp") {
        Some(f) => serde_json::from_str::<Value>(&get(f.as_str())).ok().and_then(|x| x.as_object().cloned()).unwrap_or_default(),
        None => v["extra"].as_object().cloned().unwrap_or_default(),
    };
    let i = McpInput {
        name: s(&v["name"]).unwrap_or_default(),
        transport: s(&v["transport"]).unwrap_or_else(|| "stdio".into()),
        command: s(&v["command"]),
        args,
        cwd: s(&v["cwd"]),
        url,
        env,
        headers,
        enabled: true,
        extra,
        extra_family: serde_json::from_value(v["extraFamily"].clone()).ok(),
        ..Default::default()
    };
    (i, missing)
}

/// What runs, for a one-line description (secrets masked).
pub fn summary(i: &McpInput) -> String {
    let s = from_raw(&i.name, raw_of(i), true, false);
    match &s.url {
        Some(u) if s.transport != "stdio" => format!("{} {u}", super::write::transport_label(&s.transport)),
        _ => std::iter::once(s.command.unwrap_or_default()).chain(s.args).collect::<Vec<_>>().join(" "),
    }
}

/// What runs, comparable with another entry's (see `sig`).
pub fn sig_of(i: &McpInput) -> String {
    from_raw(&i.name, raw_of(i), true, false).sig
}

/// The library's entries with their real values (for comparing with a sync file).
pub fn all() -> Vec<McpInput> {
    entries(&store::load())
}

/// Adds or replaces entries as given (from a sync file: no masked values, no rename).
pub fn put(list: Vec<McpInput>) -> Result<()> {
    store::update(|root| {
        let mut cur = entries(root);
        for i in list {
            match cur.iter().position(|e| e.name == i.name) {
                Some(n) => cur[n] = i,
                None => cur.push(i),
            }
        }
        root[KEY] = serde_json::to_value(cur)?;
        Ok(())
    })?;
    crate::sync::changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use serde_json::json;

    fn input(v: Value) -> McpInput {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn saving_masks_renaming_and_deleting() {
        let _h = TestHome::new("mcp-library");
        save(input(json!({ "name": "gh", "transport": "http", "url": "https://h/mcp", "headers": [{ "key": "Authorization", "value": "Bearer sk-live-abcdefgh9876" }] }))).unwrap();
        let shown = list();
        assert_eq!(shown[0].headers[0].value, "Bearer ••••9876");
        // Edited from what the page showed: the real header stays.
        save(input(json!({ "name": "github", "replaces": "gh", "transport": "http", "url": "https://h/v2", "headers": [{ "key": "Authorization", "value": "Bearer ••••9876" }], "from": ["library", "gh"] }))).unwrap();
        let (r, _) = raw("github").unwrap();
        assert_eq!(r.headers[0].1, "Bearer sk-live-abcdefgh9876");
        assert_eq!(r.url.as_deref(), Some("https://h/v2"));
        assert_eq!(list().len(), 1);
        assert!(save(input(json!({ "name": "x", "transport": "stdio" }))).is_err());
        delete("github").unwrap();
        assert!(list().is_empty());
    }

    #[test]
    fn sync_export_keeps_secrets_behind_fingerprints() {
        let _h = TestHome::new("mcp-library-sync");
        save(input(json!({
            "name": "k", "transport": "stdio", "command": "npx", "args": ["srv", "--api-key", "sk-secret-0123456789"],
            "env": [{ "key": "API_KEY", "value": "abcdefgh12345678" }, { "key": "MODE", "value": "ro" }, { "key": "T", "value": "${T}" }],
        }))).unwrap();
        save(input(json!({ "name": "u", "transport": "http", "url": "https://h/mcp?token=abcdef123456" }))).unwrap();
        let mut keys = std::collections::BTreeMap::new();
        let out = export(&store::load(), &mut |v| {
            let fp = crate::model::key_fingerprint(v);
            keys.insert(fp.clone(), v.to_string());
            fp
        });
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains("sk-secret") && !text.contains("abcdefgh1234") && !text.contains("abcdef123456"), "{text}");
        assert!(text.contains("\"MODE\"") && text.contains("${T}"), "{text}");
        assert_eq!(keys.len(), 3);
        // Read back with every key, then with none.
        let all_keys = |fp: &str| keys.get(fp).cloned();
        let (back, missing) = import(&out[0], &all_keys);
        assert_eq!((missing, back.args[2].as_str(), back.env[0].value.as_str()), (0, "sk-secret-0123456789", "abcdefgh12345678"));
        assert_eq!(sig_of(&back), sig_of(&all()[0]));
        let (u, _) = import(&out[1], &all_keys);
        assert_eq!(u.url.as_deref(), Some("https://h/mcp?token=abcdef123456"));
        let (none, missing) = import(&out[0], &|_| None);
        assert_eq!((missing, none.env[0].value.as_str(), none.env[1].value.as_str()), (2, "", "ro"));
    }
}
