//! API prices per million tokens (USD), to put a cost on usage. Subscription usage gets
//! the API-equivalent cost. Built-in list prices can be overridden per model in
//! store.json (`usage.prices`), and models without one can be added.

use super::Rec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

const fn p(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Price {
    Price { input, output, cache_read, cache_write }
}

/// List prices (standard tier, short context). Cache writes are the 5-minute rate.
const BUILTIN: &[(&str, Price)] = &[
    // Anthropic
    ("claude-fable-5-2", p(8.0, 40.0, 0.2, 10.0)),
    ("claude-mythos-5-2", p(8.0, 40.0, 0.2, 10.0)),
    ("claude-fable-5-1", p(10.0, 50.0, 1.0, 12.5)),
    ("claude-mythos-5-1", p(10.0, 50.0, 1.0, 12.5)),
    ("claude-fable-5", p(10.0, 50.0, 1.0, 12.5)),
    ("claude-opus-5-5", p(4.0, 20.0, 0.2, 5.0)),
    ("claude-opus-5", p(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-8", p(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-7", p(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-6", p(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-5", p(5.0, 25.0, 0.5, 6.25)),
    ("claude-opus-4-1", p(15.0, 75.0, 1.5, 18.75)),
    ("claude-opus-4", p(15.0, 75.0, 1.5, 18.75)),
    ("claude-sonnet-5-5", p(2.0, 10.0, 0.1, 2.5)),
    ("claude-sonnet-5", p(2.0, 10.0, 0.2, 2.5)),
    ("claude-sonnet-4-6", p(3.0, 15.0, 0.3, 3.75)),
    ("claude-sonnet-4-5", p(3.0, 15.0, 0.3, 3.75)),
    ("claude-sonnet-4", p(3.0, 15.0, 0.3, 3.75)),
    ("claude-3-7-sonnet", p(3.0, 15.0, 0.3, 3.75)),
    ("claude-haiku-5-5", p(0.1, 0.5, 0.01, 0.125)),
    ("claude-haiku-4-5", p(1.0, 5.0, 0.1, 1.25)),
    ("claude-3-5-haiku", p(0.8, 4.0, 0.08, 1.0)),
    // OpenAI
    ("gpt-5.2", p(1.75, 14.0, 0.175, 0.0)),
    ("gpt-5.1-codex-mini", p(0.25, 2.0, 0.025, 0.0)),
    ("gpt-5.1", p(1.25, 10.0, 0.125, 0.0)),
    ("gpt-5-mini", p(0.25, 2.0, 0.025, 0.0)),
    ("gpt-5-nano", p(0.05, 0.4, 0.005, 0.0)),
    ("gpt-5", p(1.25, 10.0, 0.125, 0.0)),
    ("gpt-4.1-mini", p(0.4, 1.6, 0.1, 0.0)),
    ("gpt-4.1", p(2.0, 8.0, 0.5, 0.0)),
    ("gpt-4o-mini", p(0.15, 0.6, 0.075, 0.0)),
    ("gpt-4o", p(2.5, 10.0, 1.25, 0.0)),
    ("o4-mini", p(1.1, 4.4, 0.275, 0.0)),
    ("o3", p(2.0, 8.0, 0.5, 0.0)),
    // Google
    ("gemini-3-pro", p(2.0, 12.0, 0.2, 0.0)),
    ("gemini-2.5-pro", p(1.25, 10.0, 0.31, 0.0)),
    ("gemini-2.5-flash-lite", p(0.1, 0.4, 0.025, 0.0)),
    ("gemini-2.5-flash", p(0.3, 2.5, 0.075, 0.0)),
];

/// Lower case, no vendor prefix (`anthropic/…`), no context tag (`[1m]`), no date
/// (`-20250929`, `@20250929`).
pub fn normalize(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    if let Some(i) = m.rfind('/') {
        m = m[i + 1..].to_string();
    }
    if let Some(i) = m.find('[') {
        m.truncate(i);
    }
    // `-2024-08-06`
    let b = m.as_bytes();
    if b.len() > 11 && b[b.len() - 11] == b'-' && m[b.len() - 10..].chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() }) && m[b.len() - 10..].starts_with("20") {
        m.truncate(b.len() - 11);
    }
    for sep in ['@', '-'] {
        if let Some(i) = m.rfind(sep) {
            let tail = &m[i + 1..];
            if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) && tail.starts_with("20") {
                m.truncate(i);
            }
        }
    }
    m.trim().to_string()
}

pub struct Table {
    /// Normalized model → price; models.dev and overrides already applied.
    prices: BTreeMap<String, Price>,
}

