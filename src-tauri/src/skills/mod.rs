//! Agent Skills (`<name>/SKILL.md` folders) across agents. A skills folder can be read by
//! several agents: `~/.agents/skills` is shared by most of them, OpenCode and Kilo also read
//! `~/.claude/skills`, MiMo reads other tools' folders when asked. So the overview is built
//! around folders ("roots"): each lists its skills and which agents read it; each agent lists
//! its roots in priority order (the first one with a name wins) and the skills its config
//! switches off. User-level only; project folders are out of scope.
//!
//! Where each agent looks (user level, highest priority first):
//! - Claude Code `~/.claude/skills`; CodeBuddy `~/.codebuddy/skills` (neither reads `~/.agents`).
//! - Codex `$CODEX_HOME/skills`, `~/.agents/skills`, then its system copies in `skills/.system`.
//! - OpenCode / Kilo: their config folder's `skill(s)`, `skills.paths`, `~/.agents/skills`,
//!   `~/.claude/skills`; Kilo also `~/.kilo/skills`.
//! - MiMo: `~/.config/mimocode/skills`, `~/.mimocode/skills`, the `skillPathCompat` folders,
//!   then the skills it ships.
//! - Gemini CLI / Qwen Code / Droid / Kimi Code / pi / ZCode / DeepSeek Harness: their own
//!   `skills` folder, then `~/.agents/skills` (Droid also `~/.agent/skills`, Kimi its
//!   `extra_skill_dirs`). OpenClaw: `~/.agents/skills`, then `~/.openclaw/skills`.
//! - Hermes: `HERMES_HOME/skills` (in category folders), then `skills.external_dirs`.

mod scan;
pub mod write;

pub use scan::{content, SkillCopy};

use crate::adapters::{self, claude, codebuddy, codex, droid, dsh, gemini, hermes, kilo, kimi, mimo, openclaw, opencode, pi, qwen, zcode};
use crate::util::{display_path, home, read_text, strip_jsonc};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// How an agent comes to read a folder.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The agent's own skills folder.
    Own,
    /// `~/.agents/skills`, read by most agents.
    Shared,
    /// Another tool's folder, read for compatibility.
    Compat,
    /// A folder the agent's config adds.
    Extra,
    /// Skills the agent ships (read-only).
    Builtin,
    /// Skills AgentPlus moved out of an agent's own folder to switch them off.
    Off,
    /// The AgentPlus skill library.
    Library,
}

/// A skills folder and the agents that read it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillRoot {
    /// Display path; also its key.
    pub path: String,
    pub kind: Kind,
    pub exists: bool,
    /// The agent whose own folder it is (None for shared and extra folders).
    pub owner: Option<String>,
    /// Agents that read it.
    pub readers: Vec<String>,
    pub skills: Vec<SkillCopy>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkills {
    pub agent: String,
    pub supported: bool,
    /// Root paths, highest priority first.
    pub roots: Vec<String>,
    /// Skills (by name) its config switches off.
    pub disabled: Vec<String>,
    /// It has a per-skill switch in its config.
    pub switchable: bool,
}

#[derive(Serialize)]
pub struct Overview {
    pub roots: Vec<SkillRoot>,
    pub agents: Vec<AgentSkills>,
}

/// The AgentPlus skill library: skill folders kept by AgentPlus, to copy into agents.
pub fn library_dir() -> PathBuf {
    crate::util::agentplus_dir().join("skills-library")
}

fn shared() -> PathBuf {
    home().join(".agents").join("skills")
}

fn parent(p: PathBuf) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or(p)
}

/// A JSON / JSONC config, or `{}`.
fn json(path: &Path) -> Value {
    read_text(path).ok().and_then(|(t, _)| serde_json::from_str(&strip_jsonc(&t).0).ok().or_else(|| json5::from_str(&t).ok())).unwrap_or(Value::Null)
}

fn yaml(path: &Path) -> Value {
    read_text(path).ok().and_then(|(t, _)| serde_yaml::from_str::<serde_yaml::Value>(&t).ok()).and_then(|y| serde_json::to_value(y).ok()).unwrap_or(Value::Null)
}

fn toml(path: &Path) -> Value {
    read_text(path).ok().and_then(|(t, _)| t.parse::<toml_edit::DocumentMut>().ok()).map(|d| crate::mcp::toml_item(d.as_item())).unwrap_or(Value::Null)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}

/// A folder named in a config: `~/…` and relative paths resolved.
fn config_dir(s: &str, base: &Path) -> PathBuf {
    let p = crate::env::resolve_path(s);
    if p.is_absolute() || s.starts_with('~') {
        p
    } else {
        base.join(p)
    }
}

