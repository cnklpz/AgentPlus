//! Notification-area (tray) icon. Closing the window can hide it here instead of quitting,
//! so the gateway keeps running; a click on the icon brings the window back.
//!
//! macOS: the same choice, worded "menu bar". While the window is hidden the app also leaves
//! the Dock and its icon shows in the menu bar; showing the window reverses both.
//!
//! The icon is the app's own or, by choice (`store.json` → `trayIcon: "mono"`), a single
//! colour one like the system's: on macOS a template image the menu bar tints, on Windows
//! white or black to go with AgentPlus's dark or light theme (the page reports the theme it
//! shows, which on "auto" follows the system; `trayDark` keeps it for the next start).
//!
//! The menu (right click; macOS: any click) has shortcuts: the gateway switch, the main
//! pages, settings and updates. The page items share their ids with the macOS menu bar
//! and are handled with it (`appmenu::handle`).

use crate::i18n::l;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const STORE_KEY: &str = "trayIcon";
const DARK_KEY: &str = "trayDark";

static APP: OnceLock<AppHandle> = OnceLock::new();
static MONO: AtomicBool = AtomicBool::new(false);
/// AgentPlus shows its dark theme: the single-colour icon is white (Windows).
static DARK: AtomicBool = AtomicBool::new(false);
/// Pixels the single-colour icon was last drawn at.
static DRAWN: AtomicU32 = AtomicU32::new(0);
/// Held through `set_mono`, so the store and `MONO` end up with the same choice.
static SET_MONO: Mutex<()> = Mutex::new(());
/// The gateway switch in the menu, rebuilt with it after a language switch.
static GATEWAY_ITEM: Mutex<Option<CheckMenuItem<Wry>>> = Mutex::new(None);

