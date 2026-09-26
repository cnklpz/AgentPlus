//! What AgentPlus knows about a model from its id alone (context window, max output, input
//! kinds, reasoning, tool calls), used to fill in a new model's settings the way ZCode does
//! with its own rule table.
//!
//! Two catalogs:
//! 1. built in (`model_catalog.json`): the vendors' own entries from models.dev, regenerated
//!    with `cargo test --lib modelinfo::tests::regen_builtin -- --ignored --nocapture`;
//! 2. models.dev's full catalog (every provider), cached in `~/.agentplus/models-dev.json`
//!    and refreshed in the background once it is a week old.
//!
//! An id is looked up exactly first (lowercase, without a vendor prefix like `anthropic/` or a
//! date suffix), then by family: the longest catalog id found in it at word boundaries, so
//! `z-ai/glm-4.6-fp8` is `glm-4.6` but `glm-4.6v` is not. Order: built-in exact, models.dev
//! exact, built-in family, models.dev family.

use crate::mfields::{self, Extra};
use crate::model::{ModelInput, Op};
use crate::util::{agentplus_dir, write_bytes_atomic};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

const MODELS_DEV: &str = "https://models.dev/api.json";
const CACHE_FILE: &str = "models-dev.json";
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// models.dev providers that are the model vendors themselves: their entries win when several
/// providers list the same model, and they make up the built-in catalog.
const FIRST_PARTY: &[&str] = &[
    "openai", "anthropic", "google", "deepseek", "zhipuai", "zai", "moonshotai", "alibaba", "xai", "minimax", "xiaomi", "mistral", "stepfun", "volcengine", "tencent-tokenhub",
    "longcat", "bailing", "meta",
];

const INPUT_KINDS: &[&str] = &["text", "image", "pdf", "video", "audio"];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// Catalog id (see `normalize`).
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    /// Input kinds (`INPUT_KINDS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
}

impl Info {
    pub fn reads(&self, kind: &str) -> bool {
        self.input.as_ref().is_some_and(|i| i.iter().any(|x| x == kind))
    }
}

/// A model's filled-in settings for one agent.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Guess {
    pub context: Option<u64>,
    /// The agent's model fields, by key.
    pub extra: Extra,
    /// The catalog id that matched.
    pub matched: String,
    /// "builtin" | "modelsDev"
    pub source: &'static str,
}

struct Catalog {
    rows: Vec<Info>,
    exact: HashMap<String, usize>,
}

impl Catalog {
    fn new(rows: Vec<Info>) -> Catalog {
        let mut exact = HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            exact.entry(r.id.clone()).or_insert(i);
        }
        Catalog { rows, exact }
    }

    fn exact(&self, n: &str) -> Option<&Info> {
        self.exact.get(n).map(|&i| &self.rows[i])
    }

    fn family(&self, n: &str) -> Option<&Info> {
        self.rows.iter().filter(|r| r.id.len() >= 3 && in_family(n, &r.id)).max_by_key(|r| r.id.len())
    }
}

/// Lowercase, without a vendor path (`anthropic/…`), a `:free` / `:0` tag, a `[1m]` marker
/// or a trailing date (`-20250929`, `-2025-09-29`, `-250929`).
pub fn normalize(id: &str) -> String {
    let mut s = id.trim().to_lowercase().replace("[1m]", "");
    if let Some(i) = s.rfind('/') {
        s.drain(..=i);
    }
    if let Some(i) = s.find(':') {
        s.truncate(i);
    }
    static DATE: OnceLock<regex::Regex> = OnceLock::new();
    let re = DATE.get_or_init(|| regex::Regex::new(r"-(?:20\d{2}-\d{2}-\d{2}|20\d{6}|2\d{5})$").unwrap());
    re.replace(&s, "").trim().to_string()
}

/// `k` appears in `n` as a whole name: after the start or a separator, and followed by the
/// end or a variant suffix (`-fp8`, `:free`, `[1m]`; a `.` only when no digit follows, so
/// `gpt-5` does not claim `gpt-5.4`).
fn in_family(n: &str, k: &str) -> bool {
    let b = n.as_bytes();
    let mut from = 0;
    while let Some(p) = n[from..].find(k).map(|p| p + from) {
        let before = p == 0 || matches!(b[p - 1], b'/' | b':' | b'.' | b'-' | b'_' | b' ' | b']');
        let end = p + k.len();
        let after = match b.get(end) {
            None => true,
            Some(b'-' | b'_' | b':' | b'@' | b'[' | b' ' | b'/') => true,
            Some(b'.') => !b.get(end + 1).is_some_and(u8::is_ascii_digit),
            _ => false,
        };
        if before && after {
            return true;
        }
        from = p + 1;
    }
    false
}

