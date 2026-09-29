//! Notification-area (tray) icon. Closing the window can hide it here instead of quitting,
//! so the gateway keeps running; a click on the icon brings the window back.
//!
//! macOS: the same choice, worded "menu bar". While the window is hidden the app also leaves
//! the Dock and its icon shows in the menu bar; showing the window reverses both.

use crate::i18n::l;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const SHOW: (&str, &str) = ("Show AgentPlus", "显示主窗口");
const QUIT: (&str, &str) = ("Quit AgentPlus", "退出 AgentPlus");

/// Menu items whose text follows the UI language.
struct TrayMenu {
    show: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", l(SHOW.0, SHOW.1), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", l(QUIT.0, QUIT.1), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("AgentPlus")
        .menu(&menu)
        // A Mac menu bar icon opens its menu on a plain click.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, e| match e.id().as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if cfg!(target_os = "macos") {
                return;
            }
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    let tray = tray.build(app)?;
    // On macOS the icon only shows while the window is hidden (see `hide_main`).
    if cfg!(target_os = "macos") {
        tray.set_visible(false)?;
    }
    app.manage(TrayMenu { show, quit });
    Ok(())
}

/// Hide the main window in the tray (macOS: in the menu bar, out of the Dock).
pub fn hide_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = app.tray_by_id("main") {
            let _ = t.set_visible(true);
        }
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

/// Bring the main window back from the tray (or from minimized) and focus it.
pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        if let Some(t) = app.tray_by_id("main") {
            let _ = t.set_visible(false);
        }
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// After a language switch.
pub fn relabel(app: &AppHandle) {
    if let Some(m) = app.try_state::<TrayMenu>() {
        let _ = m.show.set_text(l(SHOW.0, SHOW.1));
        let _ = m.quit.set_text(l(QUIT.0, QUIT.1));
    }
}
