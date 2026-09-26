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
    Fmt::new(ID, config_path(), Some(auth_path()), true)
}

/// "provider/model" for every visible model of the enabled providers (suggestions for `model`).
pub(super) fn model_choices(providers: &[Provider]) -> Vec<String> {
    providers
        .iter()
        .filter(|p| p.enabled)
        .flat_map(|p| p.models.iter().filter(|m| m.visible).map(move |m| format!("{}/{}", p.id, m.id)))
        .collect()
}

/// Read-only cards for the providers logged in with `opencode auth` that have no config
/// entry (built into OpenCode, models from models.dev).
pub(super) fn auth_only(f: &Fmt, known: &[Provider], off: &[String]) -> Vec<Provider> {
    let about = l("A provider built into OpenCode. Its model list comes from models.dev; pick models with /models in OpenCode.", "OpenCode 内置的供应商，模型列表来自 models.dev，在 OpenCode 里用 /models 选择");
    f.auth_only(known, "opencode auth", about, off)
}

pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![f.file(), display_path(&auth_path())]);
    let root = store::load();
    let Some(cfg) = f.load_for_state(&mut st, true) else { return st };
    st.providers = f.providers(&cfg, &root);
    let extra = auth_only(&f, &st.providers, &f.disabled(&cfg));
    st.providers.extend(extra);

    let ids: Vec<String> = st.providers.iter().map(|p| p.id.clone()).collect();
    st.settings = ocsettings::rows(&cfg, None, Scope::Global, &model_choices(&st.providers), &ids);
    st.current = f.summary(&cfg, &st.providers);
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
                dirty.cfg |= ocsettings::apply(&mut cfg, key, value, Scope::Global, &mut diff, &ef)?;
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("OpenCode manages providers by enabling/disabling them. Pick models with /models in OpenCode.", "OpenCode 按启用/停用管理供应商，在 OpenCode 里用 /models 选择模型"))),
            Op::SetProviderModels { .. } => return Err(msg::models_per_provider()),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            _ => unreachable!("handled by ocfmt"),
        }
    }

    f.guard_comments(&dirty, had_comments)?;
    let (written, backup_dir) = f.commit(&cfg, cfg_meta, &auth, &dirty, dry_run, |t| backup(ID, t))?;
    if !dry_run && dirty.store {
        store::save(&root)?;
    }
    Ok((diff, written, backup_dir))
}
