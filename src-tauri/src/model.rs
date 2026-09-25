//! Data shapes shared with the frontend.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentState {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub running: bool,
    /// "single": one active provider (Codex); "multi": several enabled at once.
    pub mode: String,
    pub config_dir: String,
    pub files: Vec<String>,
    pub current_provider: Option<String>,
    pub providers: Vec<Provider>,
    /// Codex keeps one global model catalog instead of per-provider lists.
    pub catalog: Option<Vec<Model>>,
    pub catalog_file: Option<String>,
    pub settings: Vec<Setting>,
    pub current: Vec<Kv>,
    /// The model the agent sends requests with, whichever provider is active (Codex `model`);
    /// None when unset or when the model belongs to one provider (e.g. Gemini's profiles).
    pub current_model: Option<String>,
    pub notes: Vec<String>,
    pub readonly: bool,
    /// Codex only: not on the fixed id yet, but could be (a custom provider is active).
    pub fixed_pending: bool,
    /// Codex only: prefill "turn on fixed id" as a pending change (user hasn't declined).
    pub fixed_prompt: bool,
    /// A desktop app AgentPlus can restart (CLI agents pick up changes on their next run).
    pub restartable: bool,
    /// Per-model settings beyond name / context this agent's config understands.
    /// Filled in adapters::state.
    pub model_fields: Vec<ModelField>,
}

/// One per-model setting (e.g. image input, max output tokens). `key` is its path inside
/// the model's entry and the key of `Model::extra` / `ModelInput::extra`.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelField {
    pub key: String,
    /// Stable group id ("io" | "gen"); `group` is its display label.
    pub gid: String,
    pub group: String,
    pub label: String,
    pub desc: String,
    /// "bool" | "number" | "chips" | "select"
    pub kind: String,
    pub options: Vec<String>,
    /// One short label per option (same order).
    pub hints: Vec<String>,
    /// Short tags for the model table's capability column: one for a bool input field (shown
    /// when on); one per option for chips, or none to use `hints`.
    pub caps: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: Option<String>,
    pub host: String,
    pub apis: Vec<String>,
    pub builtin: bool,
    pub enabled: bool,
    pub compatible: bool,
    pub reason: Option<String>,
    pub models: Vec<Model>,
    /// Extra facts for the detail panel. Never contains secret values.
    pub details: Vec<Kv>,
    /// Editable through AgentPlus (false for built-in providers).
    pub editable: bool,
    /// "responses" | "chat" | "anthropic" (for the edit form).
    pub api: String,
    pub has_key: bool,
    /// Short one-way fingerprint of the key: tells entries with the same key apart
    /// from entries with different keys (e.g. a relay's protocol groups). Filled in adapters::state.
    pub key_fp: Option<String>,
    /// "••••abcd"
    pub key_hint: Option<String>,
    /// Codex: keeps the ChatGPT sign-in while requests go to this provider (`requires_openai_auth`).
    pub official_auth: bool,
}

impl AgentState {
    /// The config can't be read: say why and show the agent read-only.
    pub fn fail(&mut self, e: impl std::fmt::Display) {
        self.notes.push(e.to_string());
        self.readonly = true;
    }
}

