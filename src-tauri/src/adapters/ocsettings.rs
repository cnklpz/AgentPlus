//! OpenCode's `opencode.json` settings as editable rows, shared by the global config and
//! project configs. Keys are dotted paths into the config (`compaction.auto`,
//! `permission.bash`).
//!
//! In a project every row can also be "unset": OpenCode deep-merges the project file over
//! the global one (objects key by key, arrays replaced, `instructions` concatenated), so a
//! missing key means "use the global value". Project rows show that value in their labels.

use crate::i18n::l;
use crate::model::{Diff, Setting};
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use crate::util::{obj_at, str_list};

#[derive(Clone, Copy, PartialEq)]
pub enum Scope {
    Global,
    Project,
}

enum Kind {
    /// On/off with OpenCode's default when the key is missing.
    Bool(bool),
    /// Fixed values: (stored value, zh label, en label). "true"/"false" are written as booleans.
    Select(&'static [(&'static str, &'static str, &'static str)], &'static str),
    /// Free text with suggestions (model ids, agent names).
    Text,
    /// One entry per line.
    List,
    /// Provider ids, picked from the known ones.
    Providers,
}

/// UI text as (zh, en), picked with `l()` when rows are built.
type Text2 = (&'static str, &'static str);

struct Spec {
    key: &'static str,
    group: Text2,
    label: Text2,
    desc: Text2,
    kind: Kind,
    global_only: bool,
}

const ACTIONS: &[(&str, &str, &str)] = &[("allow", "允许", "Allow"), ("ask", "每次询问", "Ask each time"), ("deny", "禁止", "Deny")];

/// Every tool key OpenCode's `permission` accepts; used when a single string has to be
/// spread into per-tool entries.
const PERMISSION_KEYS: &[&str] = &[
    "read", "edit", "glob", "grep", "list", "bash", "task", "external_directory", "lsp", "skill",
    "todowrite", "question", "webfetch", "websearch", "doom_loop",
];

const G_MODEL: Text2 = ("模型", "Models");
const G_PROVIDER: Text2 = ("供应商", "Providers");
const G_BEHAVIOR: Text2 = ("行为", "Behavior");
const G_PERMISSION: Text2 = ("权限", "Permissions");
const G_FILES: Text2 = ("指令与文件", "Instructions & files");

