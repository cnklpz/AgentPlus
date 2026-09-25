//! MiMo Desktop: engine config `~/.config/mimocode/mimocode.jsonc` (OpenCode-style
//! `provider.<id>.models`), app preferences in `%APPDATA%\Xiaomi MiMo\preferences.json`.
//! Provider and model editing is shared with OpenCode (see ocfmt).

use super::msg;
use super::{Plan, Endpoint};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use super::ocfmt::{Dirty, Fmt};
use serde_json::json;
use std::path::PathBuf;

pub const ID: &str = "mimo";
pub const NAME: &str = "MiMo Desktop";
pub const MARKER: &str = "mimocode.jsonc";
/// A desktop app only: nothing to find in WSL.
pub const WSL_SCRIPT: &str = "";
pub const WSL_MARKER: &str = "";
const SKILLS: [&str; 4] = ["agents", "claude", "codex", "opencode"];
/// (key, label zh, label en, description)
const PREFS: [(&str, &str, &str, &str); 4] = [
    ("trayEnabled", "托盘图标", "Tray icon", "trayEnabled"),
    ("voiceFeedback", "语音反馈", "Voice feedback", "voiceFeedback"),
    ("gitVersionPinEnabled", "Git 版本固定", "Git version pinning", "gitVersionPinEnabled"),
    ("uncommittedHintEnabled", "提示未提交的改动", "Hint about uncommitted changes", "uncommittedHintEnabled"),
];

/// `~/.config/mimocode`.
pub fn default_dir() -> PathBuf {
    home().join(".config").join("mimocode")
}

/// The desktop app (registry uninstall entry).
pub fn detect() -> Install {
    crate::process::detect_mimo()
}

fn engine_path() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir).join(MARKER)
}
fn app_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(home).join("Xiaomi MiMo")
}
fn prefs_path() -> PathBuf {
    app_dir().join("preferences.json")
}
fn catalog_path() -> PathBuf {
    app_dir().join("model-catalog.json")
}

fn fmt() -> Fmt {
    Fmt::new(ID, engine_path(), None, false)
}