impl Provider {
    /// A read-only provider built into the agent (an account sign-in, a vendor it ships
    /// with): enabled, with credentials, not editable. `label` is its badge (protocol or kind).
    pub fn builtin(id: impl Into<String>, name: impl Into<String>, host: impl Into<String>, api: &str, label: &str, details: Vec<Kv>) -> Self {
        Provider {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            apis: vec![label.into()],
            builtin: true,
            enabled: true,
            compatible: true,
            details,
            api: api.into(),
            has_key: true,
            ..Default::default()
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub visible: bool,
    pub readonly: bool,
    pub tags: Vec<Tag>,
    pub ctx: Option<String>,
    /// Display name, when the agent stores one.
    pub name: Option<String>,
    /// Raw context window in tokens, for editing.
    pub context: Option<u64>,
    /// Can be removed from the list (custom / user-added models).
    pub deletable: bool,
    /// Values of the agent's model fields that are set, by field key.
    pub extra: std::collections::BTreeMap<String, Value>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    pub group: String,
    pub label: String,
    pub desc: String,
    /// "bool" | "chips" | "select"
    pub kind: String,
    pub value: Value,
    pub options: Vec<String>,
    /// One short explanation per option (same order), may be empty.
    pub hints: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Kv {
    pub k: String,
    pub v: String,
    pub mono: bool,
}

impl Kv {
    pub fn mono(k: &str, v: impl Into<String>) -> Self {
        Kv { k: k.into(), v: v.into(), mono: true }
    }
    pub fn text(k: &str, v: impl Into<String>) -> Self {
        Kv { k: k.into(), v: v.into(), mono: false }
    }
}

/// Labels of `Kv` rows several adapters show, so each reads the same everywhere.
pub(crate) mod lbl {
    use crate::i18n::l;

    pub fn api_key() -> &'static str {
        l("密钥", "API key")
    }
    pub fn auth() -> &'static str {
        l("认证方式", "Authentication")
    }
    pub fn base_url() -> &'static str {
        l("地址", "Base URL")
    }
    pub fn config_file() -> &'static str {
        l("配置文件", "Config file")
    }
    pub fn config_id() -> &'static str {
        l("配置 ID", "Config ID")
    }
    pub fn config_location() -> &'static str {
        l("配置位置", "Config location")
    }
    pub fn credentials() -> &'static str {
        l("凭据", "Credentials")
    }
    pub fn current_model() -> &'static str {
        l("当前模型", "Current model")
    }
    pub fn custom_models() -> &'static str {
        l("自定义模型", "Custom models")
    }
    pub fn custom_providers() -> &'static str {
        l("自定义供应商", "Custom providers")
    }
    pub fn default_model() -> &'static str {
        l("默认模型", "Default model")
    }
    pub fn note() -> &'static str {
        l("说明", "Note")
    }
    pub fn provider() -> &'static str {
        l("供应商", "Provider")
    }
    pub fn small_model() -> &'static str {
        l("小模型", "Small model")
    }
    pub fn source() -> &'static str {
        l("来源", "Source")
    }
    pub fn status() -> &'static str {
        l("状态", "Status")
    }
    pub fn visible_models() -> &'static str {
        l("可见模型", "Visible models")
    }

    /// A summary row's list of names ("A、B" / "A, B"), or "None".
    pub fn names_or_none<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> String {
        let names: Vec<String> = names.into_iter().map(|s| s.as_ref().to_string()).collect();
        if names.is_empty() { l("无", "None").into() } else { crate::i18n::join(&names) }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    SetCurrentProvider { provider: String },
    SetProviderEnabled { provider: String, enabled: bool },
    SetModelVisible { provider: String, model: String, visible: bool },
    SetSetting { key: String, value: Value },
    /// Create (id = None) or edit a provider.
    UpsertProvider { provider: ProviderInput },
    DeleteProvider { provider: String },
    /// Add a model to a provider's list, or edit its name / context window.
    UpsertModel { provider: String, model: ModelInput },
    DeleteModel { provider: String, model: String },
    /// Codex: the model list that belongs to one provider (swapped into the catalog on switch).
    SetProviderModels { provider: String, models: Vec<String> },
    /// Claude Code: which model each role uses for one provider (default / opus / sonnet / haiku / subagent).
    SetModelRoles { provider: String, roles: std::collections::BTreeMap<String, String> },
    /// Copy a provider (address, key, visible models) from another agent, or from the
    /// shared library (from_agent = "library"). Resolved in the backend so the key never
    /// reaches the UI. `api` / `name` override what the source says.
    #[serde(rename_all = "camelCase")]
    ImportProvider {
        from_agent: String,
        provider: String,
        #[serde(default)]
        api: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    /// Existing provider id when editing; None creates a new one.
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    /// "responses" | "chat" | "anthropic"
    pub api: String,
    /// None = keep the current key. Never echoed back or put in diffs.
    pub api_key: Option<String>,
    /// Initial model ids for a new provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Take the key from this library entry (resolved in the backend; the UI never has it).
    #[serde(default)]
    pub key_from_library: Option<String>,
    /// Codex: official sign-in mixed with this provider. None = keep as is.
    #[serde(default)]
    pub official_auth: Option<bool>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelInput {
    pub id: String,
    pub name: Option<String>,
    pub context: Option<u64>,
    /// Model fields to change, by key; null clears one (back to the agent's default).
    #[serde(default)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

/// One-way 40-bit fingerprint of a secret (FNV-1a), for grouping only.
pub fn key_fingerprint(k: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in k.trim().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:010x}", h >> 24)
}

/// A badge on a model. `id` is stable (`fast`, `custom`, `cap:image`, `role:default`, …) for
/// the UI to act on; `label` is display text in the current language.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Tag {
    pub id: String,
    pub label: String,
}

impl Tag {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Tag { id: id.into(), label: label.into() }
    }

    /// A model the agent uses by default.
    pub fn default_model() -> Self {
        Tag::new("role:default", crate::i18n::l("默认", "Default"))
    }

    /// The model currently selected in the agent.
    pub fn current() -> Self {
        Tag::new("current", crate::i18n::l("当前", "Current"))
    }
}

/// Masks a secret for display: "••••abcd".
pub fn mask_key(k: &str) -> String {
    // A short key would be mostly (or entirely) shown by its last four characters.
    if k.chars().count() < 8 {
        return "••••".into();
    }
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

/// Turns a display name into a config-safe id ("My Relay" -> "my-relay"; Chinese is
/// spelled in pinyin: "中转站" -> "zhong-zhuan-zhan").
pub fn slug(name: &str) -> String {
    let s: String = deunicode::deunicode(name)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "provider".into() } else { s }
}