const SPECS: &[Spec] = &[
    Spec { key: "model", group: G_MODEL, label: ("默认模型", "Default model"), desc: ("provider/model 格式，OpenCode 启动时默认选中它", "provider/model format; OpenCode selects it by default on startup"), kind: Kind::Text, global_only: false },
    Spec { key: "small_model", group: G_MODEL, label: ("小模型", "Small model"), desc: ("生成会话标题等轻量任务用的模型，provider/model 格式", "Model for lightweight tasks such as session titles, in provider/model format"), kind: Kind::Text, global_only: false },
    Spec { key: "default_agent", group: G_MODEL, label: ("默认 Agent", "Default agent"), desc: ("启动时使用的主 Agent，OpenCode 默认是 build", "The primary agent used on startup; OpenCode defaults to build"), kind: Kind::Text, global_only: false },
    Spec { key: "enabled_providers", group: G_PROVIDER, label: ("只加载这些供应商", "Only load these providers"), desc: ("都不选＝不限制；选了之后只有这些供应商会出现在 OpenCode 里（停用列表优先）", "None selected = no restriction; otherwise only these providers appear in OpenCode (the disabled list takes precedence)"), kind: Kind::Providers, global_only: false },
    Spec { key: "share", group: G_BEHAVIOR, label: ("会话分享", "Session sharing"), desc: ("share：会话能否分享成公开链接", "share: whether sessions can be shared as public links"), kind: Kind::Select(&[("manual", "手动分享", "Manual"), ("auto", "自动分享", "Automatic"), ("disabled", "禁止分享", "Disabled")], "manual"), global_only: false },
    Spec { key: "autoupdate", group: G_BEHAVIOR, label: ("自动更新", "Auto update"), desc: ("autoupdate：启动时检查新版本", "autoupdate: check for new versions on startup"), kind: Kind::Select(&[("true", "自动下载更新", "Download updates automatically"), ("notify", "只提醒", "Notify only"), ("false", "不检查", "Don't check")], "true"), global_only: true },
    Spec { key: "snapshot", group: G_BEHAVIOR, label: ("改动快照", "Change snapshots"), desc: ("snapshot：记录文件改动，支持 /undo 撤销；很大的仓库可以关掉提速", "snapshot: track file changes so /undo can revert them; turn off to speed up very large repos"), kind: Kind::Bool(true), global_only: false },
    Spec { key: "compaction.auto", group: G_BEHAVIOR, label: ("自动压缩上下文", "Auto-compact context"), desc: ("compaction.auto：上下文快满时自动压缩会话", "compaction.auto: compact the session automatically when the context is nearly full"), kind: Kind::Bool(true), global_only: false },
    Spec { key: "compaction.prune", group: G_BEHAVIOR, label: ("清理旧工具输出", "Prune old tool output"), desc: ("compaction.prune：压缩时删掉较早的工具输出，省 token", "compaction.prune: drop older tool output when compacting to save tokens"), kind: Kind::Bool(false), global_only: false },
    Spec { key: "username", group: G_BEHAVIOR, label: ("显示的用户名", "Display name"), desc: ("username：会话里显示的名字，留空用系统用户名", "username: the name shown in sessions; leave empty to use the system user name"), kind: Kind::Text, global_only: false },
    Spec { key: "permission.edit", group: G_PERMISSION, label: ("修改文件", "Edit files"), desc: ("permission.edit：edit / write / patch 等改文件的工具", "permission.edit: edit / write / patch and other file-editing tools"), kind: Kind::Select(ACTIONS, "allow"), global_only: false },
    Spec { key: "permission.bash", group: G_PERMISSION, label: ("执行命令", "Run commands"), desc: ("permission.bash：运行 shell 命令", "permission.bash: run shell commands"), kind: Kind::Select(ACTIONS, "allow"), global_only: false },
    Spec { key: "permission.webfetch", group: G_PERMISSION, label: ("抓取网页", "Fetch web pages"), desc: ("permission.webfetch：读取 URL 内容", "permission.webfetch: read URL contents"), kind: Kind::Select(ACTIONS, "allow"), global_only: false },
    Spec { key: "permission.external_directory", group: G_PERMISSION, label: ("访问项目外目录", "Access outside the project"), desc: ("permission.external_directory：读写工作目录以外的文件", "permission.external_directory: read and write files outside the working directory"), kind: Kind::Select(ACTIONS, "allow"), global_only: false },
    Spec { key: "instructions", group: G_FILES, label: ("额外指令文件", "Extra instruction files"), desc: ("instructions：每行一个路径或 glob（如 CONTRIBUTING.md、docs/*.md），内容会加进系统提示", "instructions: one path or glob per line (e.g. CONTRIBUTING.md, docs/*.md); their content is added to the system prompt"), kind: Kind::List, global_only: false },
    Spec { key: "watcher.ignore", group: G_FILES, label: ("文件监视忽略", "File watcher ignore"), desc: ("watcher.ignore：每行一个 glob（如 node_modules/**、dist/**）", "watcher.ignore: one glob per line (e.g. node_modules/**, dist/**)"), kind: Kind::List, global_only: false },
];

fn get<'a>(cfg: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = cfg;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// Current value of a key, honoring `permission` given as one string for every tool.
fn read<'a>(cfg: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(tool) = key.strip_prefix("permission.") {
        return match cfg.get("permission") {
            Some(v @ Value::String(_)) => Some(v),
            Some(p) => p.get(tool),
            None => None,
        };
    }
    get(cfg, key)
}


/// A stored value in the Select's terms: "true"/"false" for booleans, "custom" for rule maps.
fn select_str(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Bool(b) => Some(b.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => Some("custom".into()),
        _ => None,
    }
}

