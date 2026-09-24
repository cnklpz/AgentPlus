//! OpenCode project configs: `<folder>/opencode.json(c)`, addressed as agent
//! `opencode@<folder>` so every provider / model / settings command works on it.
//!
//! OpenCode deep-merges the project file over the global config: objects key by key,
//! arrays replaced (`instructions` is concatenated). So a project shows its own providers
//! (fully editable), the global ones it inherits (enable / disable only, which writes the
//! project's `disabled_providers`), and settings that are either set here or inherited.
//! Keys go to the global `auth.json` like everywhere else in OpenCode, never into the
//! project file (which is usually committed).

use super::msg;
use super::{Plan, Endpoint};
use super::ocfmt::{cfg_key, is_plain_key, Dirty, Fmt};
use super::ocsettings::{self, Scope};
use super::opencode;
use crate::model::*;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const PREFIX: &str = "opencode@";

/// The project files OpenCode reads in a folder, in the order AgentPlus picks one to edit.
const PROJECT_NAMES: [&str; 2] = ["opencode.jsonc", "opencode.json"];

pub fn is_project(agent: &str) -> bool {
    agent.starts_with(PREFIX)
}

pub fn agent_id(folder: &str) -> String {
    format!("{PREFIX}{folder}")
}

fn dir_of(agent: &str) -> Result<PathBuf> {
    let raw = agent.strip_prefix(PREFIX).filter(|s| !s.trim().is_empty()).ok_or_else(|| anyhow!(tr!("不是项目 {agent}", "Not a project: {agent}")))?;
    Ok(crate::env::resolve_path(raw))
}

/// The project file AgentPlus edits: an existing opencode.jsonc / opencode.json, else a new opencode.json.
pub fn config_path(dir: &Path) -> PathBuf {
    PROJECT_NAMES.iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join(PROJECT_NAMES[1]))
}

/// Each project keeps its own AgentPlus store section (stashed models), named by its agent id.
fn fmt(agent: &str, dir: &Path) -> Fmt {
    Fmt::new(agent, config_path(dir), Some(opencode::auth_path()), true)
}

/// The nearest folder at or above `dir` that holds `.git`.
pub(crate) fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// Other files OpenCode also merges for this folder: `.opencode/opencode.json(c)` here, and
/// project files in parent folders up to the git root.
fn other_files(dir: &Path, edited: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let top = git_root(dir);
    for d in dir.ancestors() {
        // Listed .json first, then .jsonc.
        for f in PROJECT_NAMES.iter().rev() {
            for p in [d.join(f), d.join(".opencode").join(f)] {
                if p.exists() && p != edited {
                    out.push(p);
                }
            }
        }
        // Without a git repo OpenCode only looks in the folder itself.
        if top.as_deref().map(|t| t == d).unwrap_or(true) {
            break;
        }
    }
    out
}

fn global_cfg() -> Result<Value> {
    opencode::fmt().load(true).map(|x| x.0)
}

/// Effective `disabled_providers`: the project's list replaces the global one when present.
fn disabled(cfg: &Value, global: &Value) -> Vec<String> {
    str_list(cfg.get("disabled_providers")).or_else(|| str_list(global.get("disabled_providers"))).unwrap_or_default()
}

