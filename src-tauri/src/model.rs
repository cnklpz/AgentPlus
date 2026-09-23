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
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub visible: bool,
    pub readonly: bool,
    pub tags: Vec<String>,
    pub ctx: Option<String>,
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
    }
}