fn label_of(opts: &[(&'static str, &'static str, &'static str)], v: &str) -> String {
    opts.iter().find(|o| o.0 == v).map(|o| l(o.1, o.2).to_string()).unwrap_or_else(|| if v == "custom" { l("按规则细分", "Per rule").into() } else { v.to_string() })
}

fn on_off(b: bool) -> &'static str {
    if b { l("开", "On") } else { l("关", "Off") }
}

/// Rows for one config. `global` is the global config when `cfg` is a project's.
/// `models` / `providers` feed the suggestions.
pub fn rows(cfg: &Value, global: Option<&Value>, scope: Scope, models: &[String], providers: &[String]) -> Vec<Setting> {
    let mut out = vec![];
    for s in SPECS {
        if s.global_only && scope == Scope::Project {
            continue;
        }
        let cur = read(cfg, s.key);
        let inherited = global.and_then(|g| read(g, s.key));
        let mut row = Setting {
            key: s.key.into(),
            group: l(s.group.0, s.group.1).into(),
            label: l(s.label.0, s.label.1).into(),
            desc: l(s.desc.0, s.desc.1).into(),
            kind: String::new(),
            value: Value::Null,
            options: vec![],
            hints: vec![],
        };
        match &s.kind {
            Kind::Bool(def) if scope == Scope::Global => {
                row.kind = "bool".into();
                row.value = json!(cur.and_then(|v| v.as_bool()).unwrap_or(*def));
            }
            Kind::Bool(def) => {
                let g = inherited.and_then(|v| v.as_bool()).unwrap_or(*def);
                row.kind = "select".into();
                row.value = json!(cur.and_then(|v| v.as_bool()).map(|b| b.to_string()).unwrap_or_default());
                row.options = vec!["".into(), "true".into(), "false".into()];
                row.hints = vec![tr!("继承全局（{}）", "Inherit global ({})", on_off(g)), on_off(true).into(), on_off(false).into()];
            }
            Kind::Select(opts, def) => {
                row.kind = "select".into();
                let v = select_str(cur);
                if scope == Scope::Project {
                    let g = select_str(inherited).unwrap_or_else(|| def.to_string());
                    row.options.push(String::new());
                    row.hints.push(tr!("继承全局（{}）", "Inherit global ({})", label_of(opts, &g)));
                    row.value = json!(v.clone().unwrap_or_default());
                } else {
                    row.value = json!(v.clone().unwrap_or_else(|| def.to_string()));
                }
                for (val, zh, en) in opts.iter() {
                    row.options.push(val.to_string());
                    row.hints.push(l(zh, en).to_string());
                }
                if v.as_deref() == Some("custom") {
                    row.options.push("custom".into());
                    row.hints.push(l("按规则细分（保持文件里的写法）", "Per rule (kept as written in the file)").into());
                }
            }
            Kind::Text => {
                row.kind = "text".into();
                row.value = json!(cur.and_then(|v| v.as_str()).unwrap_or(""));
                row.options = match s.key {
                    "model" | "small_model" => models.to_vec(),
                    "default_agent" => {
                        let mut a = vec!["build".to_string(), "plan".to_string()];
                        for c in [Some(cfg), global].into_iter().flatten() {
                            if let Some(o) = c.get("agent").and_then(|x| x.as_object()) {
                                a.extend(o.keys().filter(|k| !a.contains(k)).cloned().collect::<Vec<_>>());
                            }
                        }
                        a
                    }
                    _ => vec![],
                };
                if scope == Scope::Project {
                    let g = inherited.and_then(|v| v.as_str()).filter(|x| !x.is_empty());
                    row.desc = tr!("{}。留空＝继承全局{}", "{}. Leave empty to inherit global{}", row.desc, g.map(|x| tr!("（{x}）", " ({x})")).unwrap_or_default());
                }
            }
            Kind::List => {
                row.kind = "list".into();
                row.value = json!(str_list(cur).unwrap_or_default());
                if scope == Scope::Project {
                    let g = str_list(inherited).unwrap_or_default();
                    let tail = if s.key == "instructions" { l("和全局的合并", "Merged with global") } else { l("留空＝继承全局", "Leave empty to inherit global") };
                    let shown = if g.is_empty() { String::new() } else { tr!("（全局：{}）", " (global: {})", g.join(l("、", ", "))) };
                    row.desc = tr!("{}。{tail}{shown}", "{}. {tail}{shown}", row.desc);
                }
            }
            Kind::Providers => {
                row.kind = "chips".into();
                let mut v = str_list(cur).unwrap_or_default();
                let mut opts: Vec<String> = providers.to_vec();
                // Ids listed in the file but not known here still show (and can be unticked).
                for x in &v {
                    if !opts.contains(x) {
                        opts.push(x.clone());
                    }
                }
                v.retain(|x| opts.contains(x));
                row.value = json!(v);
                row.options = opts;
                if scope == Scope::Project {
                    let g = str_list(inherited).unwrap_or_default();
                    if !g.is_empty() {
                        row.desc = tr!("{}。都不选＝沿用全局的 {}", "{}. None selected = use the global {}", row.desc, g.join(l("、", ", ")));
                    }
                }
            }
        }
        out.push(row);
    }
    out
}

/// What to store for a value coming from the UI; None removes the key.
fn to_stored(kind: &Kind, key: &str, v: &Value) -> Result<Option<Value>> {
    Ok(match kind {
        Kind::Bool(_) | Kind::Select(..) => match v {
            Value::Bool(b) => Some(json!(b)),
            Value::String(s) if s.is_empty() => None,
            Value::String(s) if s == "true" || s == "false" => Some(json!(s == "true")),
            Value::String(s) => Some(json!(s)),
            Value::Null => None,
            _ => return Err(anyhow!(tr!("{key} 的值无效", "Invalid value for {key}"))),
        },
        Kind::Text => match v.as_str().map(str::trim) {
            Some("") | None => None,
            Some(s) => Some(json!(s)),
        },
        Kind::List | Kind::Providers => {
            let list: Vec<String> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            if list.is_empty() { None } else { Some(json!(list)) }
        }
    })
}

/// Sets or removes a dotted key; parents left empty by a removal go too. A parent that
/// holds something other than an object is an error, not overwritten.
fn write(cfg: &mut Value, key: &str, v: Option<Value>) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() == 1 {
        match v {
            Some(v) => { obj_at(cfg, &[])?.insert(key.into(), v); }
            None => { obj_at(cfg, &[])?.remove(key); }
        }
        return Ok(());
    }
    let (head, leaf) = (parts[0], parts[1]);
    match v {
        Some(v) => {
            obj_at(cfg, &[head])?.insert(leaf.into(), v);
        }
        None => {
            let root = obj_at(cfg, &[])?;
            if let Some(p) = root.get_mut(head).and_then(|p| p.as_object_mut()) {
                p.remove(leaf);
                if p.is_empty() {
                    root.remove(head);
                }
            }
        }
    }
    Ok(())
}

