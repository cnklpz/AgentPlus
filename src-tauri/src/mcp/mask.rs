//! Secrets inside MCP definitions (env values, headers, flags, URL query strings) are masked
//! before anything reaches the UI; variable references (`${TOKEN}`, `{env:TOKEN}`) are not
//! secrets and stay readable.

use crate::model::mask_key;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Names that usually hold a secret: `API_KEY`, `Authorization`, `--token`, `clientSecret`.
pub fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // `bearer_token_env_var`, `env_vars`: these hold variable names, not values.
    if n.contains("env_var") || n.contains("envvar") {
        return false;
    }
    ["key", "token", "secret", "passw", "pwd", "auth", "credential", "cookie", "session", "bearer", "private"].iter().any(|w| n.contains(w))
}

fn re(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).unwrap())
}

/// One variable reference in any agent's syntax: `${X}`, `${X:-default}`, `$X`, `{env:X}`, `%X%`.
fn ref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-[^}]*)?\}|\$([A-Za-z_][A-Za-z0-9_]*)|\{env:([A-Za-z_][A-Za-z0-9_]*)\}|%([A-Za-z_][A-Za-z0-9_]*)%")
}

/// An auth scheme in front of a header value ("Bearer ", "Basic ", "token ").
fn scheme(v: &str) -> (&str, &str) {
    static R: OnceLock<Regex> = OnceLock::new();
    match re(&R, r"^(?i)(bearer|basic|token|apikey)\s+").find(v) {
        Some(m) => v.split_at(m.end()),
        None => ("", v),
    }
}

/// The whole value is a variable reference (after an optional auth scheme).
pub fn is_ref(v: &str) -> bool {
    let (_, rest) = scheme(v.trim());
    ref_re().find(rest).is_some_and(|m| m.start() == 0 && m.end() == rest.len())
}

/// Writes every variable reference as `${X}`, so the same server reads the same in any agent.
pub fn normalize_refs(v: &str) -> String {
    ref_re()
        .replace_all(v, |c: &regex::Captures| {
            let name = (1..=4).find_map(|i| c.get(i)).map(|m| m.as_str()).unwrap_or_default();
            // Keep a default: it changes what runs.
            if c.get(1).is_some() && c[0].contains(":-") {
                c[0].to_string()
            } else {
                format!("${{{name}}}")
            }
        })
        .into_owned()
}

