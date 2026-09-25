//! Per-model settings beyond name / context window (image input, reasoning, max output…),
//! declared per agent. A field's key is a JSON pointer into the model's entry, so JSON
//! configs read and write them with `read` / `write`; other formats map the keys themselves.

use crate::i18n::l;
use crate::model::ModelField;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub type Extra = BTreeMap<String, Value>;

#[derive(Clone, Copy)]
pub enum Kind {
    /// true / false; missing = the agent's default.
    Bool,
    /// Positive integer (tokens).
    Number,
    /// Several of (value, label zh, label en).
    Chips(&'static [(&'static str, &'static str, &'static str)]),
    /// One of (value, label zh, label en).
    Select(&'static [(&'static str, &'static str, &'static str)]),
}

pub struct Spec {
    pub path: &'static str,
    pub group: &'static str,
    /// (zh, en)
    pub label: (&'static str, &'static str),
    /// (zh, en)
    pub desc: (&'static str, &'static str),
    pub kind: Kind,
    /// (zh, en) short tags for the model table's capability column: a Bool input field has
    /// one (shown when on); Chips have one per option, or none to use the option labels.
    pub caps: &'static [(&'static str, &'static str)],
}

/// Group ids; `group_label` gives the display text.
const IO: &str = "io";
const GEN: &str = "gen";

fn group_label(id: &str) -> &'static str {
    match id {
        IO => l("输入能力", "Input"),
        _ => l("生成", "Generation"),
    }
}

