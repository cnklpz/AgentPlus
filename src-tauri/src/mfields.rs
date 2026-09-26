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
    /// Several of (value, label en, label zh).
    Chips(&'static [(&'static str, &'static str, &'static str)]),
    /// One of (value, label en, label zh).
    Select(&'static [(&'static str, &'static str, &'static str)]),
}

pub struct Spec {
    pub path: &'static str,
    pub group: &'static str,
    /// (en, zh)
    pub label: (&'static str, &'static str),
    /// (en, zh)
    pub desc: (&'static str, &'static str),
    pub kind: Kind,
    /// (en, zh) short tags for the model table's capability column: a Bool input field has
    /// one (shown when on); Chips have one per option, or none to use the option labels.
    pub caps: &'static [(&'static str, &'static str)],
}

/// Group ids; `group_label` gives the display text.
const IO: &str = "io";
const GEN: &str = "gen";

fn group_label(id: &str) -> &'static str {
    match id {
        IO => l("Input", "输入能力"),
        _ => l("Generation", "生成"),
    }
}

/// OpenCode and its forks (Kilo, MiMo Desktop): `provider.<id>.models.<id>`.
pub const OPENCODE: &[Spec] = &[
    Spec { path: "/modalities/input", group: IO, label: ("Can read", "可以读取"), desc: ("modalities.input: the input types the model accepts. When unset, OpenCode uses its built-in model info; custom models are usually treated as text-only.", "modalities.input：模型接受的输入类型。不设置时按 OpenCode 自带的模型信息，自定义模型一般只当作文本。"), kind: Kind::Chips(&[("text", "Text", "文本"), ("image", "Images", "图片"), ("pdf", "PDF", "PDF"), ("video", "Video", "视频"), ("audio", "Audio", "音频")]), caps: &[] },
    Spec { path: "/attachment", group: IO, label: ("Allow attachments", "允许附件"), desc: ("attachment: files (images, PDFs, etc.) can be attached in the conversation.", "attachment：能在对话里附加文件（图片、PDF 等）。"), kind: Kind::Bool, caps: &[("Allow attachments", "允许附件")] },
    Spec { path: "/reasoning", group: GEN, label: ("Reasoning model", "推理模型"), desc: ("reasoning: the model outputs its thinking.", "reasoning：模型会输出思考过程。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/tool_call", group: GEN, label: ("Tool calls", "工具调用"), desc: ("tool_call: supports function/tool calls; when off, OpenCode gives it no tools.", "tool_call：支持函数/工具调用；关掉后 OpenCode 不会给它工具。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/temperature", group: GEN, label: ("Supports temperature", "支持 temperature"), desc: ("temperature: requests may include a temperature parameter.", "temperature：请求里可以带温度参数。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/limit/output", group: GEN, label: ("Max output", "最大输出"), desc: ("limit.output: the maximum tokens in one reply. OpenCode requires context and max output to be set together; if only one is filled in, the other gets its default (output 32000, context 128000).", "limit.output：单次回复最多多少 token。OpenCode 要求上下文和最大输出一起写，只填一个时另一个按默认补上（输出 32000、上下文 128000）。"), kind: Kind::Number, caps: &[] },
];

/// pi `models.json`: `providers.<id>.models[]`.
pub const PI: &[Spec] = &[
    Spec { path: "/input", group: IO, label: ("Can read", "可以读取"), desc: ("input: the input types the model accepts; when unset it is treated as text-only and images are not sent to it.", "input：模型接受的输入类型；不设置时只当作文本，图片不会发给它。"), kind: Kind::Chips(&[("text", "Text", "文本"), ("image", "Images", "图片")]), caps: &[] },
    Spec { path: "/reasoning", group: GEN, label: ("Reasoning model", "推理模型"), desc: ("reasoning: supports thinking (reasoning effort can be adjusted); when unset, treated as unsupported.", "reasoning：支持思考（可以调思考强度）；不设置时按不支持。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxTokens", group: GEN, label: ("Max output", "最大输出"), desc: ("maxTokens: the maximum tokens in one reply; 16384 when unset.", "maxTokens：单次回复最多多少 token；不设置时是 16384。"), kind: Kind::Number, caps: &[] },
];

/// OpenClaw `models.providers.<id>.models[]` (same shape as pi, more input kinds).
pub const OPENCLAW: &[Spec] = &[
    Spec { path: "/input", group: IO, label: ("Can read", "可以读取"), desc: ("input: the input types the model accepts.", "input：模型接受的输入类型。"), kind: Kind::Chips(&[("text", "Text", "文本"), ("image", "Images", "图片"), ("video", "Video", "视频"), ("audio", "Audio", "音频")]), caps: &[] },
    Spec { path: "/reasoning", group: GEN, label: ("Reasoning model", "推理模型"), desc: ("reasoning: supports thinking.", "reasoning：支持思考。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/compat/supportsTools", group: GEN, label: ("Tool calls", "工具调用"), desc: ("compat.supportsTools: supports function/tool calls.", "compat.supportsTools：支持函数/工具调用。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxTokens", group: GEN, label: ("Max output", "最大输出"), desc: ("maxTokens: the maximum tokens in one reply.", "maxTokens：单次回复最多多少 token。"), kind: Kind::Number, caps: &[] },
];

/// CodeBuddy Code `models.json`: `models[]`.
pub const CODEBUDDY: &[Spec] = &[
    Spec { path: "/supportsImages", group: IO, label: ("Read images", "读取图片"), desc: ("supportsImages: images can be sent to this model.", "supportsImages：可以把图片发给这个模型。"), kind: Kind::Bool, caps: &[("Images", "图片")] },
    Spec { path: "/supportsReasoning", group: GEN, label: ("Reasoning model", "推理模型"), desc: ("supportsReasoning: the model outputs its thinking.", "supportsReasoning：模型会输出思考过程。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/supportsToolCall", group: GEN, label: ("Tool calls", "工具调用"), desc: ("supportsToolCall: supports function/tool calls.", "supportsToolCall：支持函数/工具调用。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/maxOutputTokens", group: GEN, label: ("Max output", "最大输出"), desc: ("maxOutputTokens: the maximum tokens in one reply.", "maxOutputTokens：单次回复最多多少 token。"), kind: Kind::Number, caps: &[] },
];

/// Factory Droid `settings.json`: `customModels[]`. (`noImageSupport`'s tag says what the
/// model can't read, unlike the other input tags.)
pub const DROID: &[Spec] = &[
    Spec { path: "/noImageSupport", group: IO, label: ("No image support", "不支持图片"), desc: ("noImageSupport: when on, Droid does not send images to this model.", "noImageSupport：打开后 Droid 不会把图片发给这个模型。"), kind: Kind::Bool, caps: &[("No image support", "不支持图片")] },
    Spec { path: "/maxOutputTokens", group: GEN, label: ("Max output", "最大输出"), desc: ("maxOutputTokens: the maximum tokens in one reply.", "maxOutputTokens：单次回复最多多少 token。"), kind: Kind::Number, caps: &[] },
];

/// Codex model catalog (`model_catalog_json`): `models[]`.
pub const CODEX: &[Spec] = &[
    Spec { path: "/input_modalities", group: IO, label: ("Can read", "可以读取"), desc: ("input_modalities: without images, Codex won't send screenshots or image attachments to this model; when unset, text + images is assumed.", "input_modalities：不含图片时，Codex 不会把截图和图片附件发给这个模型；不设置时按文本 + 图片。"), kind: Kind::Chips(&[("text", "Text", "文本"), ("image", "Images", "图片"), ("audio", "Audio", "音频")]), caps: &[] },
    Spec {
        path: "/default_reasoning_level",
        group: GEN,
        label: ("Default reasoning effort", "默认思考强度"),
        desc: ("default_reasoning_level: the reasoning effort new sessions start with; must be one of the levels this model supports (supported_reasoning_levels).", "default_reasoning_level：新会话默认用的思考强度，要在这个模型支持的档位里（supported_reasoning_levels）。"),
        kind: Kind::Select(&[("none", "none", "none"), ("minimal", "minimal", "minimal"), ("low", "low", "low"), ("medium", "medium", "medium"), ("high", "high", "high"), ("xhigh", "xhigh", "xhigh"), ("max", "max", "max"), ("ultra", "ultra", "ultra")]),
        caps: &[],
    },
];

/// ZCode: `config.modelConfigRules.providerModelRules[].config` (paths are inside `config`).
pub const ZCODE: &[Spec] = &[
    Spec { path: "/properties/inputFormat/supportsImage", group: IO, label: ("Read images", "读取图片"), desc: ("inputFormat.supportsImage", "inputFormat.supportsImage"), kind: Kind::Bool, caps: &[("Images", "图片")] },
    Spec { path: "/properties/inputFormat/supportsPdf", group: IO, label: ("Read PDF", "读取 PDF"), desc: ("inputFormat.supportsPdf", "inputFormat.supportsPdf"), kind: Kind::Bool, caps: &[("PDF", "PDF")] },
    Spec { path: "/properties/inputFormat/supportsVideo", group: IO, label: ("Read video", "读取视频"), desc: ("inputFormat.supportsVideo", "inputFormat.supportsVideo"), kind: Kind::Bool, caps: &[("Video", "视频")] },
    Spec { path: "/optionSpecs/maxOutputTokens/max", group: GEN, label: ("Max output", "最大输出"), desc: ("optionSpecs.maxOutputTokens.max: the maximum tokens in one reply.", "optionSpecs.maxOutputTokens.max：单次回复最多多少 token。"), kind: Kind::Number, caps: &[] },
    Spec { path: "/properties/supportsJsonSchemaOutput", group: GEN, label: ("JSON Schema output", "JSON Schema 输出"), desc: ("supportsJsonSchemaOutput: supports output constrained by a JSON Schema.", "supportsJsonSchemaOutput：支持按 JSON Schema 约束输出。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/properties/supportsNativeWebSearch", group: GEN, label: ("Native web search", "原生联网搜索"), desc: ("supportsNativeWebSearch: the model has built-in web search.", "supportsNativeWebSearch：模型自带联网搜索。"), kind: Kind::Bool, caps: &[] },
    Spec { path: "/properties/supportsMidConversationSystem", group: GEN, label: ("Mid-conversation system messages", "对话中的系统消息"), desc: ("supportsMidConversationSystem: allows system messages mid-conversation.", "supportsMidConversationSystem：允许在对话中途插入系统消息。"), kind: Kind::Bool, caps: &[] },
];

/// Qwen Code `settings.json`: `modelProviders.<protocol>[]`.
pub const QWEN: &[Spec] = &[
    Spec { path: "/generationConfig/modalities/image", group: IO, label: ("Read images", "读取图片"), desc: ("generationConfig.modalities.image. When none of the four is set, Qwen Code guesses from the model name; once any is set, the ones not turned on are treated as unsupported.", "generationConfig.modalities.image。四项都不设置时 Qwen Code 按模型名猜；设置了任意一项后，没打开的都按不支持。"), kind: Kind::Bool, caps: &[("Images", "图片")] },
    Spec { path: "/generationConfig/modalities/pdf", group: IO, label: ("Read PDF", "读取 PDF"), desc: ("generationConfig.modalities.pdf", "generationConfig.modalities.pdf"), kind: Kind::Bool, caps: &[("PDF", "PDF")] },
    Spec { path: "/generationConfig/modalities/video", group: IO, label: ("Read video", "读取视频"), desc: ("generationConfig.modalities.video", "generationConfig.modalities.video"), kind: Kind::Bool, caps: &[("Video", "视频")] },
    Spec { path: "/generationConfig/modalities/audio", group: IO, label: ("Read audio", "读取音频"), desc: ("generationConfig.modalities.audio", "generationConfig.modalities.audio"), kind: Kind::Bool, caps: &[("Audio", "音频")] },
    Spec { path: "/generationConfig/samplingParams/max_tokens", group: GEN, label: ("Max output", "最大输出"), desc: ("generationConfig.samplingParams.max_tokens: the maximum tokens in one reply.", "generationConfig.samplingParams.max_tokens：单次回复最多多少 token。"), kind: Kind::Number, caps: &[] },
];

/// Kimi Code `config.toml` `[models.<key>]` (TOML; mapped in the adapter).
pub const KIMI: &[Spec] = &[Spec {
    path: "capabilities",
    group: IO,
    label: ("Capabilities", "能力"),
    desc: ("capabilities: image_in reads images, video_in reads video, thinking lets thinking be toggled, always_thinking always thinks.", "capabilities：image_in 读图片、video_in 读视频、thinking 可开关思考、always_thinking 始终思考。"),
    kind: Kind::Chips(&[("image_in", "Read images", "读取图片"), ("video_in", "Read video", "读取视频"), ("thinking", "Thinking", "思考"), ("always_thinking", "Always thinking", "始终思考")]),
    caps: &[("Images", "图片"), ("Video", "视频"), ("Thinking", "思考"), ("Always thinking", "始终思考")],
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
    let s = specs.iter().find(|s| s.path == key).ok_or_else(|| anyhow!(tr!("Unsupported model setting {key}", "不支持的模型设置 {key}")))?;
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
        return Err(anyhow!(tr!("Invalid value for \"{}\": {v}", "「{}」的值不对：{v}", l(s.label.0, s.label.1))));
    }
    if let (Kind::Select(o), Some(x)) = (s.kind, v.as_str()) {
        if !o.iter().any(|(k, ..)| *k == x) {
            return Err(anyhow!(tr!("\"{}\" can't be {x}", "「{}」不能是 {x}", l(s.label.0, s.label.1))));
        }
    }
    Ok((s, Some(v)))
}

/// What a field says about the model, for filling it in from `modelinfo::Info`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sense {
    /// The list of input kinds.
    Input,
    /// Reads this input kind (true / false).
    Reads(&'static str),
    NoImage,
    Attachment,
    Reasoning,
    Tools,
    MaxOutput,
    /// Kimi's `capabilities` (image_in, video_in, thinking…).
    KimiCaps,
    /// Nothing the catalogs know (temperature, default reasoning level…).
    Other,
}

fn sense(path: &str) -> Sense {
    match path {
        "/modalities/input" | "/input" | "/input_modalities" => Sense::Input,
        "/supportsImages" | "/properties/inputFormat/supportsImage" | "/generationConfig/modalities/image" => Sense::Reads("image"),
        "/properties/inputFormat/supportsPdf" | "/generationConfig/modalities/pdf" => Sense::Reads("pdf"),
        "/properties/inputFormat/supportsVideo" | "/generationConfig/modalities/video" => Sense::Reads("video"),
        "/generationConfig/modalities/audio" => Sense::Reads("audio"),
        "/noImageSupport" => Sense::NoImage,
        "/attachment" => Sense::Attachment,
        "/reasoning" | "/supportsReasoning" => Sense::Reasoning,
        "/tool_call" | "/compat/supportsTools" | "/supportsToolCall" => Sense::Tools,
        "/limit/output" | "/maxTokens" | "/maxOutputTokens" | "/optionSpecs/maxOutputTokens/max" | "/generationConfig/samplingParams/max_tokens" => Sense::MaxOutput,
        "capabilities" => Sense::KimiCaps,
        _ => Sense::Other,
    }
}

/// Field values that follow from what is known about a model. Input kinds are written both
/// ways when known; reasoning only when on and tool calls only when off (the agents'
/// defaults are the other way round), so a guess adds as little as it can.
pub fn from_info(specs: &[Spec], info: &crate::modelinfo::Info) -> Extra {
    let known = info.input.is_some();
    let mut out = Extra::new();
    for s in specs {
        let v = match (sense(s.path), s.kind) {
            (Sense::Input, Kind::Chips(opts)) if known => Some(json!(opts.iter().map(|o| o.0).filter(|o| *o == "text" || info.reads(o)).collect::<Vec<_>>())),
            (Sense::Reads(k), Kind::Bool) if known => Some(json!(info.reads(k))),
            (Sense::NoImage, Kind::Bool) if known && !info.reads("image") => Some(json!(true)),
            (Sense::Attachment, Kind::Bool) if info.reads("image") || info.reads("pdf") => Some(json!(true)),
            (Sense::Reasoning, Kind::Bool) if info.reasoning == Some(true) => Some(json!(true)),
            (Sense::Tools, Kind::Bool) if info.tools == Some(false) => Some(json!(false)),
            (Sense::MaxOutput, Kind::Number) => info.output.map(|n| json!(n)),
            (Sense::KimiCaps, Kind::Chips(_)) => {
                let caps: Vec<&str> = [("image_in", info.reads("image")), ("video_in", info.reads("video")), ("thinking", info.reasoning == Some(true))]
                    .into_iter()
                    .filter_map(|(c, on)| on.then_some(c))
                    .collect();
                (!caps.is_empty()).then(|| json!(caps))
            }
            _ => None,
        };
        if let Some(v) = v {
            out.insert(s.path.to_string(), v);
        }
    }
    out
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
                    lines.push(tr!("{} removed (back to default)", "{} 删除（恢复默认）", dotted(s.path)));
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
                        // The tag is the (Chinese) label without its leading "read" verb.
                        let short = s.label.1.trim_start_matches("读取").trim_start();
                        assert_eq!(s.caps[0].1, short, "{}", s.path);
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
    fn catalog_facts_reach_every_input_and_number_field() {
        for specs in [OPENCODE, PI, OPENCLAW, CODEBUDDY, DROID, CODEX, ZCODE, QWEN, KIMI] {
            for s in specs {
                let sn = sense(s.path);
                if s.group == IO {
                    assert_ne!(sn, Sense::Other, "{}", s.path);
                }
                if matches!(s.kind, Kind::Number) {
                    assert_eq!(sn, Sense::MaxOutput, "{}", s.path);
                }
            }
        }
        let text_only = crate::modelinfo::Info { id: "t".into(), input: Some(vec!["text".into()]), tools: Some(true), reasoning: Some(false), ..Default::default() };
        assert_eq!(from_info(DROID, &text_only), ex(&[("/noImageSupport", json!(true))]));
        assert_eq!(from_info(OPENCODE, &text_only), ex(&[("/modalities/input", json!(["text"]))]), "defaults (tools on, no reasoning) are left out");
        assert!(from_info(OPENCODE, &crate::modelinfo::Info::default()).is_empty(), "unknown input kinds stay unset");
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