/// Where a price comes from.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Builtin,
    ModelsDev,
    Custom,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub model: String,
    #[serde(flatten)]
    pub price: Price,
    pub origin: Origin,
    /// A custom price that replaces a built-in or models.dev one (Reset brings that back).
    pub has_default: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Prices {
    pub entries: Vec<Entry>,
    /// When prices were last fetched from models.dev (Unix seconds).
    pub models_dev_at: Option<i64>,
}

// ---------------------------------------------------------------- models.dev

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Providers that are the model's own vendor: their price wins over resellers'.
const VENDORS: &[&str] = &[
    "anthropic", "openai", "google", "deepseek", "moonshotai", "moonshotai-cn", "zhipuai", "zai", "xai", "mistral",
    "alibaba", "alibaba-cn", "minimax", "minimax-cn", "xiaomi", "meta", "cohere", "amazon-bedrock",
];

#[derive(Serialize, Deserialize, Default)]
struct ModelsDevCache {
    at: i64,
    prices: BTreeMap<String, Price>,
}

fn models_dev_path() -> std::path::PathBuf {
    super::ledger::dir().join("models-dev.json")
}

fn models_dev_cache() -> Option<ModelsDevCache> {
    serde_json::from_slice(&std::fs::read(models_dev_path()).ok()?).ok()
}

/// Prices per normalized model from models.dev's catalog: the vendor's own listing when there
/// is one, else the first reseller (by provider id) that lists a non-zero price.
pub fn parse_models_dev(v: &Value) -> BTreeMap<String, Price> {
    let Some(providers) = v.as_object() else { return BTreeMap::new() };
    let rank = |id: &str| VENDORS.iter().position(|v| *v == id).unwrap_or(VENDORS.len());
    let mut ids: Vec<&String> = providers.keys().collect();
    ids.sort_by_key(|id| (rank(id), id.as_str()));
    let mut out: BTreeMap<String, Price> = BTreeMap::new();
    for id in ids {
        let vendor = rank(id) < VENDORS.len();
        let Some(models) = providers[id].get("models").and_then(Value::as_object) else { continue };
        for (mid, m) in models {
            let Some(c) = m.get("cost").filter(|c| c.is_object()) else { continue };
            let f = |k: &str| c.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(0.0);
            let price = p(f("input"), f("output"), f("cache_read"), f("cache_write"));
            if !vendor && price.input == 0.0 && price.output == 0.0 {
                continue;
            }
            let key = normalize(m.get("id").and_then(Value::as_str).unwrap_or(mid));
            if !key.is_empty() {
                out.entry(key).or_insert(price);
            }
        }
    }
    out
}

/// Downloads models.dev's catalog and keeps its prices; returns how many models have one.
pub fn fetch_models_dev() -> anyhow::Result<usize> {
    let client = crate::net::client_with(std::time::Duration::from_secs(30)).map_err(anyhow::Error::msg)?;
    let resp = client.get(MODELS_DEV_URL).send().map_err(|e| anyhow::anyhow!(tr!("Can't reach models.dev: {e}", "连不上 models.dev：{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("{}", tr!("models.dev answered {status}", "models.dev 返回 {status}"));
    }
    let v: Value = resp.text().map_err(anyhow::Error::from).and_then(|t| Ok(serde_json::from_str(&t)?)).map_err(|e| anyhow::anyhow!(tr!("models.dev did not return its catalog: {e}", "models.dev 返回的不是模型目录：{e}")))?;
    let prices = parse_models_dev(&v);
    if prices.is_empty() {
        anyhow::bail!("{}", crate::i18n::l("models.dev listed no prices; nothing was changed", "models.dev 没有列出价格，未做更改"));
    }
    let n = prices.len();
    let cache = ModelsDevCache { at: chrono::Local::now().timestamp(), prices };
    std::fs::create_dir_all(super::ledger::dir())?;
    crate::util::write_bytes_atomic(&models_dev_path(), &serde_json::to_vec(&cache)?)?;
    Ok(n)
}

impl Table {
    pub fn builtin() -> Table {
        Table { prices: BUILTIN.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[cfg(test)]
    pub fn with_overrides(root: &Value) -> Table {
        Table::layered(&BTreeMap::new(), root)
    }

    /// Built-in prices, then models.dev's, then the user's.
    fn layered(remote: &BTreeMap<String, Price>, root: &Value) -> Table {
        let mut t = Table::builtin();
        t.prices.extend(remote.iter().map(|(k, v)| (k.clone(), *v)));
        t.prices.extend(overrides(root));
        t
    }

    pub fn load() -> Table {
        Table::layered(&models_dev_cache().unwrap_or_default().prices, &crate::store::load())
    }

    /// The price of `model`: its own entry, else the longest entry it extends by a
    /// non-version suffix (`gpt-5-codex` → `gpt-5`, but not `claude-opus-5-7` → `claude-opus-5`).
    pub fn find(&self, model: &str) -> Option<Price> {
        let m = normalize(model);
        if let Some(p) = self.prices.get(&m) {
            return Some(*p);
        }
        self.prices
            .iter()
            .filter(|(k, _)| m.strip_prefix(k.as_str()).and_then(|rest| rest.strip_prefix('-')).is_some_and(|rest| rest.chars().next().is_some_and(|c| !c.is_ascii_digit())))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, p)| *p)
    }

    pub fn cost(&self, model: &str, r: &Rec) -> Option<f64> {
        let p = self.find(model)?;
        // A model without a separate cache-write price bills cache writes as input.
        let write = if p.cache_write > 0.0 { p.cache_write } else { p.input };
        Some((r.input as f64 * p.input + r.output as f64 * p.output + r.cache_read as f64 * p.cache_read + r.cache_write as f64 * write) / 1e6)
    }
}

fn overrides(root: &Value) -> Vec<(String, Price)> {
    root.pointer("/usage/prices")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((normalize(k), serde_json::from_value::<Price>(v.clone()).ok()?))).filter(|(k, _)| !k.is_empty()).collect())
        .unwrap_or_default()
}