/// A value that looks like a credential whatever it is called (vendor prefixes, JWTs, long
/// opaque tokens).
pub fn looks_secret(v: &str) -> bool {
    let (_, v) = scheme(v.trim());
    const PREFIXES: [&str; 14] = ["sk-", "sk_", "ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "AKIA", "AIza", "eyJ", "pat_"];
    if PREFIXES.iter().any(|p| v.starts_with(p)) && v.len() >= 16 {
        return true;
    }
    let opaque = v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let mixed = v.chars().any(|c| c.is_ascii_digit()) && v.chars().any(|c| c.is_ascii_alphabetic());
    opaque && mixed && v.len() >= 32
}

/// Masks a secret, keeping its auth scheme: "Bearer ••••abcd".
fn mask(v: &str) -> String {
    let (s, rest) = scheme(v.trim());
    format!("{s}{}", mask_key(rest))
}

/// (shown value, masked) of the value of an env var / header / field called `name`.
pub fn value(name: &str, v: &str) -> (String, bool) {
    if is_ref(v) || v.trim().is_empty() {
        return (v.to_string(), false);
    }
    if secret_name(name) || looks_secret(v) {
        return (mask(v), true);
    }
    let u = url(v);
    let masked = u != v;
    (u, masked)
}

/// Masks passwords and secret query parameters in a URL (`?api_key=…`, `user:pass@`).
pub fn url(u: &str) -> String {
    static Q: OnceLock<Regex> = OnceLock::new();
    static USER: OnceLock<Regex> = OnceLock::new();
    let u = re(&USER, r"^([a-zA-Z][a-zA-Z0-9+.-]*://[^/:@]+):([^@/]+)@").replace(u, |c: &regex::Captures| format!("{}:{}@", &c[1], mask_key(&c[2])));
    re(&Q, r"([?&])([^=&#]+)=([^&#]*)")
        .replace_all(&u, |c: &regex::Captures| {
            let (k, v) = (&c[2], &c[3]);
            if secret_name(k) && !is_ref(v) && !v.is_empty() {
                format!("{}{k}={}", &c[1], mask_key(v))
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

/// `NAME=value` (docker's `-e NAME=value`, `--env NAME=value`) or `Name: value` (a `--header`
/// of curl or mcp-remote) where the name holds a secret: the same argument with the value masked.
fn assignment(a: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let c = re(&R, r"^([A-Za-z_][A-Za-z0-9_.-]*)(=|:\s*)(.+)$").captures(a)?;
    let (name, sep, v) = (&c[1], &c[2], &c[3]);
    (secret_name(name) && !is_ref(v)).then(|| format!("{name}{sep}{}", mask(v)))
}

/// Command-line arguments: the value after a secret flag (`--api-key X`, `--token=X`), and
/// anything that looks like a credential or carries one in a URL.
pub fn args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut next_secret = false;
    for a in args {
        let flag = a.starts_with('-');
        let shown = if next_secret && !flag && !is_ref(a) {
            mask(a)
        } else if let (true, Some((f, v))) = (flag, a.split_once('=')) {
            if secret_name(f) && !is_ref(v) && !v.is_empty() {
                format!("{f}={}", mask(v))
            } else if let Some(m) = assignment(v) {
                // `--env=NAME=value`, `--header=Name: value`
                format!("{f}={m}")
            } else {
                a.clone()
            }
        } else if let Some(m) = Some(a).filter(|a| !a.starts_with('-')).and_then(|a| assignment(a)) {
            // The value of `-e NAME=value` / `--header "Name: value"`.
            m
        } else if !is_ref(a) && looks_secret(a) {
            mask(a)
        } else {
            url(a)
        };
        next_secret = flag && !a.contains('=') && secret_name(a);
        out.push(shown);
    }
    out
}

/// An agent's other fields, with the strings under secret names masked (recursively).
pub fn extra(m: &Map<String, Value>) -> Map<String, Value> {
    m.iter().map(|(k, v)| (k.clone(), field(k, v, false))).collect()
}

/// Each string is judged by its own key (`oauth.clientId` is not a secret, `oauth.clientSecret`
/// is); under Codex's `env_http_headers` the values are variable names.
fn field(name: &str, v: &Value, names: bool) -> Value {
    match v {
        Value::String(s) if names || is_ref(s) || s.is_empty() => v.clone(),
        Value::String(s) if secret_name(name) || looks_secret(s) => Value::String(mask(s)),
        Value::String(s) => Value::String(url(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| field(name, x, names)).collect()),
        Value::Object(o) => {
            let names = names || name.to_ascii_lowercase().starts_with("env_");
            Value::Object(o.iter().map(|(k, x)| (k.clone(), field(k, x, names))).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn references_stay_readable_and_secrets_are_masked() {
        assert!(is_ref("${GITHUB_TOKEN}") && is_ref("{env:X}") && is_ref("$X") && is_ref("%APPDATA%") && is_ref("Bearer ${T}") && is_ref("${X:-d}"));
        assert!(!is_ref("pre-${X}") && !is_ref("sk-abc"));
        assert_eq!(value("GITHUB_TOKEN", "${GITHUB_TOKEN}"), ("${GITHUB_TOKEN}".into(), false));
        assert_eq!(value("API_KEY", "abcdefgh1234"), ("••••1234".into(), true));
        assert_eq!(value("Authorization", "Bearer sk-live-abcdefgh9876"), ("Bearer ••••9876".into(), true));
        // A credential under an innocent name.
        assert_eq!(value("X", "ghp_0123456789abcdefghijklmnop"), ("••••mnop".into(), true));
        assert_eq!(value("NODE_ENV", "production"), ("production".into(), false));
        assert_eq!(value("PATH_HINT", "C:\\Users\\me\\bin"), ("C:\\Users\\me\\bin".into(), false));
    }

    #[test]
    fn urls_lose_their_passwords_and_secret_parameters() {
        assert_eq!(url("https://mcp.example.com/sse?api_key=abcdef123456&region=eu"), "https://mcp.example.com/sse?api_key=••••3456&region=eu");
        assert_eq!(url("https://u:hunter22pass@host/x"), "https://u:••••pass@host/x");
        assert_eq!(url("https://mcp.example.com/mcp?token=${T}"), "https://mcp.example.com/mcp?token=${T}");
        assert_eq!(url("https://mcp.example.com/mcp"), "https://mcp.example.com/mcp");
    }

    #[test]
    fn arguments_after_secret_flags_are_masked() {
        let a: Vec<String> = ["-y", "@x/server", "--api-key", "abcd1234efgh", "--token=zzzz9999yyyy", "--port", "8080", "--key", "${K}"].map(String::from).to_vec();
        assert_eq!(args(&a), ["-y", "@x/server", "--api-key", "••••efgh", "--token=••••yyyy", "--port", "8080", "--key", "${K}"]);
    }

    #[test]
    fn env_and_header_arguments_are_masked_by_their_names() {
        let a: Vec<String> = [
            "run", "-i", "-e", "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuv", "-e", "LOG_LEVEL=debug", "-e", "API_KEY=${API_KEY}", "-e", "EMPTY_TOKEN=",
            "--env=SERVICE_KEY=abcdefgh12345678", "--header", "Authorization: Bearer abcdefgh12345678", "--header=X-Api-Key: zzzzyyyyxxxx9999", "-H", "Accept: application/json",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            args(&a),
            [
                "run", "-i", "-e", "GITHUB_TOKEN=••••stuv", "-e", "LOG_LEVEL=debug", "-e", "API_KEY=${API_KEY}", "-e", "EMPTY_TOKEN=",
                "--env=SERVICE_KEY=••••5678", "--header", "Authorization: Bearer ••••5678", "--header=X-Api-Key: ••••9999", "-H", "Accept: application/json",
            ]
        );
    }

    #[test]
    fn extra_fields_mask_by_name_but_keep_variable_names() {
        let m = json!({
            "bearer_token_env_var": "GITHUB_TOKEN",
            "oauth": { "clientId": "abc", "clientSecret": "s3cr3t-value-1234" },
            "timeout": 60,
            "env_vars": ["A_TOKEN"],
            "env_http_headers": { "Authorization": "GH_TOKEN" },
            "api_keys": ["abcdefgh1234"],
        });
        let out = extra(m.as_object().unwrap());
        assert_eq!(out["bearer_token_env_var"], "GITHUB_TOKEN");
        assert_eq!(out["oauth"]["clientId"], "abc");
        assert_eq!(out["oauth"]["clientSecret"], "••••1234");
        assert_eq!(out["timeout"], 60);
        assert_eq!(out["env_vars"], json!(["A_TOKEN"]));
        assert_eq!(out["env_http_headers"]["Authorization"], "GH_TOKEN");
        assert_eq!(out["api_keys"], json!(["••••1234"]));
    }

    #[test]
    fn references_normalize_to_one_syntax() {
        assert_eq!(normalize_refs("{env:A} $B %C% ${D} ${E:-x}"), "${A} ${B} ${C} ${D} ${E:-x}");
    }
}
