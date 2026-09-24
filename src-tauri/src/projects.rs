//! Project folders the user opened in AgentPlus (for per-project agent configs), kept
//! newest first in `~/.agentplus/store.json` per environment, plus the native folder picker.

use crate::adapters::ocproject;
use crate::store;
use crate::util::{display_path, require_dir};
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};

const MAX: usize = 30;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    /// As the user gave it (the key; also part of the agent id).
    pub path: String,
    pub name: String,
    /// `opencode@<path>`: pass to get_agent / preview / apply.
    pub agent: String,
    pub last_opened: Option<String>,
    pub exists: bool,
    /// The project's opencode.json(c) when it has one.
    pub config: Option<String>,
    /// Providers defined in that file.
    pub providers: usize,
    pub git: bool,
}

/// Store key: projects are per environment (a WSL path means nothing on Windows).
fn key() -> String {
    if crate::env::is_wsl() {
        format!("projects@{}", crate::env::id())
    } else {
        "projects".into()
    }
}

fn load_list(root: &Value) -> Vec<Value> {
    root.get(key()).and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

fn save_list(root: &mut Value, list: Vec<Value>) -> Result<()> {
    if !root.is_object() {
        *root = json!({});
    }
    root[key()] = Value::Array(list);
    store::save(root)
}

/// "D:\xm\proj\" -> "D:\xm\proj" (roots like "D:\" and "/" stay).
fn normalize(path: &str) -> String {
    let t = path.trim().trim_matches('"');
    let s = t.trim_end_matches(['\\', '/']);
    if s.is_empty() || s.ends_with(':') { t.to_string() } else { s.to_string() }
}

fn entry(path: &str, last: Option<String>) -> ProjectEntry {
    let dir = crate::env::resolve_path(path);
    let cfg = ocproject::config_path(&dir);
    let providers = std::fs::read_to_string(&cfg)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&crate::util::strip_jsonc(&t).0).ok())
        .and_then(|v| v.get("provider").and_then(|p| p.as_object()).map(|o| o.len()))
        .unwrap_or(0);
    ProjectEntry {
        path: path.to_string(),
        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string()),
        agent: ocproject::agent_id(path),
        last_opened: last,
        exists: dir.is_dir(),
        config: cfg.exists().then(|| display_path(&cfg)),
        providers,
        git: dir.ancestors().any(|d| d.join(".git").exists()),
    }
}

pub fn list() -> Vec<ProjectEntry> {
    load_list(&store::load())
        .iter()
        .filter_map(|v| Some(entry(v.get("path")?.as_str()?, v.get("lastOpened").and_then(|x| x.as_str()).map(String::from))))
        .collect()
}

/// Checks the folder and moves it to the top of the history.
pub fn open(path: &str) -> Result<ProjectEntry> {
    let path = normalize(path);
    if path.is_empty() {
        return Err(anyhow!(crate::i18n::l("请输入文件夹路径", "Enter a folder path")));
    }
    require_dir(&crate::env::resolve_path(&path))?;
    let now = chrono::Local::now().to_rfc3339();
    let mut root = store::load();
    let mut list: Vec<Value> = load_list(&root).into_iter().filter(|v| v.get("path").and_then(|p| p.as_str()).is_some_and(|p| !same_path(p, &path))).collect();
    list.insert(0, json!({ "path": path, "lastOpened": now }));
    list.truncate(MAX);
    save_list(&mut root, list)?;
    Ok(entry(&path, Some(now)))
}

/// Two history entries name the same folder: Windows paths ignore case, Linux paths
/// (WSL: `/home/…`, `~/…`) don't.
fn same_path(a: &str, b: &str) -> bool {
    if a.starts_with(['/', '~']) || b.starts_with(['/', '~']) {
        a == b
    } else {
        a.eq_ignore_ascii_case(b)
    }
}

pub fn forget(path: &str) -> Result<()> {
    let mut root = store::load();
    let list = load_list(&root).into_iter().filter(|v| v.get("path").and_then(|p| p.as_str()) != Some(path)).collect();
    save_list(&mut root, list)
}

/// Native "choose folder" dialog owned by the AgentPlus window. None when cancelled.
#[cfg(windows)]
pub fn pick_folder(owner: isize, start: Option<&str>) -> Result<Option<String>> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};

    const CANCELLED: u32 = 0x800704C7; // HRESULT_FROM_WIN32(ERROR_CANCELLED)
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dlg.SetOptions(dlg.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        dlg.SetTitle(&HSTRING::from(crate::i18n::l("选择项目文件夹", "Choose project folder")))?;
        if let Some(s) = start.map(crate::env::resolve_path).filter(|p| p.is_dir()) {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(s.as_os_str()), None) {
                let _ = dlg.SetFolder(&item);
            }
        }
        if let Err(e) = dlg.Show(HWND(owner as _)) {
            return if e.code().0 as u32 == CANCELLED { Ok(None) } else { Err(e.into()) };
        }
        let p = dlg.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as _));
        Ok(Some(s?))
    }
}

#[cfg(not(windows))]
pub fn pick_folder(_owner: isize, _start: Option<&str>) -> Result<Option<String>> {
    Err(anyhow!(crate::i18n::l("当前系统不支持选择文件夹，请直接输入路径", "Folder picking isn't supported on this system. Enter the path directly")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize(r" D:\xm\proj\ "), r"D:\xm\proj");
        assert_eq!(normalize(r"D:\"), r"D:\");
        assert_eq!(normalize("/home/me/p/"), "/home/me/p");
        assert_eq!(normalize("\"C:\\a b\""), r"C:\a b");
    }

    #[test]
    fn path_case_matters_only_on_windows() {
        assert!(same_path(r"D:\Xm\Proj", r"d:\xm\proj"));
        assert!(same_path(r"\\srv\Share", r"\\SRV\share"));
        assert!(!same_path("/home/me/Proj", "/home/me/proj"));
        assert!(!same_path("~/Proj", "~/proj"));
        assert!(same_path("/home/me/proj", "/home/me/proj"));
    }
}
