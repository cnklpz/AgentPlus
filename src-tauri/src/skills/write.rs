//! Changing skills: copying a skill folder to another folder, deleting one, and switching a
//! skill off or on for one agent. Unlike config edits these happen right away (a skill is a
//! folder, not a pending config entry); anything replaced or removed is backed up first and
//! can be rolled back from History.
//!
//! Switching uses the agent's own setting when it has one (see `disabled`). Agents without
//! one (Kimi, pi, DeepSeek Harness, OpenCode, Kilo) get the folder moved out of their skills
//! folder into `~/.agentplus/skills-off/<agent>/`, and back when switched on; only folders
//! the agent owns move, since a shared folder is read by other agents too.

use super::{norm, roots, Kind};
use crate::adapters::{self, claude, codebuddy, codex, droid, gemini, hermes, mimo, msg, openclaw, qwen, zcode};
use crate::history::{REASON_SKILL_DELETE, REASON_SKILL_REPLACE, REASON_SKILL_SWITCH};
use crate::i18n::l;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// History label of skill backups (shown as "Skills").
pub const JOB: &str = "skills";
/// Store key (per agent): stashed folder → where it came from.
const OFF_KEY: &str = "skillsOff";

fn reason((en, zh): (&'static str, &'static str)) -> &'static str {
    l(en, zh)
}

/// Where AgentPlus keeps an agent's switched-off skills.
pub fn off_dir(agent: &str) -> PathBuf {
    agentplus_dir().join("skills-off").join(agent)
}