fn main_tray(app: &AppHandle) -> Option<TrayIcon> {
    app.tray_by_id("main")
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let item = |id: &str, (en, zh): (&'static str, &'static str)| MenuItem::with_id(app, id, l(en, zh), true, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let running = crate::gateway::server::running_port().is_some();
    let gateway = CheckMenuItem::with_id(app, "tray-gateway", l("Run local gateway", "运行本地网关"), true, running, None::<&str>)?;
    let menu = Menu::with_items(app, &[
        &item("tray-show", ("Show AgentPlus", "显示主窗口"))?,
        &sep()?,
        &gateway,
        &sep()?,
        &item("providers", ("Providers", "供应商"))?,
        &item("gateway", ("Local gateway", "本地网关"))?,
        &item("history", ("History & rollback", "历史与回滚"))?,
        &sep()?,
        &item("settings", ("Settings…", "设置…"))?,
        &item("check-update", ("Check for updates…", "检查更新…"))?,
        &item("data-dir", ("Open data folder", "打开数据目录"))?,
        &sep()?,
        &item("tray-quit", ("Quit AgentPlus", "退出 AgentPlus"))?,
    ])?;
    *crate::util::lock(&GATEWAY_ITEM) = Some(gateway);
    Ok(menu)
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let _ = APP.set(app.clone());
    crate::gateway::server::on_change(gateway_changed);
    let root = crate::store::load();
    MONO.store(root.get(STORE_KEY).and_then(|v| v.as_str()) == Some("mono"), Ordering::Relaxed);
    DARK.store(root.get(DARK_KEY).and_then(|v| v.as_bool()).unwrap_or(false), Ordering::Relaxed);
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("AgentPlus")
        .menu(&build_menu(app)?)
        // A Mac menu bar icon opens its menu on a plain click.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        // Every menu's events reach this handler (the menu bar's too): act on our own ids only.
        .on_menu_event(|app, e| match e.id().as_ref() {
            "tray-show" => show_main(app),
            "tray-gateway" => toggle_gateway(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if cfg!(target_os = "macos") {
                return;
            }
            // The taskbar's scale may have changed since the icon was drawn: pointing at it is enough.
            refresh_size(tray);
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = icon_image(app) {
        tray = tray.icon(icon).icon_as_template(mono());
    }
    let tray = tray.build(app)?;
    // On macOS the icon only shows while the window is hidden (see `hide_main`).
    if cfg!(target_os = "macos") {
        tray.set_visible(false)?;
    }
    // A display's scale changed: the taskbar's may have too.
    if let Some(w) = app.get_webview_window("main") {
        w.on_window_event(|e| {
            if let tauri::WindowEvent::ScaleFactorChanged { .. } = e {
                if let Some(t) = APP.get().and_then(main_tray) {
                    refresh_size(&t);
                }
            }
        });
    }
    Ok(())
}

/// Starts or stops the gateway from the menu; a failure opens the gateway page, which says why.
fn toggle_gateway(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::gateway::server::toggle() {
            crate::applog::warn("tray", format!("gateway switch failed: {e}"));
            crate::appmenu::handle(&app, "gateway");
        }
        // The item ticked itself on the click; show what actually happened.
        gateway_changed();
    });
}

/// The gateway started or stopped (from anywhere): tick the menu's switch to match.
fn gateway_changed() {
    let Some(app) = APP.get() else { return };
    // Posted rather than run here: the caller may hold the gateway's locks.
    let _ = app.run_on_main_thread(|| {
        if let Some(item) = crate::util::lock(&GATEWAY_ITEM).as_ref() {
            let _ = item.set_checked(crate::gateway::server::running_port().is_some());
        }
    });
}

pub fn mono() -> bool {
    MONO.load(Ordering::Relaxed)
}

pub fn set_mono(app: &AppHandle, on: bool) -> anyhow::Result<()> {
    let _g = crate::util::lock(&SET_MONO);
    crate::store::update(|root| {
        root[STORE_KEY] = serde_json::json!(if on { "mono" } else { "color" });
        Ok(())
    })?;
    MONO.store(on, Ordering::Relaxed);
    if let Some(t) = main_tray(app) {
        apply_icon(app, &t);
    }
    Ok(())
}

/// The theme the page shows changed (or the page loaded): recolour the single-colour icon.
/// Called in order on the main thread (a sync command), so the theme sent last wins; the
/// store write, which may wait for the store's lock, runs on its own thread.
pub fn set_dark(app: &AppHandle, dark: bool) {
    if DARK.swap(dark, Ordering::Relaxed) != dark && mono() && !cfg!(target_os = "macos") {
        if let Some(t) = main_tray(app) {
            apply_icon(app, &t);
        }
    }
    std::thread::spawn(|| {
        if let Err(e) = save_dark() {
            crate::applog::warn("tray", format!("can't save the tray theme: {e}"));
        }
    });
}

/// Keeps the current theme for the next start. `DARK` is read under the store's write lock,
/// so of two saves racing, the one writing last writes the newest theme. Compared with the
/// store rather than remembered, so a write that failed is made on the next call.
fn save_dark() -> anyhow::Result<()> {
    let now = || DARK.load(Ordering::Relaxed);
    if crate::store::load().get(DARK_KEY).and_then(|v| v.as_bool()) == Some(now()) {
        return Ok(());
    }
    crate::store::update(|root| {
        root[DARK_KEY] = serde_json::json!(now());
        Ok(())
    })
}

/// The icon to show: the app's own, or the single-colour one for the theme and taskbar scale.
fn icon_image(app: &AppHandle) -> Option<Image<'_>> {
    if !mono() {
        return app.default_window_icon().cloned();
    }
    let size = mono_size();
    DRAWN.store(size, Ordering::Relaxed);
    Some(Image::new_owned(mono_icon(size, mono_rgb()), size, size))
}

fn apply_icon(app: &AppHandle, tray: &TrayIcon) {
    if let Some(icon) = icon_image(app) {
        let _ = tray.set_icon_with_as_template(Some(icon), mono());
    }
}

/// Redraws the single-colour icon when the size it should be drawn at has changed.
fn refresh_size(tray: &TrayIcon) {
    if mono() && mono_size() != DRAWN.load(Ordering::Relaxed) {
        apply_icon(tray.app_handle(), tray);
    }
}

/// macOS: black, for a template image (only its alpha counts; the menu bar tints it).
/// Windows: white with the dark theme, black with the light one.
fn mono_rgb() -> [u8; 3] {
    if DARK.load(Ordering::Relaxed) && !cfg!(target_os = "macos") {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    }
}

/// Pixels the icon is drawn at: the size the system shows it (Windows: the small icon at the
/// taskbar's scale), so it isn't scaled afterwards.
fn mono_size() -> u32 {
    #[cfg(windows)]
    unsafe {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetSystemMetrics, SM_CXSMICON};
        // The taskbar's scale now: the system DPI this process started with stays the same
        // after the display's scale is changed.
        let dpi = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).map(|h| GetDpiForWindow(h)).unwrap_or(0);
        let n = if dpi > 0 { GetSystemMetricsForDpi(SM_CXSMICON, dpi) } else { GetSystemMetrics(SM_CXSMICON) };
        if n > 0 {
            return (n as u32).clamp(16, 64);
        }
    }
    // macOS shows it 18 points high: 36 pixels on a Retina screen.
    36
}

/// The single-colour icon as RGBA, `size` pixels square: the app icon's two cards on a
/// 32-unit grid, the front one solid with the plus cut out and the back one half transparent.
/// Edges are anti-aliased from signed distances, so any size comes out smooth; straight
/// edges sit on even units, whole pixels at 16 and 32 pixels.
fn mono_icon(size: u32, [r, g, b]: [u8; 3]) -> Vec<u8> {
    let unit = size as f32 / 32.0;
    // Coverage of a pixel by a shape `d` grid units from its edge (negative inside).
    let cov = |d: f32| (0.5 - d * unit).clamp(0.0, 1.0);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / unit, (y as f32 + 0.5) / unit);
            let front = rounded_rect(p, (2.0, 10.0, 22.0, 30.0), 5.0);
            let back = rounded_rect(p, (10.0, 2.0, 30.0, 22.0), 5.0);
            let plus = segment(p, (8.0, 20.0), (16.0, 20.0)).min(segment(p, (12.0, 16.0), (12.0, 24.0))) - 2.0;
            let front_a = cov(front) * (1.0 - cov(plus));
            // A gap of 2 units keeps the back card off the front one's edge.
            let back_a = 0.5 * cov(back) * (1.0 - cov(front - 2.0));
            let a = front_a.max(back_a);
            px.extend_from_slice(&[r, g, b, (a * 255.0).round() as u8]);
        }
    }
    px
}

