//! AgentPlus profiles: providers kept in the AgentPlus store instead of the agent's own
//! config, for agents that take a single relay through a few settings (Claude Code, Gemini
//! CLI). A profile is `{ name, baseUrl, apiKey, models: [{ id, visible }], … }`; each adapter
//! adds its own fields (Claude's `roles` / `keyEnv`, Gemini's `defaultModel`), guards its
//! built-in entries and writes the active profile into the agent's config itself.
//!
//! The op helpers push the store diff line themselves and return whether anything changed;
//! agent-specific follow-ups (Gemini's default model, …) stay with the caller.

use super::msg;
use crate::model::*;
use crate::store;
use crate::util::str_field;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub(super) fn load(root: &Value, agent: &str) -> Map<String, Value> {
    store::get_obj(root, agent, "profiles")
}

pub(super) fn save(root: &mut Value, agent: &str, profs: Map<String, Value>) {
    store::set_value(root, agent, "profiles", Value::Object(profs));
}

/// A free id for a new profile named `name`, never one of the adapter's built-in ids.
pub(super) fn new_id(profs: &Map<String, Value>, name: &str, reserved: &[&str]) -> String {
    unique_id(&slug(name), |c| profs.contains_key(c) || reserved.contains(&c))
}

/// The profile `id`, or "provider not found".
pub(super) fn get_mut<'a>(profs: &'a mut Map<String, Value>, id: &str) -> Result<&'a mut Value> {
    profs.get_mut(id).ok_or_else(|| msg::no_provider(id))
}

/// (model id, visible) in list order.
pub(super) fn model_list(p: &Value) -> Vec<(String, bool)> {
    p.get("models")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().filter_map(|m| Some((m.get("id")?.as_str()?.to_string(), m.get("visible").and_then(|v| v.as_bool()).unwrap_or(true)))).collect())
        .unwrap_or_default()
}

pub(super) fn set_model_list(p: &mut Value, list: &[(String, bool)]) {
    p["models"] = Value::Array(list.iter().map(|(id, v)| json!({ "id": id, "visible": v })).collect());
}

/// The `models` value of a new list typed by the user: cleaned (see [`clean_ids`]), all visible.
pub(super) fn models_value<S: AsRef<str>>(ids: &[S]) -> Value {
    Value::Array(clean_ids(ids).into_iter().map(|m| json!({ "id": m, "visible": true })).collect())
}

/// The diff line of a new profile: `+ "name"adopt (where · API key ••••1234)`.
pub(super) fn push_added(diff: &mut Diff, label: &str, name: &str, adopt: &str, where_: &str, key: Option<&str>) {
    diff.push(label, tr!("+ \"{}\"{} ({}{})", "+ 「{}」{}（{}{}）", name, adopt, where_, msg::key_suffix(key)), true);
}

/// Edits the name, address and (when given) key of profile `e`. Only changed fields are
/// written and reported; true when something changed.
pub(super) fn edit(e: &mut Value, id: &str, name: &str, base: &str, key: Option<&str>, diff: &mut Diff, label: &str) -> bool {
    let mut changed = false;
    for (k, v) in [("name", name), ("baseUrl", base)] {
        if str_field(e, k) != v {
            e[k] = json!(v);
            diff.push(label, tr!("\"{id}\" {k} = {v}", "「{id}」{k} = {v}"), true);
            changed = true;
        }
    }
    if let Some(k) = key.filter(|k| str_field(e, "apiKey") != *k) {
        e["apiKey"] = json!(k);
        diff.push(label, tr!("\"{id}\" API key = {}", "「{id}」密钥 = {}", mask_key(k)), true);
        changed = true;
    }
    changed
}

/// Removes profile `id`; an unknown id is an error.
pub(super) fn delete(profs: &mut Map<String, Value>, id: &str, diff: &mut Diff, label: &str) -> Result<()> {
    profs.remove(id).ok_or_else(|| msg::no_provider(id))?;
    diff.push(label, tr!("- \"{id}\"", "- 「{id}」"), false);
    Ok(())
}

/// Shows or hides `model` of profile `p` (adding it when missing); false when it already was.
pub(super) fn set_visible(p: &mut Value, id: &str, model: &str, visible: bool, diff: &mut Diff, label: &str) -> bool {
    let mut list = model_list(p);
    match list.iter_mut().find(|(m, _)| m == model) {
        Some(e) if e.1 == visible => return false,
        Some(e) => e.1 = visible,
        None => list.push((model.to_string(), visible)),
    }
    set_model_list(p, &list);
    diff.push(label, if visible { tr!("\"{id}\" {model} shown", "「{id}」{model} 显示") } else { tr!("\"{id}\" {model} hidden", "「{id}」{model} 隐藏") }, visible);
    true
}