/// Every folder some agent keeps its own skills in, or shares, or adds in its config: the
/// folders AgentPlus may write to. Built-in folders are never written.
fn writable() -> Vec<(PathBuf, Kind)> {
    let mut out: Vec<(PathBuf, Kind)> = vec![];
    for a in adapters::ALL {
        for (p, k) in roots(a).unwrap_or_default() {
            if matches!(k, Kind::Own | Kind::Shared | Kind::Extra) && !out.iter().any(|(x, _)| norm(&x.to_string_lossy()) == norm(&p.to_string_lossy())) {
                out.push((p, k));
            }
        }
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    norm(&a.to_string_lossy()) == norm(&b.to_string_lossy())
}

/// The writable folder `dir` sits in (at any depth), or an error. A built-in folder inside a
/// writable one (Codex's `skills/.system`) stays off limits.
fn root_of(dir: &Path) -> Result<PathBuf> {
    let builtin = adapters::ALL.iter().flat_map(|a| roots(a).unwrap_or_default()).any(|(p, k)| k == Kind::Builtin && dir.ancestors().skip(1).any(|a| same_path(a, &p)));
    if builtin {
        bail!("{}", l("Built-in skills can't be changed", "内置技能不能修改"));
    }
    writable()
        .into_iter()
        .map(|(p, _)| p)
        .find(|r| dir.ancestors().skip(1).any(|a| same_path(a, r)))
        .ok_or_else(|| anyhow!(l("That folder isn't one of the skills folders AgentPlus manages", "这个目录不在 AgentPlus 管理的技能目录里")))
}

fn skill_dir(s: &str) -> Result<PathBuf> {
    let d = crate::env::resolve_path(s);
    if !d.join("SKILL.md").is_file() {
        bail!("{}", tr!("No SKILL.md in {}", "{} 里没有 SKILL.md", display_path(&d)));
    }
    Ok(d)
}

/// Replaces `to` with a copy of `from`, through a temporary sibling so a failed copy leaves
/// `to` as it was.
fn put_copy(from: &Path, to: &Path) -> Result<()> {
    let tmp = to.with_file_name(format!(".{}.agentplus-tmp", to.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    if let Err(e) = copy_dir(from, &tmp) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if to.exists() {
        fs::remove_dir_all(to)?;
    }
    fs::rename(&tmp, to)?;
    Ok(())
}

/// What `copy` did.
#[derive(serde::Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Copied {
    /// A new folder.
    Added,
    /// The folder there had other files; it was backed up and replaced.
    Replaced,
    /// The folder there already had the same files.
    Same,
    /// The folder there has other files and `replace` wasn't asked for (nothing was written).
    Exists,
}

/// Copies skill folder `from` into skills folder `to_root` (under the same folder name).
pub fn copy(from: &str, to_root: &str, replace: bool) -> Result<Copied> {
    let from = skill_dir(from)?;
    let to_root = crate::env::resolve_path(to_root);
    if !writable().iter().any(|(p, _)| same_path(p, &to_root)) && !same_path(&to_root, &super::library_dir()) {
        bail!("{}", l("That folder isn't one of the skills folders AgentPlus manages", "这个目录不在 AgentPlus 管理的技能目录里"));
    }
    let name = from.file_name().ok_or_else(|| anyhow!(l("Invalid skill folder", "无效的技能目录")))?;
    let to = to_root.join(name);
    if same_path(&from, &to) {
        return Ok(Copied::Same);
    }
    if to.exists() {
        if super::content(&to).2 == super::content(&from).2 {
            return Ok(Copied::Same);
        }
        if !replace {
            return Ok(Copied::Exists);
        }
        backup_with_dirs(JOB, &[], std::slice::from_ref(&to), reason(REASON_SKILL_REPLACE))?;
        put_copy(&from, &to)?;
        return Ok(Copied::Replaced);
    }
    fs::create_dir_all(&to_root)?;
    put_copy(&from, &to)?;
    Ok(Copied::Added)
}

/// Deletes a skill folder (backed up first).
pub fn delete(dir: &str) -> Result<()> {
    let d = skill_dir(dir)?;
    let in_library = d.ancestors().skip(1).any(|a| same_path(a, &super::library_dir()));
    if !in_library {
        root_of(&d)?;
    }
    backup_with_dirs(JOB, &[], std::slice::from_ref(&d), reason(REASON_SKILL_DELETE))?;
    fs::remove_dir_all(&d)?;
    Ok(())
}

// ---------------------------------------------------------------- switching

/// `settings.json`-style JSON: read (refusing comments), changed by `f`, written back when it
/// changed; the file is backed up first. True when it changed.
fn edit_json(path: &Path, f: impl FnOnce(&mut Value) -> Result<()>) -> Result<bool> {
    let (text, meta) = read_text_or_new(path)?;
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (mut v, comments) = if text.trim().is_empty() { (json!({}), false) } else { parse_jsonc_object(&text, &file)? };
    let before = v.clone();
    f(&mut v)?;
    if v == before {
        return Ok(false);
    }
    if comments {
        return Err(msg::comments_not_written(&file));
    }
    if path.exists() {
        backup_with_dirs(JOB, &[path.to_path_buf()], &[], reason(REASON_SKILL_SWITCH))?;
    }
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    write_json(path, &v, meta)?;
    Ok(true)
}

/// Adds `name` to (off) or takes it out of (on) the string list at `ptr`.
fn listed(v: &mut Value, ptr: &[&str], name: &str, off: bool) -> Result<()> {
    let (last, parent) = ptr.split_last().unwrap();
    let obj = obj_at(v, parent)?;
    let list = obj.entry(last.to_string()).or_insert_with(|| json!([]));
    let a = list.as_array_mut().ok_or_else(|| anyhow!(tr!("{} is not a list", "{} 不是列表", ptr.join("."))))?;
    a.retain(|x| x.as_str() != Some(name));
    if off {
        a.push(json!(name));
    }
    Ok(())
}

/// A key in an object at `ptr`: set to `value`, or removed (with the object when it empties).
fn keyed(v: &mut Value, ptr: &[&str], key: &str, value: Option<Value>) -> Result<()> {
    let obj = obj_at(v, ptr)?;
    match value {
        Some(x) => {
            obj.insert(key.into(), x);
        }
        None => {
            obj.remove(key);
        }
    }
    if obj.is_empty() {
        if let Some((last, parent)) = ptr.split_last() {
            obj_at(v, parent)?.remove(*last);
        }
    }
    Ok(())
}

/// The ZCode config keys a skill by its SKILL.md path, as ZCode writes it (`C:/…/SKILL.md`).
fn zcode_key(dir: &Path) -> String {
    dir.join("SKILL.md").to_string_lossy().replace('\\', "/")
}

/// Codex `[[skills.config]] path = "…/SKILL.md"`, `enabled = false`.
fn codex_switch(dir: &Path, off: bool) -> Result<bool> {
    let path = codex::config_path();
    let (text, meta) = read_text_or_new(&path)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| anyhow!(tr!("Failed to parse config.toml: {e}", "config.toml 解析失败：{e}")))?;
    let md = dir.join("SKILL.md");
    let skills = doc.entry("skills").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        toml_edit::Item::Table(t)
    });
    let skills = skills.as_table_mut().ok_or_else(|| anyhow!(l("skills in config.toml is not a table", "config.toml 里的 skills 不是表")))?;
    let list = skills.entry("config").or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let list = list.as_array_of_tables_mut().ok_or_else(|| anyhow!(l("skills.config in config.toml is not a list of tables", "config.toml 里的 skills.config 不是表数组")))?;
    let at = list.iter().position(|t| t.get("path").and_then(|p| p.as_str()).is_some_and(|p| same_path(Path::new(p), &md)));
    let changed = match (at, off) {
        (Some(i), false) => {
            list.remove(i);
            true
        }
        (Some(i), true) => {
            let t = list.get_mut(i).unwrap();
            let was = t.get("enabled").and_then(|e| e.as_bool()) == Some(false);
            t["enabled"] = toml_edit::value(false);
            !was
        }
        (None, true) => {
            let mut t = toml_edit::Table::new();
            t["path"] = toml_edit::value(md.to_string_lossy().to_string());
            t["enabled"] = toml_edit::value(false);
            list.push(t);
            true
        }
        (None, false) => false,
    };
    if list.is_empty() {
        skills.remove("config");
    }
    if skills.is_empty() {
        doc.remove("skills");
    }
    if !changed {
        return Ok(false);
    }
    if path.exists() {
        backup_with_dirs(JOB, std::slice::from_ref(&path), &[], reason(REASON_SKILL_SWITCH))?;
    }
    write_text_atomic(&path, &doc.to_string(), meta)?;
    Ok(true)
}