/// OpenCode and its forks (Kilo, MiMo Desktop): `provider.<id>.models.<id>`.
pub const OPENCODE: &[Spec] = &[
    Spec { path: "/modalities/input", group: IO, label: ("可以读取", "Can read"), desc: ("modalities.input：模型接受的输入类型。不设置时按 OpenCode 自带的模型信息，自定义模型一般只当作文本。", "modalities.input: the input types the model accepts. When unset, OpenCode uses its built-in model info; custom models are usually treated as text-only."), kind: Kind::Chips(&[("text", "文本", "Text"), ("image", "图片", "Images"), ("pdf", "PDF", "PDF"), ("video", "视频", "Video"), ("audio", "音频", "Audio")]), caps: &[] },
    Spec { path: "/attachment", group: IO, label: ("允许附件", "Allow attachments"), desc: ("attachment：能在对话里附加文件（图片、PDF 等）。", "attachment: files (images, PDFs, etc.) can be attached in the conversation."), kind: Kind::Bool, caps: &[("允许附件", "Allow attachments")] },
    Spec { path: "/reasoning", group: GEN, label: ("推理模型", "Reasoning model"), desc: ("reasoning：模型会输出思考过程。", "reasoning: the model outputs its thinking."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/tool_call", group: GEN, label: ("工具调用", "Tool calls"), desc: ("tool_call：支持函数/工具调用；关掉后 OpenCode 不会给它工具。", "tool_call: supports function/tool calls; when off, OpenCode gives it no tools."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/temperature", group: GEN, label: ("支持 temperature", "Supports temperature"), desc: ("temperature：请求里可以带温度参数。", "temperature: requests may include a temperature parameter."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/limit/output", group: GEN, label: ("最大输出", "Max output"), desc: ("limit.output：单次回复最多多少 token。OpenCode 要求上下文和最大输出一起写，只填一个时另一个按默认补上（输出 32000、上下文 128000）。", "limit.output: the maximum tokens in one reply. OpenCode requires context and max output to be set together; if only one is filled in, the other gets its default (output 32000, context 128000)."), kind: Kind::Number, caps: &[] },
];

/// pi `models.json`: `providers.<id>.models[]`.
pub const PI: &[Spec] = &[
    Spec { path: "/input", group: IO, label: ("可以读取", "Can read"), desc: ("input：模型接受的输入类型；不设置时只当作文本，图片不会发给它。", "input: the input types the model accepts; when unset it is treated as text-only and images are not sent to it."), kind: Kind::Chips(&[("text", "文本", "Text"), ("image", "图片", "Images")]), caps: &[] },
    Spec { path: "/reasoning", group: GEN, label: ("推理模型", "Reasoning model"), desc: ("reasoning：支持思考（可以调思考强度）；不设置时按不支持。", "reasoning: supports thinking (reasoning effort can be adjusted); when unset, treated as unsupported."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxTokens", group: GEN, label: ("最大输出", "Max output"), desc: ("maxTokens：单次回复最多多少 token；不设置时是 16384。", "maxTokens: the maximum tokens in one reply; 16384 when unset."), kind: Kind::Number, caps: &[] },
];

/// OpenClaw `models.providers.<id>.models[]` (same shape as pi, more input kinds).
pub const OPENCLAW: &[Spec] = &[
    Spec { path: "/input", group: IO, label: ("可以读取", "Can read"), desc: ("input：模型接受的输入类型。", "input: the input types the model accepts."), kind: Kind::Chips(&[("text", "文本", "Text"), ("image", "图片", "Images"), ("video", "视频", "Video"), ("audio", "音频", "Audio")]), caps: &[] },
    Spec { path: "/reasoning", group: GEN, label: ("推理模型", "Reasoning model"), desc: ("reasoning：支持思考。", "reasoning: supports thinking."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/compat/supportsTools", group: GEN, label: ("工具调用", "Tool calls"), desc: ("compat.supportsTools：支持函数/工具调用。", "compat.supportsTools: supports function/tool calls."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxTokens", group: GEN, label: ("最大输出", "Max output"), desc: ("maxTokens：单次回复最多多少 token。", "maxTokens: the maximum tokens in one reply."), kind: Kind::Number, caps: &[] },
];

/// CodeBuddy Code `models.json`: `models[]`.
pub const CODEBUDDY: &[Spec] = &[
    Spec { path: "/supportsImages", group: IO, label: ("读取图片", "Read images"), desc: ("supportsImages：可以把图片发给这个模型。", "supportsImages: images can be sent to this model."), kind: Kind::Bool, caps: &[("图片", "Images")] },
    Spec { path: "/supportsReasoning", group: GEN, label: ("推理模型", "Reasoning model"), desc: ("supportsReasoning：模型会输出思考过程。", "supportsReasoning: the model outputs its thinking."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/supportsToolCall", group: GEN, label: ("工具调用", "Tool calls"), desc: ("supportsToolCall：支持函数/工具调用。", "supportsToolCall: supports function/tool calls."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxOutputTokens", group: GEN, label: ("最大输出", "Max output"), desc: ("maxOutputTokens：单次回复最多多少 token。", "maxOutputTokens: the maximum tokens in one reply."), kind: Kind::Number, caps: &[] },
];

/// Factory Droid `settings.json`: `customModels[]`. (`noImageSupport`'s tag says what the
/// model can't read, unlike the other input tags.)
pub const DROID: &[Spec] = &[
    Spec { path: "/noImageSupport", group: IO, label: ("不支持图片", "No image support"), desc: ("noImageSupport：打开后 Droid 不会把图片发给这个模型。", "noImageSupport: when on, Droid does not send images to this model."), kind: Kind::Bool, caps: &[("不支持图片", "No image support")] },
    Spec { path: "/maxOutputTokens", group: GEN, label: ("最大输出", "Max output"), desc: ("maxOutputTokens：单次回复最多多少 token。", "maxOutputTokens: the maximum tokens in one reply."), kind: Kind::Number, caps: &[] },
];

/// Codex model catalog (`model_catalog_json`): `models[]`.
pub const CODEX: &[Spec] = &[
    Spec { path: "/input_modalities", group: IO, label: ("可以读取", "Can read"), desc: ("input_modalities：不含图片时，Codex 不会把截图和图片附件发给这个模型；不设置时按文本 + 图片。", "input_modalities: without images, Codex won't send screenshots or image attachments to this model; when unset, text + images is assumed."), kind: Kind::Chips(&[("text", "文本", "Text"), ("image", "图片", "Images"), ("audio", "音频", "Audio")]), caps: &[] },
    Spec {
        path: "/default_reasoning_level",
        group: GEN,
        label: ("默认思考强度", "Default reasoning effort"),
        desc: ("default_reasoning_level：新会话默认用的思考强度，要在这个模型支持的档位里（supported_reasoning_levels）。", "default_reasoning_level: the reasoning effort new sessions start with; must be one of the levels this model supports (supported_reasoning_levels)."),
        kind: Kind::Select(&[("none", "none", "none"), ("minimal", "minimal", "minimal"), ("low", "low", "low"), ("medium", "medium", "medium"), ("high", "high", "high"), ("xhigh", "xhigh", "xhigh"), ("max", "max", "max"), ("ultra", "ultra", "ultra")]),
        caps: &[],
    },
];

/// ZCode: `config.modelConfigRules.providerModelRules[].config` (paths are inside `config`).
pub const ZCODE: &[Spec] = &[
    Spec { path: "/properties/inputFormat/supportsImage", group: IO, label: ("读取图片", "Read images"), desc: ("inputFormat.supportsImage", "inputFormat.supportsImage"), kind: Kind::Bool, caps: &[("图片", "Images")] },
    Spec { path: "/properties/inputFormat/supportsPdf", group: IO, label: ("读取 PDF", "Read PDF"), desc: ("inputFormat.supportsPdf", "inputFormat.supportsPdf"), kind: Kind::Bool, caps: &[("PDF", "PDF")] },
    Spec { path: "/properties/inputFormat/supportsVideo", group: IO, label: ("读取视频", "Read video"), desc: ("inputFormat.supportsVideo", "inputFormat.supportsVideo"), kind: Kind::Bool, caps: &[("视频", "Video")] },
    Spec { path: "/optionSpecs/maxOutputTokens/max", group: GEN, label: ("最大输出", "Max output"), desc: ("optionSpecs.maxOutputTokens.max：单次回复最多多少 token。", "optionSpecs.maxOutputTokens.max: the maximum tokens in one reply."), kind: Kind::Number, caps: &[] },
    Spec { path: "/properties/supportsJsonSchemaOutput", group: GEN, label: ("JSON Schema 输出", "JSON Schema output"), desc: ("supportsJsonSchemaOutput：支持按 JSON Schema 约束输出。", "supportsJsonSchemaOutput: supports output constrained by a JSON Schema."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/properties/supportsNativeWebSearch", group: GEN, label: ("原生联网搜索", "Native web search"), desc: ("supportsNativeWebSearch：模型自带联网搜索。", "supportsNativeWebSearch: the model has built-in web search."), kind: Kind::Bool, caps: &[] },
    Spec { path: "/properties/supportsMidConversationSystem", group: GEN, label: ("对话中的系统消息", "Mid-conversation system messages"), desc: ("supportsMidConversationSystem：允许在对话中途插入系统消息。", "supportsMidConversationSystem: allows system messages mid-conversation."), kind: Kind::Bool, caps: &[] },
];

/// Qwen Code `settings.json`: `modelProviders.<protocol>[]`.
pub const QWEN: &[Spec] = &[
    Spec { path: "/generationConfig/modalities/image", group: IO, label: ("读取图片", "Read images"), desc: ("generationConfig.modalities.image。四项都不设置时 Qwen Code 按模型名猜；设置了任意一项后，没打开的都按不支持。", "generationConfig.modalities.image. When none of the four is set, Qwen Code guesses from the model name; once any is set, the ones not turned on are treated as unsupported."), kind: Kind::Bool, caps: &[("图片", "Images")] },
    Spec { path: "/generationConfig/modalities/pdf", group: IO, label: ("读取 PDF", "Read PDF"), desc: ("generationConfig.modalities.pdf", "generationConfig.modalities.pdf"), kind: Kind::Bool, caps: &[("PDF", "PDF")] },
    Spec { path: "/generationConfig/modalities/video", group: IO, label: ("读取视频", "Read video"), desc: ("generationConfig.modalities.video", "generationConfig.modalities.video"), kind: Kind::Bool, caps: &[("视频", "Video")] },
    Spec { path: "/generationConfig/modalities/audio", group: IO, label: ("读取音频", "Read audio"), desc: ("generationConfig.modalities.audio", "generationConfig.modalities.audio"), kind: Kind::Bool, caps: &[("音频", "Audio")] },
    Spec { path: "/generationConfig/samplingParams/max_tokens", group: GEN, label: ("最大输出", "Max output"), desc: ("generationConfig.samplingParams.max_tokens：单次回复最多多少 token。", "generationConfig.samplingParams.max_tokens: the maximum tokens in one reply."), kind: Kind::Number, caps: &[] },
];

/// Kimi Code `config.toml` `[models.<key>]` (TOML; mapped in the adapter).
pub const KIMI: &[Spec] = &[Spec {
    path: "capabilities",
    group: IO,
    label: ("能力", "Capabilities"),
    desc: ("capabilities：image_in 读图片、video_in 读视频、thinking 可开关思考、always_thinking 始终思考。", "capabilities: image_in reads images, video_in reads video, thinking lets thinking be toggled, always_thinking always thinks."),
    kind: Kind::Chips(&[("image_in", "读取图片", "Read images"), ("video_in", "读取视频", "Read video"), ("thinking", "思考", "Thinking"), ("always_thinking", "始终思考", "Always thinking")]),
    caps: &[("图片", "Images"), ("视频", "Video"), ("思考", "Thinking"), ("始终思考", "Always thinking")],
}];

/// Field declarations shown for an agent's models (OpenCode's for its project configs).
pub fn for_agent(agent: &str) -> &'static [Spec] {
    use crate::adapters::{base_agent, codebuddy, codex, droid, kilo, kimi, mimo, openclaw, opencode, pi, qwen, zcode};
    match base_agent(agent) {
        opencode::ID | kilo::ID | mimo::ID => OPENCODE,
        pi::ID => PI,
        openclaw::ID => OPENCLAW,
        codebuddy::ID => CODEBUDDY,
        droid::ID => DROID,
        codex::ID => CODEX,
        zcode::ID => ZCODE,
        kimi::ID => KIMI,
        qwen::ID => QWEN,
        _ => &[],
    }
}

pub fn fields(specs: &[Spec]) -> Vec<ModelField> {
    specs
        .iter()
        .map(|s| {
            let (kind, opts): (&str, &[(&str, &str, &str)]) = match s.kind {
                Kind::Bool => ("bool", &[]),
                Kind::Number => ("number", &[]),
                Kind::Chips(o) => ("chips", o),
                Kind::Select(o) => ("select", o),
            };
            ModelField {
                key: s.path.into(),
                gid: s.group.into(),
                group: group_label(s.group).into(),
                label: l(s.label.0, s.label.1).into(),
                desc: l(s.desc.0, s.desc.1).into(),
                kind: kind.into(),
                options: opts.iter().map(|o| o.0.to_string()).collect(),
                hints: opts.iter().map(|o| l(o.1, o.2).to_string()).collect(),
                caps: s.caps.iter().map(|c| l(c.0, c.1).to_string()).collect(),
            }
        })
        .collect()
}

/// A value this field can hold (unknown chip values are kept, so reading never drops them).
pub fn valid(kind: Kind, v: &Value) -> bool {
    match kind {
        Kind::Bool => v.is_boolean(),
        Kind::Number => v.as_u64().map(|n| n > 0).unwrap_or(false),
        Kind::Select(_) => v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false),
        Kind::Chips(_) => v.as_array().map(|a| a.iter().all(|x| x.is_string())).unwrap_or(false),
    }
}

/// The fields that are set in a model entry.
pub fn read(def: &Value, specs: &[Spec]) -> Extra {
    specs
        .iter()
        .filter_map(|s| def.pointer(s.path).filter(|v| valid(s.kind, v)).map(|v| (s.path.to_string(), v.clone())))
        .collect()
}

/// Checks a requested change and returns the value to write (None = clear).
pub fn check<'a>(specs: &'a [Spec], key: &str, v: &Value) -> Result<(&'a Spec, Option<Value>)> {
    let s = specs.iter().find(|s| s.path == key).ok_or_else(|| anyhow!(tr!("不支持的模型设置 {key}", "Unsupported model setting {key}")))?;
    if v.is_null() {
        return Ok((s, None));
    }
    let v = match (s.kind, v) {
        (Kind::Chips(_), Value::Array(a)) => {
            let mut out: Vec<Value> = vec![];
            for x in a {
                if !out.contains(x) {
                    out.push(x.clone());
                }
            }
            Value::Array(out)
        }
        _ => v.clone(),
    };
    if !valid(s.kind, &v) {
        return Err(anyhow!(tr!("「{}」的值不对：{v}", "Invalid value for \"{}\": {v}", l(s.label.0, s.label.1))));
    }
    if let (Kind::Select(o), Some(x)) = (s.kind, v.as_str()) {
        if !o.iter().any(|(k, ..)| *k == x) {
            return Err(anyhow!(tr!("「{}」不能是 {x}", "\"{}\" can't be {x}", l(s.label.0, s.label.1))));
        }
    }
    Ok((s, Some(v)))
}

/// "/limit/output" -> "limit.output"
pub fn dotted(path: &str) -> String {
    path.trim_start_matches('/').replace('/', ".")
}

/// Applies `extra` to a model entry. Returns one "path = value" line per change.
pub fn write(def: &mut Value, specs: &[Spec], extra: &Extra) -> Result<Vec<String>> {
    let checked = extra.iter().map(|(k, v)| check(specs, k, v)).collect::<Result<Vec<_>>>()?;
    let mut lines = vec![];
    for (s, v) in checked {
        match v {
            None => {
                if remove(def, s.path) {
                    lines.push(tr!("{} 删除（恢复默认）", "{} removed (back to default)", dotted(s.path)));
                }
            }
            Some(v) => {
                if def.pointer(s.path) != Some(&v) {
                    set(def, s.path, v.clone());
                    lines.push(format!("{} = {v}", dotted(s.path)));
                }
            }
        }
    }
    Ok(lines)
}

/// Sets a pointer path, creating (or replacing non-object) parents.
pub fn set(def: &mut Value, path: &str, v: Value) {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut cur = def;
    for seg in &segs[..segs.len() - 1] {
        if !cur.is_object() {
            *cur = json!({});
        }
        let o = cur.as_object_mut().unwrap();
        let next = o.entry(seg.to_string()).or_insert_with(|| json!({}));
        if !next.is_object() {
            *next = json!({});
        }
        cur = next;
    }
    if !cur.is_object() {
        *cur = json!({});
    }
    cur.as_object_mut().unwrap().insert(segs[segs.len() - 1].to_string(), v);
}

/// Removes a pointer path and the parents it leaves empty. Returns whether it was there.
pub fn remove(def: &mut Value, path: &str) -> bool {
    fn go(cur: &mut Value, segs: &[&str]) -> bool {
        let Some(o) = cur.as_object_mut() else { return false };
        if segs.len() == 1 {
            return o.remove(segs[0]).is_some();
        }
        let Some(child) = o.get_mut(segs[0]) else { return false };
        let removed = go(child, &segs[1..]);
        if removed && child.as_object().map(|c| c.is_empty()).unwrap_or(false) {
            o.remove(segs[0]);
        }
        removed
    }
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    go(def, &segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(pairs: &[(&str, Value)]) -> Extra {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn write_read_clear() {
        let mut d = json!({ "name": "X", "limit": { "context": 1000 } });
        let lines = write(&mut d, OPENCODE, &ex(&[("/modalities/input", json!(["text", "image", "image"])), ("/limit/output", json!(8192)), ("/reasoning", json!(true))])).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(d["modalities"]["input"], json!(["text", "image"]));
        assert_eq!(d["limit"], json!({ "context": 1000, "output": 8192 }));
        let r = read(&d, OPENCODE);
        assert_eq!(r.len(), 3);
        // unchanged values write nothing
        assert!(write(&mut d, OPENCODE, &r).unwrap().is_empty());
        write(&mut d, OPENCODE, &ex(&[("/modalities/input", Value::Null), ("/limit/output", Value::Null)])).unwrap();
        assert!(d.get("modalities").is_none());
        assert_eq!(d["limit"], json!({ "context": 1000 }));
    }

    #[test]
    fn input_fields_have_capability_tags() {
        for specs in [OPENCODE, PI, OPENCLAW, CODEBUDDY, DROID, CODEX, ZCODE, QWEN, KIMI] {
            for s in specs {
                match s.kind {
                    Kind::Bool if s.group == IO => {
                        assert_eq!(s.caps.len(), 1, "{}", s.path);
                        // The tag is the label without its "读取" (read) verb.
                        let short = s.label.0.trim_start_matches("读取").trim_start();
                        assert_eq!(s.caps[0].0, short, "{}", s.path);
                    }
                    Kind::Chips(o) => assert!(s.caps.is_empty() || s.caps.len() == o.len(), "{}", s.path),
                    _ => assert!(s.caps.is_empty(), "{}", s.path),
                }
            }
        }
        let kimi = fields(KIMI);
        assert_eq!(kimi[0].caps, ["图片", "视频", "思考", "始终思考"]);
        assert_eq!(kimi[0].hints[0], "读取图片", "the model dialog keeps the full option labels");
    }

    #[test]
    fn rejects_bad_values() {
        let mut d = json!({});
        assert!(write(&mut d, OPENCODE, &ex(&[("/limit/output", json!("lots"))])).is_err());
        assert!(write(&mut d, OPENCODE, &ex(&[("/nope", json!(true))])).is_err());
        assert!(write(&mut d, CODEX, &ex(&[("/default_reasoning_level", json!("extreme"))])).is_err());
        assert_eq!(d, json!({}));
    }
}