/// Adds `model` (trimmed) to profile `p`; false when it is already listed, an error when blank.
pub(super) fn add_model(p: &mut Value, id: &str, model: &str, diff: &mut Diff, label: &str) -> Result<bool> {
    let model = model.trim();
    if model.is_empty() {
        return Err(msg::model_id_required());
    }
    let mut list = model_list(p);
    if list.iter().any(|(x, _)| x == model) {
        return Ok(false);
    }
    list.push((model.to_string(), true));
    set_model_list(p, &list);
    diff.push(label, tr!("\"{id}\" + {model}", "「{id}」+ {model}"), true);
    Ok(true)
}

/// Removes `model` from profile `p`; false when it wasn't listed.
pub(super) fn delete_model(p: &mut Value, id: &str, model: &str, diff: &mut Diff, label: &str) -> bool {
    let mut list = model_list(p);
    let n = list.len();
    list.retain(|(x, _)| x != model);
    if list.len() == n {
        return false;
    }
    set_model_list(p, &list);
    diff.push(label, tr!("\"{id}\" - {model}", "「{id}」- {model}"), false);
    true
}

/// Replaces the model list of profile `p` (cleaned, all visible).
pub(super) fn set_models(p: &mut Value, id: &str, models: &[String], diff: &mut Diff, label: &str) {
    let list = models_value(models);
    let n = list.as_array().map_or(0, Vec::len);
    p["models"] = list;
    diff.push(label, tr!("\"{id}\" model list: {}", "「{id}」模型列表：{} 个", n), true);
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: &str = "store";

    fn lines(d: &Diff) -> Vec<String> {
        d.groups.iter().flat_map(|g| g.lines.iter().map(|l| l.text.clone())).collect()
    }

    #[test]
    fn edit_reports_only_changes() {
        let mut e = json!({ "name": "R", "baseUrl": "https://r", "apiKey": "sk-same-1234" });
        let mut d = Diff::default();
        assert!(!edit(&mut e, "r", "R", "https://r", Some("sk-same-1234"), &mut d, L), "key re-entered unchanged");
        assert!(!edit(&mut e, "r", "R", "https://r", None, &mut d, L));
        assert!(d.groups.is_empty());
        assert!(edit(&mut e, "r", "R2", "https://r", Some("sk-new-secret-5678"), &mut d, L));
        assert_eq!(lines(&d), vec!["「r」name = R2", "「r」密钥 = ••••5678"]);
        assert_eq!(str_field(&e, "apiKey"), "sk-new-secret-5678");
    }

    #[test]
    fn model_ops() {
        let mut p = json!({ "models": models_value(&[" a ", "b", "a", ""]) });
        assert_eq!(model_list(&p), vec![("a".into(), true), ("b".into(), true)]);
        let mut d = Diff::default();
        assert!(add_model(&mut p, "r", "  ", &mut d, L).is_err());
        assert!(!add_model(&mut p, "r", " a", &mut d, L).unwrap());
        assert!(add_model(&mut p, "r", "c ", &mut d, L).unwrap());
        assert!(set_visible(&mut p, "r", "b", false, &mut d, L));
        assert!(!set_visible(&mut p, "r", "b", false, &mut d, L));
        assert!(set_visible(&mut p, "r", "x", false, &mut d, L), "a missing model is added");
        assert!(delete_model(&mut p, "r", "a", &mut d, L));
        assert!(!delete_model(&mut p, "r", "a", &mut d, L));
        assert_eq!(model_list(&p), vec![("b".into(), false), ("c".into(), true), ("x".into(), false)]);
        set_models(&mut p, "r", &["m".into(), "m".into(), " n".into()], &mut d, L);
        assert_eq!(model_list(&p), vec![("m".into(), true), ("n".into(), true)]);
        assert_eq!(lines(&d), vec!["「r」+ c", "「r」b 隐藏", "「r」x 隐藏", "「r」- a", "「r」模型列表：2 个"]);
    }

    #[test]
    fn ids_and_delete() {
        let mut profs = Map::new();
        profs.insert("my-relay".into(), json!({}));
        assert_eq!(new_id(&profs, "My Relay", &[]), "my-relay-2");
        assert_eq!(new_id(&profs, "Official", &["official"]), "official-2");
        let mut d = Diff::default();
        assert!(delete(&mut profs, "nope", &mut d, L).is_err());
        delete(&mut profs, "my-relay", &mut d, L).unwrap();
        assert!(profs.is_empty() && get_mut(&mut profs, "my-relay").is_err());
        assert_eq!(lines(&d), vec!["- 「my-relay」"]);
    }
}
