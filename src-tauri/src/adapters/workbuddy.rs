//! WorkBuddy (Tencent's desktop agent, built on CodeBuddy Code): custom models in
//! `<data folder>/models.json`, the CodeBuddy format (see `codebuddy.rs`), hot-reloaded
//! within about a second. Two editions install side by side, each with its own data
//! folder and so its own agent here: WorkBuddy (`~/.workbuddy`) and WorkBuddy AI, the
//! international one (`~/.workbuddy-ai`, in [`ai`]). The folder is `WORKBUDDY_CONFIG_DIR`
//! or `CODEBUDDY_CONFIG_DIR` when set, else `~/<folder>` as the app's `cli/product.json`
//! names it (`config.customUserDataDir`, else `dataFolderName`).
//!
//! Where it differs from CodeBuddy:
//! - WorkBuddy saves the file as a top-level array `[{ id, … }]` and drops
//!   `availableModels`; a user `availableModels` also replaces the whole model list it gets
//!   from its server, built-in models included. So AgentPlus hides a model by moving its
//!   entry out of the file into the AgentPlus store, and leaves `availableModels` alone.
//! - Model ids get a `custom-local:` prefix inside WorkBuddy; the file keeps the bare ids.
//! - `apiKey` may be stored encrypted (an object) once WorkBuddy's at-rest policy reaches
//!   "files"; 5.6.x builds with "fields", so the file stays plain text.

use super::codebuddy::{self, Flavor};
use super::{Endpoint, Plan};
use crate::model::*;
use crate::process::Install;
use crate::util::*;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const ID: &str = "workbuddy";
pub const NAME: &str = "WorkBuddy";
pub const MARKER: &str = "settings.json";
pub const WSL_SCRIPT: &str = "";
pub const WSL_MARKER: &str = "";

/// One edition of the app.
pub(crate) struct Edition {
    /// Its `dataFolderName`.
    folder: &'static str,
    /// Its executable on Windows (a copy without a readable product.json is told by it).
    exe: &'static str,
    /// Its macOS bundle id, then its bundle file name.
    bundles: [&'static str; 2],
    flavor: Flavor,
    /// The data folder the installed copy's product.json names, set by detection.
    found: Mutex<Option<String>>,
}

static CN: Edition = Edition {
    folder: ".workbuddy",
    exe: "WorkBuddy.exe",
    bundles: ["com.tencent.workbuddy.mac", "WorkBuddy.app"],
    flavor: Flavor { id: ID, name: NAME, dir, native_hide: false, array_form: true, store_label: "AgentPlus · WorkBuddy" },
    found: Mutex::new(None),
};

/// WorkBuddy AI, the international edition.
pub mod ai {
    use super::*;

    pub const ID: &str = "workbuddy-ai";
    pub const NAME: &str = "WorkBuddy AI";
    pub use super::{MARKER, WSL_MARKER, WSL_SCRIPT};

    pub(super) static AI: Edition = Edition {
        folder: ".workbuddy-ai",
        exe: "WorkBuddyAI.exe",
        bundles: ["com.workbuddy.workbuddy-ai", "WorkBuddy AI.app"],
        flavor: Flavor { id: ID, name: NAME, dir, native_hide: false, array_form: true, store_label: "AgentPlus · WorkBuddy AI" },
        found: Mutex::new(None),
    };

    pub fn default_dir() -> PathBuf {
        default_dir_of(&AI)
    }
    pub(crate) fn dir() -> PathBuf {
        crate::adapters::dir_override(ID).unwrap_or_else(default_dir)
    }
    pub fn detect() -> Install {
        detect_of(&AI)
    }
    pub fn state(inst: &Install) -> AgentState {
        state_of(&AI, inst)
    }
    pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
        codebuddy::endpoint_of(&AI.flavor, id)
    }
    pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
        plan_of(&AI, ops, dry_run)
    }
}

pub fn default_dir() -> PathBuf {
    default_dir_of(&CN)
}
pub(crate) fn dir() -> PathBuf {
    super::dir_override(ID).unwrap_or_else(default_dir)
}
pub fn detect() -> Install {
    detect_of(&CN)
}
pub fn state(inst: &Install) -> AgentState {
    state_of(&CN, inst)
}
pub fn provider_endpoint(id: &str) -> Result<Endpoint> {
    codebuddy::endpoint_of(&CN.flavor, id)
}
pub fn plan(ops: &[Op], dry_run: bool) -> Result<Plan> {
    plan_of(&CN, ops, dry_run)
}