/// The newest `builtin_skills/<version>/skills` MiMo unpacked.
fn mimo_builtin() -> Option<PathBuf> {
    let base = home().join(".local").join("share").join("mimocode").join("builtin_skills");
    let mut vs: Vec<PathBuf> = std::fs::read_dir(&base).ok()?.flatten().map(|e| e.path()).filter(|p| p.join("skills").is_dir()).collect();
    vs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>()).unwrap_or_default());
    vs.pop().map(|p| p.join("skills"))
}

/// An agent's skills folders, highest priority first; None when it has no skills.
pub fn roots(agent: &str) -> Option<Vec<(PathBuf, Kind)>> {
    use Kind::*;
    let own = |d: PathBuf| (d, Own);
    Some(match agent {
        claude::ID => vec![own(claude::dir().join("skills"))],
        codebuddy::ID => vec![own(codebuddy::dir().join("skills"))],
        codex::ID => {
            let d = parent(codex::config_path()).join("skills");
            vec![own(d.clone()), (shared(), Shared), (d.join(".system"), Builtin)]
        }
        opencode::ID | kilo::ID => {
            let (cfg, d) = if agent == kilo::ID { (kilo::config_path(), kilo::dir()) } else { (opencode::config_path(), opencode::dir()) };
            let mut v = vec![own(d.join("skills")), own(d.join("skill"))];
            if agent == kilo::ID {
                v.push(own(home().join(".kilo").join("skills")));
            }
            v.extend(strings(&json(&cfg).pointer("/skills/paths").cloned().unwrap_or_default()).iter().map(|p| (config_dir(p, &d), Extra)));
            v.push((shared(), Shared));
            v.push((home().join(".claude").join("skills"), Compat));
            v
        }
        mimo::ID => {
            let cfg = parent(mimo::engine_path());
            let prefs = json(&mimo::app_dir().join("preferences.json"));
            let on = |k: &str, dflt: bool| prefs.pointer(&format!("/skillPathCompat/{k}")).and_then(Value::as_bool).unwrap_or(dflt);
            let mut v = vec![own(cfg.join("skills")), own(home().join(".mimocode").join("skills"))];
            v.extend(strings(&json(&mimo::engine_path()).pointer("/skills/paths").cloned().unwrap_or_default()).iter().map(|p| (config_dir(p, &cfg), Extra)));
            if on("agents", true) {
                v.push((shared(), Shared));
            }
            for (k, d) in [("claude", ".claude"), ("codex", ".codex"), ("opencode", ".opencode")] {
                if on(k, false) {
                    v.push((home().join(d).join("skills"), Compat));
                }
            }
            v.push((mimo::app_dir().join("engine-config").join("skills"), Builtin));
            v.extend(mimo_builtin().map(|p| (p, Builtin)));
            v
        }
        gemini::ID => vec![own(gemini::dir().join("skills")), (shared(), Shared)],
        qwen::ID => vec![own(qwen::dir().join("skills")), (shared(), Shared)],
        droid::ID => vec![own(droid::dir().join("skills")), (shared(), Shared), (home().join(".agent").join("skills"), Shared)],
        kimi::ID => {
            let mut v = vec![own(kimi::dir().join("skills")), (shared(), Shared)];
            v.extend(strings(&toml(&kimi::config_path())["extra_skill_dirs"]).iter().map(|p| (config_dir(p, &kimi::dir()), Extra)));
            v
        }
        pi::ID => vec![own(pi::dir().join("skills")), (shared(), Shared)],
        openclaw::ID => vec![(shared(), Shared), own(openclaw::dir().join("skills"))],
        zcode::ID => vec![own(zcode_home().join("skills")), (shared(), Shared)],
        dsh::ID => {
            let agents = crate::env::agent_var("DSH_AGENTS_HOME").map(|d| crate::env::resolve_path(&d).join("skills")).unwrap_or_else(shared);
            vec![own(dsh::dir().join("skills")), (agents, Shared)]
        }
        hermes::ID => {
            let d = hermes::dir();
            let mut v = vec![own(d.join("skills"))];
            v.extend(strings(&yaml(&hermes::config_path()).pointer("/skills/external_dirs").cloned().unwrap_or_default()).iter().map(|p| (config_dir(p, &d), Extra)));
            v
        }
        _ => return None,
    })
}

/// `~/.zcode` (the adapter's folder is its `v2`).
fn zcode_home() -> PathBuf {
    let d = zcode::dir();
    if d.file_name().is_some_and(|n| n == "v2") { parent(d) } else { d }
}

