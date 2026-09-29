//! The AgentPlus MCP library: server definitions kept by AgentPlus itself
//! (`~/.agentplus/store.json` → "mcp"), independent of any agent and of the Windows / WSL
//! target, to copy into agents. Values are stored as given; the UI only sees them masked.

use super::decode::{self, Raw};
use super::{from_raw, Family, McpInput, McpServer};
use crate::adapters::msg;
use crate::i18n::l;
use crate::store;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

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
}