/// `base` when it is free, else the first free `base-2`, `base-3`, …; `taken` decides for
/// every candidate.
pub fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken(c)).unwrap()
}

/// Model ids as typed by the user: trimmed, blanks dropped, duplicates removed (the first
/// one stays, order is kept).
pub fn clean_ids<S: AsRef<str>>(ids: &[S]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in ids.iter().map(|s| s.as_ref().trim()).filter(|s| !s.is_empty()) {
        if !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Display label of an AgentPlus api ("responses" → "Responses"); "chat" and anything
/// unknown show as "Chat".
pub fn api_label(api: &str) -> &'static str {
    match api {
        "anthropic" => "Anthropic",
        "responses" => "Responses",
        "gemini" => "Gemini",
        _ => "Chat",
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct DiffLine {
    pub text: String,
    pub add: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiffGroup {
    pub file: String,
    pub lines: Vec<DiffLine>,
}

/// Collects diff lines grouped by file, in first-seen order.
#[derive(Default)]
pub struct Diff {
    pub groups: Vec<DiffGroup>,
}

impl Diff {
    pub fn push(&mut self, file: &str, text: impl Into<String>, add: bool) {
        let line = DiffLine { text: text.into(), add };
        match self.groups.iter_mut().find(|g| g.file == file) {
            Some(g) => g.lines.push(line),
            None => self.groups.push(DiffGroup { file: file.into(), lines: vec![line] }),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub state: AgentState,
    pub files: Vec<String>,
    pub backup_dir: Option<String>,
}

pub fn bool_setting(key: &str, group: &str, label: &str, desc: &str, value: bool) -> Setting {
    Setting {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        desc: desc.into(),
        kind: "bool".into(),
        value: Value::Bool(value),
        options: vec![],
        hints: vec![],
    }
}

pub fn chips_setting(key: &str, group: &str, label: &str, desc: &str, value: Vec<String>, options: &[&str]) -> Setting {
    Setting {
        key: key.into(),
        group: group.into(),
        label: label.into(),
        desc: desc.into(),
        kind: "chips".into(),
        value: Value::from(value),
        options: options.iter().map(|s| s.to_string()).collect(),
        hints: vec![],
    }
}

impl Setting {
    pub fn with_hints(mut self, hints: &[&str]) -> Self {
        self.hints = hints.iter().map(|s| s.to_string()).collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{api_label, clean_ids, mask_key, slug, unique_id};

    #[test]
    fn unique_id_checks_every_candidate() {
        assert_eq!(unique_id("relay", |_| false), "relay");
        let taken = ["relay", "relay-2", "relay-4"];
        assert_eq!(unique_id("relay", |c| taken.contains(&c)), "relay-3");
        // One rule for the base and the numbered ids (a reserved suffix is skipped too).
        assert_eq!(unique_id("x", |c| c == "x" || c.ends_with("-2")), "x-3");
    }

    #[test]
    fn clean_ids_trims_drops_and_dedupes() {
        assert_eq!(clean_ids(&[" b ", "", "a", "b", "  ", "a ", "c"]), vec!["b", "a", "c"]);
        assert!(clean_ids::<String>(&[]).is_empty());
        assert_eq!(clean_ids(&["m1".to_string(), "m1".to_string()]), vec!["m1"]);
    }

    #[test]
    fn names_or_none() {
        assert_eq!(super::lbl::names_or_none(["a", "b"]), "a、b");
        assert_eq!(super::lbl::names_or_none(Vec::<String>::new()), "无");
    }

    #[test]
    fn api_labels() {
        for (api, label) in [("anthropic", "Anthropic"), ("responses", "Responses"), ("gemini", "Gemini"), ("chat", "Chat"), ("x", "Chat"), ("", "Chat")] {
            assert_eq!(api_label(api), label);
        }
    }

    #[test]
    fn slug_spells_chinese_in_pinyin() {
        assert_eq!(slug("My Relay"), "my-relay");
        assert_eq!(slug("中转站"), "zhong-zhuan-zhan");
        assert_eq!(slug("中转站OP"), "zhong-zhuan-zhan-op");
        assert_eq!(slug("小米 MiMo"), "xiao-mi-mimo");
        assert_eq!(slug("!!!"), "provider");
    }

    #[test]
    fn slug_edge_cases() {
        assert_eq!(slug(""), "provider");
        assert_eq!(slug("   "), "provider");
        assert_eq!(slug("a.b/c\\d\"e"), slug("a b c d e"));
        assert!(!slug("😀 Relay").contains(char::is_whitespace));
        let long = slug(&"x".repeat(500));
        assert!(!long.is_empty() && long.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn mask_key_never_shows_short_keys() {
        assert_eq!(mask_key("sk-abcdef-1234"), "••••1234");
        assert_eq!(mask_key("密钥密钥密钥密钥"), "••••密钥密钥");
        for short in ["", "abc", "abcd", "1234567"] {
            assert_eq!(mask_key(short), "••••", "{short:?}");
        }
    }
}
