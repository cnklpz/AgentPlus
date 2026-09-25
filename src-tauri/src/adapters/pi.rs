//! pi (badlogic/pi-mono coding agent): custom providers in `~/.pi/agent/models.json`
//! (`providers.<id> = { name, baseUrl, api, apiKey, models: [...] }`), keys inline or in
//! `~/.pi/agent/auth.json` (which wins), default model in `settings.json`
//! (`defaultProvider` + `defaultModel`). pi never writes models.json itself, and a schema
//! error there disables the whole file, so only known-valid shapes are written.
//! Provider / model editing is shared with OpenClaw (see pimodels).


use super::msg;
use super::{Plan, Endpoint};
use super::pimodels::{Dirty, Flavor, Fmt};
use crate::model::*;
use crate::process::Install;
use crate::store;
use crate::util::*;
use crate::i18n::l;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const ID: &str = "pi";
pub const NAME: &str = "pi";
pub const MARKER: &str = "settings.json";
pub const WSL_SCRIPT: &str = "pi --version 2>/dev/null; pgrep -x pi >/dev/null && echo @running; true";
pub const WSL_MARKER: &str = ".pi/agent/settings.json";
/// npm package names, new and old.
const PACKAGES: [&str; 2] = ["@earendil-works/pi-coding-agent", "@mariozechner/pi-coding-agent"];

/// `$PI_CODING_AGENT_DIR` (Windows side only), else `~/.pi/agent`.
pub fn default_dir() -> PathBuf {
    if let Some(d) = crate::env::agent_var("PI_CODING_AGENT_DIR") {
        return crate::env::resolve_path(&d);
    }
    home().join(".pi").join("agent")
}

/// The folder picked in AgentPlus, else the default.
fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}

fn models_path() -> PathBuf {
    dir().join("models.json")
}
fn auth_path() -> PathBuf {
    dir().join("auth.json")
}
fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

fn fmt() -> Fmt {
    Fmt { agent: ID, flavor: Flavor::Pi, path: models_path(), ptr: "/providers", auth: Some(auth_path()), env_file: None }
}

fn load_models() -> Result<(Value, TextMeta, bool)> {
    read_jsonc_object_or(&models_path(), json!({ "providers": {} }))
}

pub fn detect() -> Install {
    let mut inst = Install::default();
    if let Some(v) = PACKAGES.iter().find_map(|p| crate::process::npm_global_version(p)) {
        inst.installed = true;
        inst.version = Some(v);
        return inst;
    }
    let npm = dirs::data_dir().map(|d| d.join("npm"));
    let shims = [npm.map(|d| d.join("pi.cmd")), dirs::data_local_dir().map(|d| d.join("pnpm").join("pi.cmd")), dirs::home_dir().map(|h| h.join(".bun").join("bin").join("pi.exe"))];
    inst.installed = shims.iter().flatten().any(|p| p.exists());
    inst
}

/// (defaultProvider, defaultModel) from settings.json.
fn defaults() -> (Option<String>, Option<String>) {
    let Ok((v, _)) = read_json(&settings_path()) else { return (None, None) };
    let g = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from).filter(|x| !x.is_empty());
    (g("defaultProvider"), g("defaultModel"))
}

