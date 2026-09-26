//! Self-update from GitHub Releases through tauri-plugin-updater. The endpoint (the release's
//! `latest.json`) and the public key that installers must be signed with are in tauri.conf.json.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The update found by the last check; installing uses exactly this one.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    /// Release notes (Markdown, as written on the GitHub release).
    pub notes: Option<String>,
    /// RFC 3339, when the release says.
    pub date: Option<String>,
}

#[derive(serde::Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Progress {
    Download { done: u64, total: Option<u64> },
    /// Downloaded and verified; the installer is starting (on Windows the app then exits).
    Install,
}

fn info(u: &Update) -> UpdateInfo {
    UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone().filter(|s| !s.trim().is_empty()),
        date: u.raw_json.get("pub_date").and_then(|v| v.as_str()).map(String::from),
    }
}

/// A newer release, or `None` when this is the latest.
#[tauri::command]
pub async fn update_check(app: AppHandle, pending: State<'_, Pending>) -> Result<Option<UpdateInfo>, String> {
    let fail = |e: tauri_plugin_updater::Error| tr!("Update check failed: {e}", "检查更新失败：{e}");
    let updater = app.updater_builder().timeout(Duration::from_secs(30)).build().map_err(fail)?;
    let found = updater.check().await.map_err(fail)?;
    let out = found.as_ref().map(info);
    *pending.0.lock().unwrap() = found;
    Ok(out)
}

/// Downloads the update found by the last check, verifies its signature and installs it.
/// Windows: the installer runs in passive mode, this process exits and the new version starts.
/// Elsewhere the app restarts itself once the new bundle is in place.
#[tauri::command]
pub async fn update_install(app: AppHandle, pending: State<'_, Pending>, on_progress: Channel<Progress>) -> Result<(), String> {
    let update = pending.0.lock().unwrap().clone().ok_or(crate::i18n::l("No update to install; check for updates first", "没有待安装的更新，请先检查更新"))?;
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err(crate::i18n::l("An update is already being installed", "正在安装更新").into());
    }
    let r = install(&update, &on_progress).await;
    INSTALLING.store(false, Ordering::SeqCst);
    r?;
    app.restart();
}

async fn install(update: &Update, on_progress: &Channel<Progress>) -> Result<(), String> {
    let mut done = 0u64;
    let mut last = Instant::now() - Duration::from_secs(1);
    let bytes = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                // A few updates a second is plenty for a progress bar.
                if last.elapsed() >= Duration::from_millis(100) || total == Some(done) {
                    last = Instant::now();
                    let _ = on_progress.send(Progress::Download { done, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| tr!("Download failed: {e}", "下载更新失败：{e}"))?;
    let _ = on_progress.send(Progress::Install);
    update.install(bytes).map_err(|e| tr!("Install failed: {e}", "安装更新失败：{e}"))
}