/// Hermes `skills.disabled` in config.yaml, rewriting only the `skills:` block.
fn hermes_switch(name: &str, off: bool) -> Result<bool> {
    let path = hermes::config_path();
    let (text, meta) = read_text_or_new(&path)?;
    let root: serde_yaml::Value = if text.trim().is_empty() { serde_yaml::Value::Null } else { serde_yaml::from_str(&text).map_err(|e| anyhow!(tr!("Failed to parse config.yaml: {e}", "config.yaml 解析失败：{e}")))? };
    let mut skills = serde_json::to_value(root.get("skills").cloned().unwrap_or(serde_yaml::Value::Null))?;
    if skills.is_null() {
        skills = json!({});
    }
    let before = skills.clone();
    listed(&mut skills, &["disabled"], name, off)?;
    if skills["disabled"].as_array().is_some_and(|a| a.is_empty()) {
        skills.as_object_mut().unwrap().remove("disabled");
    }
    if skills == before {
        return Ok(false);
    }
    let block = serde_yaml::to_value(&skills)?;
    let empty = skills.as_object().is_some_and(|o| o.is_empty());
    let out = hermes::rewrite(&text, &[("skills", (!empty).then_some(&block))])?;
    if path.exists() {
        backup_with_dirs(JOB, std::slice::from_ref(&path), &[], reason(REASON_SKILL_SWITCH))?;
    }
    write_text_atomic(&path, &out, meta)?;
    Ok(true)
}

fn stash(agent: &str, dir: &Path) -> Result<()> {
    let root = roots(agent).unwrap_or_default().into_iter().find(|(r, k)| *k == Kind::Own && dir.ancestors().skip(1).any(|a| same_path(a, r)));
    let Some((root, _)) = root else {
        bail!("{}", tr!(
            "{} has no switch for single skills, and this one isn't in its own skills folder (moving it would hide it from other agents too)",
            "{} 没有单个技能的开关，而这个技能不在它自己的技能目录里（挪走会让其他 Agent 也看不到）",
            adapters::display_name(agent)
        ));
    };
    let rel = dir.strip_prefix(&root).map_err(|_| anyhow!(l("Invalid skill folder", "无效的技能目录")))?.to_path_buf();
    let to = off_dir(agent).join(&rel);
    if to.exists() {
        fs::remove_dir_all(&to)?;
    }
    fs::create_dir_all(to.parent().unwrap())?;
    if fs::rename(dir, &to).is_err() {
        // Another drive: copy, then remove.
        copy_dir(dir, &to)?;
        fs::remove_dir_all(dir)?;
    }
    store::update(|s| {
        store::section(s, agent, OFF_KEY).insert(rel.to_string_lossy().replace('\\', "/"), json!(dir.to_string_lossy()));
        Ok(())
    })
}