pub fn state(inst: &Install) -> AgentState {
    let f = fmt();
    let mut st = super::new_state(ID, NAME, inst, "multi", &dir(), vec![f.file(), display_path(&auth_path()), display_path(&settings_path())]);
    st.notes.push(l("改动在新开的 pi 会话里生效（运行中的会话可用 /model 重新选择）。", "Changes take effect in new pi sessions (running sessions can pick again with /model).").into());
    let root = store::load();
    let cfg = match load_models() {
        Ok((cfg, _, had_comments)) => {
            if had_comments {
                st.readonly = true;
                st.notes.push(msg::comments_readonly("models.json"));
            }
            cfg
        }
        Err(e) => {
            st.fail(e);
            return st;
        }
    };
    let auth = f.load_auth().map(|a| a.0);
    st.providers = f.providers(&cfg, &root, auth.as_ref());

    // Built-in providers logged in through `pi` (/login or an API key) but not configured here.
    let about = l("pi 内置的供应商，模型列表随 pi 发布，在 pi 里用 /model 选择", "A provider built into pi. Its model list ships with pi; pick models with /model in pi.");
    let cards = super::ocfmt::auth_cards(auth.as_ref(), &st.providers, "/login", about, &[]);
    st.providers.extend(cards);

    let default = match defaults() {
        (Some(p), Some(m)) => format!("{p}/{m}"),
        (None, Some(m)) => m,
        (Some(p), None) => tr!("{p}/（未指定）", "{p}/(not set)"),
        _ => "-".into(),
    };
    st.current = f.summary(&st.providers, default, None);
    st
}

pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    let (cfg, _, _) = load_models()?;
    fmt().endpoint(id, &cfg, &store::load())
}

pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    let f = fmt();
    let (mut cfg, meta, had_comments) = load_models()?;
    let cfg0 = cfg.clone();
    let mut auth = f.load_auth();
    let mut root = store::load();
    let mut diff = Diff::default();
    let mut dirty = Dirty::default();

    for op in ops {
        if f.apply(op, &mut cfg, &mut root, &mut auth, &mut diff, &mut dirty)? {
            continue;
        }
        match op {
            Op::SetCurrentProvider { .. } => return Err(anyhow!(l("pi 可以同时用多个供应商：按启用/停用管理，在 pi 里用 /model 选择模型", "pi can use several providers at once: manage them by enabling/disabling, and pick models with /model in pi."))),
            Op::SetModelRoles { .. } => return Err(msg::no_model_roles()),
            Op::SetSetting { key, .. } => return Err(msg::unknown_setting(key)),
            Op::ImportProvider { .. } => unreachable!("resolved in adapters::plan"),
            _ => unreachable!("handled by pimodels"),
        }
    }

    // Don't leave settings.json pointing at a provider / model that was just removed.
    let mut settings: Option<(Value, TextMeta)> = None;
    if dirty.cfg {
        let (dp, dm) = defaults();
        if let Some(dp) = dp {
            if f.has_model(&cfg0, &dp, dm.as_deref()) && !f.has_model(&cfg, &dp, dm.as_deref()) {
                let (mut s, m) = read_json(&settings_path())?;
                if let Some(o) = s.as_object_mut() {
                    o.remove("defaultProvider");
                    o.remove("defaultModel");
                }
                diff.push(&display_path(&settings_path()), tr!("- defaultProvider / defaultModel（{dp}/{} 已移除，pi 会另选可用模型）", "- defaultProvider / defaultModel ({dp}/{} was removed; pi will pick another available model)", dm.unwrap_or_default()), false);
                settings = Some((s, m));
            }
        }
    }

    if dirty.cfg && had_comments {
        return Err(msg::comments_not_written("models.json"));
    }
    let mut written = vec![];
    let mut backup_dir = None;
    if !dry_run && (dirty.cfg || dirty.auth || settings.is_some()) {
        let mut targets = vec![];
        if dirty.cfg { targets.push(models_path()) }
        if dirty.auth { targets.push(auth_path()) }
        if settings.is_some() { targets.push(settings_path()) }
        backup_dir = Some(backup(ID, &targets)?);
        std::fs::create_dir_all(dir())?;
        if dirty.cfg {
            write_json(&models_path(), &cfg, meta)?;
            written.push(models_path());
        }
        if let (true, Some((a, _))) = (dirty.auth, &auth) {
            super::ocfmt::write_auth(&auth_path(), a)?;
            written.push(auth_path());
        }
        if let Some((s, m)) = &settings {
            // Atomic (tmp + rename): pi rewrites settings.json itself under a lock.
            write_json(&settings_path(), s, *m)?;
            written.push(settings_path());
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
    use crate::model::{ModelInput, ProviderInput};

    const MODELS: &str = r#"{
  "providers": {
    "myproxy": {
      "baseUrl": "https://example.com/v1",
      "api": "openai-completions",
      "apiKey": "sk-inline-secret-1234",
      "headers": { "X-Team": "a" },
      "models": [
        {
          "id": "glm-5",
          "name": "GLM 5",
          "reasoning": true,
          "input": ["text", "image"],
          "contextWindow": 200000,
          "maxTokens": 32000,
          "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 }
        },
        { "id": "glm-4.6" }
      ]
    },
    "envy": {
      "baseUrl": "https://env.example.com",
      "api": "anthropic-messages",
      "apiKey": "${ENVY_KEY}",
      "models": [{ "id": "claude-x" }]
    },
    "cmd": {
      "baseUrl": "https://cmd.example.com/v1",
      "api": "openai-responses",
      "apiKey": "!op read op://vault/key",
      "models": []
    },
    "bedrock-ish": {
      "baseUrl": "https://bedrock.example.com",
      "api": "bedrock-converse-stream",
      "apiKey": "x",
      "models": [{ "id": "b1" }]
    },
    "openai": { "baseUrl": "https://oai-proxy.example.com/v1" }
  },
  "futureTopLevel": { "keep": true }
}
"#;
    const AUTH: &str = r#"{
  "envy": { "type": "api_key", "key": "sk-auth-secret-9876" },
  "openai": { "type": "api_key", "key": "sk-auth-openai-1111" },
  "anthropic": { "type": "oauth", "refresh": "r", "access": "a", "expires": 1 }
}
"#;
    const SETTINGS: &str = r#"{
  "lastChangelogVersion": "0.60.0",
  "defaultProvider": "myproxy",
  "defaultModel": "glm-5",
  "theme": "dark"
}
"#;

    fn setup(name: &str, models: Option<&str>) -> TestHome {
        let home = TestHome::new(&format!("pi-{name}"));
        crate::env::set_test_vars(&[("ENVY_KEY", "sk-env-value-5555")]);
        let d = default_dir();
        std::fs::create_dir_all(&d).unwrap();
        if let Some(m) = models {
            std::fs::write(d.join("models.json"), m).unwrap();
        }
        std::fs::write(d.join("auth.json"), AUTH).unwrap();
        std::fs::write(d.join("settings.json"), SETTINGS).unwrap();
        home
    }

    fn read(p: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(default_dir().join(p)).unwrap()).unwrap()
    }

    fn lines(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(move |l| format!("{} | {}", g.file, l.text))).collect::<Vec<_>>().join("\n")
    }

    fn prov(name: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput { id: None, name: name.into(), base_url: "https://new.example.com/v1".into(), api: api.into(), api_key: key.map(String::from), models: models.iter().map(|s| s.to_string()).collect(), key_from_library: None, official_auth: None },
        }
    }

    #[test]
    fn reads_state() {
        let _g = setup("state", Some(MODELS));
        let st = state(&Install::default());
        assert!(!st.readonly, "{:?}", st.notes);
        let p = st.providers.iter().find(|p| p.id == "myproxy").unwrap();
        assert_eq!(p.api, "chat");
        assert!(p.has_key && p.compatible && p.enabled);
        assert_eq!(p.models.len(), 2);
        assert_eq!(p.models[0].context, Some(200000));
        assert_eq!(p.models[0].name.as_deref(), Some("GLM 5"));
        assert!(p.models[0].tags.iter().any(|t| t.id == "cap:image" && t.label == "图片"));
        let b = st.providers.iter().find(|p| p.id == "bedrock-ish").unwrap();
        assert!(!b.compatible);
        assert_eq!(b.api, "bedrock-converse-stream");
        let o = st.providers.iter().find(|p| p.id == "openai").unwrap();
        assert_eq!(o.api, "responses");
        // anthropic: logged in via OAuth, shown as built-in.
        assert!(st.providers.iter().any(|p| p.id == "anthropic" && p.builtin));
        assert!(st.current.iter().any(|k| k.v == "myproxy/glm-5"));
        let dump = format!("{:?}", st);
        assert!(!dump.contains("sk-inline-secret") && !dump.contains("sk-auth-secret"));
    }

    #[test]
    fn endpoints_resolve_keys() {
        let _g = setup("endpoint", Some(MODELS));
        assert_eq!(provider_endpoint("myproxy").unwrap(), ("https://example.com/v1".into(), Some("sk-inline-secret-1234".into()), "chat".into()));
        // auth.json wins over ${ENVY_KEY}.
        assert_eq!(provider_endpoint("envy").unwrap().1.as_deref(), Some("sk-auth-secret-9876"));
        assert_eq!(provider_endpoint("cmd").unwrap().1, None);
        assert_eq!(provider_endpoint("cmd").unwrap().2, "responses");
        // $VAR with no auth entry.
        let mut v: Value = serde_json::from_str(MODELS).unwrap();
        v["providers"]["myproxy"]["apiKey"] = json!("$ENVY_KEY");
        std::fs::write(models_path(), serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(provider_endpoint("myproxy").unwrap().1.as_deref(), Some("sk-env-value-5555"));
        let key_desc = || {
            let st = state(&Install::default());
            let p = st.providers.into_iter().find(|p| p.id == "myproxy").unwrap();
            p.details.into_iter().find(|k| k.k == lbl::api_key()).unwrap().v
        };
        assert_eq!(key_desc(), "环境变量 ENVY_KEY");
        // A bare name of a set variable is read from the environment too, and shown that way.
        v["providers"]["myproxy"]["apiKey"] = json!("ENVY_KEY");
        std::fs::write(models_path(), serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(provider_endpoint("myproxy").unwrap().1.as_deref(), Some("sk-env-value-5555"));
        assert_eq!(key_desc(), "环境变量 ENVY_KEY");
        // One that isn't set is the key itself.
        v["providers"]["myproxy"]["apiKey"] = json!("NOPE_KEY");
        std::fs::write(models_path(), serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(provider_endpoint("myproxy").unwrap().1.as_deref(), Some("NOPE_KEY"));
        assert_eq!(key_desc(), "明文保存在 models.json");
    }

    #[test]
    fn create_edit_delete_provider() {
        let _g = setup("crud", Some(MODELS));
        let (d, w, _) = plan(&[prov("My Relay", "anthropic", Some("sk-new-key-abcd"), &["m1", "m2"])], false).unwrap();
        let text = lines(&d);
        assert!(text.contains("••••abcd") && !text.contains("sk-new-key"), "{text}");
        assert_eq!(w, vec![models_path()]);
        let v = read("models.json");
        let p = &v["providers"]["my-relay"];
        assert_eq!(p["api"], "anthropic-messages");
        assert_eq!(p["name"], "My Relay");
        assert_eq!(p["apiKey"], "sk-new-key-abcd");
        assert_eq!(p["models"], json!([{ "id": "m1" }, { "id": "m2" }]));
        assert_eq!(v["futureTopLevel"], json!({ "keep": true }));

        // A name clashing with a built-in id gets a suffix.
        plan(&[prov("OpenAI", "chat", None, &[])], false).unwrap();
        assert!(read("models.json")["providers"]["openai-2"].is_object());

        // Edit: name, url, api; key kept when None.
        let edit = Op::UpsertProvider { provider: ProviderInput { id: Some("myproxy".into()), name: "Proxy".into(), base_url: "https://b.example.com/v1".into(), api: "responses".into(), api_key: None, models: vec![], key_from_library: None, official_auth: None } };
        plan(&[edit], false).unwrap();
        let v = read("models.json");
        assert_eq!(v["providers"]["myproxy"]["api"], "openai-responses");
        assert_eq!(v["providers"]["myproxy"]["name"], "Proxy");
        assert_eq!(v["providers"]["myproxy"]["apiKey"], "sk-inline-secret-1234");
        assert_eq!(v["providers"]["myproxy"]["headers"], json!({ "X-Team": "a" }));

        // Changing the api of a provider on an unsupported protocol is refused.
        let bad = Op::UpsertProvider { provider: ProviderInput { id: Some("bedrock-ish".into()), name: "bedrock-ish".into(), base_url: "https://bedrock.example.com".into(), api: "chat".into(), api_key: None, models: vec![], key_from_library: None, official_auth: None } };
        assert!(plan(&[bad], true).is_err());

        // Key for a provider whose key lives in auth.json stays in auth.json.
        let k = Op::UpsertProvider { provider: ProviderInput { id: Some("envy".into()), name: "envy".into(), base_url: "https://env.example.com".into(), api: "anthropic".into(), api_key: Some("sk-rotated-7777".into()), models: vec![], key_from_library: None, official_auth: None } };
        let (_, w, _) = plan(&[k], false).unwrap();
        assert_eq!(w, vec![auth_path()]);
        assert_eq!(read("auth.json")["envy"]["key"], "sk-rotated-7777");
        assert_eq!(read("models.json")["providers"]["envy"]["apiKey"], "${ENVY_KEY}");

        // Deleting a custom provider drops its auth.json key too, so it doesn't come back as
        // an undeletable built-in provider.
        let (d, w, _) = plan(&[Op::DeleteProvider { provider: "envy".into() }], false).unwrap();
        assert!(lines(&d).contains("含它的模型和密钥"), "{}", lines(&d));
        assert!(w.contains(&auth_path()));
        let a = read("auth.json");
        assert!(a.get("envy").is_none());
        assert_eq!(a["anthropic"]["type"], "oauth");
        assert!(!state(&Install::default()).providers.iter().any(|p| p.id == "envy"));
        // A provider pi ships keeps its auth.json key: pi goes on using it.
        let (d, w, _) = plan(&[Op::DeleteProvider { provider: "openai".into() }], false).unwrap();
        assert!(lines(&d).contains("auth.json 里的密钥保留"), "{}", lines(&d));
        assert!(!w.contains(&auth_path()));
        assert_eq!(read("auth.json")["openai"]["key"], "sk-auth-openai-1111");
        assert!(state(&Install::default()).providers.iter().any(|p| p.id == "openai" && p.builtin));

        // Delete the default provider: settings.json default cleared, other keys kept.
        let (d, w, _) = plan(&[Op::DeleteProvider { provider: "myproxy".into() }], false).unwrap();
        assert!(lines(&d).contains("defaultProvider"));
        assert!(w.contains(&settings_path()));
        assert!(read("models.json")["providers"].get("myproxy").is_none());
        let s = read("settings.json");
        assert!(s.get("defaultProvider").is_none() && s.get("defaultModel").is_none());
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["lastChangelogVersion"], "0.60.0");
    }

    #[test]
    fn models_hide_show_add_delete_replace() {
        let _g = setup("models", Some(MODELS));
        let hide = Op::SetModelVisible { provider: "myproxy".into(), model: "glm-5".into(), visible: false };
        let (d, _, _) = plan(&[hide], false).unwrap();
        // Hiding the default model clears the default.
        assert!(lines(&d).contains("defaultModel"));
        assert_eq!(read("models.json")["providers"]["myproxy"]["models"].as_array().unwrap().len(), 1);
        let st = state(&Install::default());
        let m = st.providers.iter().find(|p| p.id == "myproxy").unwrap().models.iter().find(|m| m.id == "glm-5").unwrap().clone();
        assert!(!m.visible && m.context == Some(200000));

        // Edit the hidden model (stays stashed), then show it: definition comes back intact.
        plan(&[Op::UpsertModel { provider: "myproxy".into(), model: ModelInput { id: "glm-5".into(), name: Some("GLM Five".into()), context: None, ..Default::default() } }], false).unwrap();
        plan(&[Op::SetModelVisible { provider: "myproxy".into(), model: "glm-5".into(), visible: true }], false).unwrap();
        let v = read("models.json");
        let back = v["providers"]["myproxy"]["models"].as_array().unwrap().iter().find(|m| m["id"] == "glm-5").unwrap().clone();
        assert_eq!(back["name"], "GLM Five");
        assert_eq!(back["maxTokens"], 32000);
        assert_eq!(back["cost"]["cacheWrite"], 0);

        // Add + edit.
        plan(&[Op::UpsertModel { provider: "myproxy".into(), model: ModelInput { id: "kimi".into(), name: None, context: Some(131072), ..Default::default() } }], false).unwrap();
        plan(&[Op::UpsertModel { provider: "myproxy".into(), model: ModelInput { id: "glm-4.6".into(), name: None, context: Some(64000), ..Default::default() } }], false).unwrap();
        let v = read("models.json");
        let ms = v["providers"]["myproxy"]["models"].as_array().unwrap();
        assert!(ms.iter().any(|m| m == &json!({ "id": "kimi", "contextWindow": 131072 })));
        assert!(ms.iter().any(|m| m == &json!({ "id": "glm-4.6", "contextWindow": 64000 })));

        // Delete.
        plan(&[Op::DeleteModel { provider: "myproxy".into(), model: "kimi".into() }], false).unwrap();
        assert!(!read("models.json")["providers"]["myproxy"]["models"].as_array().unwrap().iter().any(|m| m["id"] == "kimi"));

        // Replace the list: kept ids keep their definitions.
        plan(&[Op::SetProviderModels { provider: "myproxy".into(), models: vec!["glm-5".into(), "new-one".into()] }], false).unwrap();
        let v = read("models.json");
        let ms = v["providers"]["myproxy"]["models"].as_array().unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0]["maxTokens"], 32000);
        assert_eq!(ms[1], json!({ "id": "new-one" }));
    }

    #[test]
    fn enable_disable_and_dry_run() {
        let _g = setup("toggle", Some(MODELS));
        let before = std::fs::read(models_path()).unwrap();
        let off = Op::SetProviderEnabled { provider: "envy".into(), enabled: false };
        let (d, w, b) = plan(std::slice::from_ref(&off), true).unwrap();
        assert!(!d.groups.is_empty() && w.is_empty() && b.is_none());
        assert_eq!(std::fs::read(models_path()).unwrap(), before);
        assert!(!agentplus_dir().join("store.json").exists());

        plan(&[off], false).unwrap();
        assert!(read("models.json")["providers"].get("envy").is_none());
        let st = state(&Install::default());
        let p = st.providers.iter().find(|p| p.id == "envy").unwrap();
        assert!(!p.enabled && p.has_key);
        assert_eq!(provider_endpoint("envy").unwrap().0, "https://env.example.com");
        plan(&[Op::SetProviderEnabled { provider: "envy".into(), enabled: true }], false).unwrap();
        assert_eq!(read("models.json")["providers"]["envy"]["apiKey"], "${ENVY_KEY}");

        assert!(plan(&[Op::SetCurrentProvider { provider: "envy".into() }], true).is_err());
        assert!(plan(&[Op::SetModelRoles { provider: "envy".into(), roles: Default::default() }], true).is_err());
        assert!(plan(&[Op::SetSetting { key: "x".into(), value: json!(true) }], true).is_err());
    }

    #[test]
    fn untouched_roundtrip_and_missing_file() {
        // A file already in pretty-printed shape (CRLF, 4-space indent) survives an edit + undo byte for byte.
        let pretty = {
            let v: Value = serde_json::from_str(MODELS).unwrap();
            let mut buf = Vec::new();
            let mut ser = serde_json::Serializer::with_formatter(&mut buf, serde_json::ser::PrettyFormatter::with_indent(b"    "));
            serde::Serialize::serialize(&v, &mut ser).unwrap();
            String::from_utf8(buf).unwrap().replace('\n', "\r\n") + "\r\n"
        };
        let _g = setup("roundtrip", Some(&pretty));
        plan(&[Op::UpsertModel { provider: "envy".into(), model: ModelInput { id: "claude-x".into(), name: Some("X".into()), context: None, ..Default::default() } }], false).unwrap();
        plan(&[Op::UpsertModel { provider: "envy".into(), model: ModelInput { id: "claude-x".into(), name: None, context: None, ..Default::default() } }], false).unwrap();
        let mut v = read("models.json");
        assert_eq!(v["providers"]["envy"]["models"][0]["name"], "X");
        // Take the name out again through the file itself, then compare bytes.
        v["providers"]["envy"]["models"][0].as_object_mut().unwrap().remove("name");
        let (_, meta) = read_text(&models_path()).unwrap();
        write_json(&models_path(), &v, meta).unwrap();
        assert_eq!(std::fs::read_to_string(models_path()).unwrap(), pretty);
        // And an edit leaves every other byte alone: only the added line differs.
        plan(&[Op::UpsertModel { provider: "envy".into(), model: ModelInput { id: "claude-x".into(), name: None, context: Some(1000), ..Default::default() } }], false).unwrap();
        let after = std::fs::read_to_string(models_path()).unwrap();
        let removed: Vec<&str> = pretty.lines().filter(|l| !after.lines().any(|a| a == *l)).collect();
        assert!(removed.iter().all(|l| l.trim() == "\"id\": \"claude-x\""), "{removed:?}");
        assert!(after.contains("\r\n                    \"contextWindow\": 1000\r\n"));

        // Missing models.json: empty state, and plan creates it.
        std::fs::remove_file(models_path()).unwrap();
        let st = state(&Install::default());
        assert!(!st.readonly);
        assert!(st.providers.iter().all(|p| p.builtin));
        plan(&[prov("Fresh", "chat", Some("sk-fresh-0000"), &["a"])], false).unwrap();
        assert_eq!(read("models.json")["providers"]["fresh"]["models"], json!([{ "id": "a" }]));
    }

    #[test]
    fn comments_make_it_readonly() {
        let _g = setup("comments", Some("{\n  // my relay\n  \"providers\": {}\n}\n"));
        assert!(state(&Install::default()).readonly);
        assert!(plan(&[prov("X", "chat", None, &[])], true).is_err());
    }

    #[test]
    fn non_object_models_json_is_refused() {
        let _g = setup("array", Some("[]"));
        let st = state(&Install::default());
        assert!(st.readonly && st.notes.iter().any(|n| n.contains("models.json 顶层不是对象")), "{:?}", st.notes);
        assert!(plan(&[prov("X", "chat", None, &["m"])], false).is_err());
        assert_eq!(std::fs::read_to_string(default_dir().join("models.json")).unwrap(), "[]");
    }

    /// Read-only look at the real machine: `cargo test --lib dump_pi -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_pi() {
        let inst = detect();
        println!("installed={} version={:?}", inst.installed, inst.version);
        let st = state(&inst);
        println!("dir={} readonly={} files={:?}", st.config_dir, st.readonly, st.files);
        for p in &st.providers {
            println!("  [{}] {} api={} host={} key={} enabled={} models={:?}", p.id, p.name, p.api, p.host, p.has_key, p.enabled, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
        }
        for k in &st.current {
            println!("  {} = {}", k.k, k.v);
        }
        println!("notes={:?}", st.notes);
        let (d, written, backup) = plan(&[prov("Dump Probe", "chat", Some("sk-dump-probe-0000"), &["m"])], true).unwrap();
        for g in &d.groups {
            for l in &g.lines {
                println!("  diff {} | {}", g.file, l.text);
            }
        }
        assert!(written.is_empty() && backup.is_none());
    }
}