/// Every known price, sorted by model.
pub fn list(root: &Value) -> Prices {
    let cache = models_dev_cache();
    Prices { entries: entries(&cache.as_ref().map(|c| c.prices.clone()).unwrap_or_default(), root), models_dev_at: cache.map(|c| c.at) }
}

fn entries(remote: &BTreeMap<String, Price>, root: &Value) -> Vec<Entry> {
    let entry = |k: &str, v: Price, origin| Entry { model: k.to_string(), price: v, origin, has_default: false };
    let mut out: BTreeMap<String, Entry> = BUILTIN.iter().map(|(k, v)| (k.to_string(), entry(k, *v, Origin::Builtin))).collect();
    for (k, v) in remote {
        out.insert(k.clone(), entry(k, *v, Origin::ModelsDev));
    }
    for (k, v) in overrides(root) {
        let has_default = out.contains_key(&k);
        out.insert(k.clone(), Entry { model: k, price: v, origin: Origin::Custom, has_default });
    }
    out.into_values().collect()
}

/// Sets (or with None, removes) the custom price of `model`.
pub fn set(root: &mut Value, model: &str, price: Option<Price>) -> anyhow::Result<()> {
    let key = normalize(model);
    if key.is_empty() {
        anyhow::bail!("{}", crate::i18n::l("Enter a model id", "请输入模型 ID"));
    }
    if let Some(p) = price {
        if [p.input, p.output, p.cache_read, p.cache_write].iter().any(|x| !x.is_finite() || *x < 0.0) {
            anyhow::bail!("{}", crate::i18n::l("Prices must be zero or more", "价格不能小于 0"));
        }
    }
    if !root.is_object() {
        *root = json!({});
    }
    if !root.get("usage").is_some_and(Value::is_object) {
        root["usage"] = json!({});
    }
    if !root["usage"].get("prices").is_some_and(Value::is_object) {
        root["usage"]["prices"] = json!({});
    }
    let m = root["usage"]["prices"].as_object_mut().unwrap();
    // Drop other spellings of the same model.
    m.retain(|k, _| normalize(k) != key);
    if let Some(p) = price {
        m.insert(key, serde_json::to_value(p)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_model_ids() {
        assert_eq!(normalize("Claude-Sonnet-4-5-20250929"), "claude-sonnet-4-5");
        assert_eq!(normalize("claude-sonnet-4-5@20250929"), "claude-sonnet-4-5");
        assert_eq!(normalize("anthropic/claude-opus-5-5[1m]"), "claude-opus-5-5");
        assert_eq!(normalize("gpt-5.1-codex"), "gpt-5.1-codex");
        assert_eq!(normalize("glm-4-32b-0414"), "glm-4-32b-0414");
        assert_eq!(normalize("gpt-4o-mini-2024-07-18"), "gpt-4o-mini");
        assert_eq!(normalize("gpt-4o-2024-08-06"), "gpt-4o");
    }

    #[test]
    fn finds_exact_then_suffixed_but_not_newer_versions() {
        let t = Table::builtin();
        assert_eq!(t.find("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(t.find("claude-opus-4-20250514").unwrap().input, 15.0);
        assert_eq!(t.find("gpt-5-codex").unwrap().input, 1.25);
        assert_eq!(t.find("gpt-5.1-codex-max").unwrap().input, 1.25);
        assert_eq!(t.find("gpt-5-mini").unwrap().input, 0.25, "own entry beats the gpt-5 prefix");
        assert!(t.find("claude-opus-5-7").is_none(), "a newer version is not priced like an older one");
        assert!(t.find("gpt-5.3").is_none());
        assert!(t.find("glm-5").is_none());
    }

    #[test]
    fn cost_uses_input_price_for_cache_writes_without_one() {
        let t = Table::builtin();
        let r = Rec { input: 1_000_000, output: 1_000_000, cache_read: 1_000_000, cache_write: 1_000_000, ..Default::default() };
        assert!((t.cost("gpt-5", &r).unwrap() - (1.25 + 10.0 + 0.125 + 1.25)).abs() < 1e-9);
        assert!((t.cost("claude-opus-5-5", &r).unwrap() - (4.0 + 20.0 + 0.2 + 5.0)).abs() < 1e-9);
    }

    #[test]
    fn overrides_and_custom_models() {
        let mut root = json!({});
        set(&mut root, "GLM-5", Some(p(0.5, 2.0, 0.1, 0.0))).unwrap();
        set(&mut root, "claude-opus-5-5", Some(p(1.0, 1.0, 0.0, 0.0))).unwrap();
        let t = Table::with_overrides(&root);
        assert_eq!(t.find("glm-5").unwrap().output, 2.0);
        assert_eq!(t.find("claude-opus-5-5").unwrap().input, 1.0);
        let l = entries(&BTreeMap::new(), &root);
        let opus = l.iter().find(|e| e.model == "claude-opus-5-5").unwrap();
        assert!(opus.origin == Origin::Custom && opus.has_default);
        let glm = l.iter().find(|e| e.model == "glm-5").unwrap();
        assert!(glm.origin == Origin::Custom && !glm.has_default);
        set(&mut root, "claude-opus-5-5", None).unwrap();
        assert_eq!(Table::with_overrides(&root).find("claude-opus-5-5").unwrap().input, 4.0, "reset to the list price");
        assert!(set(&mut root, "x", Some(p(-1.0, 0.0, 0.0, 0.0))).is_err());
        assert!(set(&mut root, "  ", None).is_err());
    }

    #[test]
    fn models_dev_prefers_the_vendor_and_layers_under_custom() {
        let v = json!({
            "openrouter": { "models": {
                "anthropic/claude-x-1": { "id": "anthropic/claude-x-1", "cost": { "input": 9, "output": 9 } },
                "glm-9": { "id": "glm-9", "cost": { "input": 0.7, "output": 2.5 } },
                "free-1": { "id": "free-1", "cost": { "input": 0, "output": 0 } },
                "nocost": { "id": "nocost" }
            } },
            "anthropic": { "models": { "claude-x-1": { "id": "claude-x-1", "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 } } } },
            "xiaomi": { "models": { "mimo-free": { "id": "mimo-free", "cost": { "input": 0, "output": 0 } } } },
            "broken": "x"
        });
        let m = parse_models_dev(&v);
        assert_eq!(m["claude-x-1"], p(3.0, 15.0, 0.3, 3.75), "the vendor beats the reseller");
        assert_eq!(m["glm-9"].input, 0.7);
        assert!(!m.contains_key("free-1"), "a reseller's zero price is not a price");
        assert_eq!(m["mimo-free"], p(0.0, 0.0, 0.0, 0.0), "the vendor's own free price counts");
        assert!(!m.contains_key("nocost"));
        assert!(parse_models_dev(&json!([])).is_empty());

        let mut root = json!({});
        set(&mut root, "glm-9", Some(p(1.0, 1.0, 0.0, 0.0))).unwrap();
        let t = Table::layered(&m, &root);
        assert_eq!(t.find("claude-x-1").unwrap().input, 3.0);
        assert_eq!(t.find("glm-9").unwrap().input, 1.0, "custom beats models.dev");
        let mut remote = BTreeMap::new();
        remote.insert("claude-opus-5-5".to_string(), p(1.0, 2.0, 0.0, 0.0));
        assert_eq!(Table::layered(&remote, &json!({})).find("claude-opus-5-5").unwrap().input, 1.0, "models.dev beats built-in");
        let e = entries(&m, &root);
        let glm = e.iter().find(|x| x.model == "glm-9").unwrap();
        assert!(glm.origin == Origin::Custom && glm.has_default);
        assert_eq!(e.iter().find(|x| x.model == "claude-x-1").unwrap().origin, Origin::ModelsDev);
    }

    /// Fetches the live catalog: `cargo test real_models_dev -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models_dev() {
        let n = fetch_models_dev().unwrap();
        let t = Table::load();
        println!("{n} models; glm-4.6 {:?}; claude-opus-4-8 {:?}", t.find("glm-4.6"), t.find("claude-opus-4-8"));
        assert!(n > 500);
    }
}
