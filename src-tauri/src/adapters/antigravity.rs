//! Antigravity: Google's agentic desktop app (Electron, with a bundled language server).
//! It signs in with a Google account only, so there are no providers or keys to manage.
//! Antigravity 2 keeps the global customizations in `~/.gemini/config` (`mcp_config.json`,
//! `skills/`, `plugins/`) and its own state in `~/.gemini/antigravity`; Antigravity 1.x (the
//! VS Code fork) read `~/.gemini/antigravity/mcp_config.json`.

use super::{Endpoint, Plan};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::util::*;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const ID: &str = "antigravity";
pub const NAME: &str = "Antigravity";
pub const MARKER: &str = "mcp_config.json";
/// A desktop app only: nothing to find in WSL.
pub const WSL_SCRIPT: &str = "";
pub const WSL_MARKER: &str = "";

/// `~/.gemini/config`.
pub fn default_dir() -> PathBuf {
    home().join(".gemini").join("config")
}

pub fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

pub fn detect() -> Install {
    crate::process::detect_antigravity()
}

/// The major version in a version string ("2.23.1" → 2).
fn major_of(version: Option<&str>) -> Option<u32> {
    version?.split('.').next()?.trim().parse().ok()
}

/// The detected release's major version, kept for a minute: the MCP and skills folders
/// depend on it and are asked for often, while a detection walks the registry and the
/// processes. `state` refreshes it with the detection it already has.
fn major() -> Option<u32> {
    let mut c = CACHED.lock().unwrap_or_else(|e| e.into_inner());
    match *c {
        Some((at, m)) if at.elapsed() < Duration::from_secs(60) => m,
        _ => {
            let m = major_of(detect().version.as_deref());
            *c = Some((Instant::now(), m));
            m
        }
    }
}

static CACHED: Mutex<Option<(Instant, Option<u32>)>> = Mutex::new(None);

/// Antigravity 1.x: the global MCP file sat in its own folder.
fn legacy(major: Option<u32>) -> bool {
    major.is_some_and(|m| m < 2)
}

/// The folder of global customizations for a release: Antigravity 1.x kept them in its own
/// folder (unless one was picked by hand).
fn customizations_dir(major: Option<u32>) -> PathBuf {
    if legacy(major) && super::dir_override(ID).is_none() {
        home().join(".gemini").join("antigravity")
    } else {
        dir()
    }
}

/// Where the global MCP servers are kept for the installed release.
pub fn mcp_path() -> PathBuf {
    customizations_dir(major()).join(MARKER)
}

/// Where the global skills are kept for the installed release.
pub fn skills_dir() -> PathBuf {
    customizations_dir(major()).join("skills")
}

/// Skills Antigravity ships and mounts by name (read-only).
pub fn builtin_skills_dir() -> PathBuf {
    home().join(".gemini").join("antigravity").join("builtin").join("skills")
}

pub fn state(inst: &Install) -> AgentState {
    *CACHED.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), major_of(inst.version.as_deref())));
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![display_path(&mcp_path())]);
    st.account_only = true;
    st.providers.push(Provider::builtin(
        "google",
        l("Google account (built-in)", "Google 账号内置"),
        l("Google sign-in", "Google 账号登录"),
        "gemini",
        l("Account", "账号"),
        vec![
            Kv::text(lbl::auth(), l("Google sign-in", "Google 账号登录")),
            Kv::text(lbl::note(), l("Antigravity chooses the models; it has no custom providers or API keys", "模型由 Antigravity 提供，不支持自定义供应商和 API 密钥")),
        ],
    ));
    st.current = vec![Kv::text(lbl::auth(), l("Google sign-in", "Google 账号登录"))];
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    Err(super::msg::no_provider(id))
}

fn no_providers() -> anyhow::Error {
    anyhow!(l("Antigravity signs in with a Google account; it has no custom providers or models", "Antigravity 用 Google 账号登录，不支持自定义供应商和模型"))
}

pub fn plan(ops: &[Op], _dry_run: bool) -> Result<Plan> {
    // Nothing in Antigravity's own config is AgentPlus's to write.
    match ops.first() {
        None => Ok((Diff::default(), vec![], None)),
        Some(Op::SetSetting { key, .. }) => Err(super::msg::unknown_setting(key)),
        Some(Op::SetModelRoles { .. }) => Err(super::msg::no_model_roles()),
        Some(Op::UpsertMcp { .. } | Op::DeleteMcp { .. } | Op::SetMcpEnabled { .. } | Op::SetPluginEnabled { .. }) => unreachable!("planned in mcp::write or plugins"),
        Some(_) => Err(no_providers()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_and_models_are_refused() {
        let _h = TestHome::new("antigravity-providers");
        let op = Op::DeleteProvider { provider: "google".into() };
        assert_eq!(plan(&[op], true).err().unwrap().to_string(), "Antigravity 用 Google 账号登录，不支持自定义供应商和模型");
        assert!(provider_endpoint("google").is_err());
    }

    #[test]
    fn release_one_kept_its_customizations_elsewhere() {
        assert!(legacy(Some(1)));
        assert!(!legacy(Some(2)) && !legacy(None));
        assert_eq!(major_of(Some("2.23.1")), Some(2));
        assert_eq!(major_of(Some("1.11.5")), Some(1));
        assert_eq!(major_of(Some("")), None);
        assert_eq!(major_of(None), None);
        let _h = TestHome::new("antigravity-legacy");
        assert_eq!(customizations_dir(Some(1)), home().join(".gemini").join("antigravity"));
        assert_eq!(customizations_dir(Some(2)), home().join(".gemini").join("config"));
        assert_eq!(customizations_dir(None), home().join(".gemini").join("config"));
    }
}
