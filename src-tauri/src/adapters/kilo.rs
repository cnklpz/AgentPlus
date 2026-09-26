//! Kilo Code 7.x (CLI `kilo` and the VS Code extension; an OpenCode fork): the same
//! config format as OpenCode, in `~/.config/kilo/kilo.json(c)` (or `opencode.json` there,
//! or the file named by `KILO_CONFIG`), keys in `~/.local/share/kilo/auth.json`.
//! Disabling uses the native `disabled_providers` list; providers logged in through
//! `kilo auth` without a config entry show up read-only.

use super::msg;
use super::{Plan, Endpoint};
use super::ocfmt::{Dirty, Fmt};
use super::ocsettings::{self, Scope};
use crate::i18n::l;
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

pub const ID: &str = "kilo";
pub const NAME: &str = "Kilo Code";
/// Any of `CONFIG_FILES` also counts.
pub const MARKER: &str = "kilo.json";
/// The config files Kilo reads in a folder, in the order AgentPlus picks one to edit
/// (opencode.jsonc last, so it never wins over a file picked before it was listed).
pub const CONFIG_FILES: [&str; 4] = ["kilo.jsonc", MARKER, "opencode.json", "opencode.jsonc"];
pub const WSL_SCRIPT: &str = "(kilo --version || kilocode --version) 2>/dev/null | head -n 1; pgrep -x kilo >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".config/kilo";

/// The folder of `KILO_CONFIG` (Windows side only), else `~/.config/kilo`.
pub fn default_dir() -> PathBuf {
    env_config().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| home().join(".config").join("kilo"))
}

fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

/// `KILO_CONFIG` (a file path) only applies to the Windows side.
fn env_config() -> Option<PathBuf> {
    crate::env::agent_var("KILO_CONFIG").map(PathBuf::from)
}

/// The config file Kilo reads: `KILO_CONFIG`, else the first existing of `CONFIG_FILES`,
/// else a new kilo.json.
fn config_path() -> PathBuf {
    if super::dir_override(ID).is_none() {
        if let Some(p) = env_config() {
            return p;
        }
    }
    let d = dir();
    CONFIG_FILES.iter().map(|n| d.join(n)).find(|p| p.exists()).unwrap_or_else(|| d.join(MARKER))
}

fn auth_path() -> PathBuf {
    home().join(".local").join("share").join("kilo").join("auth.json")
}

fn fmt() -> Fmt {
    Fmt::new(ID, config_path(), Some(auth_path()), true)
}

// ---------- detection ----------

/// Newest `kilocode.kilo-code-<version>` folder in the VS Code extensions dir.
fn vscode_extension() -> Option<String> {
    let dir = dirs::home_dir()?.join(".vscode").join("extensions");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.strip_prefix("kilocode.kilo-code-").map(|v| v.split('-').next().unwrap_or(v).to_string())
        })
        .max_by_key(|v| v.split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>())
}

/// The `kilo` CLI (npm `@kilocode/cli`, also under another npm prefix on PATH, a `kilo`
/// binary on PATH or from the installer in ~/.kilo/bin) or the VS Code extension.
pub fn detect() -> Install {
    let mut inst = Install::default();
    let installer = dirs::home_dir().map(|h| h.join(".kilo").join("bin").join(crate::process::exe("kilo"))).filter(|p| p.is_file());
    let on_path = crate::process::on_path(&["kilo.exe", "kilo.cmd", "kilocode.cmd"]).or(installer);
    if let Some(v) = crate::process::npm_version_near("@kilocode/cli", on_path.as_deref()) {
        inst.installed = true;
        inst.version = Some(v);
    } else if let Some(exe) = on_path {
        inst.installed = true;
        if crate::process::is_exe(&exe) {
            inst.version = crate::process::cli_version(&exe);
        }
    } else if let Some(v) = vscode_extension() {
        inst.installed = true;
        inst.version = Some(v);
    }
    let name = crate::process::exe("kilo");
    inst.running = inst.installed && crate::process::any_process(|n, _| n.eq_ignore_ascii_case(&name));
    inst
}