fn unstash(agent: &str, dir: &Path) -> Result<()> {
    let off = off_dir(agent);
    let rel = dir.strip_prefix(&off).map_err(|_| anyhow!(l("This skill isn't switched off by AgentPlus", "这个技能不是由 AgentPlus 停用的")))?.to_string_lossy().replace('\\', "/");
    let back = store::get_obj(&store::load(), agent, OFF_KEY).get(&rel).and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(|| {
        let own = roots(agent).unwrap_or_default().into_iter().find(|(_, k)| *k == Kind::Own).map(|(p, _)| p).unwrap_or_default();
        own.join(&rel)
    });
    if back.exists() {
        bail!("{}", tr!("{} is back in place already; delete one of the two first", "{} 已经有同名技能，请先删掉其中一个", display_path(&back)));
    }
    fs::create_dir_all(back.parent().unwrap())?;
    if fs::rename(dir, &back).is_err() {
        copy_dir(dir, &back)?;
        fs::remove_dir_all(dir)?;
    }
    store::update(|s| {
        store::section(s, agent, OFF_KEY).remove(&rel);
        Ok(())
    })
}

/// Switches skill `name` (the copy at `dir`, the one the agent loads) off or on for `agent`.
pub fn set_enabled(agent: &str, name: &str, dir: &str, on: bool) -> Result<()> {
    let d = crate::env::resolve_path(dir);
    let off = !on;
    match agent {
        claude::ID | codebuddy::ID => {
            let path = if agent == claude::ID { claude::settings_path() } else { codebuddy::settings_path() };
            edit_json(&path, |v| keyed(v, &["skillOverrides"], name, off.then(|| json!("off"))))?;
        }
        gemini::ID | qwen::ID => {
            let path = if agent == gemini::ID { gemini::settings_path() } else { qwen::settings_path() };
            edit_json(&path, |v| {
                listed(v, &["skills", "disabled"], name, off)?;
                if v.pointer("/skills/disabled").and_then(Value::as_array).is_some_and(|a| a.is_empty()) {
                    keyed(v, &["skills"], "disabled", None)?;
                }
                Ok(())
            })?;
        }
        droid::ID => {
            edit_json(&droid::settings_path(), |v| {
                listed(v, &["disabledSkills"], name, off)?;
                if v["disabledSkills"].as_array().is_some_and(|a| a.is_empty()) {
                    v.as_object_mut().unwrap().remove("disabledSkills");
                }
                Ok(())
            })?;
        }
        openclaw::ID => {
            edit_json(&openclaw::config_path(), |v| keyed(v, &["skills", "entries", name], "enabled", off.then(|| json!(false))))?;
        }
        zcode::ID => {
            let key = zcode_key(&d);
            edit_json(&super::zcode_home().join("cli").join("config.json"), |v| {
                // ZCode's own switch may name the path with other slashes or case.
                if let Some(o) = v.get_mut("skills").and_then(Value::as_object_mut) {
                    o.retain(|k, _| norm(k) != norm(&key));
                }
                keyed(v, &["skills"], &key, off.then(|| json!({ "enable": false })))
            })?;
        }
        mimo::ID => {
            edit_json(&mimo::app_dir().join("skill-states.json"), |v| {
                if v.get("version").is_none() {
                    v["version"] = json!(1);
                }
                keyed(v, &["skills"], name, off.then(|| json!(false)))
            })?;
        }
        codex::ID => {
            codex_switch(&d, off)?;
        }
        hermes::ID => {
            hermes_switch(name, off)?;
        }
        _ if off => stash(agent, &skill_dir(dir)?)?,
        _ => unstash(agent, &skill_dir(dir)?)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    fn skill(dir: &Path, name: &str, body: &str) -> PathBuf {
        let d = dir.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: {body}\n---\n")).unwrap();
        d
    }

    fn off(agent: &str, name: &str) -> bool {
        let o = super::super::overview(&[agent.to_string()]);
        o.agents[0].disabled.iter().any(|d| d == name)
    }

    #[test]
    fn copying_adds_skips_the_same_and_asks_before_replacing() {
        let h = TestHome::new("skills-copy");
        let src = skill(&h.0.join(".codex/skills"), "pdf", "one");
        let shared = h.0.join(".agents/skills");
        let s = |p: &Path| display_path(p);
        assert_eq!(copy(&s(&src), &s(&shared), false).unwrap(), Copied::Added);
        assert_eq!(copy(&s(&src), &s(&shared), false).unwrap(), Copied::Same);
        fs::write(src.join("SKILL.md"), "---\nname: pdf\ndescription: two\n---\n").unwrap();
        assert_eq!(copy(&s(&src), &s(&shared), false).unwrap(), Copied::Exists);
        assert!(fs::read_to_string(shared.join("pdf/SKILL.md")).unwrap().contains("one"));
        assert_eq!(copy(&s(&src), &s(&shared), true).unwrap(), Copied::Replaced);
        assert!(fs::read_to_string(shared.join("pdf/SKILL.md")).unwrap().contains("two"));
        // The replaced copy is in History.
        assert!(crate::history::list().unwrap().iter().any(|e| e.agent == JOB && e.files[0].dir));
        // Not a skills folder: refused.
        assert!(copy(&s(&src), &s(&h.0.join("Desktop")), false).is_err());
    }

    #[test]
    fn deleting_backs_the_folder_up() {
        let h = TestHome::new("skills-delete");
        let d = skill(&h.0.join(".agents/skills"), "gone", "x");
        delete(&display_path(&d)).unwrap();
        assert!(!d.exists());
        let e = crate::history::list().unwrap().into_iter().find(|e| e.agent == JOB).unwrap();
        crate::history::restore(&e.id).unwrap();
        assert!(d.join("SKILL.md").is_file());
        // Built-in folders can't be deleted.
        let sys = skill(&h.0.join(".codex/skills/.system"), "imagegen", "x");
        assert!(delete(&display_path(&sys)).is_err());
    }

    #[test]
    fn switches_use_each_agents_setting() {
        let h = TestHome::new("skills-switch");
        let shared = skill(&h.0.join(".agents/skills"), "pdf", "x");
        let own = skill(&h.0.join(".codex/skills"), "fd", "x");
        let zc = skill(&h.0.join(".zcode/skills"), "shadcn", "x");
        let claude_own = skill(&h.0.join(".claude/skills"), "cl", "x");
        let hermes_own = skill(&h.0.join(".hermes/skills/creative"), "art", "x");
        fs::create_dir_all(h.0.join(".hermes")).unwrap();
        fs::write(h.0.join(".hermes/config.yaml"), "model:\n  default: x\nskills:\n  external_dirs: []\n").unwrap();
        let cases: Vec<(&str, &str, &Path)> = vec![
            (codex::ID, "fd", &own),
            (gemini::ID, "pdf", &shared),
            (zcode::ID, "shadcn", &zc),
            (claude::ID, "cl", &claude_own),
            (hermes::ID, "art", &hermes_own),
            (droid::ID, "pdf", &shared),
        ];
        for (agent, name, dir) in &cases {
            set_enabled(agent, name, &display_path(dir), false).unwrap_or_else(|e| panic!("{agent}: {e}"));
            assert!(off(agent, name), "{agent} off");
            set_enabled(agent, name, &display_path(dir), true).unwrap();
            assert!(!off(agent, name), "{agent} on");
        }
        // Switched back on, the settings are as they were.
        assert!(!fs::read_to_string(h.0.join(".codex/config.toml")).unwrap_or_default().contains("skills"));
        assert!(fs::read_to_string(h.0.join(".hermes/config.yaml")).unwrap().contains("model:\n  default: x\n"));
        assert_eq!(fs::read_to_string(h.0.join(".gemini/settings.json")).unwrap().trim(), "{}");
    }

    #[test]
    fn agents_without_a_switch_move_only_their_own_skills() {
        let h = TestHome::new("skills-stash");
        let own = skill(&h.0.join(".kimi-code/skills"), "notes", "x");
        let shared = skill(&h.0.join(".agents/skills"), "pdf", "x");
        set_enabled(adapters::kimi::ID, "notes", &display_path(&own), false).unwrap();
        assert!(!own.exists());
        let parked = off_dir(adapters::kimi::ID).join("notes");
        assert!(parked.join("SKILL.md").is_file());
        set_enabled(adapters::kimi::ID, "notes", &display_path(&parked), true).unwrap();
        assert!(own.join("SKILL.md").is_file() && !parked.exists());
        let err = set_enabled(adapters::kimi::ID, "pdf", &display_path(&shared), false).unwrap_err().to_string();
        assert!(err.contains("没有单个技能的开关"), "{err}");
    }
}