/// The config-dir variables WorkBuddy honours, else `~/<data folder>`: the one the
/// installed copy names, else the edition's usual one.
fn default_dir_of(e: &Edition) -> PathBuf {
    for var in ["WORKBUDDY_CONFIG_DIR", "CODEBUDDY_CONFIG_DIR"] {
        if let Some(d) = crate::env::agent_var(var).filter(|d| !d.trim().is_empty()) {
            return PathBuf::from(d.trim());
        }
    }
    home().join(lock(&e.found).clone().unwrap_or_else(|| e.folder.to_string()))
}

/// The data folder `product.json` names: an enterprise build's `config.customUserDataDir`,
/// else `dataFolderName`. None when it names neither.
fn folder_of(product: &Value) -> Option<String> {
    let pick = |v: Option<&Value>| v.and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    pick(product.pointer("/config/customUserDataDir")).or_else(|| pick(product.get("dataFolderName")))
}

/// `cli/product.json` next to the app's `app.asar`.
fn product_json(exe: &Path) -> Option<PathBuf> {
    let resources = match crate::process::bundle_of(exe) {
        Some(b) => b.join("Contents").join("Resources"),
        None => exe.parent()?.join("resources"),
    };
    Some(resources.join("app.asar.unpacked").join("cli").join("product.json"))
}

/// Whether a copy is of edition `e`: by the `dataFolderName` its product.json names, else
/// (no product.json) by its executable's name.
fn is_edition(e: &Edition, product: Option<&Value>, exe: &Path) -> bool {
    match product.and_then(|p| p.get("dataFolderName")).and_then(|x| x.as_str()) {
        Some(f) => f.trim() == e.folder,
        None => exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(e.exe)),
    }
}

// ---------- detection ----------

/// The desktop app: its app bundle on macOS; on Windows, the registry uninstall entry
/// ("WorkBuddy 5.6.2", "WorkBuddy AI 5.6.2") whose copy is this edition.
fn detect_of(e: &Edition) -> Install {
    let mut inst = Install::default();
    let product = |exe: &Path| product_json(exe).and_then(|p| read_json(&p).ok()).map(|(v, _)| v);
    if let Some(c) = crate::process::app_bundles(&e.bundles).first() {
        crate::process::use_copy(&mut inst, c);
    } else if let Some((u, exe)) = crate::process::uninstall_entries("WorkBuddy").into_iter().find_map(|u| {
        let exe = u.icon.as_deref().and_then(crate::process::unquote_exe).filter(|p| crate::process::is_exe(p) && p.is_file())?;
        is_edition(e, product(&exe).as_ref(), &exe).then_some((u, exe))
    }) {
        inst.installed = true;
        inst.version = u.version;
        inst.dir = exe.parent().map(Path::to_path_buf);
        inst.exe = Some(exe);
    }
    *lock(&e.found) = inst.exe.as_deref().and_then(product).as_ref().and_then(folder_of);
    crate::process::set_app_running(&mut inst);
    inst
}

/// Whether agent `id` is really there to share a folder with: its folder picked in AgentPlus,
/// or its app found. A config-dir variable alone points both agents at one folder even when
/// only one of them is installed (WorkBuddy also honours `CODEBUDDY_CONFIG_DIR`). WorkBuddy is
/// a Windows app, so nothing is shared with it in WSL. Tests go by the picked folder only,
/// since the machine running them may have either app.
pub(crate) fn present(id: &str) -> bool {
    !crate::env::is_wsl() && (super::dir_override(id).is_some() || (!cfg!(test) && crate::process::detect(id).installed))
}

/// The agent that owns the same models.json as edition `e` (a config-dir variable or a
/// custom folder pointing both at one folder): CodeBuddy, else (for WorkBuddy AI) WorkBuddy.
/// They hide models differently and keep separate stores, so only the owner writes it.
/// Detection runs only once the folders match.
fn shared_with(e: &Edition) -> Option<&'static str> {
    let d = (e.flavor.dir)();
    if same_dir(&d, &codebuddy::dir()) && present(codebuddy::ID) {
        Some(codebuddy::NAME)
    } else if !std::ptr::eq(e, &CN) && same_dir(&d, &dir()) && present(ID) {
        Some(NAME)
    } else {
        None
    }
}

fn plan_of(e: &'static Edition, ops: &[Op], dry_run: bool) -> Result<Plan> {
    if let Some(owner) = shared_with(e) {
        let name = e.flavor.name;
        anyhow::bail!("{}", tr!("{name} uses the same models.json as {owner}; manage it under {owner}", "{name} 和 {owner} 用的是同一个 models.json，请在 {owner} 里管理"));
    }
    codebuddy::plan_of(&e.flavor, ops, dry_run)
}