// ---------- state / plan ----------

/// The opencode.json settings Kilo's page offers (Kilo keeps OpenCode's schema for them).
const SETTINGS: [&str; 2] = ["autoupdate", "share"];

pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![f.file(), display_path(&auth_path())]);
    let root = store::load();
    let Some(cfg) = f.load_for_state(&mut st, true) else { return st };
    st.providers = f.providers(&cfg, &root);

    // Logged in with `kilo auth` (Kilo account or a built-in provider) but not configured here.
    let about = l("A provider built into Kilo Code. Its model list comes from models.dev / the Kilo gateway; pick models with /models in Kilo.", "Kilo Code 内置的供应商，模型列表来自 models.dev / Kilo 网关，在 Kilo 里用 /models 选择");
    let extra = f.auth_only(&st.providers, "kilo auth", about, &f.disabled(&cfg));
    st.providers.extend(extra);

    st.settings = ocsettings::rows_only(&cfg, &SETTINGS);
    for s in st.settings.iter_mut() {
        s.group = NAME.into();
    }
    st.current = f.summary(&cfg, &st.providers);
    st.notes.push(
        l(
            "The Kilo Code CLI and VS Code extension share this config; changes apply to new sessions.",
            "Kilo Code CLI 与 VS Code 扩展共用这份配置；改动对新会话生效。",
        )
        .into(),
    );
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    fmt().endpoint(id)
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let f = fmt();
    let existed = f.path.exists();
    let (mut cfg, cfg_meta, had_comments) = f.load(true)?;
    if !existed {
        // ocfmt seeds OpenCode's schema URL; a new Kilo file goes without one.
        if let Some(o) = cfg.as_object_mut() {
            o.remove("$schema");
        }
    }
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
            Op::SetSetting { key, value } if SETTINGS.contains(&key.as_str()) => {
                dirty.cfg |= ocsettings::apply(&mut cfg, key, value, Scope::Global, &mut diff, &ef)?;
            }
            Op::SetSetting { key, .. } => return Err(msg::unknown_setting(key)),
            Op::SetCurrentProvider { .. } => {
                return Err(anyhow!(l(
                    "Kilo Code manages providers by enabling and disabling them; pick a model with /models in Kilo",
                    "Kilo Code 按启用/停用管理供应商，在 Kilo 里用 /models 选择模型"
                )))
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const SAMPLE: &str = r#"{
  "$schema": "https://app.kilo.ai/config.json",
  "theme": "kilo",
  "model": "relay/glm-4.6",
  "provider": {
    "relay": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "My Relay",
      "options": { "baseURL": "https://relay.example.com/v1" },
      "models": {
        "glm-4.6": { "name": "GLM 4.6", "limit": { "context": 200000 } },
        "kimi-k2": {}
      }
    }
  },
  "mcp": { "x": { "type": "local", "command": ["x"] } }
}
"#;

    type Home = TestHome;

    fn setup(name: &str, cfg: Option<&str>, auth: Option<&str>) -> Home {
        let home = TestHome::new(&format!("kilo-{name}"));
        let h = home.0.clone();
        std::fs::create_dir_all(h.join(".config/kilo")).unwrap();
        if let Some(c) = cfg {
            std::fs::write(h.join(".config/kilo/kilo.json"), c).unwrap();
        }
        if let Some(a) = auth {
            std::fs::create_dir_all(h.join(".local/share/kilo")).unwrap();
            std::fs::write(h.join(".local/share/kilo/auth.json"), a).unwrap();
        }
        home
    }

    fn cfg_of(h: &Home) -> Value {
        serde_json::from_str(&std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap()).unwrap()
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.clone())).collect::<Vec<_>>().join("\n")
    }

    fn upsert(id: Option<&str>, name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput {
                id: id.map(String::from),
                name: name.into(),
                base_url: base.into(),
                api: api.into(),
                api_key: key.map(String::from),
                models: models.iter().map(|s| s.to_string()).collect(),
                key_from_library: None,
                official_auth: None,
            },
        }
    }

    #[test]
    fn reads_state() {
        let _h = setup("state", Some(SAMPLE), Some(r#"{"relay":{"type":"api","key":"sk-relay-abcd"},"kilo":{"type":"oauth","access":"x"}}"#));
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert_eq!(st.providers.len(), 2);
        let p = &st.providers[0];
        assert_eq!((p.id.as_str(), p.name.as_str(), p.api.as_str(), p.has_key), ("relay", "My Relay", "chat", true));
        assert_eq!(p.models.len(), 2);
        assert_eq!(p.models[0].context, Some(200000));
        assert!(st.providers[1].builtin);
        let (base, key, api) = provider_endpoint("relay").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://relay.example.com/v1", Some("sk-relay-abcd"), "chat"));
    }

    #[test]
    fn reads_and_edits_a_lone_opencode_jsonc() {
        // A folder detection accepts must be the one the adapter edits, not a new kilo.json beside it.
        let h = setup("oc-jsonc", None, None);
        let file = h.0.join(".config/kilo/opencode.jsonc");
        std::fs::write(&file, SAMPLE).unwrap();
        let st = state(&Install::default());
        assert_eq!(st.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["relay"]);
        let (_, w, _) = plan(&[upsert(None, "New One", "https://n.example.com/v1", "chat", None, &["m1"])], false).unwrap();
        assert_eq!(w, [file.as_path()]);
        assert!(!h.0.join(".config/kilo/kilo.json").exists());
        let c: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(c.pointer("/provider/relay").is_some() && c.pointer("/provider/new-one").is_some());
    }

    #[test]
    fn missing_file_and_create() {
        let h = setup("create", None, None);
        let st = state(&Install::default());
        assert!(!st.readonly && st.providers.is_empty());
        let (d, w, _) = plan(&[upsert(None, "New One", "https://n.example.com/v1", "anthropic", Some("sk-new-9876"), &["m1"])], true).unwrap();
        assert!(w.is_empty());
        assert!(!h.0.join(".config/kilo/kilo.json").exists());
        let t = diff_text(&d);
        assert!(t.contains("••••9876") && !t.contains("sk-new-9876"), "{t}");
        let (_, w, _) = plan(&[upsert(None, "New One", "https://n.example.com/v1", "anthropic", Some("sk-new-9876"), &["m1"])], false).unwrap();
        assert_eq!(w.len(), 2); // config + auth.json (key goes to auth.json)
        let c = cfg_of(&h);
        assert!(c.get("$schema").is_none());
        assert_eq!(c.pointer("/provider/new-one/npm").unwrap(), "@ai-sdk/anthropic");
        let a: Value = serde_json::from_str(&std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap()).unwrap();
        assert_eq!(a.pointer("/new-one/key").unwrap(), "sk-new-9876");
    }

    #[test]
    fn edit_hide_disable_delete_roundtrip() {
        let h = setup("ops", Some(SAMPLE), None);
        let ops = vec![
            upsert(Some("relay"), "Relay 2", "https://relay2.example.com/v1", "responses", None, &[]),
            Op::SetModelVisible { provider: "relay".into(), model: "kimi-k2".into(), visible: false },
            Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: "qwen3".into(), name: Some("Qwen 3".into()), context: Some(128000), ..Default::default() } },
            Op::SetProviderEnabled { provider: "relay".into(), enabled: false },
        ];
        let (_, w, _) = plan(&ops, false).unwrap();
        assert_eq!(w.len(), 1);
        let c = cfg_of(&h);
        assert_eq!(c.pointer("/provider/relay/options/baseURL").unwrap(), "https://relay2.example.com/v1");
        assert_eq!(c.pointer("/provider/relay/npm").unwrap(), "@ai-sdk/openai");
        assert!(c.pointer("/provider/relay/models/kimi-k2").is_none());
        assert_eq!(c.pointer("/provider/relay/models/qwen3/limit/context").unwrap(), 128000);
        assert_eq!(c["disabled_providers"], json!(["relay"]));
        // unrelated content kept, in order
        assert_eq!(c["theme"], "kilo");
        assert_eq!(c["mcp"]["x"]["type"], "local");
        assert_eq!(c.as_object().unwrap().keys().next().unwrap(), "$schema");
        let st = state(&Install::default());
        let p = &st.providers[0];
        assert!(!p.enabled);
        assert!(p.models.iter().any(|m| m.id == "kimi-k2" && !m.visible));
        // show again + enable
        plan(&[Op::SetProviderEnabled { provider: "relay".into(), enabled: true }, Op::SetModelVisible { provider: "relay".into(), model: "kimi-k2".into(), visible: true }], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!(c["disabled_providers"], json!([]));
        assert!(c.pointer("/provider/relay/models/kimi-k2").is_some());
        plan(&[Op::DeleteModel { provider: "relay".into(), model: "qwen3".into() }, Op::DeleteProvider { provider: "relay".into() }], false).unwrap();
        assert!(cfg_of(&h).pointer("/provider/relay").is_none());
        assert!(plan(&[Op::SetModelRoles { provider: "relay".into(), roles: Default::default() }], true).is_err());
    }

    #[test]
    fn model_ids_with_slashes() {
        let h = setup("slash", Some(SAMPLE), None);
        let id = "anthropic/claude-sonnet-4~beta";
        let add = |ctx| Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: id.into(), context: Some(ctx), ..Default::default() } };
        plan(&[add(200000)], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["models"][id]["limit"]["context"], 200000);
        // Editing the existing entry keeps its other fields.
        plan(&[Op::UpsertModel { provider: "relay".into(), model: ModelInput { id: id.into(), name: Some("Sonnet".into()), ..Default::default() } }], false).unwrap();
        let m = &cfg_of(&h)["provider"]["relay"]["models"][id];
        assert_eq!((m["name"].as_str(), m["limit"]["context"].as_u64()), (Some("Sonnet"), Some(200000)));
        plan(&[Op::SetModelVisible { provider: "relay".into(), model: id.into(), visible: false }], false).unwrap();
        assert!(cfg_of(&h)["provider"]["relay"]["models"].get(id).is_none());
        plan(&[Op::SetModelVisible { provider: "relay".into(), model: id.into(), visible: true }], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["models"][id]["name"], "Sonnet");
        plan(&[Op::DeleteModel { provider: "relay".into(), model: id.into() }], false).unwrap();
        assert!(cfg_of(&h)["provider"]["relay"]["models"].get(id).is_none());
    }

    #[test]
    fn unreadable_auth_is_never_rewritten() {
        let broken = r#"{"kilo":{"type":"oauth","access":"x"},"#;
        let h = setup("badauth", Some(SAMPLE), Some(broken));
        let cfg0 = std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap();
        // Setting a key fails instead of rewriting auth.json or putting the key in the config.
        assert!(plan(&[upsert(Some("relay"), "My Relay", "https://relay.example.com/v1", "chat", Some("sk-new-1234"), &[])], false).is_err());
        assert_eq!(std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap(), broken);
        assert_eq!(std::fs::read_to_string(h.0.join(".config/kilo/kilo.json")).unwrap(), cfg0);
        // Everything that doesn't touch keys still works.
        plan(&[upsert(Some("relay"), "Relay 2", "https://relay.example.com/v1", "chat", None, &[])], false).unwrap();
        assert_eq!(cfg_of(&h)["provider"]["relay"]["name"], "Relay 2");
        assert!(cfg_of(&h)["provider"]["relay"]["options"].get("apiKey").is_none());
        // A non-object auth.json is left alone too.
        std::fs::write(h.0.join(".local/share/kilo/auth.json"), "[]").unwrap();
        assert!(plan(&[upsert(Some("relay"), "My Relay", "https://relay.example.com/v1", "chat", Some("sk-new-5678"), &[])], false).is_err());
        assert_eq!(std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap(), "[]");
    }

    #[test]
    fn new_provider_never_takes_an_auth_json_id() {
        // The Kilo account login must survive adding a provider named "Kilo".
        let h = setup("authid", Some(SAMPLE), Some(r#"{"kilo":{"type":"oauth","access":"x"}}"#));
        plan(&[upsert(None, "Kilo", "https://k.example.com/v1", "chat", Some("sk-new-1234"), &["m"])], false).unwrap();
        let c = cfg_of(&h);
        assert!(c.pointer("/provider/kilo").is_none() && c.pointer("/provider/kilo-2").is_some());
        let a: Value = serde_json::from_str(&std::fs::read_to_string(h.0.join(".local/share/kilo/auth.json")).unwrap()).unwrap();
        assert_eq!(a["kilo"], json!({"type": "oauth", "access": "x"}));
        assert_eq!(a.pointer("/kilo-2/key").unwrap(), "sk-new-1234");
    }

    #[test]
    fn disabled_login_cards_show_disabled() {
        let cfg = SAMPLE.replace("\"theme\": \"kilo\",", "\"theme\": \"kilo\", \"disabled_providers\": [\"kilo\"],");
        let _h = setup("offcard", Some(&cfg), Some(r#"{"kilo":{"type":"oauth","access":"x"}}"#));
        let st = state(&Install::default());
        let card = st.providers.iter().find(|p| p.id == "kilo").unwrap();
        assert!(card.builtin && !card.enabled);
    }

    #[test]
    fn settings_keep_opencode_values() {
        // Kilo keeps OpenCode's schema: autoupdate true | false | "notify", share manual | auto | disabled.
        let cfg = SAMPLE.replace("\"theme\": \"kilo\",", "\"theme\": \"kilo\", \"autoupdate\": \"notify\", \"share\": \"auto\",");
        let h = setup("settings", Some(&cfg), None);
        let st = state(&Install::default());
        let val = |k: &str| st.settings.iter().find(|s| s.key == k).unwrap().value.clone();
        assert_eq!((val("autoupdate"), val("share")), (json!("notify"), json!("auto")));
        assert!(st.settings.iter().all(|s| s.group == NAME));
        let set = |k: &str, v: Value| Op::SetSetting { key: k.into(), value: v };
        plan(&[set("autoupdate", json!("false")), set("share", json!("disabled"))], false).unwrap();
        let c = cfg_of(&h);
        assert_eq!((c["autoupdate"].clone(), c["share"].clone()), (json!(false), json!("disabled")));
        assert!(plan(&[set("snapshot", json!(false))], true).is_err());
    }

    #[test]
    fn comments_make_readonly() {
        let _h = setup("jsonc", Some("{\n  // hi\n  \"provider\": {}\n}\n"), None);
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[upsert(None, "x", "https://x/v1", "chat", None, &["m"])], true).is_err());
    }

    /// Read-only look at the real machine: state and a dry-run plan (nothing is written).
    #[test]
    #[ignore]
    fn dump_kilo() {
        let inst = detect();
        println!("detect: installed={} version={:?} running={}", inst.installed, inst.version, inst.running);
        let st = state(&inst);
        println!("dir={} files={:?} readonly={} notes={:?}", st.config_dir, st.files, st.readonly, st.notes);
        for p in &st.providers {
            println!("  provider {} ({}) api={} enabled={} has_key={} models={:?}", p.id, p.name, p.api, p.enabled, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        for kv in &st.current {
            println!("  {} = {}", kv.k, kv.v);
        }
        let (d, w, b) = plan(&[upsert(None, "Dry Run", "https://dry.example.com/v1", "chat", Some("sk-dryrun-0000"), &["m1"])], true).unwrap();
        println!("dry-run diff:\n{}", diff_text(&d));
        assert!(w.is_empty() && b.is_none());
    }
}
