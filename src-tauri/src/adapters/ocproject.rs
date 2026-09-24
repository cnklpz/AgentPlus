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
use super::ocfmt::{Dirty, Fmt, RESERVED};
use super::ocsettings::{self, Scope};
use super::opencode;
use crate::model::*;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const PREFIX: &str = "opencode@";

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
    ["opencode.jsonc", "opencode.json"].iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join("opencode.json"))
}

/// `Fmt::agent` names the AgentPlus store section for stashed (hidden) models; each project gets its own.
fn intern(s: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut m = NAMES.get_or_init(Default::default).lock().unwrap();
    if let Some(x) = m.get(s) {
        return x;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    m.insert(s.to_string(), leaked);
    leaked
}

fn fmt(agent: &str, dir: &Path) -> Fmt {
    Fmt { agent: intern(agent), path: config_path(dir), auth: Some(opencode::auth_path()), native_disable: true }
}

fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// Other files OpenCode also merges for this folder: `.opencode/opencode.json(c)` here, and
/// project files in parent folders up to the git root.
fn other_files(dir: &Path, edited: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let top = git_root(dir);
    for d in dir.ancestors() {
        for f in ["opencode.json", "opencode.jsonc"] {
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

fn global_cfg() -> Value {
    opencode::fmt().load(true).map(|x| x.0).unwrap_or_else(|_| json!({}))
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
    let mut st = AgentState {
        id: agent.into(),
        name,
        installed: true,
        version: None,
        running: false,
        mode: "multi".into(),
        config_dir: dir.to_string_lossy().to_string(),
        files: vec![f.file(), display_path(&opencode::auth_path())],
        current_provider: None,
        providers: vec![],
        catalog: None,
        catalog_file: None,
        settings: vec![],
        current: vec![],
        notes: vec![],
        readonly: false,
        fixed_pending: false,
        fixed_prompt: false,
        restartable: false,
        model_fields: vec![],
    };
    if !dir.is_dir() {
        st.readonly = true;
        st.notes.push(tr!("找不到文件夹 {}，可能已经移动或删除。", "Folder {} not found. It may have been moved or deleted.", display_path(&dir)));
        return Ok(st);
    }
    let exists = f.path.exists();
    let cfg = match f.load(true) {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push(msg::comments_readonly(&f.path.file_name().unwrap().to_string_lossy()));
            }
            cfg
        }
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return Ok(st);
        }
    };
    let gf = opencode::fmt();
    let gcfg = global_cfg();
    let root = store::load();
    let off = disabled(&cfg, &gcfg);
    let global_ids = provider_ids(&gcfg);

    let mut own = f.providers(&cfg, &root);
    for p in own.iter_mut() {
        p.enabled = !off.contains(&p.id);
        p.details.push(Kv::text(l("来源", "Source"), l("项目配置", "Project config")));
        if global_ids.contains(&p.id) {
            p.details.push(Kv::text(l("注意", "Note"), l("全局配置里也有同名供应商，OpenCode 会把两份合并（项目的优先）", "The global config has a provider with the same id. OpenCode merges the two (the project wins).")));
        }
    }
    let mut inherited: Vec<Provider> = gf.providers(&gcfg, &root).into_iter().filter(|g| !own.iter().any(|p| p.id == g.id)).collect();
    for g in inherited.iter_mut() {
        g.enabled = !off.contains(&g.id);
        g.editable = false;
        g.apis.push(l("全局", "Global").into());
        // Hidden global models are not in OpenCode's config at all.
        g.models.retain(|m| m.visible);
        for m in g.models.iter_mut() {
            m.readonly = true;
            m.deletable = false;
        }
        g.details.retain(|kv| kv.k != l("状态", "Status"));
        g.details.push(Kv::text(l("来源", "Source"), l("全局配置（继承）：项目里只能启用或停用；要单独改地址或模型，先复制到项目", "Global config (inherited): the project can only enable or disable it. To change its base URL or models, copy it to the project first.")));
    }
    let mut all = own;
    all.append(&mut inherited);
    let extra = opencode::auth_only(&f, &all);
    all.extend(extra);
    for p in all.iter_mut().filter(|p| p.has_key && p.base_url.is_some()) {
        if let Ok((_, Some(k), _)) = endpoint(agent, &p.id) {
            p.key_fp = Some(key_fingerprint(&k));
            p.key_hint = Some(mask_key(&k));
        }
    }
    st.providers = all;

    let ids: Vec<String> = st.providers.iter().map(|p| p.id.clone()).collect();
    st.settings = ocsettings::rows(&cfg, Some(&gcfg), Scope::Project, &opencode::model_choices(&st.providers), &ids);

    let own_names: Vec<&str> = st.providers.iter().filter(|p| p.editable).map(|p| p.name.as_str()).collect();
    let model = cfg.get("model").and_then(|x| x.as_str()).map(String::from).or_else(|| gcfg.get("model").and_then(|x| x.as_str()).map(|m| tr!("{m}（继承全局）", "{m} (inherited from global)")));
    st.current = vec![
        Kv::mono(l("项目配置", "Project config"), if exists { f.file() } else { tr!("{}（还没有，应用时创建）", "{} (not created yet, will be created on apply)", f.file()) }),
        Kv::mono(l("全局配置", "Global config"), gf.file()),
        Kv::text(l("项目供应商", "Project providers"), if own_names.is_empty() { l("无", "None").into() } else { own_names.join(l("、", ", ")) }),
        Kv::mono(l("默认模型", "Default model"), model.unwrap_or_else(|| "-".into())),
    ];

    if !exists {
        st.notes.push(l("这个文件夹还没有 opencode.json：添加供应商或改设置并应用后会创建它。", "This folder has no opencode.json yet. It will be created when you add a provider or change a setting and apply.").into());
    }
    st.notes.push(l("项目配置和全局配置合并生效，同名的键以项目为准。API Key 统一存进 ~/.local/share/opencode/auth.json，不写进项目文件。", "The project config is merged with the global config; for keys in both, the project wins. API keys are stored in ~/.local/share/opencode/auth.json, never in the project file.").into());
    let others = other_files(&dir, &f.path);
    if !others.is_empty() {
        st.notes.push(tr!("OpenCode 还会读取：{}（AgentPlus 只编辑 {}）", "OpenCode also reads: {} (AgentPlus only edits {})", others.iter().map(|p| display_path(p)).collect::<Vec<_>>().join(l("、", ", ")), f.file()));
    }
    let inline: Vec<String> = cfg
        .get("provider")
        .and_then(|p| p.as_object())
        .map(|o| o.iter().filter(|(_, d)| d.pointer("/options/apiKey").and_then(|k| k.as_str()).map(|k| !k.is_empty() && !k.starts_with('{')).unwrap_or(false)).map(|(id, _)| id.clone()).collect())
        .unwrap_or_default();
    if !inline.is_empty() && git_root(&dir).is_some() {
        st.notes.push(tr!("项目文件里有明文 apiKey（{}），提交到 git 前记得处理。", "The project file contains a plain-text apiKey ({}). Remember to deal with it before committing to git.", inline.join(l("、", ", "))));
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

/// Clears the reserved ids even when applying an op fails.
struct Reserve;

impl Reserve {
    fn set(ids: Vec<String>) -> Self {
        RESERVED.with(|r| *r.borrow_mut() = ids);
        Reserve
    }
}

impl Drop for Reserve {
    fn drop(&mut self) {
        RESERVED.with(|r| r.borrow_mut().clear());
    }
}

pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<Plan> {
    let dir = dir_of(agent)?;
    if !dir.is_dir() {
        return Err(anyhow!(tr!("找不到文件夹 {}", "Folder not found: {}", display_path(&dir))));
    }
    let f = fmt(agent, &dir);
    let path = f.path.clone();
    let (mut cfg, cfg_meta, had_comments) = f.load(true)?;
    let mut auth = f.load_auth();
    let mut root = store::load();
    let gcfg = global_cfg();
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();
    let ef = f.file();

    // New project providers stay clear of global ids (and keys already in auth.json).
    let mut reserved = provider_ids(&gcfg);
    if let Some((a, _)) = &auth {
        reserved.extend(a.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
    }
    let _guard = Reserve::set(reserved);

    let global_only = |cfg: &Value, id: &str| cfg.pointer(&crate::util::jptr(&["provider", id])).is_none() && gcfg.pointer(&crate::util::jptr(&["provider", id])).is_some();
    let inherited_err = |id: &str| anyhow!(tr!("「{id}」来自全局配置，在项目里只能启用或停用；要单独修改，先把它复制到项目", "\"{id}\" comes from the global config and can only be enabled or disabled in a project. To change it, copy it to the project first."));

    for op in ops {
        match op {
            Op::SetSetting { key, value } => {
                dirty.cfg |= ocsettings::apply(&mut cfg, key, value, &mut diff, &ef)?;
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

    if dirty.cfg && had_comments {
        return Err(msg::comments_not_written(&path.file_name().unwrap().to_string_lossy()));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (dirty.cfg || dirty.auth) {
        let mut targets = vec![];
        if dirty.cfg { targets.push(path.clone()) }
        if dirty.auth { targets.push(opencode::auth_path()) }
        backup_dir = Some(backup_tagged(opencode::ID, &targets, &tr!("项目配置 · {}", "Project config · {}", display_path(&dir)))?);
        if dirty.cfg {
            write_json(&path, &cfg, cfg_meta)?;
            written.push(path.clone());
        }
        if dirty.auth {
            // Written in place (not tmp + rename) so the file keeps its owner-only permissions.
            let (a, _) = auth.as_ref().unwrap();
            if let Some(d) = opencode::auth_path().parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(opencode::auth_path(), serde_json::to_string_pretty(a)? + "\n")?;
            written.push(opencode::auth_path());
        }
    }
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
        assert_eq!(intern("opencode@x").as_ptr(), intern("opencode@x").as_ptr());
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

    #[test]
    fn new_project_provider_avoids_reserved_ids() {
        let tmp = std::env::temp_dir().join(format!("agentplus-oc-proj-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let agent = agent_id(&tmp.to_string_lossy());
        let f = Fmt { agent: intern(&agent), path: config_path(&tmp), auth: None, native_disable: true };
        let (mut cfg, _, _) = f.load(true).unwrap();
        let mut root = json!({});
        let mut auth = None;
        let mut diff = Diff::default();
        let mut dirty = Dirty::default();
        let _g = Reserve::set(vec!["relay".into()]);
        let op: Op = serde_json::from_value(json!({"op": "upsert_provider", "provider": {"name": "Relay", "baseUrl": "https://r.example.com/v1", "api": "chat", "models": ["m1"]}})).unwrap();
        f.apply(&op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty).unwrap();
        assert!(cfg.pointer("/provider/relay-2/options/baseURL").is_some());
        assert!(cfg.pointer("/provider/relay").is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