fn state_of(e: &'static Edition, inst: &Install) -> AgentState {
    let (mut st, cfg) = codebuddy::state_of(&e.flavor, inst);
    let name = e.flavor.name;
    if let Some(owner) = shared_with(e) {
        st.readonly = true;
        st.notes.insert(0, tr!("{name} uses the same models.json as {owner} (a config-dir variable or custom folder points both at it). It is read-only here; manage it under {owner}.", "{name} 和 {owner} 用的是同一个 models.json（配置目录变量或自定义目录让两者指向了同一处）。这里只读，请在 {owner} 里管理。"));
    }
    if cfg.is_some_and(|c| c.get("availableModels").is_some()) {
        st.notes.push(tr!("models.json has an availableModels list, which replaces {name}'s whole model list (built-in models included); AgentPlus leaves it as it is.", "models.json 里有 availableModels，它会整体替换 {name} 的模型列表（内置模型也算在内）；AgentPlus 不改动它。"));
    }
    st.notes.push(tr!("Hiding a model moves its entry out of models.json into AgentPlus ({name} has no hide switch of its own); showing it puts the entry back.", "隐藏模型会把条目移出 models.json、暂存在 AgentPlus（{name} 自己没有隐藏开关）；重新显示时放回。"));
    st.notes.push(tr!("Changes to models.json take effect in about a second; custom models show under Settings → Models in {name}.", "models.json 改动约 1 秒后自动生效；自定义模型显示在 {name} 的 设置 → 模型 里。"));
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// As WorkBuddy saves it: a top-level array.
    const SAMPLE: &str = r#"[
  {
    "id": "deepseek-chat",
    "name": "DeepSeek V3",
    "vendor": "DeepSeek",
    "url": "https://api.deepseek.com/v1/chat/completions",
    "apiKey": "sk-ds-1111",
    "maxInputTokens": 128000,
    "supportsToolCall": true
  },
  {
    "id": "deepseek-reasoner",
    "vendor": "DeepSeek",
    "url": "https://api.deepseek.com/v1/chat/completions",
    "apiKey": "sk-ds-1111",
    "supportsReasoning": true,
    "reasoning": { "defaultEffort": "high" }
  },
  {
    "id": "glm-4.6",
    "vendor": "Zhipu",
    "url": "https://open.bigmodel.cn/api/paas/v4/chat/completions",
    "apiKey": "sk-zp-2222"
  }
]
"#;

    fn setup(name: &str, models: Option<&str>) -> TestHome {
        let home = TestHome::new(&format!("workbuddy-{name}"));
        let d = home.0.join(".workbuddy");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(home.0.join(".workbuddy-ai")).unwrap();
        std::fs::write(d.join("settings.json"), r#"{"enabledPlugins":{}}"#).unwrap();
        if let Some(c) = models {
            std::fs::write(d.join("models.json"), c).unwrap();
        }
        home
    }

    fn file(h: &TestHome) -> Value {
        serde_json::from_str(&std::fs::read_to_string(h.0.join(".workbuddy/models.json")).unwrap()).unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|e| str_field(e, "id")).collect()
    }

    /// (id, visible) of a provider's models, by id.
    fn models(st: &AgentState, pid: &str) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = st.providers.iter().find(|p| p.id == pid).unwrap().models.iter().map(|m| (m.id.clone(), m.visible)).collect();
        v.sort();
        v
    }

    fn vis(pid: &str, mid: &str, visible: bool) -> Op {
        Op::SetModelVisible { provider: pid.into(), model: mid.into(), visible }
    }

    fn enable(pid: &str, enabled: bool) -> Op {
        Op::SetProviderEnabled { provider: pid.into(), enabled }
    }

    fn upsert(name: &str, base: &str, api: &str, key: Option<&str>, models: &[&str]) -> Op {
        Op::UpsertProvider {
            provider: ProviderInput {
                id: None,
                name: name.into(),
                base_url: base.into(),
                api: api.into(),
                api_key: key.map(String::from),
                models: models.iter().map(|s| s.to_string()).collect(),
                key_from_library: None,
                key_from_sync: None,
                official_auth: None,
            },
        }
    }

    fn diff_text(d: &Diff) -> String {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.clone())).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn each_edition_has_its_folder() {
        let h = TestHome::new("workbuddy-dir");
        assert_eq!(default_dir(), h.0.join(".workbuddy"));
        assert_eq!(ai::default_dir(), h.0.join(".workbuddy-ai"));
        crate::env::set_test_vars(&[("CODEBUDDY_CONFIG_DIR", r"D:\cb")]);
        assert_eq!(default_dir(), PathBuf::from(r"D:\cb"));
        crate::env::set_test_vars(&[("CODEBUDDY_CONFIG_DIR", r"D:\cb"), ("WORKBUDDY_CONFIG_DIR", r" D:\wb ")]);
        assert_eq!(ai::default_dir(), PathBuf::from(r"D:\wb"));
    }

    #[test]
    fn tells_the_editions_apart() {
        let cn = json!({ "productName": "WorkBuddy", "dataFolderName": ".workbuddy" });
        let intl = json!({ "productName": "WorkBuddy AI", "dataFolderName": ".workbuddy-ai" });
        // Joined, not a Windows literal: elsewhere `\` isn't a separator and the file name is lost.
        let exe = &Path::new("x").join("WorkBuddy.exe");
        assert!(is_edition(&CN, Some(&cn), exe) && !is_edition(&ai::AI, Some(&cn), exe));
        assert!(is_edition(&ai::AI, Some(&intl), exe) && !is_edition(&CN, Some(&intl), exe));
        // No product.json: by the executable.
        assert!(is_edition(&CN, None, exe) && !is_edition(&ai::AI, None, exe));
        assert!(is_edition(&ai::AI, None, &Path::new("workbuddy").join("WorkBuddyAI").join("workbuddyai.EXE")));
    }

    #[test]
    fn folder_from_product_json() {
        assert_eq!(folder_of(&json!({ "dataFolderName": ".workbuddy-ai" })).as_deref(), Some(".workbuddy-ai"));
        assert_eq!(folder_of(&json!({ "dataFolderName": ".workbuddy", "config": { "customUserDataDir": " .aimea " } })).as_deref(), Some(".aimea"));
        assert_eq!(folder_of(&json!({ "dataFolderName": " ", "config": {} })), None);
        let win = product_json(Path::new(r"D:\workbuddy\WorkBuddyAI\WorkBuddyAI.exe")).unwrap();
        assert!(win.ends_with("resources/app.asar.unpacked/cli/product.json"), "{win:?}");
        let mac = product_json(Path::new("/Applications/WorkBuddy AI.app/Contents/MacOS/WorkBuddy AI")).unwrap();
        assert_eq!(mac, PathBuf::from("/Applications/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/product.json"));
    }

    #[test]
    fn editions_keep_separate_files_and_stores() {
        let h = setup("separate", Some(SAMPLE));
        std::fs::write(h.0.join(".workbuddy-ai/models.json"), r#"[{"id":"gpt-5","vendor":"Relay","url":"https://r.example.com/v1/chat/completions"}]"#).unwrap();
        assert_eq!(ai::state(&Install::default()).providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["relay"]);
        ai::plan(&[vis("relay", "gpt-5", false)], false).unwrap();
        // The other edition's file and hidden models are untouched.
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "deepseek-reasoner", "glm-4.6"]);
        assert!(state(&Install::default()).providers.iter().all(|p| p.models.iter().all(|m| m.visible)));
        assert_eq!(models(&ai::state(&Install::default()), "relay"), [("gpt-5".into(), false)]);
        assert!(ai::state(&Install::default()).notes.iter().any(|n| n.contains("WorkBuddy AI")));
    }

    #[test]
    fn reads_the_array_form() {
        let _h = setup("read", Some(SAMPLE));
        let st = state(&Install::default());
        assert!(!st.readonly, "{:?}", st.notes);
        assert_eq!(st.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["deepseek", "zhipu"]);
        assert_eq!(models(&st, "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), true)]);
        assert_eq!(st.providers[0].base_url.as_deref(), Some("https://api.deepseek.com/v1"));
        let (base, key, api) = provider_endpoint("zhipu").unwrap();
        assert_eq!((base.as_str(), key.as_deref(), api.as_str()), ("https://open.bigmodel.cn/api/paas/v4", Some("sk-zp-2222"), "chat"));
    }

    #[test]
    fn new_file_is_an_array() {
        let h = setup("create", None);
        let err = plan(&[upsert("R", "https://r.example.com/v1", "anthropic", None, &["m"])], true).err().unwrap();
        assert!(err.to_string().contains("WorkBuddy 的自定义模型只支持"), "{err}");
        plan(&[upsert("My Relay", "https://r.example.com/v1/", "chat", Some("sk-new-9876"), &["gpt-5"])], false).unwrap();
        let v = file(&h);
        assert_eq!(ids(&v), ["gpt-5"]);
        assert_eq!(v[0]["url"], "https://r.example.com/v1/chat/completions");
        assert_eq!(v[0]["vendor"], "My Relay");
    }

    #[test]
    fn hide_moves_the_entry_to_the_store() {
        let h = setup("hide", Some(SAMPLE));
        let (d, w, _) = plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        assert!(diff_text(&d).contains("隐藏"), "{}", diff_text(&d));
        assert_eq!(w.len(), 1);
        let v = file(&h);
        // Still an array, no availableModels written.
        assert_eq!(ids(&v), ["deepseek-chat", "glm-4.6"]);
        let st = state(&Install::default());
        assert_eq!(models(&st, "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), false)]);
        assert!(st.providers[0].enabled);
        // A hidden model keeps its id: no other provider may take it.
        assert!(plan(&[Op::UpsertModel { provider: "zhipu".into(), model: ModelInput { id: "deepseek-reasoner".into(), ..Default::default() } }], true).is_err());
        // Editing a hidden model edits the kept entry.
        plan(&[Op::UpsertModel { provider: "deepseek".into(), model: ModelInput { id: "deepseek-reasoner".into(), context: Some(64000), ..Default::default() } }], false).unwrap();
        plan(&[vis("deepseek", "deepseek-reasoner", true)], false).unwrap();
        let v = file(&h);
        assert_eq!(ids(&v), ["deepseek-chat", "glm-4.6", "deepseek-reasoner"]);
        // Fields AgentPlus doesn't know come back unchanged.
        assert_eq!(v[2]["reasoning"], json!({ "defaultEffort": "high" }));
        assert_eq!(v[2]["maxInputTokens"], 64000);
        // Showing a shown model changes nothing.
        let (d, _, _) = plan(&[vis("deepseek", "deepseek-chat", true)], true).unwrap();
        assert!(d.groups.is_empty());
    }

    #[test]
    fn disable_keeps_hidden_models_hidden() {
        let h = setup("disable", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        plan(&[enable("deepseek", false)], false).unwrap();
        assert_eq!(ids(&file(&h)), ["glm-4.6"]);
        let st = state(&Install::default());
        let ds = st.providers.iter().find(|p| p.id == "deepseek").unwrap();
        assert!(!ds.enabled);
        assert_eq!(models(&st, "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), false)]);
        assert!(plan(&[vis("deepseek", "deepseek-chat", false)], true).is_err());
        assert_eq!(provider_endpoint("deepseek").unwrap().1.as_deref(), Some("sk-ds-1111"));

        plan(&[enable("deepseek", true)], false).unwrap();
        assert_eq!(ids(&file(&h)), ["glm-4.6", "deepseek-chat"]);
        let st = state(&Install::default());
        assert!(st.providers.iter().find(|p| p.id == "deepseek").unwrap().enabled);
        assert_eq!(models(&st, "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), false)]);
    }

    #[test]
    fn disable_with_every_model_hidden() {
        let h = setup("allhidden", Some(SAMPLE));
        plan(&[vis("zhipu", "glm-4.6", false)], false).unwrap();
        let (d, w, _) = plan(&[enable("zhipu", false)], false).unwrap();
        assert!(w.is_empty() && !d.groups.is_empty(), "{}", diff_text(&d));
        assert!(!state(&Install::default()).providers.iter().find(|p| p.id == "zhipu").unwrap().enabled);
        let store0 = crate::store::load();
        let (preview, written, _) = plan(&[enable("zhipu", true)], true).unwrap();
        assert!(written.is_empty());
        assert_eq!(crate::store::load(), store0);
        assert_eq!(preview.groups.iter().map(|g| g.file.as_str()).collect::<Vec<_>>(), ["AgentPlus · WorkBuddy"]);
        assert!(diff_text(&preview).contains("已启用"));
        let (applied, written, _) = plan(&[enable("zhipu", true)], false).unwrap();
        assert!(written.is_empty());
        assert_eq!(diff_text(&applied), diff_text(&preview));
        let st = state(&Install::default());
        assert!(st.providers.iter().find(|p| p.id == "zhipu").unwrap().enabled);
        assert_eq!(models(&st, "zhipu"), [("glm-4.6".into(), false)]);
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "deepseek-reasoner"]);
        assert!(plan(&[enable("zhipu", true)], true).unwrap().0.groups.is_empty());
    }

    #[test]
    fn provider_ids_survive_hiding_restoring_and_edits() {
        let h = setup("stable-ids", Some(r#"[{"id":"a","vendor":"V","url":"https://a.example/v1/chat/completions","apiKey":"sk-a"},{"id":"b","vendor":"V","url":"https://b.example/v1/chat/completions"}]"#));
        let before = state(&Install::default());
        let a = before.providers.iter().find(|p| p.base_url.as_deref() == Some("https://a.example/v1")).unwrap().id.clone();
        let b = before.providers.iter().find(|p| p.base_url.as_deref() == Some("https://b.example/v1")).unwrap().id.clone();
        assert_eq!((&a[..], &b[..]), ("v", "v-2"));
        plan(&[vis(&a, "a", false)], true).unwrap();
        assert!(!agentplus_dir().join("store.json").exists());
        plan(&[vis(&a, "a", false)], false).unwrap();
        assert_eq!(provider_endpoint(&a).unwrap().0, "https://a.example/v1");
        assert_eq!(provider_endpoint(&b).unwrap().0, "https://b.example/v1");
        plan(&[vis(&a, "a", true)], false).unwrap();
        assert_eq!(provider_endpoint(&a).unwrap().0, "https://a.example/v1");
        plan(&[enable(&b, false)], false).unwrap();
        plan(&[enable(&b, true)], false).unwrap();
        assert_eq!(provider_endpoint(&b).unwrap().0, "https://b.example/v1");

        let mut edit = upsert("Renamed", "https://moved.example/v1", "chat", Some("sk-new"), &[]);
        if let Op::UpsertProvider { provider } = &mut edit {
            provider.id = Some(a.clone());
        }
        plan(&[edit], false).unwrap();
        assert_eq!(provider_endpoint(&a).unwrap().0, "https://moved.example/v1");
        // A newly added native entry ahead of the others cannot take a saved ID.
        let mut v = file(&h);
        v.as_array_mut().unwrap().insert(0, json!({ "id": "c", "vendor": "V", "url": "https://c.example/v1/chat/completions" }));
        std::fs::write(h.0.join(".workbuddy/models.json"), v.to_string()).unwrap();
        assert_eq!(provider_endpoint(&a).unwrap().0, "https://moved.example/v1");
        assert_eq!(provider_endpoint(&b).unwrap().0, "https://b.example/v1");
        let registry = crate::store::get_obj(&crate::store::load(), ID, "providerIds");
        assert!(registry.keys().all(|k| k.len() == 64));
        assert!(!serde_json::to_string(&registry).unwrap().contains("sk-"));
    }

    #[test]
    fn rehiding_a_readded_model_merges_its_kept_copy() {
        let h = setup("rehide", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        plan(&[Op::UpsertModel { provider: "deepseek".into(), model: ModelInput { id: "deepseek-reasoner".into(), context: Some(64000), ..Default::default() } }], false).unwrap();
        let mut v = file(&h);
        v.as_array_mut().unwrap().push(json!({ "id": "deepseek-reasoner", "name": "R1", "vendor": "DeepSeek", "url": "https://api.deepseek.com/v1/chat/completions", "apiKey": "sk-ds-1111", "maxInputTokens": 128000, "nativeField": 42 }));
        std::fs::write(h.0.join(".workbuddy/models.json"), v.to_string()).unwrap();
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), true)]);
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        let kept = crate::store::get_arr(&crate::store::load(), ID, "parked");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["entry"]["maxInputTokens"], 64000);
        plan(&[vis("deepseek", "deepseek-reasoner", true)], false).unwrap();
        assert!(crate::store::get_arr(&crate::store::load(), ID, "parked").is_empty());
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), true)]);
        assert_eq!(file(&h)[2]["nativeField"], 42);
        assert_eq!(file(&h)[2]["maxInputTokens"], 64000);
    }

    #[test]
    fn old_duplicate_hidden_copies_are_reconciled() {
        for show in [false, true] {
            let h = setup(&format!("duplicate-{show}"), Some(SAMPLE));
            plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
            let mut root = crate::store::load();
            let mut parked = crate::store::get_arr(&root, ID, "parked");
            let mut duplicate = parked[0].clone();
            duplicate["entry"]["nativeField"] = json!(42);
            parked.push(duplicate);
            crate::store::set_value(&mut root, ID, "parked", json!(parked));
            crate::store::save(&root).unwrap();
            assert_eq!(models(&state(&Install::default()), "deepseek").len(), 2);
            let (diff, _, _) = plan(&[vis("deepseek", "deepseek-reasoner", show)], false).unwrap();
            assert!(!diff.groups.is_empty());
            let parked = crate::store::get_arr(&crate::store::load(), ID, "parked");
            assert_eq!(parked.len(), usize::from(!show));
            if show {
                assert_eq!(file(&h)[2]["nativeField"], 42);
            } else {
                assert_eq!(parked[0]["entry"]["nativeField"], 42);
            }
            assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), show)]);
        }
    }

    #[test]
    fn disabling_a_readded_model_keeps_one_restorable_copy() {
        let h = setup("disable-readded", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        let mut v = file(&h);
        let sample: Value = serde_json::from_str(SAMPLE).unwrap();
        v.as_array_mut().unwrap().push(sample[1].clone());
        std::fs::write(h.0.join(".workbuddy/models.json"), v.to_string()).unwrap();
        plan(&[enable("deepseek", false)], false).unwrap();
        assert_eq!(crate::store::get_arr(&crate::store::load(), ID, "parked").len(), 2);
        plan(&[enable("deepseek", true)], false).unwrap();
        assert!(crate::store::get_arr(&crate::store::load(), ID, "parked").is_empty());
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), true)]);
    }

    #[test]
    fn shared_directory_aliases_are_read_only() {
        let h = setup("shared-alias", Some(SAMPLE));
        let cb = h.0.join(".workbuddy").to_string_lossy().into_owned();
        // Both agents there (their folders picked), each by a different spelling of one folder.
        crate::adapters::set_dir(codebuddy::ID, Some(&cb)).unwrap();
        for alias in [h.0.join(".workbuddy").join("."), h.0.join(".workbuddy-ai").join("..").join(".workbuddy")] {
            let wb = alias.to_string_lossy().into_owned();
            crate::adapters::set_dir(ID, Some(&wb)).unwrap();
            assert!(state(&Install::default()).readonly);
            assert!(plan(&[vis("deepseek", "deepseek-chat", false)], true).is_err());
            assert!(codebuddy::plan(&[vis("deepseek", "deepseek-chat", false)], true).is_err());
        }
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "deepseek-reasoner", "glm-4.6"]);
    }

    #[test]
    fn delete_and_replace_models_include_hidden_ones() {
        let h = setup("delete", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        plan(&[Op::SetProviderModels { provider: "deepseek".into(), models: vec!["deepseek-chat".into(), "deepseek-v4".into()] }], false).unwrap();
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "glm-4.6", "deepseek-v4"]);
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-v4".into(), true)]);
        plan(&[vis("zhipu", "glm-4.6", false)], false).unwrap();
        plan(&[Op::DeleteProvider { provider: "zhipu".into() }], false).unwrap();
        let st = state(&Install::default());
        assert!(st.providers.iter().all(|p| p.id != "zhipu"));
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "deepseek-v4"]);
    }

    #[test]
    fn show_after_the_id_came_back_in_the_file() {
        let h = setup("readded", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        // Edited while hidden.
        plan(&[Op::UpsertModel { provider: "deepseek".into(), model: ModelInput { id: "deepseek-reasoner".into(), context: Some(64000), ..Default::default() } }], false).unwrap();
        // Added again in WorkBuddy itself, by the same provider: the kept copy is merged into it.
        let mut v = file(&h);
        v.as_array_mut().unwrap().push(json!({ "id": "deepseek-reasoner", "name": "R1", "vendor": "DeepSeek", "url": "https://api.deepseek.com/v1/chat/completions", "apiKey": "sk-ds-1111", "maxInputTokens": 128000 }));
        std::fs::write(h.0.join(".workbuddy/models.json"), v.to_string()).unwrap();
        let (d, w, _) = plan(&[vis("deepseek", "deepseek-reasoner", true)], false).unwrap();
        assert_eq!(w.len(), 1);
        assert!(diff_text(&d).contains("models.deepseek-reasoner.maxInputTokens = 64000"), "{}", diff_text(&d));
        let v = file(&h);
        assert_eq!(ids(&v), ["deepseek-chat", "glm-4.6", "deepseek-reasoner"]);
        assert_eq!((&v[2]["maxInputTokens"], &v[2]["name"], &v[2]["reasoning"]), (&json!(64000), &json!("R1"), &json!({ "defaultEffort": "high" })));
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true), ("deepseek-reasoner".into(), true)]);
        // By another provider: showing is refused, nothing duplicated.
        plan(&[vis("zhipu", "glm-4.6", false)], false).unwrap();
        let mut v = file(&h);
        v.as_array_mut().unwrap().push(json!({ "id": "glm-4.6", "vendor": "Other", "url": "https://o.example/v1/chat/completions" }));
        std::fs::write(h.0.join(".workbuddy/models.json"), v.to_string()).unwrap();
        assert!(plan(&[vis("zhipu", "glm-4.6", true)], true).is_err());
    }

    #[test]
    fn deleting_a_hidden_model_changes_only_the_store() {
        let h = setup("delhidden", Some(SAMPLE));
        plan(&[vis("deepseek", "deepseek-reasoner", false)], false).unwrap();
        let (d, w, _) = plan(&[Op::DeleteModel { provider: "deepseek".into(), model: "deepseek-reasoner".into() }], false).unwrap();
        assert!(w.is_empty());
        assert_eq!(d.groups.iter().map(|g| g.file.as_str()).collect::<Vec<_>>(), ["AgentPlus · WorkBuddy"]);
        assert_eq!(models(&state(&Install::default()), "deepseek"), [("deepseek-chat".into(), true)]);
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "glm-4.6"]);
    }

    #[test]
    fn codebuddy_sharing_the_folder_creates_no_available_models() {
        let h = setup("cbshared", Some(SAMPLE));
        let wb = h.0.join(".workbuddy").to_string_lossy().to_string();
        crate::env::set_test_vars(&[("CODEBUDDY_CONFIG_DIR", &wb)]);
        // The variable alone also points WorkBuddy here, but WorkBuddy isn't there: CodeBuddy on its own.
        assert!(!codebuddy::state(&Install::default()).notes.iter().any(|n| n.contains("WorkBuddy")));
        assert!(codebuddy::plan(&[vis("deepseek", "deepseek-chat", false)], true).is_ok());
        crate::adapters::set_dir(ID, Some(&wb)).unwrap();
        assert!(codebuddy::state(&Install::default()).notes[0].contains("WorkBuddy"));
        let err = codebuddy::plan(&[vis("deepseek", "deepseek-chat", false)], true).err().unwrap();
        assert!(err.to_string().contains("availableModels"), "{err}");
        // Other edits still work, and the file keeps its array form.
        codebuddy::plan(&[Op::DeleteModel { provider: "zhipu".into(), model: "glm-4.6".into() }], false).unwrap();
        assert_eq!(ids(&file(&h)), ["deepseek-chat", "deepseek-reasoner"]);
        // A list the file already has is edited as usual.
        std::fs::write(h.0.join(".workbuddy/models.json"), r#"{"models":[{"id":"a","vendor":"V","url":"https://x/v1/chat/completions"}],"availableModels":["a"]}"#).unwrap();
        codebuddy::plan(&[vis("v", "a", false)], false).unwrap();
        assert_eq!(file(&h)["availableModels"], json!([]));
        crate::env::set_test_vars(&[]);
    }

    #[test]
    fn a_shared_folder_is_read_only() {
        let h = setup("shared", Some(SAMPLE));
        let wb = h.0.join(".workbuddy").to_string_lossy().to_string();
        crate::env::set_test_vars(&[("CODEBUDDY_CONFIG_DIR", &wb)]);
        // CodeBuddy isn't there: the variable alone doesn't make the folder shared.
        assert!(!state(&Install::default()).readonly);
        crate::adapters::set_dir(codebuddy::ID, Some(&wb)).unwrap();
        let st = state(&Install::default());
        assert!(st.readonly && st.notes[0].contains("CodeBuddy"), "{:?}", st.notes);
        assert!(plan(&[vis("deepseek", "deepseek-chat", false)], true).is_err());
        crate::adapters::set_dir(codebuddy::ID, None).unwrap();
        // Both editions in one folder: WorkBuddy owns it, once it is there.
        crate::env::set_test_vars(&[("WORKBUDDY_CONFIG_DIR", &wb)]);
        assert!(!ai::state(&Install::default()).readonly);
        crate::adapters::set_dir(ID, Some(&wb)).unwrap();
        assert!(!state(&Install::default()).readonly);
        let st = ai::state(&Install::default());
        assert!(st.readonly && st.notes[0].contains("和 WorkBuddy 用的是同一个"), "{:?}", st.notes);
        assert!(ai::plan(&[vis("deepseek", "deepseek-chat", false)], true).is_err());
        crate::env::set_test_vars(&[]);
    }

    #[test]
    fn object_form_keeps_available_models() {
        let h = setup("object", Some(r#"{"models":[{"id":"a","vendor":"V","url":"https://x/v1/chat/completions"},{"id":"b","vendor":"V","url":"https://x/v1/chat/completions"}],"availableModels":["a"]}"#));
        let st = state(&Install::default());
        // availableModels isn't read as visibility here.
        assert_eq!(models(&st, "v"), [("a".into(), true), ("b".into(), true)]);
        assert!(st.notes.iter().any(|n| n.contains("availableModels")));
        plan(&[vis("v", "b", false)], false).unwrap();
        let v = file(&h);
        assert_eq!(v["models"].as_array().unwrap().len(), 1);
        assert_eq!(v["availableModels"], json!(["a"]));
    }

    /// Read-only look at the real machine: both editions' state and a dry-run plan.
    #[test]
    #[ignore]
    fn dump_workbuddy() {
        type Fns = (fn() -> Install, fn(&Install) -> AgentState, fn(&[Op], bool) -> Result<Plan>);
        let editions: [(&Edition, Fns); 2] = [(&CN, (detect, state, plan)), (&ai::AI, (ai::detect, ai::state, ai::plan))];
        for (e, (detect, state, plan)) in editions {
            let inst = detect();
            println!("== {}: installed={} version={:?} running={} exe={:?} folder={:?}", e.flavor.name, inst.installed, inst.version, inst.running, inst.exe, lock(&e.found).clone());
            let st = state(&inst);
            println!("dir={} files={:?} readonly={} notes={:?}", st.config_dir, st.files, st.readonly, st.notes);
            for p in &st.providers {
                println!("  provider {} ({}) enabled={} has_key={} models={:?}", p.id, p.name, p.enabled, p.has_key, p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
            }
            let (d, w, b) = plan(&[upsert("Dry Run", "https://dry.example.com/v1", "chat", Some("sk-dryrun-0000"), &["m1"])], true).unwrap();
            println!("dry-run diff:\n{}", diff_text(&d));
            assert!(w.is_empty() && b.is_none());
        }
    }
}