fn provider_ids(cfg: &Value) -> Vec<String> {
    cfg.get("provider").and_then(|p| p.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

pub fn state(agent: &str) -> Result<AgentState> {
    let dir = dir_of(agent)?;
    let f = fmt(agent, &dir);
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| display_path(&dir));
    // A folder is always "installed"; OpenCode itself is detected on the global entry.
    let found = crate::process::Install { installed: true, ..Default::default() };
    let mut st = super::new_state(agent, &name, &found, "multi", &dir, vec![f.file(), display_path(&opencode::auth_path())]);
    if !dir.is_dir() {
        st.readonly = true;
        st.notes.push(tr!("找不到文件夹 {}，可能已经移动或删除。", "Folder {} not found. It may have been moved or deleted.", display_path(&dir)));
        return Ok(st);
    }
    let exists = f.path.exists();
    let Some(cfg) = f.load_for_state(&mut st, true) else { return Ok(st) };
    let gf = opencode::fmt();
    let gcfg = global_cfg().unwrap_or_else(|e| {
        st.notes.push(tr!("全局配置读取失败，继承的内容没有显示：{e}", "Couldn't read the global config, so inherited items aren't shown: {e}"));
        json!({})
    });
    let root = store::load();
    let off = disabled(&cfg, &gcfg);
    let global_ids = provider_ids(&gcfg);

    let mut own = f.providers_with(&cfg, &root, &off);
    for p in own.iter_mut() {
        p.details.push(Kv::text(lbl::source(), l("项目配置", "Project config")));
        if global_ids.contains(&p.id) {
            p.details.push(Kv::text(l("注意", "Note"), l("全局配置里也有同名供应商，OpenCode 会把两份合并（项目的优先）", "The global config has a provider with the same id. OpenCode merges the two (the project wins).")));
        }
    }
    let mut inherited: Vec<Provider> = gf.providers_with(&gcfg, &root, &off).into_iter().filter(|g| !own.iter().any(|p| p.id == g.id)).collect();
    for g in inherited.iter_mut() {
        g.editable = false;
        g.apis.push(l("全局", "Global").into());
        // Hidden global models are not in OpenCode's config at all.
        g.models.retain(|m| m.visible);
        for m in g.models.iter_mut() {
            m.readonly = true;
            m.deletable = false;
        }
        g.details.push(Kv::text(lbl::source(), l("全局配置（继承）：项目里只能启用或停用；要单独改地址或模型，先复制到项目", "Global config (inherited): the project can only enable or disable it. To change its base URL or models, copy it to the project first.")));
    }
    let mut all = own;
    all.append(&mut inherited);
    let extra = opencode::auth_only(&f, &all, &off);
    all.extend(extra);
    st.providers = all;

    let ids: Vec<String> = st.providers.iter().map(|p| p.id.clone()).collect();
    st.settings = ocsettings::rows(&cfg, Some(&gcfg), Scope::Project, &opencode::model_choices(&st.providers), &ids);

    let own_names: Vec<&str> = st.providers.iter().filter(|p| p.editable).map(|p| p.name.as_str()).collect();
    let model = cfg.get("model").and_then(|x| x.as_str()).map(String::from).or_else(|| gcfg.get("model").and_then(|x| x.as_str()).map(|m| tr!("{m}（继承全局）", "{m} (inherited from global)")));
    st.current = vec![
        Kv::mono(l("项目配置", "Project config"), if exists { f.file() } else { tr!("{}（还没有，应用时创建）", "{} (not created yet, will be created on apply)", f.file()) }),
        Kv::mono(l("全局配置", "Global config"), gf.file()),
        Kv::text(l("项目供应商", "Project providers"), lbl::names_or_none(&own_names)),
        Kv::mono(lbl::default_model(), model.unwrap_or_else(|| "-".into())),
    ];

    if !exists {
        st.notes.push(l("这个文件夹还没有 opencode.json：添加供应商或改设置并应用后会创建它。", "This folder has no opencode.json yet. It will be created when you add a provider or change a setting and apply.").into());
    }
    st.notes.push(l("项目配置和全局配置合并生效，同名的键以项目为准。API Key 统一存进 ~/.local/share/opencode/auth.json，不写进项目文件。", "The project config is merged with the global config; for keys in both, the project wins. API keys are stored in ~/.local/share/opencode/auth.json, never in the project file.").into());
    let others = other_files(&dir, &f.path);
    if !others.is_empty() {
        st.notes.push(tr!("OpenCode 还会读取：{}（AgentPlus 只编辑 {}）", "OpenCode also reads: {} (AgentPlus only edits {})", crate::i18n::join(&others.iter().map(|p| display_path(p)).collect::<Vec<_>>()), f.file()));
    }
    let inline: Vec<String> = cfg
        .get("provider")
        .and_then(|p| p.as_object())
        .map(|o| o.iter().filter(|(_, d)| cfg_key(d).is_some_and(is_plain_key)).map(|(id, _)| id.clone()).collect())
        .unwrap_or_default();
    if !inline.is_empty() && git_root(&dir).is_some() {
        st.notes.push(tr!("项目文件里有明文 apiKey（{}），提交到 git 前记得处理。", "The project file contains a plain-text apiKey ({}). Remember to deal with it before committing to git.", crate::i18n::join(&inline)));
    }
    Ok(st)
}

/// (base_url, key, api) of a project provider, or of an inherited global one.
pub fn endpoint(agent: &str, id: &str) -> Result<Endpoint> {
    let dir = dir_of(agent)?;
    let f = fmt(agent, &dir);
    let (cfg, _, _) = f.load(true)?;
    if cfg.pointer(&crate::util::jptr(&["provider", id])).is_some() {
        f.endpoint(id)
    } else {
        opencode::provider_endpoint(id)
    }
}

pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    let dir = dir_of(agent)?;
    require_dir(&dir)?;
    // Checks below need the global ids: never write a project against an unknown global config.
    let gcfg = global_cfg()?;
    let mut f = fmt(agent, &dir);
    // New project providers stay clear of global ids (Fmt also skips ids already in auth.json).
    f.reserved = provider_ids(&gcfg);
    let (mut cfg, cfg_meta, had_comments) = f.load(true)?;
    let mut auth = f.load_auth();
    let mut root = store::load();
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();
    let ef = f.file();

    let global_only = |cfg: &Value, id: &str| cfg.pointer(&crate::util::jptr(&["provider", id])).is_none() && gcfg.pointer(&crate::util::jptr(&["provider", id])).is_some();
    let inherited_err = |id: &str| anyhow!(tr!("「{id}」来自全局配置，在项目里只能启用或停用；要单独修改，先把它复制到项目", "\"{id}\" comes from the global config and can only be enabled or disabled in a project. To change it, copy it to the project first."));

    for op in ops {
        match op {
            Op::SetSetting { key, value } => {
                dirty.cfg |= ocsettings::apply(&mut cfg, key, value, Scope::Project, &mut diff, &ef)?;
                continue;
            }
            Op::SetProviderEnabled { .. } => {
                // The project list replaces the global one, so start from the global list.
                if cfg.get("disabled_providers").is_none() {
                    if let Some(g) = str_list(gcfg.get("disabled_providers")).filter(|g| !g.is_empty()) {
                        diff.push(&ef, tr!("disabled_providers = 全局的 {g:?}（项目的列表会整体替换全局的）", "disabled_providers = global {g:?} (the project list replaces the global one entirely)"), true);
                        cfg["disabled_providers"] = json!(g);
                        dirty.cfg = true;
                    }
                }
            }
            Op::SetModelVisible { provider, .. } | Op::UpsertModel { provider, .. } | Op::DeleteModel { provider, .. } | Op::DeleteProvider { provider }
                if global_only(&cfg, provider) =>
            {
                return Err(inherited_err(provider));
            }
            Op::UpsertProvider { provider: p } if p.id.as_deref().map(|id| global_only(&cfg, id)).unwrap_or(false) => {
                return Err(inherited_err(p.id.as_deref().unwrap()));
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("OpenCode 按启用/停用管理供应商，默认模型在「其他设置」里选", "OpenCode manages providers by enabling/disabling them. Choose the default model under \"Other settings\"."))),
            Op::SetProviderModels { .. } => return Err(msg::models_per_provider()),
            Op::SetModelRoles { .. } => return Err(msg::roles_claude_only()),
            _ => {}
        }
        if !f.apply(op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty)? {
            return Err(anyhow!(l("OpenCode 项目不支持这项改动", "OpenCode projects don't support this change")));
        }
    }

    f.guard_comments(&dirty, had_comments)?;
    let reason = tr!("项目配置 · {}", "Project config · {}", display_path(&dir));
    let (written, backup_dir) = f.commit(&cfg, cfg_meta, &auth, &dirty, dry_run, |t| backup_tagged(opencode::ID, t, &reason))?;
    if !dry_run && dirty.store {
        store::save(&root)?;
    }
    Ok((diff, written, backup_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ids() {
        assert!(is_project(&agent_id(r"D:\xm\demo")));
        assert!(!is_project("opencode"));
        assert!(dir_of("opencode@").is_err());
    }

    /// Read-only against this machine's OpenCode config: a temp project, dry runs only.
    /// `cargo test dump_project -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_project() {
        let tmp = std::env::temp_dir().join("agentplus-oc-dump");
        std::fs::create_dir_all(&tmp).unwrap();
        let agent = agent_id(&tmp.to_string_lossy());
        let st = crate::adapters::state(&agent).unwrap();
        println!("{} readonly={} notes={:?}", st.name, st.readonly, st.notes);
        for p in &st.providers {
            println!("   {} [{}] editable={} builtin={} enabled={} key={} models={}", p.id, p.apis.join("/"), p.editable, p.builtin, p.enabled, p.has_key, p.models.len());
        }
        for s in &st.settings {
            println!("   set {} {} = {} {:?}", s.kind, s.key, s.value, s.hints);
        }
        let first_global = st.providers.iter().find(|p| !p.editable && !p.builtin).map(|p| p.id.clone());
        let mut ops = vec![
            json!({"op": "set_setting", "key": "share", "value": "disabled"}),
            json!({"op": "set_setting", "key": "permission.bash", "value": "ask"}),
            json!({"op": "set_setting", "key": "instructions", "value": ["CONTRIBUTING.md"]}),
        ];
        if let Some(g) = &first_global {
            ops.push(json!({"op": "import_provider", "fromAgent": "opencode", "provider": g}));
            ops.push(json!({"op": "set_provider_enabled", "provider": g, "enabled": false}));
        }
        let ops: Vec<Op> = serde_json::from_value(json!(ops)).unwrap();
        let (d, w, b) = crate::adapters::plan(&agent, &ops, true).unwrap();
        assert!(w.is_empty() && b.is_none());
        for g in d.groups {
            println!("[{}] {:?}", g.file, g.lines.iter().map(|l| &l.text).collect::<Vec<_>>());
        }
        if let Some(g) = &first_global {
            let bad: Vec<Op> = serde_json::from_value(json!([{"op": "delete_model", "provider": g, "model": "x"}])).unwrap();
            println!("inherited edit → {}", crate::adapters::plan(&agent, &bad, true).err().unwrap());
        }
        let g = crate::adapters::state("opencode").unwrap();
        for s in &g.settings {
            println!("   global {} {} = {}", s.kind, s.key, s.value);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A temp home with a global OpenCode config and an empty project folder in it.
    fn setup(tag: &str, global: &str, project: Option<&str>) -> (TestHome, String) {
        let h = TestHome::new(tag);
        let g = h.0.join(".config").join("opencode");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("opencode.json"), global).unwrap();
        let dir = h.0.join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(p) = project {
            std::fs::write(dir.join("opencode.json"), p).unwrap();
        }
        let agent = agent_id(&dir.to_string_lossy());
        (h, agent)
    }

    #[test]
    fn status_follows_the_effective_disabled_list() {
        let prov = r#"{ "options": { "baseURL": "https://r.example.com/v1" } }"#;
        let global = format!(r#"{{ "disabled_providers": ["relay", "g2"], "provider": {{ "g1": {prov}, "g2": {prov} }} }}"#);
        let (_h, agent) = setup("ocp-status", &global, Some(&format!(r#"{{ "provider": {{ "relay": {prov} }} }}"#)));
        let st = state(&agent).unwrap();
        let status = |id: &str| {
            let p = st.providers.iter().find(|p| p.id == id).unwrap();
            let rows: Vec<&str> = p.details.iter().filter(|kv| kv.k == lbl::status()).map(|kv| kv.v.as_str()).collect();
            (p.enabled, rows)
        };
        // The project has no list of its own, so the global one applies to its providers too.
        assert_eq!(status("relay"), (false, vec!["已停用（disabled_providers）"]));
        assert_eq!(status("g1"), (true, vec!["已启用"]));
        assert_eq!(status("g2"), (false, vec!["已停用（disabled_providers）"]));
    }

    #[test]
    fn broken_global_config_blocks_writes() {
        let (_h, agent) = setup("ocp-broken", "{ \"provider\": ", None);
        let st = state(&agent).unwrap();
        assert!(st.notes.iter().any(|n| n.starts_with("全局配置读取失败")), "{:?}", st.notes);
        let op: Op = serde_json::from_value(json!({"op": "upsert_provider", "provider": {"name": "Relay", "baseUrl": "https://r.example.com/v1", "api": "chat", "models": ["m1"]}})).unwrap();
        assert!(plan(&agent, &[op], true).is_err());
    }

    #[test]
    fn notes_name_other_files_and_plain_keys() {
        let key = |k: &str| format!(r#"{{ "options": {{ "baseURL": "https://r.example.com/v1", "apiKey": "{k}" }} }}"#);
        let project = format!(r#"{{ "provider": {{ "a": {}, "b": {}, "c": {}, "d": {} }} }}"#, key("sk-plain"), key("{env:B_KEY}"), key(" {file:~/k}"), key("{literal}"));
        let (h, agent) = setup("ocp-notes", "{}", Some(&project));
        let dir = h.0.join("proj");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".opencode")).unwrap();
        std::fs::write(dir.join(".opencode").join("opencode.jsonc"), "{}").unwrap();
        std::fs::write(dir.join(".opencode").join("opencode.json"), "{}").unwrap();
        let st = state(&agent).unwrap();
        let others = crate::i18n::join(&[display_path(&dir.join(".opencode").join("opencode.json")), display_path(&dir.join(".opencode").join("opencode.jsonc"))]);
        assert!(st.notes.iter().any(|n| n.starts_with(&format!("OpenCode 还会读取：{others}（"))), "{:?}", st.notes);
        // Only keys that are not {env:}/{file:} references count as plain text.
        assert!(st.notes.iter().any(|n| n.starts_with("项目文件里有明文 apiKey（a、d）")), "{:?}", st.notes);
    }

    #[test]
    fn missing_project_folder_is_an_error() {
        let (h, agent) = setup("ocp-missing", "{}", None);
        std::fs::remove_dir_all(h.0.join("proj")).unwrap();
        let op: Op = serde_json::from_value(json!({"op": "set_setting", "key": "share", "value": "disabled"})).unwrap();
        let e = plan(&agent, &[op], true).err().unwrap().to_string();
        assert!(e.starts_with("找不到文件夹："), "{e}");
    }

    #[test]
    fn new_project_provider_avoids_reserved_ids() {
        let tmp = std::env::temp_dir().join(format!("agentplus-oc-proj-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let agent = agent_id(&tmp.to_string_lossy());
        let mut f = Fmt::new(&agent, config_path(&tmp), None, true);
        f.reserved = vec!["relay".into()];
        let (mut cfg, _, _) = f.load(true).unwrap();
        let mut root = json!({});
        let mut auth = None;
        let mut diff = Diff::default();
        let mut dirty = Dirty::default();
        let op: Op = serde_json::from_value(json!({"op": "upsert_provider", "provider": {"name": "Relay", "baseUrl": "https://r.example.com/v1", "api": "chat", "models": ["m1"]}})).unwrap();
        f.apply(&op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty).unwrap();
        assert!(cfg.pointer("/provider/relay-2/options/baseURL").is_some());
        assert!(cfg.pointer("/provider/relay").is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