/// Catalog rows from models.dev's `api.json` (vendors first); `vendors_only` keeps just
/// `FIRST_PARTY`. Text-generating models with a context window only.
fn from_models_dev(v: &Value, vendors_only: bool) -> Vec<Info> {
    let Some(all) = v.as_object() else { return vec![] };
    let mut order: Vec<&str> = FIRST_PARTY.iter().copied().filter(|p| all.contains_key(*p)).collect();
    if !vendors_only {
        let mut rest: Vec<&str> = all.keys().map(String::as_str).filter(|p| !FIRST_PARTY.contains(p)).collect();
        rest.sort_unstable();
        order.extend(rest);
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for p in order {
        let Some(models) = all[p].get("models").and_then(|m| m.as_object()) else { continue };
        for (key, m) in models {
            let raw = m.get("id").and_then(|x| x.as_str()).unwrap_or(key);
            let id = normalize(raw);
            let strs = |ptr: &str| -> Vec<String> {
                m.pointer(ptr).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
            };
            let context = m.pointer("/limit/context").and_then(|x| x.as_u64()).filter(|c| *c > 0);
            if id.is_empty() || id.contains("embed") || context.is_none() || !strs("/modalities/output").iter().any(|x| x == "text") || !seen.insert(id.clone()) {
                continue;
            }
            let input: Vec<String> = INPUT_KINDS.iter().filter(|k| strs("/modalities/input").iter().any(|x| x == *k)).map(|k| k.to_string()).collect();
            out.push(Info {
                id,
                context,
                output: m.pointer("/limit/output").and_then(|x| x.as_u64()).filter(|c| *c > 0),
                input: (!input.is_empty()).then_some(input),
                reasoning: m.get("reasoning").and_then(|x| x.as_bool()),
                tools: m.get("tool_call").and_then(|x| x.as_bool()),
            });
        }
    }
    out
}

fn builtin() -> &'static Catalog {
    static B: OnceLock<Catalog> = OnceLock::new();
    B.get_or_init(|| Catalog::new(serde_json::from_str(include_str!("model_catalog.json")).expect("model_catalog.json")))
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    models: Vec<Info>,
}

fn cache_path() -> PathBuf {
    agentplus_dir().join(CACHE_FILE)
}

/// The models.dev cache, re-read when the file changes.
fn cached() -> Option<Arc<Catalog>> {
    static C: Mutex<Option<(PathBuf, SystemTime, Arc<Catalog>)>> = Mutex::new(None);
    let path = cache_path();
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let mut c = C.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, t, cat)) = c.as_ref() {
        if *p == path && *t == mtime {
            return Some(cat.clone());
        }
    }
    let file: CacheFile = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let cat = Arc::new(Catalog::new(file.models));
    *c = Some((path, mtime, cat.clone()));
    Some(cat)
}

/// Downloads models.dev's catalog into the cache. Returns the number of models.
pub fn refresh() -> Result<usize> {
    let text = crate::net::get_text(MODELS_DEV, Duration::from_secs(60), 64 << 20).map_err(anyhow::Error::msg)?;
    let rows = from_models_dev(&serde_json::from_str(&text)?, false);
    anyhow::ensure!(!rows.is_empty(), "models.dev: no models");
    let n = rows.len();
    std::fs::create_dir_all(agentplus_dir())?;
    write_bytes_atomic(&cache_path(), &serde_json::to_vec(&CacheFile { models: rows })?)?;
    Ok(n)
}

/// `refresh` when the cache is missing or a week old (startup, off the main thread).
pub fn refresh_if_stale() {
    let fresh = std::fs::metadata(cache_path()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < MAX_AGE);
    if !fresh {
        if let Err(e) = refresh() {
            eprintln!("models.dev refresh failed: {e:#}");
        }
    }
}

/// What the catalogs say about a model id, and which one said it.
pub fn lookup(id: &str) -> Option<(Info, &'static str)> {
    let n = normalize(id);
    if n.is_empty() {
        return None;
    }
    let dev = cached();
    let (info, source) = builtin()
        .exact(&n)
        .map(|i| (i, "builtin"))
        .or_else(|| dev.as_ref().and_then(|d| d.exact(&n)).map(|i| (i, "modelsDev")))
        .or_else(|| builtin().family(&n).map(|i| (i, "builtin")))
        .or_else(|| dev.as_ref().and_then(|d| d.family(&n)).map(|i| (i, "modelsDev")))?;
    let mut info = info.clone();
    // `claude-sonnet-4-5[1m]`: the 1M-context variant (the marker is dropped by `normalize`).
    if id.to_lowercase().contains("[1m]") {
        info.context = Some(1_000_000);
    }
    Some((info, source))
}

