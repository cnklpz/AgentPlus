//! DeepSeek Harness: MCP servers are loader rows of `@deepseek-ai/dsh-mcp-client` (config
//! `serverName`, `transport: stdio | streamable-http`, `command` / `args` / `env` / `cwd` or
//! `url` / `headers`) in the patch layers: the profile AgentPlus edits, then the home layer that
//! outranks it. AgentPlus adds each server as its own `- insert: [row]` entry in the profile
//! layer; an `- id:` override (in either layer) replaces a row's whole config or switches it,
//! and is edited where it is.
//!
//! dsh has no `${VAR}`: variables are `!!js process.env.X` expressions (or a template literal
//! of them). serde_yaml reads those as plain strings; they read as `${X}` here and are written
//! back as `!!js`.

use super::decode::{self, Raw};
use super::write::{encode, onoff, summary};
use super::{from_raw, AgentMcp, Family, McpInput, McpServer};
use crate::adapters::dsh::{self, inserted, row_id, Patch};
use crate::adapters::hermes::js;
use crate::adapters::{msg, Plan};
use crate::i18n::l;
use crate::model::{unique_id, Diff, Op};
use crate::util::display_path;
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde_json::{json, Map, Value as J};
use serde_yaml::{Mapping, Value as Y};
use std::sync::OnceLock;

pub const MODULE: &str = "@deepseek-ai/dsh-mcp-client";
/// The layers, lowest first.
const PROFILE: usize = 0;

fn re(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).unwrap())
}

/// `process.env.X` → `${X}`; a template literal of them (`` `Bearer ${process.env.T}` ``) →
/// `Bearer ${T}`. Anything else stays as it is.
fn from_js(s: &str) -> String {
    static WHOLE: OnceLock<Regex> = OnceLock::new();
    static TPL: OnceLock<Regex> = OnceLock::new();
    static SUB: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = re(&WHOLE, r"^process\.env\.([A-Za-z_][A-Za-z0-9_]*)$").captures(s) {
        return format!("${{{}}}", &c[1]);
    }
    let sub = re(&SUB, r"\$\{process\.env\.([A-Za-z_][A-Za-z0-9_]*)\}");
    if re(&TPL, r"^`[^`\\]*`$").is_match(s) {
        let inner = &s[1..s.len() - 1];
        // Only when every substitution is an environment variable.
        if inner.matches("${").count() == sub.find_iter(inner).count() {
            return sub.replace_all(inner, "$${$1}").into_owned();
        }
    }
    s.to_string()
}

/// Whether serde_yaml's plain string was (most likely) an environment `!!js` expression.
fn is_env_js(s: &str) -> bool {
    from_js(s) != s
}

/// `${X}` → `!!js process.env.X` (a template literal when there is more around it).
fn to_js(s: &str) -> Result<Y> {
    static REF: OnceLock<Regex> = OnceLock::new();
    static WHOLE: OnceLock<Regex> = OnceLock::new();
    if s.contains("${") && s.contains(":-") {
        bail!("{}", tr!("DeepSeek Harness has no defaults for variables ({s})", "DeepSeek Harness 的变量不支持默认值（{s}）"));
    }
    if let Some(c) = re(&WHOLE, r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$").captures(s) {
        return Ok(js(&format!("process.env.{}", &c[1])));
    }
    let r = re(&REF, r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}");
    if !r.is_match(s) {
        return Ok(Y::String(s.into()));
    }
    let lit = s.replace('\\', "\\\\").replace('`', "\\`");
    Ok(js(&format!("`{}`", r.replace_all(&lit, "$${process.env.$1}"))))
}

/// A config as the page reads it: JSON, `!!js` variables as `${X}`.
fn to_page(v: &Y) -> J {
    match v {
        Y::String(s) => J::String(from_js(s)),
        Y::Mapping(m) => J::Object(m.iter().filter_map(|(k, x)| Some((k.as_str()?.to_string(), to_page(x)))).collect()),
        Y::Sequence(s) => J::Array(s.iter().map(to_page).collect()),
        other => serde_json::to_value(other).unwrap_or(J::Null),
    }
}

