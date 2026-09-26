//! AgentPlus's own state in `~/.agentplus/store.json`:
//! per-agent switches and definitions stashed while a model/provider is hidden.
//!
//! The file is read from many threads at once (the gateway reads it on every request),
//! so it is only ever replaced whole (temp file + rename): a reader sees the old or the
//! new content, never half of it. Writes are serialized by `WRITE`; code that runs off
//! the main thread and changes the store uses `update` so the load and save happen
//! under that one lock.

use crate::util::agentplus_dir;
use anyhow::Context;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

static WRITE: Mutex<()> = Mutex::new(());

fn read(p: &Path) -> Option<Value> {
    let text = fs::read_to_string(p).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    // Every writer indexes into the top level as an object: `[]` or `1` counts as broken.
    serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object())
}

pub fn load() -> Value {
    load_in(&agentplus_dir())
}

fn load_in(dir: &Path) -> Value {
    let p = dir.join("store.json");
    // Another program may be halfway through writing it: give it a moment. A file that
    // has been broken for a while is not retried (the gateway loads this on every request).
    let fresh = || fs::metadata(&p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(2));
    for i in 0..3 {
        if let Some(v) = read(&p) {
            return v;
        }
        if i == 2 || !fresh() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    json!({})
}

thread_local! {
    /// This thread holds `WRITE` through `transaction`.
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Takes `WRITE` unless this thread already holds it (inside `transaction`).
fn lock() -> Option<std::sync::MutexGuard<'static, ()>> {
    if HELD.with(|h| h.get()) {
        return None;
    }
    Some(crate::util::lock(&WRITE))
}

/// Runs `f` holding the write lock, so a `load` … `save` inside it can't lose a change
/// another thread makes with `update` in between. `save` / `update` inside `f` don't lock again.
pub fn transaction<T>(f: impl FnOnce() -> T) -> T {
    struct Release(Option<std::sync::MutexGuard<'static, ()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if self.0.is_some() {
                HELD.with(|h| h.set(false));
            }
        }
    }
    let _r = Release(lock());
    HELD.with(|h| h.set(true));
    f()
}

pub fn save(v: &Value) -> anyhow::Result<()> {
    let _g = lock();
    write(v)
}

/// Load, change, save under the write lock (for callers outside the main thread).
pub fn update<T>(f: impl FnOnce(&mut Value) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let _g = lock();
    let mut v = load();
    let out = f(&mut v)?;
    write(&v)?;
    Ok(out)
}

fn write(v: &Value) -> anyhow::Result<()> {
    write_in(&agentplus_dir(), v)
}

fn write_in(dir: &Path, v: &Value) -> anyhow::Result<()> {
    crate::util::ensure_private_dir(dir)?;
    let path = dir.join("store.json");
    // A file that exists but does not parse loaded as {}: keep it instead of overwriting it.
    if path.exists() && read(&path).is_none() && fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
        let keep = dir.join(format!("store.broken-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        crate::util::write_private_atomic(&keep, &fs::read(&path)?).with_context(|| tr!("Failed to back up unparsable {}", "备份无法解析的 {} 失败", path.display()))?;
    }
    crate::util::write_private_atomic(&path, &serde_json::to_vec_pretty(v)?)
}

/// Per-agent entries are kept apart per environment: "codex" on Windows, "codex@wsl:Ubuntu" in WSL.
pub(crate) fn scoped(agent: &str) -> String {
    if crate::env::is_wsl() && crate::adapters::ALL.contains(&agent) {
        format!("{agent}@{}", crate::env::id())
    } else {
        agent.to_string()
    }
}

fn agent_obj<'a>(root: &'a mut Value, agent: &str) -> &'a mut Map<String, Value> {
    let agent = &scoped(agent);
    if !root.is_object() {
        *root = json!({});
    }
    let a = root
        .as_object_mut()
        .unwrap()
        .entry(agent.to_string())
        .or_insert_with(|| json!({}));
    if !a.is_object() {
        *a = json!({});
    }
    a.as_object_mut().unwrap()
}

/// Returns `store[agent][key]` as an object, creating it as needed.
pub fn section<'a>(root: &'a mut Value, agent: &str, key: &str) -> &'a mut Map<String, Value> {
    let s = agent_obj(root, agent).entry(key.to_string()).or_insert_with(|| json!({}));
    if !s.is_object() {
        *s = json!({});
    }
    s.as_object_mut().unwrap()
}

