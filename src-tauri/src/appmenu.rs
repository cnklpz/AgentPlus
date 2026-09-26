//! macOS menu bar. The window's own controls (the settings button, search, the context menu)
//! stay; the menu bar adds the Mac places for them: Settings… (⌘,) and Check for Updates… in
//! the app menu, the pages under Go, and the standard Edit / Window items (without an Edit
//! menu ⌘C / ⌘V don't reach the page).
//!
//! AgentPlus's own items are handled by the page: a click sends the item's id to the main
//! window as a `menu` event (bringing the window back first if it sits hidden), except the
//! ones that only open something outside the app.

use crate::i18n::l;
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Wry};

pub const GITHUB_URL: &str = "https://github.com/cnklpz/AgentPlus";

/// Item ids the page acts on (App.tsx listens for them).
const PAGE_ITEMS: &[&str] = &["settings", "check-update", "search", "reload", "privacy", "providers", "gateway", "history"];

fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let item = |id: &'static str, (en, zh): (&'static str, &'static str), keys: Option<&'static str>| MenuItem::with_id(app, id, l(en, zh), true, keys);
    let sep = || PredefinedMenuItem::separator(app);
    let about = AboutMetadata { website: Some(GITHUB_URL.into()), ..Default::default() };

    let app_menu = Submenu::with_items(app, "AgentPlus", true, &[
        &PredefinedMenuItem::about(app, Some(l("About AgentPlus", "关于 AgentPlus")), Some(about))?,
        &sep()?,
        &item("settings", ("Settings…", "设置…"), Some("CmdOrCtrl+,"))?,
        &item("check-update", ("Check for Updates…", "检查更新…"), None)?,
        &sep()?,
        &PredefinedMenuItem::services(app, Some(l("Services", "服务")))?,
        &sep()?,
        &PredefinedMenuItem::hide(app, Some(l("Hide AgentPlus", "隐藏 AgentPlus")))?,
        &PredefinedMenuItem::hide_others(app, Some(l("Hide Others", "隐藏其他")))?,
        &PredefinedMenuItem::show_all(app, Some(l("Show All", "全部显示")))?,
        &sep()?,
        &PredefinedMenuItem::quit(app, Some(l("Quit AgentPlus", "退出 AgentPlus")))?,
    ])?;
    let edit = Submenu::with_items(app, l("Edit", "编辑"), true, &[
        &PredefinedMenuItem::undo(app, Some(l("Undo", "撤销")))?,
        &PredefinedMenuItem::redo(app, Some(l("Redo", "重做")))?,
        &sep()?,
        &PredefinedMenuItem::cut(app, Some(l("Cut", "剪切")))?,
        &PredefinedMenuItem::copy(app, Some(l("Copy", "拷贝")))?,
        &PredefinedMenuItem::paste(app, Some(l("Paste", "粘贴")))?,
        &PredefinedMenuItem::select_all(app, Some(l("Select All", "全选")))?,
    ])?;
    let view = Submenu::with_items(app, l("View", "显示"), true, &[
        &item("search", ("Search…", "搜索…"), Some("CmdOrCtrl+K"))?,
        &item("reload", ("Reload Configs", "重新读取配置"), Some("CmdOrCtrl+R"))?,
        &item("privacy", ("Privacy Mode", "隐私模式"), Some("CmdOrCtrl+Shift+H"))?,
        &sep()?,
        &PredefinedMenuItem::fullscreen(app, Some(l("Enter Full Screen", "进入全屏幕")))?,
    ])?;
    let go = Submenu::with_items(app, l("Go", "前往"), true, &[
        &item("providers", ("Providers", "供应商"), Some("CmdOrCtrl+1"))?,
        &item("gateway", ("Local Gateway", "本地网关"), Some("CmdOrCtrl+2"))?,
        &item("history", ("History & Rollback", "历史与回滚"), Some("CmdOrCtrl+3"))?,
        &sep()?,
        &item("data-dir", ("Open AgentPlus Data Folder", "打开 AgentPlus 数据目录"), None)?,
    ])?;
    let window = Submenu::with_items(app, l("Window", "窗口"), true, &[
        &PredefinedMenuItem::minimize(app, Some(l("Minimize", "最小化")))?,
        &PredefinedMenuItem::maximize(app, Some(l("Zoom", "缩放")))?,
        &sep()?,
        &PredefinedMenuItem::close_window(app, Some(l("Close Window", "关闭窗口")))?,
    ])?;
    let help = Submenu::with_items(app, l("Help", "帮助"), true, &[
        &item("github", ("AgentPlus on GitHub", "AgentPlus 的 GitHub 主页"), None)?,
        &item("issue", ("Report an Issue", "反馈问题"), None)?,
    ])?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &go, &window, &help])
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    app.set_menu(build(app)?)?;
    app.on_menu_event(|app, e| {
        let id = e.id().as_ref();
        match id {
            "data-dir" => {
                let d = crate::util::agentplus_dir();
                let _ = std::fs::create_dir_all(&d);
                let _ = crate::process::open_dir(&d.to_string_lossy());
            }
            "github" => {
                let _ = crate::process::open_dir(GITHUB_URL);
            }
            "issue" => {
                let _ = crate::process::open_dir(&format!("{GITHUB_URL}/issues/new/choose"));
            }
            _ if PAGE_ITEMS.contains(&id) => {
                crate::tray::show_main(app);
                let _ = app.emit_to("main", "menu", id);
            }
            _ => {}
        }
    });
    Ok(())
}

/// After a language switch.
pub fn relabel(app: &AppHandle) {
    if cfg!(target_os = "macos") {
        if let Ok(menu) = build(app) {
            let _ = app.set_menu(menu);
        }
    }
}