/// Hermes keeps skills in category folders; the others one level down (a few look deeper,
/// but nobody nests skills in practice).
fn depth(agent: &str) -> usize {
    if agent == hermes::ID { 2 } else { 3 }
}

/// A path as a comparable key (case-insensitive on Windows, either slash).
fn norm(p: &str) -> String {
    let p = p.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) { p.to_lowercase() } else { p }
}

/// Skills an agent's config switches off, by name; `copies` maps SKILL.md paths to names for
/// the agents that key the switch by path. (switchable, names)
fn disabled(agent: &str, copies: &[(String, String)]) -> (bool, Vec<String>) {
    let by_path = |paths: Vec<String>| -> Vec<String> {
        paths.iter().filter_map(|p| copies.iter().find(|(md, _)| norm(md) == norm(p)).map(|(_, n)| n.clone())).collect()
    };
    let overrides = |v: &Value| -> Vec<String> { v["skillOverrides"].as_object().map(|o| o.iter().filter(|(_, x)| x.as_str() == Some("off")).map(|(k, _)| k.clone()).collect()).unwrap_or_default() };
    match agent {
        claude::ID => (true, overrides(&json(&claude::settings_path()))),
        codebuddy::ID => (true, overrides(&json(&codebuddy::settings_path()))),
        codex::ID => {
            let cfg = toml(&codex::config_path());
            let off = cfg.pointer("/skills/config").and_then(Value::as_array).map(|a| a.iter().filter(|e| e["enabled"] == false).filter_map(|e| e["path"].as_str().map(String::from)).collect()).unwrap_or_default();
            (true, by_path(off))
        }
        gemini::ID => (true, strings(&json(&gemini::settings_path()).pointer("/skills/disabled").cloned().unwrap_or_default())),
        qwen::ID => (true, strings(&json(&qwen::settings_path()).pointer("/skills/disabled").cloned().unwrap_or_default())),
        hermes::ID => (true, strings(&yaml(&hermes::config_path()).pointer("/skills/disabled").cloned().unwrap_or_default())),
        droid::ID => (true, strings(&json(&droid::settings_path())["disabledSkills"])),
        openclaw::ID => {
            let v = json(&openclaw::config_path());
            let off = v.pointer("/skills/entries").and_then(Value::as_object).map(|o| o.iter().filter(|(_, e)| e["enabled"] == false).map(|(k, _)| k.clone()).collect()).unwrap_or_default();
            (true, off)
        }
        zcode::ID => {
            let v = json(&zcode_home().join("cli").join("config.json"));
            let off = v["skills"].as_object().map(|o| o.iter().filter(|(_, e)| e["enable"] == false).map(|(k, _)| k.clone()).collect()).unwrap_or_default();
            (true, by_path(off))
        }
        mimo::ID => {
            let v = json(&mimo::app_dir().join("skill-states.json"));
            let off = v["skills"].as_object().map(|o| o.iter().filter(|(_, x)| **x == false).map(|(k, _)| k.clone()).collect()).unwrap_or_default();
            (true, off)
        }
        _ => (false, vec![]),
    }
}