/// Signed distance from `p` to a rectangle `(left, top, right, bottom)` with corner radius `r`.
fn rounded_rect((x, y): (f32, f32), (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let qx = (x - (x0 + x1) / 2.0).abs() - (x1 - x0) / 2.0 + r;
    let qy = (y - (y0 + y1) / 2.0).abs() - (y1 - y0) / 2.0 + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Distance from `p` to the segment `a`–`b`.
fn segment((x, y): (f32, f32), (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
    let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - dx * t).hypot(py - dy * t)
}

/// Hide the main window in the tray (macOS: in the menu bar, out of the Dock).
pub fn hide_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = main_tray(app) {
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
        if let Some(t) = main_tray(app) {
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
    if let (Some(t), Ok(menu)) = (main_tray(app), build_menu(app)) {
        let _ = t.set_menu(Some(menu));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: &[u8], size: u32, x: u32, y: u32) -> u8 {
        px[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn mono_icon_draws_both_cards_and_the_plus() {
        for size in [16, 20, 24, 32, 36] {
            let px = mono_icon(size, [255, 255, 255]);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert!(px.chunks(4).all(|c| c[..3] == [255, 255, 255]));
            // The pixel holding a grid point.
            let at = |gx: f32, gy: f32| alpha(&px, size, (gx * size as f32 / 32.0) as u32, (gy * size as f32 / 32.0) as u32);
            assert_eq!(at(0.5, 0.5), 0, "corner is empty at {size}");
            assert_eq!(at(7.0, 25.0), 255, "front card is solid at {size}");
            assert_eq!(at(11.5, 19.5), 0, "plus is cut out at {size}");
            assert_eq!(at(25.0, 7.0), 128, "back card is half transparent at {size}");
        }
    }

    #[test]
    fn mono_icon_is_pixel_sharp_at_16() {
        let px = mono_icon(16, [0, 0, 0]);
        // Straight edges fall between pixels: the front card's left edge, the plus's bar
        // (rows 9–10) and the one-pixel gap between the cards (column 11).
        assert_eq!((alpha(&px, 16, 0, 12), alpha(&px, 16, 1, 12)), (0, 255));
        assert_eq!([8, 9, 10, 11].map(|y| alpha(&px, 16, 4, y)), [255, 0, 0, 255]);
        assert_eq!((alpha(&px, 16, 10, 9), alpha(&px, 16, 11, 9), alpha(&px, 16, 12, 9)), (255, 0, 128));
    }

    /// Tests that set the global `DARK` run one at a time.
    static DARK_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mono_colour_follows_the_theme() {
        let _dark = crate::util::lock(&DARK_LOCK);
        DARK.store(true, Ordering::Relaxed);
        assert_eq!(mono_rgb(), if cfg!(target_os = "macos") { [0, 0, 0] } else { [255, 255, 255] });
        DARK.store(false, Ordering::Relaxed);
        assert_eq!(mono_rgb(), [0, 0, 0]);
    }

    #[test]
    fn theme_is_saved_until_the_store_holds_it() {
        let _dark = crate::util::lock(&DARK_LOCK);
        let _home = crate::util::TestHome::new("tray-dark");
        let stored = || crate::store::load().get(DARK_KEY).and_then(|v| v.as_bool());
        assert_eq!(stored(), None);
        DARK.store(true, Ordering::Relaxed);
        save_dark().unwrap();
        assert_eq!(stored(), Some(true));
        save_dark().unwrap();
        assert_eq!(stored(), Some(true));
        // A write that didn't land (here: undone by hand) is made on the next call, even
        // though the theme itself didn't change in between.
        crate::store::update(|root| {
            root[DARK_KEY] = serde_json::json!(false);
            Ok(())
        })
        .unwrap();
        save_dark().unwrap();
        assert_eq!(stored(), Some(true));
        // It writes the theme current when it runs, not the one it was started for.
        DARK.store(false, Ordering::Relaxed);
        save_dark().unwrap();
        assert_eq!(stored(), Some(false));
    }

    #[test]
    fn distances() {
        let r = (0.0, 0.0, 10.0, 10.0);
        assert!((rounded_rect((5.0, 5.0), r, 2.0) + 5.0).abs() < 1e-5);
        assert!((rounded_rect((12.0, 5.0), r, 2.0) - 2.0).abs() < 1e-5);
        // Past a rounded corner: distance to the arc, not to the square corner.
        assert!((rounded_rect((12.0, 12.0), r, 2.0) - (32f32.sqrt() - 2.0)).abs() < 1e-5);
        assert!((segment((5.0, 3.0), (0.0, 0.0), (10.0, 0.0)) - 3.0).abs() < 1e-5);
        assert!((segment((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-5);
    }
}
