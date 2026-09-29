//! Installed plugins (extensions, bundles) of the agents that have them, and switching one on
//! or off. Installing and removing stay in each agent (they fetch from marketplaces and run
//! install steps); AgentPlus lists what is installed and flips the agent's own switch, through
//! the usual pending changes.
//!
//! - Claude Code: install records in `plugins/installed_plugins.json`, the switch in
//!   `settings.json` → `enabledPlugins { "name@marketplace": bool }`. CodeBuddy and Droid keep
//!   only that map (plugins load in place); ZCode keeps it in `cli/config.json` under
//!   `plugins`, with records in `cli/plugins/installed_plugins.json`.
//! - Codex: `[plugins."name@marketplace"] enabled` in config.toml; manifests in
//!   `plugins/cache/<marketplace>/<plugin>/<version>/.codex-plugin/plugin.json`.
//! - Gemini CLI / Qwen Code: extensions in `extensions/<name>/`; the user-level switch is an
//!   override `"/<home>/*"` (on) or `"!/<home>/*"` (off) in `extension-enablement.json`.
//! - Hermes: `plugins/<name>/plugin.yaml`, opt-in through `plugins.enabled` / `disabled`.
//! - OpenClaw: `plugins.entries.<id>.enabled` in openclaw.json.
//! - OpenCode / Kilo: the `plugin` array of npm specs has no switch: switching one off takes it
//!   out and keeps it in the AgentPlus store until it is switched back on.
//! - DeepSeek Harness: bundles, written by its adapter (a setting; see `adapters::dsh`).

use crate::adapters::{claude, codebuddy, codex, droid, dsh, gemini, hermes, kilo, opencode, openclaw, qwen, zcode, Plan};
use crate::i18n::l;
use crate::model::{Diff, Op, PluginInfo};
use crate::store;
use crate::util::*;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Store key (per agent) of OpenCode-style plugin specs switched off by AgentPlus.
const OFF_KEY: &str = "pluginsOff";

fn json_at(path: &Path) -> Value {
    read_text(path).ok().and_then(|(t, _)| serde_json::from_str(&strip_jsonc(&t).0).ok().or_else(|| json5::from_str(&t).ok())).unwrap_or(Value::Null)
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// A description in the UI language, from manifests that carry both.
fn described(m: &Value) -> Option<String> {
    let zh = !crate::i18n::is_en();
    let localized = if zh { text(&m["description_i18n"]["zh-CN"]).or_else(|| text(&m["description"])) } else { text(&m["description_i18n"]["en"]).or_else(|| text(&m["description_en"])) };
    localized.or_else(|| text(&m["description"])).map(|d| clip(&d, 300))
}

/// The newest version folder of `<dir>` (a `latest` link wins).
fn newest(dir: &Path) -> Option<PathBuf> {
    if dir.join("latest").is_dir() {
        return Some(dir.join("latest"));
    }
    let mut vs: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    vs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().split(['.', '-']).map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>()).unwrap_or_default());
    vs.pop()
}

/// `name@marketplace` split.
fn split_id(id: &str) -> (&str, Option<&str>) {
    match id.rsplit_once('@') {
        Some((n, m)) if !n.is_empty() => (n, Some(m)),
        _ => (id, None),
    }
}

/// A plugin from a manifest (`.claude-plugin/plugin.json` and friends), falling back to the id.
fn from_manifest(id: &str, manifest: &Value, enabled: bool) -> PluginInfo {
    let (name, source) = split_id(id);
    let display = text(&manifest["interface"]["displayName"]).or_else(|| text(&manifest["displayName"])).or_else(|| text(&manifest["name"])).unwrap_or_else(|| name.to_string());
    PluginInfo {
        id: id.into(),
        name: display,
        description: text(&manifest["interface"]["shortDescription"]).or_else(|| described(manifest)),
        version: text(&manifest["version"]),
        source: source.map(String::from),
        enabled,
        locked: None,
    }
}

