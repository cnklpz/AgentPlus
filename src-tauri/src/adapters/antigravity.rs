//! Antigravity: Google's agentic desktop app (Electron, with a bundled language server).
//! It signs in with a Google account only, so there are no providers or keys to manage.
//! Antigravity 2 keeps the global customizations in `~/.gemini/config` (`mcp_config.json`,
//! `skills/`, `plugins/`) and its own state in `~/.gemini/antigravity`; Antigravity 1.x (the
//! VS Code fork) read `~/.gemini/antigravity/mcp_config.json`.
//! AgentPlus adds a Chinese UI to Antigravity 2 (see `agzh`).

use super::{Endpoint, Plan};
use crate::agzh;
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
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
/// Setting keys (stored by AgentPlus, see `agzh::set_on` / `agzh::set_lines`).
const ZH_SETTING: &str = "zh_ui";
const ZH_CUSTOM_SETTING: &str = "zh_custom";

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
    let old = legacy(major_of(inst.version.as_deref()));
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
    let root = store::load();
    let on = agzh::is_on(&root);
    let custom = agzh::lines(&root);
    st.settings = vec![
        bool_setting(
            ZH_SETTING,
            "AgentPlus",
            l("Chinese UI", "汉化界面"),
            l(
                "While AgentPlus runs, translates Antigravity's own menus, buttons and settings into Chinese through its local debug port. Conversations, code and files stay as they are. Open windows switch within a few seconds; no restart needed.",
                "AgentPlus 运行期间，通过 Antigravity 的本地调试端口把它自己的菜单、按钮和设置翻译成中文；对话、代码和文件内容不动。已打开的窗口几秒内生效，不用重启。",
            ),
            on,
        ),
        Setting {
            kind: "list".into(),
            value: serde_json::json!(custom),
            ..bool_setting(
                ZH_CUSTOM_SETTING,
                "AgentPlus",
                l("Custom translations", "自定义译文"),
                l(
                    "One per line: the English text, \"=\", then the translation. Overrides the built-in dictionary; leave the right side empty to keep a text in English. In the English text, {n} matches a number and {x} any text; use the same names in the translation.",
                    "每行一条：英文 = 中文。会覆盖内置词典；右边留空表示这句保持英文。英文里 {n} 匹配数字、{x} 匹配任意文字，译文里用同样的名字引用，例如 {n} tasks = {n} 个任务。",
                ),
                false,
            )
        },
    ];
    // Right after applying, the thread is already at it: wait (briefly) for that check.
    if on && inst.running {
        agzh::settle(Duration::from_secs(2));
    }
    let zh = if !on {
        l("Off", "关").to_string()
    } else if !inst.running {
        l("On · Antigravity isn't running", "开 · Antigravity 未运行").to_string()
    } else {
        tr!("On · {} window(s) translated", "开 · 已翻译 {} 个窗口", agzh::windows())
    };
    st.current = vec![Kv::text(lbl::auth(), l("Google sign-in", "Google 账号登录")), Kv::text(l("Chinese UI", "汉化界面"), zh)];
    if old && on {
        st.notes.push(l("The Chinese UI works with Antigravity 2 and later; this is Antigravity 1.", "汉化界面需要 Antigravity 2 及以上版本，当前是 Antigravity 1。").into());
    }
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    Err(super::msg::no_provider(id))
}

fn no_providers() -> anyhow::Error {
    anyhow!(l("Antigravity signs in with a Google account; it has no custom providers or models", "Antigravity 用 Google 账号登录，不支持自定义供应商和模型"))
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let mut root = store::load();
    let mut diff = Diff::default();
    let file = l("AgentPlus settings", "AgentPlus 设置");
    let mut dirty = false;
    for op in ops {
        match op {
            Op::SetSetting { key, value } if key == ZH_SETTING => {
                let on = value.as_bool().unwrap_or(false);
                if agzh::is_on(&root) != on {
                    diff.push(file, if on { l("Chinese UI → on", "汉化界面 → 开") } else { l("Chinese UI → off", "汉化界面 → 关") }, on);
                    agzh::set_on(&mut root, on);
                    dirty = true;
                }
            }
            Op::SetSetting { key, value } if key == ZH_CUSTOM_SETTING => {
                let lines: Vec<String> = str_list(Some(value)).unwrap_or_default().into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                agzh::parse_custom(&lines)?;
                let was = agzh::lines(&root);
                if was != lines {
                    for gone in was.iter().filter(|w| !lines.contains(w)) {
                        diff.push(file, &tr!("Translation: {gone}", "译文：{gone}"), false);
                    }
                    for new in lines.iter().filter(|x| !was.contains(x)) {
                        diff.push(file, &tr!("Translation: {new}", "译文：{new}"), true);
                    }
                    agzh::set_lines(&mut root, &lines);
                    dirty = true;
                }
            }
            Op::SetSetting { key, .. } => return Err(super::msg::unknown_setting(key)),
            Op::UpsertProvider { .. } | Op::ImportProvider { .. } | Op::DeleteProvider { .. } | Op::SetProviderEnabled { .. } | Op::SetCurrentProvider { .. } | Op::SetModelVisible { .. } | Op::UpsertModel { .. } | Op::DeleteModel { .. } | Op::SetProviderModels { .. } => return Err(no_providers()),
            Op::SetModelRoles { .. } => return Err(super::msg::no_model_roles()),
            Op::UpsertMcp { .. } | Op::DeleteMcp { .. } | Op::SetMcpEnabled { .. } | Op::SetPluginEnabled { .. } => unreachable!("planned in mcp::write or plugins"),
        }
    }
    if !dry_run && dirty {
        store::save(&root)?;
        agzh::sync();
    }
    Ok((diff, vec![], None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(key: &str, value: serde_json::Value) -> Op {
        Op::SetSetting { key: key.into(), value }
    }

    #[test]
    fn the_chinese_ui_switch_and_entries_are_stored() {
        let _h = TestHome::new("antigravity-zh");
        let (d, written, _) = plan(&[set(ZH_SETTING, json!(true)), set(ZH_CUSTOM_SETTING, json!([" Turbo = 极速 ", "", "Review ="]))], false).unwrap();
        assert!(written.is_empty());
        assert_eq!(d.groups[0].lines.len(), 3);
        let root = store::load();
        assert!(agzh::is_on(&root));
        assert_eq!(agzh::lines(&root), ["Turbo = 极速", "Review ="]);
        // Unchanged: nothing to apply.
        let (d, _, _) = plan(&[set(ZH_SETTING, json!(true)), set(ZH_CUSTOM_SETTING, json!(["Turbo = 极速", "Review ="]))], true).unwrap();
        assert!(d.groups.is_empty());
        // A line that isn't an entry is refused and nothing is stored.
        let err = plan(&[set(ZH_SETTING, json!(false)), set(ZH_CUSTOM_SETTING, json!(["oops"]))], false).err().unwrap().to_string();
        assert_eq!(err, "不是「英文 = 中文」的格式：oops");
        assert!(agzh::is_on(&store::load()));
    }

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