/// A model's settings for `agent`, filled in from the catalogs. None for ZCode (it matches
/// models against its own rules) and for models nothing is known about.
pub fn guess(agent: &str, id: &str) -> Option<Guess> {
    if crate::adapters::base_agent(agent) == crate::adapters::zcode::ID {
        return None;
    }
    let (info, source) = lookup(id)?;
    let extra = mfields::from_info(mfields::for_agent(agent), &info);
    if info.context.is_none() && extra.is_empty() {
        return None;
    }
    Some(Guess { context: info.context, extra, matched: info.id, source })
}

pub fn guesses(agent: &str, ids: &[String]) -> BTreeMap<String, Guess> {
    ids.iter().filter_map(|id| guess(agent, id.trim()).map(|g| (id.clone(), g))).collect()
}

/// UpsertModel ops that fill in a new provider's models (only the ones something is known about).
pub fn seed_ops(agent: &str, provider: &str, ids: &[String]) -> Vec<Op> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && seen.insert(id.to_string()))
        .filter_map(|id| {
            let g = guess(agent, id)?;
            let model = ModelInput { id: id.to_string(), name: None, context: g.context, extra: g.extra };
            Some(Op::UpsertModel { provider: provider.to_string(), model })
        })
        .collect()
}

/// Tests: a models.dev cache (in the test home) with just these models.
#[cfg(test)]
pub fn test_cache(rows: Vec<Info>) {
    std::fs::create_dir_all(agentplus_dir()).unwrap();
    std::fs::write(cache_path(), serde_json::to_vec(&CacheFile { models: rows }).unwrap()).unwrap();
}