// ---------------------------------------------------------------- where things are

fn claude_settings(agent: &str) -> PathBuf {
    match agent {
        codebuddy::ID => codebuddy::settings_path(),
        droid::ID => droid::settings_path(),
        _ => claude::settings_path(),
    }
}

fn zcode_cli() -> PathBuf {
    let d = zcode::dir();
    let home = if d.file_name().is_some_and(|n| n == "v2") { d.parent().map(Path::to_path_buf).unwrap_or(d) } else { d };
    home.join("cli")
}

fn codex_home() -> PathBuf {
    codex::config_path().parent().map(Path::to_path_buf).unwrap_or_default()
}

fn ext_dir(agent: &str) -> PathBuf {
    if agent == qwen::ID { qwen::dir().join("extensions") } else { gemini::dir().join("extensions") }
}

/// The OpenCode-format config with the `plugin` array.
fn oc_config(agent: &str) -> PathBuf {
    if agent == kilo::ID { kilo::config_path() } else { opencode::config_path() }
}

// ---------------------------------------------------------------- Gemini overrides

/// Gemini's rule form of a folder: forward slashes, a leading and trailing `/`.
fn rule_base(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Whether `rule` (without `!`) covers folder `base` (as `rule_base`): `<x>/*` covers `x`
/// and everything under it, a rule without `*` only that folder.
fn rule_covers(rule: &str, base: &str) -> bool {
    match rule.strip_suffix('*') {
        Some(prefix) => base.starts_with(prefix) || base.trim_end_matches('/') == prefix.trim_end_matches('/'),
        None => rule == base,
    }
}

/// Enabled at user level: the last override that covers the home folder decides; none = on.
fn ext_enabled(overrides: &[String], home: &str) -> bool {
    let mut on = true;
    for r in overrides {
        let (off, rule) = match r.strip_prefix('!') {
            Some(x) => (true, x),
            None => (false, r.as_str()),
        };
        if rule_covers(rule, home) {
            on = !off;
        }
    }
    on
}

/// Gemini's own `enable` / `disable` at user scope: drops the rules for the home folder and
/// those under it, then adds `[!]/<home>/*`.
fn set_ext(overrides: &mut Vec<String>, home: &str, on: bool) {
    overrides.retain(|r| {
        let rule = r.trim_start_matches('!');
        let base = rule.trim_end_matches('*');
        !(base == home || base.starts_with(home))
    });
    overrides.push(format!("{}{home}*", if on { "" } else { "!" }));
}

// ---------------------------------------------------------------- listing

fn claude_style(agent: &str) -> Vec<PluginInfo> {
    let settings = json_at(&claude_settings(agent));
    let switches = settings["enabledPlugins"].as_object().cloned().unwrap_or_default();
    let mut ids: Vec<String> = vec![];
    let mut manifests: Vec<(String, Value)> = vec![];
    if agent == claude::ID {
        // installed_plugins.json: { plugins: { id: [ { installPath, version, … } ] } } (or the map itself).
        let rec = json_at(&claude::dir().join("plugins").join("installed_plugins.json"));
        let map = rec["plugins"].as_object().or_else(|| rec.as_object()).cloned().unwrap_or_default();
        for (id, v) in map.iter().filter(|(k, _)| k.contains('@')) {
            let first = if v.is_array() { v[0].clone() } else { v.clone() };
            let path = text(&first["installPath"]).map(PathBuf::from);
            let m = path.map(|p| json_at(&p.join(".claude-plugin").join("plugin.json"))).unwrap_or(Value::Null);
            let m = if m.is_null() { json!({ "version": first["version"] }) } else { m };
            ids.push(id.clone());
            manifests.push((id.clone(), m));
        }
    }
    for id in switches.keys() {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids.iter()
        .map(|id| {
            let m = manifests.iter().find(|(k, _)| k == id).map(|(_, m)| m.clone()).unwrap_or_else(|| {
                let (name, mkt) = split_id(id);
                match (agent, mkt) {
                    (codebuddy::ID, Some(mkt)) => json_at(&codebuddy::dir().join("plugins").join("marketplaces").join(mkt).join("plugins").join(name).join(".codebuddy-plugin").join("plugin.json")),
                    _ => Value::Null,
                }
            });
            // Claude Code: a plugin without a switch follows its manifest's default (on).
            let on = switches.get(id).and_then(Value::as_bool).unwrap_or_else(|| m["defaultEnabled"].as_bool().unwrap_or(true));
            from_manifest(id, &m, on)
        })
        .collect()
}

fn zcode_list() -> Vec<PluginInfo> {
    let cli = zcode_cli();
    let switches = json_at(&cli.join("config.json")).pointer("/plugins/enabledPlugins").and_then(Value::as_object).cloned().unwrap_or_default();
    let records = json_at(&cli.join("plugins").join("installed_plugins.json"));
    let mut ids: Vec<String> = records["plugins"].as_array().into_iter().flatten().filter_map(|r| text(&r["id"])).collect();
    ids.extend(switches.keys().filter(|k| !ids.contains(k)).cloned().collect::<Vec<_>>());
    ids.iter()
        .map(|id| {
            let (name, mkt) = split_id(id);
            let m = mkt.and_then(|mkt| newest(&cli.join("plugins").join("cache").join(mkt).join(name))).map(|d| json_at(&d.join(".zcode-plugin").join("plugin.json"))).unwrap_or(Value::Null);
            from_manifest(id, &m, switches.get(id).and_then(Value::as_bool).unwrap_or(true))
        })
        .collect()
}

fn codex_list() -> Vec<PluginInfo> {
    let cfg = read_text(&codex::config_path()).ok().and_then(|(t, _)| t.parse::<toml_edit::DocumentMut>().ok()).map(|d| crate::mcp::toml_item(d.as_item())).unwrap_or(Value::Null);
    let cache = codex_home().join("plugins").join("cache");
    let manifest = |id: &str| {
        let (name, mkt) = split_id(id);
        mkt.and_then(|m| newest(&cache.join(m).join(name))).map(|d| json_at(&d.join(".codex-plugin").join("plugin.json"))).unwrap_or(Value::Null)
    };
    let table = cfg["plugins"].as_object().cloned().unwrap_or_default();
    let mut out: Vec<PluginInfo> = table.iter().map(|(id, t)| from_manifest(id, &manifest(id), t["enabled"].as_bool().unwrap_or(true))).collect();
    // Plugins Codex installed from its remote catalog have no config entry: Codex manages them.
    for e in fs::read_dir(cache.join("openai-curated-remote")).into_iter().flatten().flatten() {
        let id = format!("{}@openai-curated-remote", e.file_name().to_string_lossy());
        if !out.iter().any(|p| p.id == id) && e.path().is_dir() {
            let mut p = from_manifest(&id, &manifest(&id), true);
            p.locked = Some(l("Installed from Codex's online catalog; manage it in Codex", "从 Codex 在线目录安装，请在 Codex 里管理").into());
            out.push(p);
        }
    }
    out
}

fn extensions(agent: &str) -> Vec<PluginInfo> {
    let dir = ext_dir(agent);
    let file = if agent == qwen::ID { "qwen-extension.json" } else { "gemini-extension.json" };
    let enablement = json_at(&dir.join("extension-enablement.json"));
    let home = rule_base(&home());
    let mut out = vec![];
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let m = json_at(&e.path().join(file));
        if m.is_null() {
            continue;
        }
        let name = text(&m["name"]).unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
        let rules: Vec<String> = enablement[&name]["overrides"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
        let mut p = from_manifest(&name, &m, ext_enabled(&rules, &home));
        p.source = None;
        out.push(p);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn hermes_list() -> Vec<PluginInfo> {
    let cfg = read_text(&hermes::config_path()).ok().and_then(|(t, _)| serde_yaml::from_str::<serde_yaml::Value>(&t).ok()).and_then(|y| serde_json::to_value(y).ok()).unwrap_or(Value::Null);
    let list = |k: &str| cfg["plugins"][k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>();
    let (on, off) = (list("enabled"), list("disabled"));
    let mut out = vec![];
    for e in fs::read_dir(hermes::dir().join("plugins")).into_iter().flatten().flatten() {
        let y = read_text(&e.path().join("plugin.yaml")).ok().and_then(|(t, _)| serde_yaml::from_str::<serde_yaml::Value>(&t).ok()).and_then(|y| serde_json::to_value(y).ok());
        let Some(m) = y else { continue };
        let name = text(&m["name"]).unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
        out.push(from_manifest(&name, &m, on.contains(&name) && !off.contains(&name)));
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn openclaw_list() -> Vec<PluginInfo> {
    let v = json_at(&openclaw::config_path());
    let entries = v.pointer("/plugins/entries").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut ids: Vec<String> = entries.keys().cloned().collect();
    if let Some(i) = v.pointer("/plugins/installs").and_then(Value::as_object) {
        ids.extend(i.keys().filter(|k| !entries.contains_key(*k)).cloned());
    }
    ids.iter().map(|id| from_manifest(id, &Value::Null, entries.get(id).and_then(|e| e["enabled"].as_bool()).unwrap_or(true))).collect()
}

/// `name@version` split in two; a scoped name keeps its leading @. The first character is
/// skipped whole: it can take several bytes, and slicing inside it would panic.
fn npm_spec(s: &str) -> (&str, Option<String>) {
    let first = s.chars().next().map_or(0, char::len_utf8);
    match s[first..].rfind('@') {
        Some(i) => (&s[..first + i], Some(s[first + i + 1..].to_string())),
        None => (s, None),
    }
}

fn oc_list(agent: &str) -> Vec<PluginInfo> {
    let cfg = json_at(&oc_config(agent));
    let spec = |s: &str, on: bool| {
        let (name, version) = npm_spec(s);
        PluginInfo { id: s.into(), name: name.into(), version, source: Some("npm".into()), enabled: on, ..Default::default() }
    };
    let mut out: Vec<PluginInfo> = cfg["plugin"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).filter(|s| !s.is_empty()).map(|s| spec(s, true)).collect();
    for s in store::get_arr(&store::load(), agent, OFF_KEY).iter().filter_map(|x| x.as_str()) {
        if !out.iter().any(|p| p.id == s) {
            out.push(spec(s, false));
        }
    }
    out
}

/// An agent's installed plugins; None when it has no plugins.
pub fn list(agent: &str) -> Option<Vec<PluginInfo>> {
    Some(match agent {
        claude::ID | codebuddy::ID | droid::ID => claude_style(agent),
        zcode::ID => zcode_list(),
        codex::ID => codex_list(),
        gemini::ID | qwen::ID => extensions(agent),
        hermes::ID => hermes_list(),
        openclaw::ID => openclaw_list(),
        opencode::ID | kilo::ID => oc_list(agent),
        dsh::ID => dsh::plugin_list(),
        _ => return None,
    })
}

// ---------------------------------------------------------------- switching

fn onoff(id: &str, on: bool) -> String {
    if on {
        tr!("Plugin \"{id}\" → on", "插件「{id}」→ 启用")
    } else {
        tr!("Plugin \"{id}\" → off", "插件「{id}」→ 停用")
    }
}

/// A JSON / JSONC config edited by `f` (true = it changed something). Written, after a
/// backup, unless `dry_run`; a file with comments is refused rather than losing them.
fn edit_json(agent: &str, path: &Path, dry_run: bool, f: impl FnOnce(&mut Value) -> Result<bool>) -> Result<Option<PathBuf>> {
    let (text, meta) = read_text_or_new(path)?;
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (mut v, comments) = if text.trim().is_empty() { (json!({}), false) } else { parse_jsonc_object(&text, &file)? };
    if !f(&mut v)? {
        return Ok(None);
    }
    if comments {
        return Err(crate::adapters::msg::comments_not_written(&file));
    }
    if dry_run {
        return Ok(None);
    }
    let backup = if path.exists() { Some(backup(agent, &[path.to_path_buf()])?) } else { None };
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    write_json(path, &v, meta)?;
    Ok(backup)
}

fn switches(ops: &[Op]) -> Vec<(String, bool)> {
    ops.iter().filter_map(|o| match o {
        Op::SetPluginEnabled { plugin, enabled } => Some((plugin.clone(), *enabled)),
        _ => None,
    }).collect()
}

/// The plugin switches of one apply (see `adapters::plan_resolved`).
pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    let Some(installed) = list(agent) else {
        bail!("{}", tr!("{} has no plugins", "{} 没有插件", crate::adapters::display_name(agent)));
    };
    let mut diff = Diff::default();
    let mut todo = vec![];
    for (id, on) in switches(ops) {
        let p = installed.iter().find(|p| p.id == id).ok_or_else(|| anyhow!(tr!("No plugin \"{id}\" is installed", "没有安装插件「{id}」")))?;
        if let Some(why) = &p.locked {
            bail!("{why}");
        }
        if p.enabled != on {
            todo.push((id, on));
        }
    }
    if todo.is_empty() {
        return Ok((diff, vec![], None));
    }
    let (path, backup) = match agent {
        claude::ID | codebuddy::ID | droid::ID => {
            let path = claude_settings(agent);
            let b = edit_json(agent, &path, dry_run, |v| {
                let m = obj_at(v, &["enabledPlugins"])?;
                for (id, on) in &todo {
                    m.insert(id.clone(), json!(on));
                }
                Ok(true)
            })?;
            (path, b)
        }
        zcode::ID => {
            let path = zcode_cli().join("config.json");
            let b = edit_json(agent, &path, dry_run, |v| {
                let m = obj_at(v, &["plugins", "enabledPlugins"])?;
                for (id, on) in &todo {
                    m.insert(id.clone(), json!(on));
                }
                Ok(true)
            })?;
            (path, b)
        }
        openclaw::ID => {
            let path = openclaw::config_path();
            let (t, _) = read_text_or_new(&path)?;
            if !t.trim().is_empty() && serde_json::from_str::<Value>(&t).is_err() {
                bail!("{}", l("openclaw.json uses JSON5 syntax that AgentPlus would lose on write, so it's read-only.", "openclaw.json 用了 JSON5 写法，AgentPlus 写回会丢掉这些格式，已切换为只读。"));
            }
            let b = edit_json(agent, &path, dry_run, |v| {
                for (id, on) in &todo {
                    obj_at(v, &["plugins", "entries", id.as_str()])?.insert("enabled".into(), json!(on));
                }
                Ok(true)
            })?;
            (path, b)
        }
        gemini::ID | qwen::ID => {
            let path = ext_dir(agent).join("extension-enablement.json");
            let home = rule_base(&home());
            let b = edit_json(agent, &path, dry_run, |v| {
                for (id, on) in &todo {
                    let e = obj_at(v, &[id.as_str()])?;
                    let mut rules: Vec<String> = e.get("overrides").and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
                    set_ext(&mut rules, &home, *on);
                    e.insert("overrides".into(), json!(rules));
                }
                Ok(true)
            })?;
            (path, b)
        }
        opencode::ID | kilo::ID => {
            let path = oc_config(agent);
            let b = edit_json(agent, &path, dry_run, |v| {
                let obj = v.as_object_mut().ok_or_else(|| anyhow!(l("The config's top level is not an object", "配置顶层不是对象")))?;
                let list = obj.entry("plugin").or_insert_with(|| json!([]));
                let a = list.as_array_mut().ok_or_else(|| anyhow!(l("plugin in the config is not a list", "配置里的 plugin 不是列表")))?;
                for (id, on) in &todo {
                    a.retain(|x| x.as_str() != Some(id.as_str()));
                    if *on {
                        a.push(json!(id));
                    }
                }
                Ok(true)
            })?;
            if !dry_run {
                store::update(|s| {
                    let mut off: Vec<Value> = store::get_arr(s, agent, OFF_KEY);
                    for (id, on) in &todo {
                        off.retain(|x| x.as_str() != Some(id.as_str()));
                        if !on {
                            off.push(json!(id));
                        }
                    }
                    store::set_value(s, agent, OFF_KEY, if off.is_empty() { Value::Null } else { Value::Array(off) });
                    Ok(())
                })?;
            }
            (path, b)
        }
        codex::ID => {
            let path = codex::config_path();
            let (text, meta) = read_text_or_new(&path)?;
            let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| anyhow!(tr!("Failed to parse config.toml: {e}", "config.toml 解析失败：{e}")))?;
            for (id, on) in &todo {
                let t = doc.get_mut("plugins").and_then(|p| p.as_table_mut()).and_then(|p| p.get_mut(id)).and_then(|t| t.as_table_mut());
                let t = t.ok_or_else(|| anyhow!(tr!("No plugin \"{id}\" is installed", "没有安装插件「{id}」")))?;
                t["enabled"] = toml_edit::value(*on);
            }
            let b = if dry_run { None } else {
                let b = backup(agent, std::slice::from_ref(&path))?;
                write_text_atomic(&path, &doc.to_string(), meta)?;
                Some(b)
            };
            (path, b)
        }
        hermes::ID => {
            let path = hermes::config_path();
            let (text, meta) = read_text_or_new(&path)?;
            let root: serde_yaml::Value = if text.trim().is_empty() { serde_yaml::Value::Null } else { serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("Failed to parse config.yaml: {e}", "config.yaml 解析失败：{e}")))? };
            let mut plugins = serde_json::to_value(root.get("plugins").cloned().unwrap_or(serde_yaml::Value::Null))?;
            if plugins.is_null() {
                plugins = json!({});
            }
            for (id, on) in &todo {
                for (k, add) in [("enabled", *on), ("disabled", !*on)] {
                    let o = obj_at(&mut plugins, &[])?;
                    let list = o.entry(k.to_string()).or_insert_with(|| json!([]));
                    let a = list.as_array_mut().ok_or_else(|| anyhow!(tr!("plugins.{k} is not a list", "plugins.{k} 不是列表")))?;
                    a.retain(|x| x.as_str() != Some(id.as_str()));
                    if add {
                        a.push(json!(id));
                    }
                }
            }
            let block = serde_yaml::to_value(&plugins)?;
            let out = crate::adapters::hermes::rewrite(&text, &[("plugins", Some(&block))])?;
            let b = if dry_run { None } else {
                let b = backup(agent, std::slice::from_ref(&path))?;
                write_text_atomic(&path, &out, meta)?;
                Some(b)
            };
            (path, b)
        }
        _ => bail!("{}", l("Switching plugins isn't supported for this agent", "不支持切换这个 Agent 的插件")),
    };
    let file = display_path(&path);
    for (id, on) in &todo {
        diff.push(&file, onoff(id, *on), *on);
    }
    Ok((diff, if dry_run { vec![] } else { vec![path] }, backup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    fn write(p: &Path, t: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, t).unwrap();
    }

    fn switch(agent: &str, id: &str, on: bool) -> Vec<String> {
        let ops = vec![Op::SetPluginEnabled { plugin: id.into(), enabled: on }];
        let (d, _, _) = store::transaction(|| crate::adapters::plan_resolved(agent, &ops, false)).unwrap();
        d.groups.into_iter().flat_map(|g| g.lines).map(|l| l.text).collect()
    }

    fn state(agent: &str, id: &str) -> Option<bool> {
        list(agent).unwrap().into_iter().find(|p| p.id == id).map(|p| p.enabled)
    }

    #[test]
    fn npm_specs_split_at_the_version() {
        assert_eq!(npm_spec("opencode-x@1.2.3"), ("opencode-x", Some("1.2.3".into())));
        assert_eq!(npm_spec("@scope/pkg@latest"), ("@scope/pkg", Some("latest".into())));
        assert_eq!(npm_spec("@scope/pkg"), ("@scope/pkg", None));
        assert_eq!(npm_spec("插件@2"), ("插件", Some("2".into())));
        assert_eq!(npm_spec("插件"), ("插件", None));
    }

    #[test]
    fn claude_style_switches() {
        let h = TestHome::new("plugins-claude");
        write(&h.0.join(".claude/plugins/installed_plugins.json"), r#"{"version":2,"plugins":{"fmt@official":[{"scope":"user","installPath":"P","version":"1.2.0"}]}}"#);
        write(&h.0.join(".codebuddy/settings.json"), r#"{"enabledPlugins":{"pdf@official":true},"model":"x"}"#);
        write(&h.0.join(".codebuddy/plugins/marketplaces/official/plugins/pdf/.codebuddy-plugin/plugin.json"), r#"{"name":"pdf","description":"处理 PDF","description_en":"PDFs"}"#);
        let fmt = &list(claude::ID).unwrap()[0];
        assert_eq!((fmt.id.as_str(), fmt.version.as_deref(), fmt.source.as_deref(), fmt.enabled), ("fmt@official", Some("1.2.0"), Some("official"), true));
        assert_eq!(switch(claude::ID, "fmt@official", false), ["插件「fmt@official」→ 停用"]);
        assert_eq!(state(claude::ID, "fmt@official"), Some(false));
        assert_eq!(list(codebuddy::ID).unwrap()[0].description.as_deref(), Some("处理 PDF"));
        switch(codebuddy::ID, "pdf@official", false);
        let s = fs::read_to_string(h.0.join(".codebuddy/settings.json")).unwrap();
        assert!(s.contains("\"pdf@official\": false") && s.contains("\"model\": \"x\""), "{s}");
    }

    #[test]
    fn codex_and_zcode_switch_in_their_own_files() {
        let h = TestHome::new("plugins-codex");
        write(&h.0.join(".codex/config.toml"), "model = \"m\"\n\n[plugins.\"latex@openai-bundled\"]\nenabled = true\n");
        write(&h.0.join(".codex/plugins/cache/openai-bundled/latex/1.0.0/.codex-plugin/plugin.json"), r#"{"name":"latex","version":"1.0.0","interface":{"displayName":"LaTeX","shortDescription":"Compile LaTeX"}}"#);
        fs::create_dir_all(h.0.join(".codex/plugins/cache/openai-curated-remote/github")).unwrap();
        let l = list(codex::ID).unwrap();
        assert_eq!((l[0].name.as_str(), l[0].description.as_deref()), ("LaTeX", Some("Compile LaTeX")));
        assert!(l[1].locked.is_some());
        switch(codex::ID, "latex@openai-bundled", false);
        assert!(fs::read_to_string(h.0.join(".codex/config.toml")).unwrap().ends_with("[plugins.\"latex@openai-bundled\"]\nenabled = false\n"));
        write(&h.0.join(".zcode/cli/config.json"), r#"{"plugins":{"enabledPlugins":{"github@z":true}},"skills":{}}"#);
        switch(zcode::ID, "github@z", false);
        assert_eq!(state(zcode::ID, "github@z"), Some(false));
    }

    #[test]
    fn gemini_overrides_follow_geminis_rules() {
        let home = "/C:/Users/me/";
        assert!(ext_enabled(&[], home));
        assert!(!ext_enabled(&["!/C:/Users/me/*".into()], home));
        assert!(ext_enabled(&["!/C:/Users/me/*".into(), "/C:/Users/me/*".into()], home));
        // A rule for a project below home doesn't decide the user level.
        assert!(ext_enabled(&["!/C:/Users/me/proj/*".into()], home));
        let mut rules = vec!["/C:/Users/me/proj/*".to_string(), "/D:/other/*".into()];
        set_ext(&mut rules, home, false);
        assert_eq!(rules, ["/D:/other/*", "!/C:/Users/me/*"]);
        assert_eq!(rule_base(Path::new("C:\\Users\\me")), "/C:/Users/me/");
        let h = TestHome::new("plugins-gemini");
        write(&h.0.join(".gemini/extensions/conductor/gemini-extension.json"), r#"{"name":"conductor","version":"0.3.0","description":"Plans"}"#);
        assert_eq!(state(gemini::ID, "conductor"), Some(true));
        switch(gemini::ID, "conductor", false);
        assert_eq!(state(gemini::ID, "conductor"), Some(false));
        let e = json_at(&h.0.join(".gemini/extensions/extension-enablement.json"));
        assert_eq!(e["conductor"]["overrides"][0], format!("!{}*", rule_base(&h.0)));
    }

    #[test]
    fn opencode_plugins_are_kept_while_off() {
        let h = TestHome::new("plugins-opencode");
        write(&h.0.join(".config/opencode/opencode.json"), r#"{"plugin":["opencode-mystatus","@scope/auth@1.2.3"]}"#);
        let l = list(opencode::ID).unwrap();
        assert_eq!((l[1].name.as_str(), l[1].version.as_deref()), ("@scope/auth", Some("1.2.3")));
        switch(opencode::ID, "@scope/auth@1.2.3", false);
        assert_eq!(state(opencode::ID, "@scope/auth@1.2.3"), Some(false));
        assert_eq!(json_at(&h.0.join(".config/opencode/opencode.json"))["plugin"], json!(["opencode-mystatus"]));
        switch(opencode::ID, "@scope/auth@1.2.3", true);
        assert_eq!(json_at(&h.0.join(".config/opencode/opencode.json"))["plugin"], json!(["opencode-mystatus", "@scope/auth@1.2.3"]));
        assert!(store::get_arr(&store::load(), opencode::ID, OFF_KEY).is_empty());
    }

    #[test]
    fn hermes_and_openclaw() {
        let h = TestHome::new("plugins-hermes");
        write(&h.0.join(".hermes/config.yaml"), "model:\n  default: x\n");
        write(&h.0.join(".hermes/plugins/notes/plugin.yaml"), "name: notes\nversion: 0.1.0\ndescription: Notes\n");
        assert_eq!(state(hermes::ID, "notes"), Some(false));
        switch(hermes::ID, "notes", true);
        assert_eq!(state(hermes::ID, "notes"), Some(true));
        assert!(fs::read_to_string(h.0.join(".hermes/config.yaml")).unwrap().starts_with("model:\n  default: x\n"));
        write(&h.0.join(".openclaw/openclaw.json"), r#"{"plugins":{"entries":{"voice":{"enabled":true,"config":{"a":1}}}}}"#);
        switch(openclaw::ID, "voice", false);
        let v = json_at(&h.0.join(".openclaw/openclaw.json"));
        assert_eq!((v["plugins"]["entries"]["voice"]["enabled"].clone(), v["plugins"]["entries"]["voice"]["config"]["a"].clone()), (json!(false), json!(1)));
        assert!(list(crate::adapters::pi::ID).is_none());
    }
}

#[cfg(test)]
mod dump {
    /// Read-only list of the plugins on this machine.
    /// `cargo test --lib plugins::dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_plugins() {
        for a in crate::adapters::ALL {
            if let Some(l) = super::list(a) {
                println!("{a:<10} {}", l.iter().map(|p| format!("{}{}{}", p.id, if p.enabled { "" } else { "(off)" }, if p.locked.is_some() { "(locked)" } else { "" })).collect::<Vec<_>>().join(", "));
            }
        }
    }
}
