//! OpenCode (desktop app and CLI): `~/.config/opencode/opencode.json(c)` with
//! `provider.<id>` entries, keys in `~/.local/share/opencode/auth.json` (or inline
//! `options.apiKey`). Disabling uses OpenCode's own `disabled_providers` list. Providers
//! logged in through `opencode auth` without a config entry show up read-only.

use super::{Plan, Endpoint};
use super::ocfmt::{Dirty, Fmt};
use super::ocsettings::{self, Scope};
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub const ID: &str = "opencode";

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(|| home().join(".config").join("opencode"))
}

/// The config file OpenCode reads: an existing opencode.jsonc, else opencode.json.
fn config_path() -> PathBuf {
    let d = dir();
    ["opencode.jsonc", "opencode.json", "config.json"]
        .iter()
        .map(|n| d.join(n))
        .find(|p| p.exists())
        .unwrap_or_else(|| d.join("opencode.json"))
}

pub(super) fn auth_path() -> PathBuf {
    home().join(".local").join("share").join("opencode").join("auth.json")
}

pub(super) fn fmt() -> Fmt {
    Fmt { agent: ID, path: config_path(), auth: Some(auth_path()), native_disable: true }
}

/// "provider/model" for every visible model of the enabled providers (suggestions for `model`).
pub(super) fn model_choices(providers: &[Provider]) -> Vec<String> {
    providers
        .iter()
        .filter(|p| p.enabled)
        .flat_map(|p| p.models.iter().filter(|m| m.visible).map(move |m| format!("{}/{}", p.id, m.id)))
        .collect()
}

/// Providers logged in with `opencode auth` that have no entry in `cfg` (read-only cards).
pub(super) fn auth_only(f: &Fmt, known: &[Provider]) -> Vec<Provider> {
    let mut out = vec![];
    if let Some((auth, _)) = f.load_auth() {
        if let Some(obj) = auth.as_object() {
            for (id, e) in obj {
                if known.iter().any(|p| &p.id == id) {
                    continue;
                }
                let kind = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
                out.push(Provider {
                    id: id.clone(),
                    name: id.clone(),
                    base_url: None,
                    host: if kind == "oauth" { l("账号登录（opencode auth）", "Account login (opencode auth)").into() } else { l("内置供应商 · API Key", "Built-in provider · API Key").into() },
                    apis: vec![l("内置", "Built-in").into()],
                    builtin: true,
                    enabled: true,
                    compatible: true,
                    reason: None,
                    models: vec![],
                    details: vec![
                        Kv::mono(l("凭据", "Credentials"), format!("auth.json · {id} · {}", if kind == "oauth" { l("OAuth 登录", "OAuth login") } else { "API Key" })),
                        Kv::text(l("说明", "About"), l("OpenCode 内置的供应商，模型列表来自 models.dev，在 OpenCode 里用 /models 选择", "A provider built into OpenCode. Its model list comes from models.dev; pick models with /models in OpenCode.")),
                    ],
                    editable: false,
                    api: "chat".into(),
                    has_key: true,
                    key_fp: None,
                    key_hint: None,
                    official_auth: false,
                });
            }
        }
    }
    out
}

pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = AgentState {
        id: ID.into(),
        name: "OpenCode".into(),
        installed: inst.installed,
        version: inst.version.clone(),
        running: inst.running,
        mode: "multi".into(),
        config_dir: dir().to_string_lossy().to_string(),
        files: vec![f.file(), display_path(&auth_path())],
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
    let root = store::load();
    let cfg = match f.load(true) {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push(tr!("{} 含注释，写回会丢失注释，已切换为只读。", "{} contains comments, which would be lost on write. Switched to read-only.", config_path().file_name().unwrap().to_string_lossy()));
            }
            cfg
        }
        Err(e) => {
            st.notes.push(e.to_string());
            st.readonly = true;
            return st;
        }
    };
    st.providers = f.providers(&cfg, &root);

    // Logged in with `opencode auth` but not configured here: built-in providers (models.dev).
    let extra = auth_only(&f, &st.providers);
    st.providers.extend(extra);

    let get_s = |k: &str| cfg.get(k).and_then(|x| x.as_str()).map(String::from);
    let ids: Vec<String> = st.providers.iter().map(|p| p.id.clone()).collect();
    st.settings = ocsettings::rows(&cfg, None, Scope::Global, &model_choices(&st.providers), &ids);
    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled && !p.builtin).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::text(l("自定义供应商", "Custom providers"), if on.is_empty() { l("无", "None").into() } else { on.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(l("、", ", ")) }),
        Kv::mono(l("默认模型", "Default model"), get_s("model").unwrap_or_else(|| "-".into())),
        Kv::mono(l("小模型", "Small model"), get_s("small_model").unwrap_or_else(|| "-".into())),
        Kv::text(l("可见模型", "Visible models"), tr!("{vis} 个", "{vis}")),
        Kv::mono(l("配置文件", "Config file"), f.file()),
    ];
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    fmt().endpoint(id)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let f = fmt();
    let (mut cfg, cfg_meta, had_comments) = f.load(true)?;
    let mut auth = f.load_auth();
    let mut root = store::load();
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();
    let ef = f.file();

    for op in ops {
        if f.apply(op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty)? {
            continue;
        }
        match op {
            Op::SetSetting { key, value } => {
                dirty.cfg |= ocsettings::apply(&mut cfg, key, value, &mut diff, &ef)?;
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("OpenCode 按启用/停用管理供应商，在 OpenCode 里用 /models 选择模型", "OpenCode manages providers by enabling/disabling them. Pick models with /models in OpenCode."))),
            Op::SetProviderModels { .. } => return Err(anyhow!(l("每个供应商的模型已经各自独立，请直接编辑模型", "Each provider already has its own models. Edit the models directly."))),
            Op::SetModelRoles { .. } => return Err(anyhow!(l("只有 Claude Code 需要分配模型角色", "Only Claude Code needs model roles."))),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            _ => unreachable!("handled by ocfmt"),
        }
    }

    if dirty.cfg && had_comments {
        return Err(anyhow!(l("配置文件含注释，为避免丢失注释不写入", "The config file contains comments; not writing to avoid losing them")));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (dirty.cfg || dirty.auth) {
        let mut targets = vec![];
        if dirty.cfg { targets.push(config_path()) }
        if dirty.auth { targets.push(auth_path()) }
        backup_dir = Some(backup(ID, &targets)?);
        if dirty.cfg {
            if let Some(d) = config_path().parent() {
                std::fs::create_dir_all(d)?;
            }
            write_json(&config_path(), &cfg, cfg_meta)?;
            written.push(config_path());
        }
        if dirty.auth {
            // Written in place (not tmp + rename) so the file keeps its owner-only permissions.
            let (a, _) = auth.as_ref().unwrap();
            if let Some(d) = auth_path().parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(auth_path(), serde_json::to_string_pretty(a)? + "\n")?;
            written.push(auth_path());
        }
    }
    if !dry_run && dirty.store {
        store::save(&root)?;
    }
    Ok((diff, written, backup_dir))
}