/// Applies one setting to `cfg`. Returns whether the file changed.
pub fn apply(cfg: &mut Value, key: &str, value: &Value, diff: &mut Diff, file: &str) -> Result<bool> {
    let spec = SPECS.iter().find(|s| s.key == key).ok_or_else(|| anyhow!(tr!("未知设置 {key}", "Unknown setting: {key}")))?;
    if value.as_str() == Some("custom") {
        return Ok(false); // "keep the rule map as it is"
    }
    let want = to_stored(&spec.kind, key, value)?;
    // `permission` as one action for every tool: spread it so the other tools keep their behavior.
    let spread = key.starts_with("permission.").then(|| cfg.get("permission").and_then(|p| p.as_str()).map(String::from)).flatten();
    match &spread {
        Some(all) if want.as_ref().and_then(|w| w.as_str()) == Some(all.as_str()) => return Ok(false),
        Some(all) => {
            let map: Map<String, Value> = PERMISSION_KEYS.iter().map(|k| (k.to_string(), json!(all))).collect();
            cfg["permission"] = Value::Object(map);
            diff.push(file, tr!("permission = \"{all}\" → 按工具展开", "permission = \"{all}\" → expanded per tool"), true);
        }
        None if read(cfg, key) == want.as_ref() => return Ok(false),
        None => {}
    }
    let text = match &want {
        Some(v) => format!("{key} = {}", serde_json::to_string(v).unwrap_or_default()),
        None => format!("- {key}"),
    };
    let add = want.is_some();
    write(cfg, key, want)?;
    diff.push(file, text, add);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_unset_nested() {
        let mut cfg = json!({ "model": "a/b" });
        let mut d = Diff::default();
        assert!(apply(&mut cfg, "compaction.auto", &json!(false), &mut d, "f").unwrap());
        assert_eq!(cfg["compaction"]["auto"], json!(false));
        assert!(apply(&mut cfg, "compaction.auto", &json!(""), &mut d, "f").unwrap());
        assert!(cfg.get("compaction").is_none());
        assert!(!apply(&mut cfg, "model", &json!("a/b"), &mut d, "f").unwrap());
        assert!(apply(&mut cfg, "model", &json!(" "), &mut d, "f").unwrap());
        assert!(cfg.get("model").is_none());
    }

    #[test]
    fn permission_string_is_spread() {
        let mut cfg = json!({ "permission": "ask" });
        let mut d = Diff::default();
        assert!(apply(&mut cfg, "permission.bash", &json!("deny"), &mut d, "f").unwrap());
        assert_eq!(cfg["permission"]["bash"], json!("deny"));
        assert_eq!(cfg["permission"]["edit"], json!("ask"));
        assert_eq!(cfg["permission"]["read"], json!("ask"));
        // Rule maps are kept when "custom" comes back.
        let mut cfg = json!({ "permission": { "bash": { "git *": "allow" } } });
        assert!(!apply(&mut cfg, "permission.bash", &json!("custom"), &mut d, "f").unwrap());
    }

    #[test]
    fn project_rows_inherit() {
        let global = json!({ "share": "disabled", "snapshot": false, "autoupdate": false });
        let rows = rows(&json!({}), Some(&global), Scope::Project, &[], &["a".into()]);
        assert!(rows.iter().all(|r| r.key != "autoupdate"));
        let share = rows.iter().find(|r| r.key == "share").unwrap();
        assert_eq!(share.value, json!(""));
        assert_eq!(share.hints[0], "继承全局（禁止分享）");
        let snap = rows.iter().find(|r| r.key == "snapshot").unwrap();
        assert_eq!(snap.kind, "select");
        assert_eq!(snap.hints[0], "继承全局（关）");
    }
}
