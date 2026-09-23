//! Data shapes shared with the frontend.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
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
    pub notes: Vec<String>,
    pub readonly: bool,
    /// Codex only: not on the fixed id yet, but could be (a custom provider is active).
    #[serde(default)]
    pub fixed_pending: bool,
    /// Codex only: prefill "turn on fixed id" as a pending change (user hasn't declined).
    #[serde(default)]
    pub fixed_prompt: bool,
}

#[derive(Serialize, Clone, Debug)]
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
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub visible: bool,
    pub readonly: bool,
    pub tags: Vec<String>,
    pub ctx: Option<String>,
    /// Display name, when the agent stores one.
    pub name: Option<String>,
    /// Raw context window in tokens, for editing.
    pub context: Option<u64>,
    /// Can be removed from the list (custom / user-added models).
    pub deletable: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    pub group: String,
    pub label: String,
    pub desc: String,
    /// "bool" or "chips"
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
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelInput {
    pub id: String,
    pub name: Option<String>,
    pub context: Option<u64>,
}

/// Masks a secret for display: "••••abcd".
pub fn mask_key(k: &str) -> String {
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

/// Turns a display name into a config-safe id ("My Relay" -> "my-relay").
pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "provider".into() } else { s }
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
