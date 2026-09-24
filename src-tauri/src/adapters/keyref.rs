//! Helpers shared by the adapters whose config holds one entry per model, each carrying its
//! own base URL and API key (Droid's `customModels`, CodeBuddy's `models.json`). AgentPlus
//! groups those entries into providers by a three-part key, and the API key may be a
//! literal or a `${VAR}` / `$VAR` reference to an environment variable.

use crate::i18n::l;
use crate::util::host_of;
use anyhow::{anyhow, Result};

/// What groups entries into one provider; each adapter documents its field order.
pub(super) type Key = (String, String, String);

/// One AgentPlus provider: the entries sharing `key`.
#[derive(Clone, Debug)]
pub(super) struct Group {
    pub id: String,
    pub key: Key,
    pub name: String,
}

/// `${VAR}` / `$VAR` → the variable name.
pub(super) fn env_ref(k: &str) -> Option<&str> {
    let k = k.trim();
    k.strip_prefix("${").and_then(|r| r.strip_suffix('}')).or_else(|| k.strip_prefix('$')).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// The key a request would use: a literal, or the value of the referenced env variable.
pub(super) fn resolve_key(k: &str) -> Option<String> {
    match env_ref(k) {
        Some(var) => crate::env::agent_var(var).filter(|v| !v.trim().is_empty()),
        None => Some(k.trim().to_string()).filter(|v| !v.is_empty()),
    }
}

/// The provider has a key: a literal, or an env reference (even one that is unset here —
/// the agent may see it).
pub(super) fn has_key(k: &str) -> bool {
    resolve_key(k).is_some() || env_ref(k).is_some()
}

/// Where the key comes from, for the provider details; `file` holds the entries.
pub(super) fn key_note(k: &str, file: &str) -> String {
    match env_ref(k) {
        Some(var) => if resolve_key(k).is_some() { tr!("环境变量 ${{{var}}}（已设置）", "Environment variable ${{{var}}} (set)") } else { tr!("环境变量 ${{{var}}}（未设置）", "Environment variable ${{{var}}} (not set)") },
        None if k.trim().is_empty() => l("未填写", "Not set").into(),
        None => tr!("明文保存在 {file}", "Stored in plain text in {file}"),
    }
}

/// The host name of a base URL (a provider's default name), else `host_of`.
pub(super) fn host(base: &str) -> String {
    url::Url::parse(base).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| host_of(base))
}

/// Changing the models of a disabled provider (its entries are parked in the store).
pub(super) fn require_enabled(g: &Group, enabled: bool) -> Result<()> {
    if enabled { Ok(()) } else { Err(anyhow!(tr!("供应商「{}」已停用，先启用再调整模型", "Provider \"{}\" is disabled; enable it before changing its models", g.name))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    #[test]
    fn env_refs() {
        assert_eq!(env_ref("${OPENAI_KEY}"), Some("OPENAI_KEY"));
        assert_eq!(env_ref("  $OPENAI_KEY "), Some("OPENAI_KEY"));
        // A leading digit is accepted (pimodels' stricter is_env_name rejects it).
        assert_eq!(env_ref("$1KEY"), Some("1KEY"));
        for k in ["", "$", "${}", "${A-B}", "${A", "sk-$abc", "$A B"] {
            assert_eq!(env_ref(k), None, "{k:?}");
        }
    }

    #[test]
    fn resolves_keys() {
        let _h = TestHome::new("keyref");
        crate::env::set_test_vars(&[("SET_KEY", "sk-env"), ("BLANK_KEY", "  ")]);
        assert_eq!(resolve_key(" sk-lit "), Some("sk-lit".into()));
        assert_eq!(resolve_key("  "), None);
        assert_eq!(resolve_key("${SET_KEY}"), Some("sk-env".into()));
        assert_eq!(resolve_key("$BLANK_KEY"), None);
        assert_eq!(resolve_key("${MISSING_KEY}"), None);
        assert!(has_key("${MISSING_KEY}") && has_key("sk-lit") && !has_key(""));
        assert_eq!(key_note("${SET_KEY}", "x.json"), "环境变量 ${SET_KEY}（已设置）");
        assert_eq!(key_note("$MISSING_KEY", "x.json"), "环境变量 ${MISSING_KEY}（未设置）");
        assert_eq!(key_note("", "x.json"), "未填写");
        assert_eq!(key_note("sk-lit", "models.json"), "明文保存在 models.json");
    }
}