pub fn state(inst: &Install) -> AgentState {
    let mut st = super::new_state(ID, NAME, inst, "multi", engine_path().parent().unwrap(), vec![display_path(&engine_path()), display_path(&prefs_path())]);
    let root = store::load();
    let f = fmt();

    // Account models shipped by MiMo itself (read-only).
    if let Ok((cat, _)) = read_json(&catalog_path()) {
        let models: Vec<Model> = cat
            .get("models")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter(|m| m.get("modelType").and_then(|x| x.as_str()) == Some("TEXT"))
                    .filter_map(|m| {
                        Some(Model {
                            id: m.get("id")?.as_str()?.into(),
                            name: m.get("name").and_then(|x| x.as_str()).map(String::from),
                            visible: true,
                            readonly: true,
                            ..Default::default()
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        st.providers.push(Provider {
            models,
            ..Provider::builtin(
                "account",
                l("MiMo 账号内置", "MiMo account (built-in)"),
                l("小米账号登录", "Xiaomi account sign-in"),
                "chat",
                l("账号", "Account"),
                vec![
                    Kv::text(lbl::auth(), l("小米账号登录", "Xiaomi account sign-in")),
                    Kv::mono(lbl::source(), "model-catalog.json"),
                    Kv::text(lbl::note(), l("MiMo Desktop 内置，模型列表由 MiMo 管理", "Built into MiMo Desktop; MiMo manages the model list")),
                ],
            )
        });
    }

    if let Some(cfg) = f.load_for_state(&mut st, false) {
        st.providers.extend(f.providers(&cfg, &root));
    }

    let prefs = read_json(&prefs_path()).ok().map(|x| x.0).unwrap_or(json!({}));
    let get_b = |k: &str| prefs.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let skills: Vec<String> = SKILLS
        .iter()
        .filter(|s| prefs.pointer(&format!("/skillPathCompat/{s}")).and_then(|x| x.as_bool()).unwrap_or(false))
        .map(|s| format!("~/.{s}"))
        .collect();
    st.settings = vec![chips_setting(
        "skills",
        l("技能", "Skills"),
        l("读取其他工具的技能目录", "Read other tools' skill folders"),
        l("skillPathCompat：只读兼容，MiMo 不会写入这些目录", "skillPathCompat: read-only compatibility; MiMo never writes to these folders"),
        skills.clone(),
        &["~/.agents", "~/.claude", "~/.codex", "~/.opencode"],
    )
    .with_hints(&[
        l("通用 Agent 技能目录", "Shared agent skills folder"),
        l("Claude Code 的技能", "Claude Code skills"),
        l("Codex 的技能", "Codex skills"),
        l("OpenCode 的技能", "OpenCode skills"),
    ])];
    st.settings.extend(PREFS.iter().map(|(k, zh, en, d)| bool_setting(k, l("应用", "App"), l(zh, en), d, get_b(k))));

    let on: Vec<&Provider> = st.providers.iter().filter(|p| p.enabled).collect();
    let vis: usize = on.iter().map(|p| p.models.iter().filter(|m| m.visible).count()).sum();
    st.current = vec![
        Kv::text(l("供应商", "Providers"), lbl::names_or_none(on.iter().map(|p| &p.name))),
        Kv::mono(lbl::default_model(), prefs.get("model").and_then(|x| x.as_str()).unwrap_or("-").to_string()),
        Kv::text(lbl::visible_models(), tr!("{vis} 个", "{vis}")),
        Kv::mono(l("技能兼容", "Skill compatibility"), if skills.is_empty() { l("无", "None").into() } else { skills.join(" ") }),
        Kv::text(l("托盘图标", "Tray icon"), crate::i18n::on_off(get_b("trayEnabled"))),
    ];
    st
}

/// Base URL, key and API kind of a provider, for fetching its model list.
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    fmt().endpoint(id)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let f = fmt();
    let (mut cfg, cfg_meta, had_comments) = f.load(false)?;
    // preferences.json is optional (state() shows defaults without it).
    let (mut prefs, prefs_meta) = read_json_object_or_new(&prefs_path())?;
    let mut root = store::load();
    let pf = display_path(&prefs_path());
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();
    let mut prefs_dirty = false;
    let mut no_auth = None;

    for op in ops {
        if f.apply(op, &mut cfg, &mut root, &mut no_auth, &mut diff, &mut dirty)? {
            continue;
        }
        match op {
            Op::SetSetting { key, value } => {
                if key == "skills" {
                    let want: Vec<String> = str_list(Some(value)).unwrap_or_default();
                    for s in SKILLS {
                        let on = want.iter().any(|w| w == &format!("~/.{s}"));
                        let was = prefs.pointer(&format!("/skillPathCompat/{s}")).and_then(|x| x.as_bool()).unwrap_or(false);
                        if on != was {
                            if !prefs.get("skillPathCompat").map(|x| x.is_object()).unwrap_or(false) {
                                prefs["skillPathCompat"] = json!({});
                            }
                            prefs["skillPathCompat"][s] = json!(on);
                            diff.push(&pf, format!("skillPathCompat.{s} = {on}"), on);
                            prefs_dirty = true;
                        }
                    }
                } else if PREFS.iter().any(|(k, ..)| k == key) {
                    let on = value.as_bool().unwrap_or(false);
                    if prefs.get(key).and_then(|x| x.as_bool()).unwrap_or(false) != on {
                        prefs[key.as_str()] = json!(on);
                        diff.push(&pf, format!("{key} = {on}"), on);
                        prefs_dirty = true;
                    }
                } else {
                    return Err(msg::unknown_setting(key));
                }
            }
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("MiMo Desktop 按启用/停用管理供应商", "MiMo Desktop manages providers by enabling and disabling them"))),
            Op::UpsertProvider { .. } | Op::DeleteProvider { .. } | Op::SetProviderEnabled { .. } | Op::SetModelVisible { .. } | Op::UpsertModel { .. } | Op::DeleteModel { .. } => unreachable!("handled by ocfmt"),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            Op::SetProviderModels { .. } => return Err(msg::models_per_provider()),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
        }
    }

    f.guard_comments(&dirty, had_comments)?;
    let (cfg_dirty, store_dirty) = (dirty.cfg, dirty.store);
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (cfg_dirty || prefs_dirty) {
        let mut targets = vec![];
        if cfg_dirty { targets.push(engine_path()) }
        if prefs_dirty { targets.push(prefs_path()) }
        backup_dir = Some(backup(ID, &targets)?);
        if cfg_dirty {
            write_json(&engine_path(), &cfg, cfg_meta)?;
            written.push(engine_path());
        }
        if prefs_dirty {
            write_json(&prefs_path(), &prefs, prefs_meta)?;
            written.push(prefs_path());
        }
    }
    if !dry_run && store_dirty {
        store::save(&root)?;
    }
    Ok((diff, written, backup_dir))
}