/// A config to write: `${X}` as `!!js`.
fn from_page(v: &J) -> Result<Y> {
    Ok(match v {
        J::String(s) => to_js(s)?,
        J::Object(m) => Y::Mapping(m.iter().map(|(k, x)| Ok((Y::from(k.as_str()), from_page(x)?))).collect::<Result<Mapping>>()?),
        J::Array(a) => Y::Sequence(a.iter().map(from_page).collect::<Result<_>>()?),
        other => serde_yaml::to_value(other)?,
    })
}

/// An entry read back from a file, with its environment `!!js` scalars tagged again (serde_yaml
/// dropped the tags); set before writing an entry that was read.
fn retag(v: &Y) -> Y {
    match v {
        Y::String(s) if is_env_js(s) => js(s),
        Y::Mapping(m) => Y::Mapping(m.iter().map(|(k, x)| (k.clone(), retag(x))).collect()),
        Y::Sequence(s) => Y::Sequence(s.iter().map(retag).collect()),
        other => other.clone(),
    }
}

/// One MCP row and where its config and switch come from.
#[derive(Clone, Debug)]
struct Row {
    id: String,
    /// (layer, entry, position in the insert list)
    at: (usize, usize, usize),
    /// Effective config (the last override's, else the row's).
    config: Y,
    /// The override whose config wins: (layer, entry).
    config_at: Option<(usize, usize)>,
    disabled: bool,
    /// The last override that switches the row: (layer, entry).
    disabled_at: Option<(usize, usize)>,
}

impl Row {
    fn name(&self) -> String {
        self.config.get("serverName").and_then(Y::as_str).map(String::from).unwrap_or_else(|| self.id.clone())
    }

    /// The config in the common shape (without `serverName`, which is the name).
    fn raw(&self) -> Raw {
        let mut c = to_page(&self.config);
        if let Some(o) = c.as_object_mut() {
            o.remove("serverName");
        }
        decode::decode(Family::Dsh, &c)
    }
}

struct Layers {
    patches: Vec<Patch>,
}

impl Layers {
    fn load() -> Result<Layers> {
        let mut patches = vec![Patch::load(&dsh::patch_path())?, Patch::load(&dsh::home_patch_path())?];
        for p in &mut patches {
            p.allow_env_js();
        }
        Ok(Layers { patches })
    }

    fn rows(&self) -> Vec<Row> {
        let mut out: Vec<Row> = vec![];
        for (l, p) in self.patches.iter().enumerate() {
            for (e, v) in p.entries() {
                for (k, r) in inserted(v).into_iter().flatten().enumerate() {
                    if r.get("name").and_then(Y::as_str) != Some(MODULE) {
                        continue;
                    }
                    if let Some(id) = row_id(r) {
                        let disabled = r.get("disabled").and_then(Y::as_bool).unwrap_or(false);
                        out.push(Row { id: id.into(), at: (l, e, k), config: r.get("config").cloned().unwrap_or(Y::Null), config_at: None, disabled, disabled_at: None });
                    }
                }
            }
        }
        for (l, p) in self.patches.iter().enumerate() {
            for (e, v) in p.entries().filter(|(_, v)| inserted(v).is_none()) {
                let Some(id) = row_id(v) else { continue };
                for r in out.iter_mut().filter(|r| r.id == id) {
                    if let Some(c) = v.get("config") {
                        r.config = c.clone();
                        r.config_at = Some((l, e));
                    }
                    if let Some(d) = v.get("disabled") {
                        r.disabled = d.as_bool().unwrap_or(false);
                        r.disabled_at = Some((l, e));
                    }
                }
            }
        }
        out
    }

    fn find(&self, name: &str) -> Option<Row> {
        self.rows().into_iter().rfind(|r| r.name() == name)
    }

    fn entry(&self, l: usize, e: usize) -> Y {
        self.patches[l].get(e).clone()
    }