/// `store[agent][key]` for the current environment.
pub fn agent_get<'a>(root: &'a Value, agent: &str, key: &str) -> Option<&'a Value> {
    root.get(scoped(agent)).and_then(|a| a.get(key))
}

/// A copy of `store[agent][key]` as an object; empty when it is missing or not an object.
pub fn get_obj(root: &Value, agent: &str, key: &str) -> Map<String, Value> {
    agent_get(root, agent, key).and_then(|x| x.as_object()).cloned().unwrap_or_default()
}

/// A copy of `store[agent][key]` as an array; empty when it is missing or not an array.
pub fn get_arr(root: &Value, agent: &str, key: &str) -> Vec<Value> {
    agent_get(root, agent, key).and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

pub fn get_flag(root: &Value, agent: &str, key: &str) -> bool {
    root.get(scoped(agent)).and_then(|a| a.get(key)).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_flag(root: &mut Value, agent: &str, key: &str, v: bool) {
    agent_obj(root, agent).insert(key.to_string(), Value::Bool(v));
}

pub fn get_str(root: &Value, agent: &str, key: &str) -> Option<String> {
    root.get(scoped(agent)).and_then(|a| a.get(key)).and_then(|v| v.as_str()).map(String::from)
}

pub fn set_str(root: &mut Value, agent: &str, key: &str, v: &str) {
    agent_obj(root, agent).insert(key.to_string(), Value::from(v));
}

pub fn set_value(root: &mut Value, agent: &str, key: &str, v: Value) {
    agent_obj(root, agent).insert(key.to_string(), v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("agentplus-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn store_and_broken_copies_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let h = crate::util::TestHome::new("store-private");
        let d = h.0.join(".agentplus");
        let p = d.join("store.json");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        write_in(&d, &json!({ "library": [{ "apiKey": "dummy" }] })).unwrap();
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&p), 0o600);
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&p, "{ broken secret").unwrap();
        write_in(&d, &json!({})).unwrap();
        assert_eq!(mode(&d), 0o700);
        for e in fs::read_dir(&d).unwrap().flatten() {
            assert_eq!(mode(&e.path()), 0o600);
        }
    }

    /// Readers running while the store is rewritten over and over never see a partial file.
    #[test]
    fn readers_never_see_a_torn_file() {
        let d = tmp_dir("torn");
        let big: Vec<String> = (0..2000).map(|i| format!("entry-{i}")).collect();
        write_in(&d, &json!({ "library": big, "n": 0 })).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (d, stop) = (d.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut n = 0;
                    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                        let v = load_in(&d);
                        assert_eq!(v["library"].as_array().map(|a| a.len()), Some(2000), "read a partial store");
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        for i in 1..40 {
            write_in(&d, &json!({ "library": big, "n": i })).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        for r in readers {
            assert!(r.join().unwrap() > 0);
        }
        assert_eq!(load_in(&d)["n"], 39);
        assert!(!fs::read_dir(&d).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")), "temp file left behind");
        let _ = fs::remove_dir_all(&d);
    }

    /// A store that does not parse is kept aside before anything overwrites it.
    #[test]
    fn broken_store_is_backed_up_before_overwrite() {
        let d = tmp_dir("broken");
        fs::write(d.join("store.json"), "{ \"library\": [ half written").unwrap();
        assert_eq!(load_in(&d), json!({}));
        write_in(&d, &json!({ "gateway": {} })).unwrap();
        let kept: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("store.broken-")).collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(fs::read_to_string(d.join(&kept[0])).unwrap(), "{ \"library\": [ half written");
        assert_eq!(load_in(&d), json!({ "gateway": {} }));
        let _ = fs::remove_dir_all(&d);
    }

    /// A top level that parses but is not an object (a hand edit) loads as {} and is kept
    /// aside too, instead of making `root["library"] = …` panic.
    #[test]
    fn non_object_store_loads_empty() {
        for (i, body) in ["[]", "1", "\"x\"", "null"].into_iter().enumerate() {
            let d = tmp_dir(&format!("nonobj{i}"));
            fs::write(d.join("store.json"), body).unwrap();
            assert_eq!(load_in(&d), json!({}), "{body}");
            write_in(&d, &json!({ "a": 1 })).unwrap();
            assert!(fs::read_dir(&d).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("store.broken-")), "{body}");
            assert_eq!(load_in(&d), json!({ "a": 1 }));
            let _ = fs::remove_dir_all(&d);
        }
    }
}