/// Tests: `acme-vision-9`, a made-up model no real catalog lists (64K context, 8K output,
/// reads images, reasons, no tool calls), in the test home's models.dev cache.
#[cfg(test)]
pub fn test_cache_acme() {
    test_cache(vec![Info { id: "acme-vision-9".into(), context: Some(64000), output: Some(8192), input: Some(vec!["text".into(), "image".into()]), reasoning: Some(true), tools: Some(false) }]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use serde_json::json;

    #[test]
    fn normalizes_vendor_prefixes_tags_and_dates() {
        assert_eq!(normalize("anthropic/Claude-Sonnet-4-5-20250929"), "claude-sonnet-4-5");
        assert_eq!(normalize("z-ai/glm-4.6:free"), "glm-4.6");
        assert_eq!(normalize("claude-opus-4-5[1m]"), "claude-opus-4-5");
        assert_eq!(normalize("gpt-4o-2024-08-06"), "gpt-4o");
        assert_eq!(normalize("doubao-seed-1-6-251015"), "doubao-seed-1-6");
        assert_eq!(normalize("qwen2-5-72b-instruct"), "qwen2-5-72b-instruct");
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn family_needs_whole_names() {
        assert!(in_family("glm-4.6", "glm-4.6"));
        assert!(in_family("glm-4.6-fp8", "glm-4.6"));
        assert!(in_family("us.anthropic.claude-sonnet-4-5-v1", "claude-sonnet-4-5"));
        assert!(in_family("cc-kimi-k2.6", "kimi-k2.6"));
        assert!(!in_family("glm-4.6v", "glm-4.6"));
        assert!(!in_family("gpt-5.4", "gpt-5"));
        assert!(!in_family("chatgpt-5", "gpt-5"));
        assert!(!in_family("glm-4.61", "glm-4.6"));
    }

    fn cat() -> Catalog {
        let row = |id: &str, ctx: u64| Info { id: id.into(), context: Some(ctx), ..Default::default() };
        Catalog::new(vec![row("gpt-5", 400_000), row("gpt-5-mini", 200_000), row("glm-4.6", 200_000), row("glm-4.6v", 128_000)])
    }

    #[test]
    fn family_prefers_the_most_specific_id() {
        let c = cat();
        assert_eq!(c.family("gpt-5-mini-high").unwrap().id, "gpt-5-mini");
        assert_eq!(c.family("gpt-5-high").unwrap().id, "gpt-5");
        assert_eq!(c.family("glm-4.6v-flash").unwrap().id, "glm-4.6v");
        assert!(c.family("gpt-5.9").is_none());
        assert!(c.exact("glm-4.6").is_some() && c.exact("glm-4.6-air").is_none());
    }

    #[test]
    fn reads_models_dev_rows() {
        let v = json!({
            "someproxy": { "models": { "glm-4.6": { "id": "glm-4.6", "limit": { "context": 1 }, "modalities": { "input": ["text"], "output": ["text"] } } } },
            "zhipuai": { "models": {
                "glm-4.6": { "id": "glm-4.6", "reasoning": true, "tool_call": true, "limit": { "context": 200000, "output": 131072 }, "modalities": { "input": ["text"], "output": ["text"] } },
                "embedding-3": { "id": "embedding-3", "limit": { "context": 8192 }, "modalities": { "input": ["text"], "output": ["text"] } },
                "cogview": { "id": "cogview", "limit": { "context": 1000 }, "modalities": { "input": ["text"], "output": ["image"] } }
            } },
            "other": { "models": { "x/vision-1": { "id": "x/vision-1", "limit": { "context": 32000 }, "modalities": { "input": ["image", "text", "file"], "output": ["text"] } } } }
        });
        let rows = from_models_dev(&v, false);
        assert_eq!(rows.len(), 2, "vendor first; no embeddings or image models: {rows:?}");
        assert_eq!(rows[0], Info { id: "glm-4.6".into(), context: Some(200000), output: Some(131072), input: Some(vec!["text".into()]), reasoning: Some(true), tools: Some(true) });
        assert_eq!(rows[1].input, Some(vec!["text".into(), "image".into()]));
        assert_eq!(from_models_dev(&v, true).len(), 1);
    }

    #[test]
    fn guesses_map_to_each_agents_fields() {
        let _h = TestHome::new("modelinfo-guess");
        test_cache_acme();

        let g = guess("opencode", "relay/acme-vision-9-latest").unwrap();
        assert_eq!((g.context, g.matched.as_str(), g.source), (Some(64000), "acme-vision-9", "modelsDev"));
        assert_eq!(g.extra["/modalities/input"], json!(["text", "image"]));
        assert_eq!(g.extra["/attachment"], json!(true));
        assert_eq!(g.extra["/reasoning"], json!(true));
        assert_eq!(g.extra["/tool_call"], json!(false));
        assert_eq!(g.extra["/limit/output"], json!(8192));
        let d = guess("droid", "acme-vision-9").unwrap();
        assert!(!d.extra.contains_key("/noImageSupport"), "it reads images: {:?}", d.extra);
        let k = guess("kimi", "acme-vision-9").unwrap();
        assert_eq!(k.extra["capabilities"], json!(["image_in", "thinking"]));
        let q = guess("qwen", "acme-vision-9").unwrap();
        assert_eq!((&q.extra["/generationConfig/modalities/image"], &q.extra["/generationConfig/modalities/pdf"]), (&json!(true), &json!(false)));
        assert!(guess("zcode", "acme-vision-9").is_none(), "ZCode matches models itself");
        assert!(guess("opencode", "no-such-model-anywhere").is_none());
        assert_eq!(lookup("acme-vision-9[1m]").unwrap().0.context, Some(1_000_000));

        let ops = seed_ops("pi", "relay", &["acme-vision-9".into(), " acme-vision-9 ".into(), "unknown-x".into()]);
        assert_eq!(ops.len(), 1);
        let Op::UpsertModel { provider, model } = &ops[0] else { panic!() };
        assert_eq!((provider.as_str(), model.context), ("relay", Some(64000)));
        assert_eq!(model.extra["/input"], json!(["text", "image"]));
        assert_eq!(model.extra["/maxTokens"], json!(8192));
    }

    #[test]
    fn builtin_catalog_parses() {
        assert!(builtin().rows.iter().all(|r| !r.id.is_empty() && r.id == normalize(&r.id)));
    }

    /// Rewrites `model_catalog.json` from models.dev (network).
    #[test]
    #[ignore]
    fn regen_builtin() {
        let text = crate::net::get_text(MODELS_DEV, Duration::from_secs(60), 64 << 20).unwrap();
        let mut rows = from_models_dev(&serde_json::from_str(&text).unwrap(), true);
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let lines: Vec<String> = rows.iter().map(|r| serde_json::to_string(r).unwrap()).collect();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("model_catalog.json");
        std::fs::write(&path, format!("[\n{}\n]\n", lines.join(",\n"))).unwrap();
        println!("{} models -> {}", rows.len(), path.display());
    }
}