    fn set(&mut self, l: usize, e: usize, v: Y) -> Result<()> {
        self.patches[l].set(e, retag(&v))
    }

    /// Puts `f` over the row inside its insert entry.
    fn edit_row(&mut self, r: &Row, f: impl FnOnce(&mut Mapping)) -> Result<()> {
        let (l, e, k) = r.at;
        let mut v = self.entry(l, e);
        let row = v.get_mut("insert").and_then(Y::as_sequence_mut).and_then(|s| s.get_mut(k)).and_then(Y::as_mapping_mut).ok_or_else(|| anyhow!(l_bad(&r.id)))?;
        f(row);
        self.set(l, e, v)
    }

    fn set_config(&mut self, r: &Row, config: Y) -> Result<()> {
        match r.config_at {
            Some((l, e)) => {
                let mut v = self.entry(l, e);
                v.as_mapping_mut().ok_or_else(|| anyhow!(l_bad(&r.id)))?.insert(Y::from("config"), config);
                self.set(l, e, v)
            }
            None => self.edit_row(r, |m| {
                m.insert(Y::from("config"), config);
            }),
        }
    }

    fn set_disabled(&mut self, r: &Row, off: bool) -> Result<()> {
        match r.disabled_at {
            Some((l, e)) => {
                let mut v = self.entry(l, e);
                v.as_mapping_mut().ok_or_else(|| anyhow!(l_bad(&r.id)))?.insert(Y::from("disabled"), Y::Bool(off));
                self.set(l, e, v)
            }
            None => self.edit_row(r, |m| {
                if off {
                    m.insert(Y::from("disabled"), Y::Bool(true));
                } else {
                    m.remove("disabled");
                }
            }),
        }
    }

    /// Takes the row out of its insert entry (the whole entry when it was the only row), and
    /// drops the overrides that point at it.
    fn remove(&mut self, r: &Row) -> Result<()> {
        let (l, e, k) = r.at;
        let mut v = self.entry(l, e);
        let rows = v.get_mut("insert").and_then(Y::as_sequence_mut).ok_or_else(|| anyhow!(l_bad(&r.id)))?;
        rows.remove(k);
        if rows.is_empty() && v.as_mapping().is_some_and(|m| m.len() == 1) {
            self.patches[l].remove(e)?;
        } else {
            self.set(l, e, v)?;
        }
        for p in &mut self.patches {
            let hits: Vec<usize> = p.entries().filter(|(_, v)| inserted(v).is_none() && row_id(v) == Some(&r.id)).map(|(i, _)| i).collect();
            for i in hits {
                p.remove(i)?;
            }
        }
        Ok(())
    }

    fn taken(&self, id: &str) -> bool {
        self.patches.iter().any(|p| p.entries().any(|(_, v)| row_id(v) == Some(id) || inserted(v).is_some_and(|rows| rows.iter().any(|r| row_id(r) == Some(id)))))
    }
}

fn l_bad(id: &str) -> String {
    tr!("The {id} entry in the dsh patch file isn't laid out as expected", "dsh 补丁文件里的 {id} 条目格式不符合预期")
}

fn server(r: &Row) -> McpServer {
    from_raw(&r.name(), r.raw(), !r.disabled, false)
}

pub fn read() -> AgentMcp {
    let path = dsh::patch_path();
    let base = AgentMcp { agent: dsh::ID.into(), supported: true, file: Some(display_path(&path)), exists: path.exists(), servers: vec![], error: None };
    match Layers::load() {
        Ok(ls) => AgentMcp { servers: ls.rows().iter().map(server).collect(), ..base },
        Err(e) => AgentMcp { error: Some(e.to_string()), ..base },
    }
}

/// A server with its real values, for copying it elsewhere.
pub fn raw(name: &str) -> Result<Raw> {
    Layers::load()?.find(name).map(|r| r.raw()).ok_or_else(|| anyhow!(tr!("DeepSeek Harness has no MCP server named \"{name}\"", "DeepSeek Harness 没有名为「{name}」的 MCP 服务器")))
}

