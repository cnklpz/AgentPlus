//! OpenCode (desktop app and CLI): `~/.config/opencode/opencode.json(c)` with
//! `provider.<id>` entries, keys in `~/.local/share/opencode/auth.json` (or inline
//! `options.apiKey`). Disabling uses OpenCode's own `disabled_providers` list. Providers
//! logged in through `opencode auth` without a config entry show up read-only.

use super::msg;
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
pub const NAME: &str = "OpenCode";
/// Any of `CONFIG_FILES` also counts.
pub const MARKER: &str = "opencode.json";
/// The global config files OpenCode reads, in the order AgentPlus picks one to edit.
pub const CONFIG_FILES: [&str; 3] = ["opencode.jsonc", MARKER, "config.json"];
pub const WSL_SCRIPT: &str = "(command -v opencode >/dev/null && opencode --version || $HOME/.opencode/bin/opencode --version) 2>/dev/null; pgrep -x opencode >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".config/opencode";

/// `~/.config/opencode`.
pub fn default_dir() -> PathBuf {
    home().join(".config").join("opencode")
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

/// The desktop app, else the CLI.
pub fn detect() -> Install {
    crate::process::detect_opencode()
}

/// The config file OpenCode reads: the first existing of `CONFIG_FILES`, else a new opencode.json.
fn config_path() -> PathBuf {
    let d = dir();
    CONFIG_FILES.iter().map(|n| d.join(n)).find(|p| p.exists()).unwrap_or_else(|| d.join(MARKER))
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
                out.push(Provider::builtin(
                    id.clone(),
                    id.clone(),
                    if kind == "oauth" { l("账号登录（opencode auth）", "Account sign-in (opencode auth)") } else { l("内置供应商 · API Key", "Built-in provider · API key") },
                    "chat",
                    l("内置", "Built-in"),
                    vec![
                        Kv::mono(lbl::credentials(), format!("auth.json · {id} · {}", if kind == "oauth" { l("OAuth 登录", "OAuth sign-in") } else { l("API Key", "API key") })),
                        Kv::text(lbl::note(), l("OpenCode 内置的供应商，模型列表来自 models.dev，在 OpenCode 里用 /models 选择", "A provider built into OpenCode. Its model list comes from models.dev; pick models with /models in OpenCode.")),
                    ],
                ));
            }
        }
    }
    out
}

pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![f.file(), display_path(&auth_path())]);
    let root = store::load();
    let cfg = match f.load(true) {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push(msg::comments_readonly(&config_path().file_name().unwrap().to_string_lossy()));
            }
            cfg
        }
        Err(e) => {
            st.fail(e);
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
        Kv::text(lbl::custom_providers(), lbl::names_or_none(on.iter().map(|p| &p.name))),
        Kv::mono(lbl::default_model(), get_s("model").unwrap_or_else(|| "-".into())),
        Kv::mono(lbl::small_model(), get_s("small_model").unwrap_or_else(|| "-".into())),
        Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")),
        Kv::mono(lbl::config_file(), f.file()),
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
            Op::SetProviderModels { .. } => return Err(msg::models_per_provider()),
            Op::SetModelRoles { .. } => return Err(msg::roles_claude_only()),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            _ => unreachable!("handled by ocfmt"),
        }
    }

    if dirty.cfg && had_comments {
        return Err(msg::comments_not_written(&config_path().file_name().unwrap().to_string_lossy()));
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