/// Every folder the `agents` read, with their skills, and each agent's view of them.
pub fn overview(agents: &[String]) -> Overview {
    let mut found: Vec<SkillRoot> = vec![];
    let mut out: Vec<AgentSkills> = vec![];
    for a in agents.iter().filter(|a| adapters::ext(a).is_some()) {
        let Some(list) = roots(a) else {
            out.push(AgentSkills { agent: a.clone(), supported: false, roots: vec![], disabled: vec![], switchable: false });
            continue;
        };
        let mut keys = vec![];
        for (path, kind) in list {
            let key = display_path(&path);
            if keys.contains(&key) {
                continue;
            }
            match found.iter_mut().find(|r| norm(&r.path) == norm(&key)) {
                Some(r) => {
                    if !r.readers.contains(a) {
                        r.readers.push(a.clone());
                    }
                    // An agent's own folder read by others stays theirs.
                    if kind == Kind::Own && r.owner.is_none() {
                        r.owner = Some(a.clone());
                        r.kind = Kind::Own;
                    }
                }
                None => found.push(SkillRoot {
                    path: key.clone(),
                    kind,
                    exists: path.is_dir(),
                    owner: (kind == Kind::Own).then(|| a.clone()),
                    readers: vec![a.clone()],
                    skills: scan::scan(&path, depth(a)),
                }),
            }
            keys.push(key);
        }
        let copies: Vec<(String, String)> = found
            .iter()
            .filter(|r| keys.contains(&r.path))
            .flat_map(|r| r.skills.iter().map(|s| (format!("{}/SKILL.md", crate::env::resolve_path(&s.dir).to_string_lossy()), s.name.clone())))
            .collect();
        let (switchable, mut off) = disabled(a, &copies);
        // Without a switch of its own: what AgentPlus moved out of its folder.
        let parked = write::off_dir(a);
        if !switchable && parked.is_dir() {
            let skills = scan::scan(&parked, depth(a));
            off.extend(skills.iter().map(|s| s.name.clone()));
            found.push(SkillRoot { path: display_path(&parked), kind: Kind::Off, exists: true, owner: Some(a.clone()), readers: vec![], skills });
        }
        out.push(AgentSkills { agent: a.clone(), supported: true, roots: keys, disabled: off, switchable });
    }
    let lib = library_dir();
    found.push(SkillRoot { path: display_path(&lib), kind: Kind::Library, exists: lib.is_dir(), owner: None, readers: vec![], skills: scan::scan(&lib, 1) });
    Overview { roots: found, agents: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use std::fs;

    fn skill(dir: &Path, name: &str) {
        let d = dir.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: {name} skill\n---\n")).unwrap();
    }

    #[test]
    fn shared_folders_are_listed_once_with_their_readers() {
        let h = TestHome::new("skills-overview");
        skill(&h.0.join(".agents/skills"), "shared-one");
        skill(&h.0.join(".codex/skills"), "frontend-design");
        skill(&h.0.join(".codex/skills/.system"), "imagegen");
        skill(&h.0.join(".claude/skills"), "claude-only");
        skill(&h.0.join(".hermes/skills/creative"), "ascii-art");
        fs::create_dir_all(h.0.join(".codex")).unwrap();
        fs::write(h.0.join(".codex/config.toml"), format!("[[skills.config]]\npath = '{}'\nenabled = false\n", h.0.join(".codex/skills/frontend-design/SKILL.md").display())).unwrap();
        fs::create_dir_all(h.0.join(".gemini")).unwrap();
        fs::write(h.0.join(".gemini/settings.json"), r#"{"skills":{"disabled":["shared-one"]}}"#).unwrap();
        let ids: Vec<String> = [codex::ID, gemini::ID, opencode::ID, claude::ID, hermes::ID, pi::ID].map(String::from).to_vec();
        let o = overview(&ids);
        let root = |p: &str| o.roots.iter().find(|r| norm(&r.path).ends_with(p)).unwrap_or_else(|| panic!("{p}: {:?}", o.roots.iter().map(|r| &r.path).collect::<Vec<_>>()));
        let sh = root(".agents/skills");
        assert_eq!((sh.kind, sh.readers.clone()), (Kind::Shared, vec!["codex".to_string(), "gemini".into(), "opencode".into(), "pi".into()]));
        assert_eq!(sh.skills[0].name, "shared-one");
        let cl = root(".claude/skills");
        // Claude's own folder, also read by OpenCode.
        assert_eq!((cl.kind, cl.owner.as_deref(), cl.readers.len()), (Kind::Own, Some("claude"), 2));
        assert_eq!(root(".system").kind, Kind::Builtin);
        assert_eq!(root(".hermes/skills").skills[0].id, "creative/ascii-art");
        let agent = |a: &str| o.agents.iter().find(|x| x.agent == a).unwrap();
        assert_eq!(agent(codex::ID).disabled, ["frontend-design"]);
        assert_eq!(agent(gemini::ID).disabled, ["shared-one"]);
        assert!(!agent(pi::ID).switchable);
        assert_eq!(agent(codex::ID).roots.len(), 3);
    }
}

#[cfg(test)]
mod dump {
    /// Read-only summary of the skills folders on this machine.
    /// `cargo test --lib skills::dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_skills() {
        let t = std::time::Instant::now();
        let ids: Vec<String> = crate::adapters::ALL.iter().map(|s| s.to_string()).collect();
        let o = super::overview(&ids);
        for r in &o.roots {
            let bad = r.skills.iter().filter(|s| s.problem.is_some()).count();
            println!("{:<60} {:?} exists={} owner={:?} readers={:?} skills={} problems={}", r.path, r.kind, r.exists, r.owner, r.readers, r.skills.len(), bad);
        }
        for a in &o.agents {
            println!("{:<10} supported={} switchable={} disabled={:?} roots={}", a.agent, a.supported, a.switchable, a.disabled, a.roots.len());
        }
        println!("{} ms", t.elapsed().as_millis());
    }
}