/// dsh's `serverName`: `[A-Za-z0-9_-]{1,32}`.
fn check_name(name: &str) -> Result<()> {
    static R: OnceLock<Regex> = OnceLock::new();
    if !re(&R, r"^[A-Za-z0-9_-]{1,32}$").is_match(name) {
        bail!("{}", l("DeepSeek Harness server names use only letters, digits, - and _ (up to 32)", "DeepSeek Harness 的服务器名称只能用字母、数字、- 和 _（最多 32 个）"));
    }
    Ok(())
}

/// The config dsh gets: `serverName` first, then what `encode` writes.
fn config_for(name: &str, i: &McpInput, base: Option<&Row>) -> Result<J> {
    let base = base.map(|r| {
        let mut c = to_page(&r.config);
        if let Some(o) = c.as_object_mut() {
            o.remove("serverName");
        }
        c
    });
    let def = encode(dsh::ID, Family::Dsh, i, base.as_ref().and_then(J::as_object), true)?;
    let mut out = Map::new();
    out.insert("serverName".into(), json!(name));
    out.extend(def.as_object().cloned().unwrap_or_default());
    Ok(J::Object(out))
}

fn apply(ls: &mut Layers, op: &Op, diff: &mut Diff, file: &str) -> Result<()> {
    match op {
        Op::UpsertMcp { server: i } => {
            let name = i.name.trim();
            if name.is_empty() {
                return Err(msg::name_required());
            }
            check_name(name)?;
            let old = i.replaces.as_deref().map(str::trim).filter(|r| *r != name).and_then(|r| ls.find(r));
            if old.is_some() && ls.find(name).is_some() {
                bail!("{}", tr!("DeepSeek Harness already has an MCP server named \"{name}\"", "DeepSeek Harness 已经有名为「{name}」的 MCP 服务器"));
            }
            let row = old.clone().or_else(|| ls.find(name));
            let cfg = config_for(name, i, row.as_ref())?;
            let what = summary(Family::Dsh, &cfg);
            match row {
                None => {
                    let id = unique_id(&format!("mcp-{}", name.to_lowercase()), |c| ls.taken(c));
                    let mut r = Mapping::new();
                    r.insert(Y::from("id"), Y::from(id));
                    r.insert(Y::from("name"), Y::from(MODULE));
                    r.insert(Y::from("config"), from_page(&cfg)?);
                    if !i.enabled {
                        r.insert(Y::from("disabled"), Y::Bool(true));
                    }
                    let mut entry = Mapping::new();
                    entry.insert(Y::from("insert"), Y::Sequence(vec![Y::Mapping(r)]));
                    ls.patches[PROFILE].push(Y::Mapping(entry))?;
                    diff.push(file, tr!("+ MCP \"{name}\": {what}", "+ MCP「{name}」：{what}"), true);
                    if !i.enabled {
                        diff.push(file, onoff(name, false), false);
                    }
                }
                Some(r) => {
                    if let Some(o) = &old {
                        diff.push(file, tr!("MCP \"{}\" renamed to \"{name}\"", "MCP「{}」改名为「{name}」", o.name()), true);
                    }
                    if to_page(&r.config) != cfg {
                        ls.set_config(&r, from_page(&cfg)?)?;
                        if old.is_none() {
                            diff.push(file, tr!("MCP \"{name}\" updated: {what}", "MCP「{name}」修改为：{what}"), true);
                        }
                    }
                    if r.disabled == i.enabled {
                        // Re-read: the config edit may have rewritten the same entry.
                        let r = ls.rows().into_iter().find(|x| x.id == r.id).unwrap_or(r);
                        ls.set_disabled(&r, !i.enabled)?;
                        diff.push(file, onoff(name, i.enabled), i.enabled);
                    }
                }
            }
            Ok(())
        }
        Op::DeleteMcp { name } => {
            let r = ls.find(name).ok_or_else(|| anyhow!(tr!("No MCP server named \"{name}\"", "没有名为「{name}」的 MCP 服务器")))?;
            ls.remove(&r)?;
            diff.push(file, tr!("− MCP \"{name}\"", "− MCP「{name}」"), false);
            Ok(())
        }
        Op::SetMcpEnabled { name, enabled } => {
            let r = ls.find(name).ok_or_else(|| anyhow!(tr!("No MCP server named \"{name}\"", "没有名为「{name}」的 MCP 服务器")))?;
            if r.disabled != *enabled {
                return Ok(());
            }
            ls.set_disabled(&r, !enabled)?;
            diff.push(file, onoff(name, *enabled), *enabled);
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let mut ls = Layers::load()?;
    let mut diff = Diff::default();
    let file = ls.patches[PROFILE].file();
    for op in ops {
        apply(&mut ls, op, &mut diff, &file)?;
    }
    let changed: Vec<usize> = (0..ls.patches.len()).filter(|&l| ls.patches[l].changed()).collect();
    // Rendered even for a preview: it checks the result parses back to the same entries.
    for &l in &changed {
        ls.patches[l].render()?;
    }
    if dry_run || changed.is_empty() {
        return Ok((diff, changed.iter().map(|&l| ls.patches[l].path.clone()).collect(), None));
    }
    let files: Vec<_> = changed.iter().map(|&l| ls.patches[l].path.clone()).collect();
    let existing: Vec<_> = files.iter().filter(|p| p.exists()).cloned().collect();
    let backup = if existing.is_empty() { None } else { Some(crate::util::backup(dsh::ID, &existing)?) };
    for &l in &changed {
        ls.patches[l].save()?;
    }
    Ok((diff, files, backup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::hermes::js_expr;
    use crate::util::TestHome;
    use std::fs;

    const PATCH: &str = "# Your patch layer for this dsh profile\n- id: ui-settings-general\n  name: \"@deepseek-ai/dsh-client-ui-settings-general\"\n  config:\n    welcomeNoticeVersion: 2026-08-13.1\n";

    fn setup(tag: &str, patch: &str) -> TestHome {
        let h = TestHome::new(tag);
        let p = h.0.join(".dsh/profiles/web");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("package.json"), "{}").unwrap();
        fs::write(p.join("cordis.patch.yml"), patch).unwrap();
        h
    }

    fn text(h: &TestHome) -> String {
        fs::read_to_string(h.0.join(".dsh/profiles/web/cordis.patch.yml")).unwrap()
    }

    fn run(ops: Vec<J>) -> Result<Vec<String>> {
        let ops: Vec<Op> = ops.into_iter().map(|v| serde_json::from_value(v).unwrap()).collect();
        let ops = crate::adapters::resolve(dsh::ID, &ops)?;
        let (d, _, _) = crate::store::transaction(|| crate::adapters::plan_resolved(dsh::ID, &ops, false))?;
        Ok(d.groups.into_iter().flat_map(|g| g.lines).map(|l| l.text).collect())
    }

    fn upsert(v: J) -> J {
        json!({ "op": "upsert_mcp", "server": v })
    }

    #[test]
    fn variables_convert_both_ways() {
        assert_eq!(from_js("process.env.GH"), "${GH}");
        assert_eq!(from_js("`Bearer ${process.env.T}`"), "Bearer ${T}");
        assert_eq!(from_js("`${ctx.x}`"), "`${ctx.x}`");
        assert_eq!(from_js("plain"), "plain");
        assert_eq!(js_expr(&to_js("${GH}").unwrap()), Some("process.env.GH"));
        assert_eq!(js_expr(&to_js("Bearer ${T}").unwrap()), Some("`Bearer ${process.env.T}`"));
        assert_eq!(to_js("plain").unwrap(), Y::from("plain"));
        assert!(to_js("${A:-x}").is_err());
    }

    #[test]
    fn servers_are_added_edited_switched_and_removed() {
        let h = setup("mcp-dsh", PATCH);
        run(vec![upsert(json!({ "name": "github", "transport": "stdio", "command": "npx", "args": ["-y", "@m/gh"], "env": [{ "key": "GITHUB_TOKEN", "value": "${GITHUB_TOKEN}" }] }))]).unwrap();
        let t = text(&h);
        assert!(t.starts_with(PATCH), "{t}");
        assert!(t.contains("- insert:\n    - id: mcp-github\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: github\n        transport: stdio\n"), "{t}");
        assert!(t.contains("GITHUB_TOKEN: !!js process.env.GITHUB_TOKEN"), "{t}");
        let s = &read().servers[0];
        assert_eq!((s.name.as_str(), s.transport.as_str(), s.enabled), ("github", "stdio", true));
        assert_eq!(s.env[0].value, "${GITHUB_TOKEN}");

        // Off and on again, then an edit: the !!js variable survives the rewrite.
        assert_eq!(run(vec![json!({ "op": "set_mcp_enabled", "name": "github", "enabled": false })]).unwrap(), ["MCP「github」→ 停用"]);
        assert!(text(&h).contains("disabled: true") && !read().servers[0].enabled);
        run(vec![json!({ "op": "set_mcp_enabled", "name": "github", "enabled": true })]).unwrap();
        assert!(!text(&h).contains("disabled") && read().servers[0].enabled);
        run(vec![upsert(json!({ "name": "github", "transport": "stdio", "command": "uvx", "args": ["gh"], "env": [{ "key": "GITHUB_TOKEN", "value": "${GITHUB_TOKEN}" }], "from": ["dsh", "github"] }))]).unwrap();
        let t = text(&h);
        assert!(t.contains("command: uvx") && t.contains("GITHUB_TOKEN: !!js process.env.GITHUB_TOKEN"), "{t}");

        run(vec![json!({ "op": "delete_mcp", "name": "github" })]).unwrap();
        assert_eq!(text(&h), PATCH);
    }

    #[test]
    fn overrides_and_the_home_layer_are_edited_where_they_are() {
        let h = setup("mcp-dsh-layers", &format!("{PATCH}- insert:\n    - id: mcp-web\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: web\n        transport: streamable-http\n        url: http://localhost:3000/mcp\n        headers:\n          Authorization: !!js '`Bearer ${{process.env.MCP_TOKEN}}`'\n"));
        fs::write(h.0.join(".dsh/cordis.patch.yml"), "- id: mcp-web\n  disabled: true\n").unwrap();
        let s = &read().servers[0];
        assert_eq!((s.transport.as_str(), s.enabled), ("http", false));
        assert_eq!(s.headers[0].value, "Bearer ${MCP_TOKEN}");
        run(vec![json!({ "op": "set_mcp_enabled", "name": "web", "enabled": true })]).unwrap();
        // The home layer holds the switch, so it's flipped there.
        assert_eq!(fs::read_to_string(h.0.join(".dsh/cordis.patch.yml")).unwrap(), "- id: mcp-web\n  disabled: false\n");
        assert!(read().servers[0].enabled);
        // The header template was never touched.
        assert!(text(&h).contains("!!js '`Bearer ${process.env.MCP_TOKEN}`'"));
    }

    #[test]
    fn limits() {
        let _h = setup("mcp-dsh-limits", PATCH);
        let sse = json!({ "name": "s", "transport": "sse", "url": "https://h/sse" });
        assert_eq!(run(vec![upsert(sse)]).unwrap_err().to_string(), "DeepSeek Harness 不支持 SSE 类型的 MCP 服务器");
        let bad = json!({ "name": "has space", "transport": "stdio", "command": "x" });
        assert!(run(vec![upsert(bad)]).unwrap_err().to_string().contains("字母、数字"));
        // Copies from dsh bring the real values.
        run(vec![upsert(json!({ "name": "k", "transport": "stdio", "command": "x", "env": [{ "key": "API_KEY", "value": "abcdefgh12345678" }] }))]).unwrap();
        assert_eq!(read().servers[0].env[0].value, "••••5678");
        assert_eq!(raw("k").unwrap().env[0].1, "abcdefgh12345678");
    }
}
