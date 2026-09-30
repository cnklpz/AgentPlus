#[macro_use]
mod i18n;
mod adapters;
mod asar;
mod attribution;
mod applog;
mod appmenu;
mod cdp;
mod deeplink;
mod dotenv;
mod env;
mod gateway;
mod history;
mod library;
mod mcp;
mod model;
mod plugins;
mod mfields;
mod modelinfo;
mod net;
mod official;
mod process;
mod projects;
mod sessions;
mod store;
mod dialog;
mod seal;
mod skills;
mod sync;
mod tray;
mod update;
mod util;

use model::*;
use tauri::Manager;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Runs a blocking closure off the UI thread. A command without `async` runs on the main
/// thread and freezes the window while it waits, so anything that touches the disk (a
/// `\\wsl.localhost` path can wake a stopped distro), spawns a process or waits goes here.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(err)
}

/// `blocking` inside one store transaction (a load … save that must not interleave).
async fn blocking_tx<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, String> {
    blocking(move || store::transaction(f)).await
}

/// A gateway change, then the gateway's new status for the page.
fn with_status(r: anyhow::Result<()>) -> Result<gateway::server::Status, String> {
    r.map_err(err)?;
    Ok(gateway::server::status())
}

/// Every agent's state, read side by side: each can wait on a `--version` probe, PowerShell
/// or WSL, and one slow agent shouldn't hold up the others.
#[tauri::command]
async fn list_agents() -> Result<Vec<AgentState>, String> {
    blocking(|| {
        let t0 = std::time::Instant::now();
        let results: Vec<(&str, anyhow::Result<AgentState>, std::time::Duration)> = std::thread::scope(|s| {
            let jobs: Vec<_> = adapters::ALL
                .iter()
                .map(|a| {
                    s.spawn(move || {
                        let t = std::time::Instant::now();
                        (*a, adapters::state(a), t.elapsed())
                    })
                })
                .collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        });
        let total = t0.elapsed();
        if total > std::time::Duration::from_secs(3) {
            let slow: Vec<String> = results.iter().filter(|(_, _, d)| d.as_millis() >= 500).map(|(a, _, d)| format!("{a} {:.1}s", d.as_secs_f32())).collect();
            applog::warn("detect", format!("agent list took {:.1}s; slowest: {}", total.as_secs_f32(), slow.join(", ")));
        }
        Ok(results
            .into_iter()
            .filter_map(|(a, r, _)| r.map_err(|e| applog::error("state", format!("{a}: {e:#}"))).ok())
            .collect())
    })
    .await
}

#[tauri::command]
async fn get_agent(agent: String) -> Result<AgentState, String> {
    blocking(move || adapters::state(&agent)).await
}

#[tauri::command]
async fn preview(agent: String, ops: Vec<Op>) -> Result<Vec<DiffGroup>, String> {
    blocking(move || adapters::plan(&agent, &ops, true).map(|(d, ..)| d.groups)).await
}

#[tauri::command]
async fn apply(agent: String, ops: Vec<Op>) -> Result<ApplyResult, String> {
    blocking(move || {
        // Reading state spawns `--version` probes, WSL and PowerShell, so these commands run
        // on worker threads; the transaction keeps each write's store load/save atomic.
        let ops = adapters::resolve(&agent, &ops)?;
        let (_, files, backup) = store::transaction(|| adapters::plan_resolved(&agent, &ops, false))?;
        sync::changed();
        Ok(ApplyResult {
            state: adapters::state(&agent)?,
            files: files.iter().map(|f| util::display_path(f)).collect(),
            backup_dir: backup.map(|b| b.to_string_lossy().to_string()),
        })
    })
    .await
}

#[tauri::command]
async fn test_latency(url: String) -> Result<u64, String> {
    blocking(move || net::latency(&url).map_err(anyhow::Error::msg)).await
}

/// Restarts an agent (or starts it when it isn't running). For Codex with UI injection on
/// (Fast, full model names, send after quota, hidden usage banners), starts it with a local DevTools port and patches the UI once
/// it is up. Each step is reported on `on_progress` while it runs. `plain` skips the port and
/// the patching (faster) for a restart that only has to make Codex re-read its files.
#[tauri::command]
async fn restart_agent(agent: String, on_progress: tauri::ipc::Channel<process::Progress>, plain: Option<bool>) -> Result<String, String> {
    // One at a time: a second one would clear the first's cancel, and the two would stop and
    // start the same processes over each other.
    let Some(_running) = process::RestartGuard::take() else {
        return Err(i18n::l("A restart is already running", "已有一个重启在进行中").to_string());
    };
    process::reset_cancel();
    blocking(move || {
        let _awake = process::stay_awake("Restarting an agent");
        let report = |p: process::Progress| {
            let _ = on_progress.send(p);
        };
        let patches = if agent == adapters::codex::ID && plain != Some(true) { adapters::codex::ui_patches() } else { cdp::Patches::default() };
        let inject = patches.any();
        let mut steps = vec!["stop", "start"];
        if inject {
            steps.extend(["port", "patch"]);
        }
        report(process::Progress::Plan { steps });
        // Failures are logged: the restart dialog offers the log for export.
        let logged = |e: anyhow::Error| {
            applog::error("restart", format!("{agent}: {e:#}"));
            e
        };
        // The port can be taken between picking it and Codex binding it (or be unusable
        // for a reason the check can't see): retry once with another one, quicker to give up.
        let mut tried: Vec<u16> = vec![];
        let (r, injected) = loop {
            let port = cdp::pick_port(&tried);
            tried.push(port);
            let args = if inject { format!("--remote-debugging-port={port}") } else { String::new() };
            let r = process::restart(&agent, &args, &report).map_err(logged)?;
            if !inject {
                break (r, None);
            }
            let last = tried.len() >= 2;
            let wait = std::time::Duration::from_secs(if last { 60 } else { 25 });
            match cdp::inject(port, wait, patches.for_codex(r.version.as_deref()), &report) {
                Err(e) if !last && e.is::<cdp::PortTimeout>() => applog::warn("restart", format!("{agent}: {e:#}; retrying with another port")),
                res => break (r, Some(res.map_err(logged)?)),
            }
        };
        let mut msg = if let Some(m) = injected {
            m
        } else if r.was_running {
            i18n::l("Restarted", "已重启").into()
        } else {
            i18n::l("Started", "已启动").into()
        };
        if r.cli_sessions > 0 {
            let n = r.cli_sessions;
            msg.push_str(&tr!("; reopen the {n} CLI session(s) in terminals for the change to apply", "；终端里的 {n} 个 CLI 会话要重新打开才会生效"));
        }
        Ok(msg)
    })
    .await
}

/// Stops the running `restart_agent` at its next wait; it then fails with "Cancelled".
#[tauri::command]
fn cancel_restart() {
    process::cancel();
}

/// Whether an agent's app is running right now, and how it was started.
#[derive(serde::Serialize)]
struct RunState {
    running: bool,
    launch: Option<Launch>,
}

#[tauri::command]
async fn agent_running(agent: String) -> Result<RunState, String> {
    blocking(move || {
        let (running, launch) = process::running(&agent);
        Ok(RunState { running, launch })
    })
    .await
}

#[tauri::command]
async fn codex_sessions() -> Result<sessions::SessionList, String> {
    blocking(sessions::list).await
}

#[tauri::command]
async fn codex_health() -> Result<Vec<sessions::HealthItem>, String> {
    blocking(sessions::health).await
}

#[tauri::command]
async fn codex_cleanup_preview(days: u32) -> Result<sessions::CleanupPreview, String> {
    blocking(move || sessions::cleanup_preview(days)).await
}

#[tauri::command]
async fn codex_cleanup(tmp: bool, logs_days: Option<u32>, wal: bool) -> Result<String, String> {
    blocking(move || sessions::cleanup(tmp, logs_days, wal)).await
}

#[tauri::command]
async fn codex_repair(ids: Vec<String>, target: String) -> Result<String, String> {
    blocking(move || sessions::repair(&ids, &target)).await
}

#[tauri::command]
async fn codex_undo_repair(stamp: String) -> Result<String, String> {
    blocking(move || sessions::undo_repair(&stamp)).await
}

/// Model ids offered by an existing provider (key resolved in the backend).
#[tauri::command]
async fn fetch_models(agent: String, provider: String) -> Result<Vec<String>, String> {
    blocking(move || {
        let (base, key, api) = adapters::provider_endpoint(&agent, &provider)?;
        net::list_models(&base, key.as_deref(), &api).map_err(anyhow::Error::msg)
    })
    .await
}

/// Sends one small real request through a provider (agent entry, or "library").
#[tauri::command]
async fn test_provider(agent: String, provider: String, model: String) -> Result<net::TestResult, String> {
    blocking(move || {
        let (base, key, api) = adapters::provider_endpoint(&agent, &provider)?;
        Ok(net::test_call(&base, key.as_deref(), &api, model.trim()))
    })
    .await
}

/// What an attribution test of `model` in `provider`'s list (or Codex's catalog, "*") goes to.
#[tauri::command]
async fn attribution_target(agent: String, provider: String, model: String) -> Result<attribution::Target, String> {
    blocking(move || attribution::target(&agent, &provider, &model)).await
}

/// Sends one attribution challenge; `fp` is the target's, so every sample of a run goes to
/// the same endpoint. The answer streams to `on_live` as it arrives.
#[tauri::command]
async fn attribution_sample(agent: String, provider: String, model: String, prompt: String, fp: String, on_live: tauri::ipc::Channel<attribution::Live>) -> Result<attribution::SampleResult, String> {
    blocking(move || attribution::sample(&agent, &provider, &model, &prompt, &fp, &|e| { let _ = on_live.send(e); })).await
}

/// Past attribution results, newest first.
#[tauri::command]
async fn attribution_history() -> Result<Vec<serde_json::Value>, String> {
    blocking(|| Ok(attribution::history())).await
}

#[tauri::command]
async fn attribution_history_add(entry: serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    blocking(move || attribution::history_add(entry)).await
}

/// Deletes these results, or all of them with no ids.
#[tauri::command]
async fn attribution_history_delete(ids: Option<Vec<String>>) -> Result<Vec<serde_json::Value>, String> {
    blocking(move || attribution::history_delete(ids)).await
}

/// Model ids straight from a library entry's upstream (used while an agent goes through the gateway).
#[tauri::command]
async fn fetch_models_lib(id: String) -> Result<Vec<String>, String> {
    fetch_models(library::FROM.to_string(), id).await
}

/// What the model catalogs say about these ids, as `agent`'s model settings (unknown ids are left out).
#[tauri::command]
async fn guess_models(agent: String, ids: Vec<String>) -> Result<std::collections::BTreeMap<String, modelinfo::Guess>, String> {
    blocking(move || Ok(modelinfo::guesses(&agent, &ids))).await
}

/// Model ids for a provider being added (key typed in the form).
#[tauri::command]
async fn fetch_models_url(base_url: String, api_key: Option<String>, api: String) -> Result<Vec<String>, String> {
    blocking(move || net::list_models(&base_url, api_key.as_deref(), &api).map_err(anyhow::Error::msg)).await
}

#[tauri::command]
async fn gateway_models(routes: Vec<String>) -> Result<Vec<String>, String> {
    blocking(move || gateway::server::list_models(&routes)).await
}

/// The skills folders these agents read, with their skills, and each agent's view of them.
#[tauri::command]
async fn skills_list(agents: Vec<String>) -> Result<skills::Overview, String> {
    blocking(move || Ok(skills::overview(&agents))).await
}

/// Copies a skill folder into a skills folder; `replace` overwrites one with other files.
#[tauri::command]
async fn skills_copy(from: String, to: String, replace: bool) -> Result<skills::write::Copied, String> {
    blocking(move || skills::write::copy(&from, &to, replace)).await
}

/// Imports skills (a folder, a .zip or a Git address) into the skill library; names in
/// `replace` overwrite another version there.
#[tauri::command]
async fn skills_import(source: String, replace: Vec<String>) -> Result<Vec<skills::import::Imported>, String> {
    blocking(move || skills::import::import(&source, &replace)).await
}

/// Deletes a skill folder (backed up first).
#[tauri::command]
async fn skills_delete(dir: String) -> Result<(), String> {
    blocking(move || skills::write::delete(&dir)).await
}

/// Switches a skill off or on for one agent.
#[tauri::command]
async fn skills_set_enabled(agent: String, name: String, dir: String, on: bool) -> Result<(), String> {
    blocking(move || store::transaction(|| skills::write::set_enabled(&agent, &name, &dir, on))).await
}

/// Global MCP servers of these agents and the MCP library, secrets masked.
#[tauri::command]
async fn mcp_list(agents: Vec<String>) -> Result<mcp::Overview, String> {
    blocking(move || Ok(mcp::overview(&agents))).await
}

#[tauri::command]
async fn mcp_library_save(server: mcp::McpInput) -> Result<(), String> {
    blocking(move || mcp::library::save(server)).await
}

#[tauri::command]
async fn mcp_library_delete(name: String) -> Result<(), String> {
    blocking(move || mcp::library::delete(&name)).await
}

/// Starts or calls an MCP server as `source` (an agent, or "library") has it, and lists its tools.
#[tauri::command]
async fn mcp_probe(source: String, name: String) -> Result<mcp::probe::Probe, String> {
    blocking(move || mcp::probe(&source, &name)).await
}

/// MCP servers in pasted text (a README snippet or a piece of any agent's config).
#[tauri::command]
async fn mcp_parse(text: String) -> Result<Vec<mcp::McpInput>, String> {
    blocking(move || mcp::parse::parse(&text)).await
}

#[tauri::command]
async fn list_backups() -> Result<Vec<history::BackupEntry>, String> {
    blocking(history::list).await
}

#[tauri::command]
async fn backup_detail(id: String) -> Result<history::BackupDetail, String> {
    blocking(move || history::detail(&id)).await
}

#[tauri::command]
async fn restore_backup(id: String) -> Result<String, String> {
    blocking(move || {
        let msg = history::restore(&id)?;
        sync::changed();
        Ok(msg)
    })
    .await
}

#[tauri::command]
async fn sync_status() -> Result<sync::SyncStatus, String> {
    blocking(|| Ok(sync::status())).await
}

/// No outer transaction: set_folder locks the store itself, after checking the folder
/// (which may be a slow network share).
#[tauri::command]
async fn sync_set_folder(path: String) -> Result<(), String> {
    blocking(move || sync::set_folder(&path)).await
}

#[tauri::command]
async fn sync_export() -> Result<String, String> {
    blocking(sync::export).await
}

#[tauri::command]
async fn sync_preview(snapshot: Option<String>) -> Result<Vec<sync::Suggestion>, String> {
    blocking(move || sync::preview_import(snapshot.as_deref())).await
}

#[tauri::command]
async fn sync_history() -> Result<Vec<sync::HistoryEntry>, String> {
    blocking(|| Ok(sync::history())).await
}

#[tauri::command]
async fn sync_delete_records(ids: Vec<String>) -> Result<String, String> {
    blocking(move || sync::delete_records(&ids)).await
}

#[tauri::command]
async fn sync_set_options(options: sync::SyncOptions) -> Result<(), String> {
    blocking(move || sync::set_options(options)).await
}

/// `trigger`: "start" (the window calls it once after loading). "change" runs from the backend.
#[tauri::command]
async fn sync_auto(trigger: String) -> Result<sync::AutoResult, String> {
    blocking(move || Ok(sync::auto(&trigger))).await
}

/// `password: None` turns encryption off. `verify`: only accept a password that opens the current file.
#[tauri::command]
async fn sync_set_password(password: Option<String>, verify: bool) -> Result<String, String> {
    blocking(move || sync::set_password(password.as_deref(), verify)).await
}

#[tauri::command]
fn sync_generate_password() -> Result<String, String> {
    sync::generate_password().map_err(err)
}

/// Saves a generated sync key as a .txt file through the save dialog. None when cancelled.
#[tauri::command]
async fn sync_save_key(window: tauri::WebviewWindow, key: String) -> Result<Option<String>, String> {
    #[cfg(windows)]
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(windows))]
    let owner = {
        let _ = window;
        0
    };
    blocking(move || sync::save_key(owner, &key)).await
}

#[tauri::command]
async fn sync_set_include_keys(on: bool) -> Result<(), String> {
    blocking(move || sync::set_include_keys(on)).await
}

#[tauri::command]
async fn sync_adopt_library(changes: Vec<sync::LibChange>) -> Result<String, String> {
    blocking(move || sync::adopt_library(changes)).await
}

#[tauri::command]
async fn sync_adopt_skills(changes: Vec<sync::SkillChange>) -> Result<String, String> {
    blocking(move || sync::adopt_skills(changes)).await
}

#[tauri::command]
async fn sync_adopt_mcp(changes: Vec<sync::McpChange>) -> Result<String, String> {
    blocking(move || sync::adopt_mcp(changes)).await
}

#[tauri::command]
fn codex_dismiss_fixed_prompt() -> Result<(), String> {
    store::transaction(adapters::codex::dismiss_fixed_prompt).map_err(err)
}

/// Opens a folder in Explorer. Only folders: handing Explorer a file would run it with its
/// default program (.exe, .bat…). WSL paths (`~/…`, `/home/…`) are resolved first, which can
/// wake a stopped distro, so this runs off the UI thread.
#[tauri::command]
async fn open_path(path: String) -> Result<(), String> {
    blocking(move || {
        let p = env::resolve_path(path.trim());
        util::require_dir(&p)?;
        process::open_dir(&p.to_string_lossy())
    })
    .await
}

/// Opens a web page (a provider's console) in the default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") || url.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err(i18n::l("Only https links can be opened", "只能打开 https 链接").into());
    }
    process::open_url(&url).map_err(err)
}

/// Opens the web UI an agent serves (dsh web) in the default browser. The link can carry
/// its sign-in token, so it stays in the backend. Some(note) when the link can't sign the
/// browser in by itself.
#[tauri::command]
async fn open_web_ui(agent: String) -> Result<Option<String>, String> {
    blocking(move || {
        if agent != adapters::dsh::ID {
            anyhow::bail!("{}", tr!("{agent} has no web UI", "{agent} 没有网页界面"));
        }
        let (link, note) = adapters::dsh::web_link()?;
        process::open_url(&link)?;
        Ok(note)
    })
    .await
}

/// Stops the web UI server an agent runs in the background (dsh web), whoever started it.
#[tauri::command]
async fn stop_agent(agent: String) -> Result<String, String> {
    blocking(move || {
        let n = process::stop_server(&agent)?;
        Ok(if n > 0 { i18n::l("Stopped", "已停止") } else { i18n::l("Not running", "没有在运行") }.to_string())
    })
    .await
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    process::reveal(&path).map_err(err)
}

#[tauri::command]
async fn list_envs() -> Vec<env::EnvInfo> {
    tauri::async_runtime::spawn_blocking(env::list).await.unwrap_or_default()
}

#[tauri::command]
async fn set_env(id: String) -> Result<(), String> {
    blocking(move || env::set(&id)).await
}

#[tauri::command]
async fn detect_agents() -> Vec<adapters::Detect> {
    tauri::async_runtime::spawn_blocking(adapters::detect_all).await.unwrap_or_default()
}

#[tauri::command]
async fn set_agent_dir(agent: String, path: Option<String>) -> Result<(), String> {
    blocking_tx(move || adapters::set_dir(&agent, path.as_deref())).await
}

#[tauri::command]
fn gateway_status() -> gateway::server::Status {
    gateway::server::status()
}

/// Stopping the old listener can wait a few seconds.
#[tauri::command]
async fn gateway_set(enabled: bool, port: Option<u16>) -> Result<gateway::server::Status, String> {
    blocking(move || {
        gateway::server::set_enabled(enabled, port)?;
        Ok(gateway::server::status())
    })
    .await
}

#[tauri::command]
fn gateway_save_route(route: gateway::server::Route, old_id: Option<String>) -> Result<gateway::server::Status, String> {
    with_status(gateway::server::save_route(route, old_id))
}

#[tauri::command]
fn gateway_delete_route(id: String) -> Result<gateway::server::Status, String> {
    with_status(gateway::server::delete_route(&id))
}

#[tauri::command]
fn gateway_set_breaker(breaker: gateway::breaker::Config) -> Result<gateway::server::Status, String> {
    with_status(gateway::server::set_breaker(breaker))
}

/// Time zone the gateway puts in Codex's environment context; null stops rewriting it.
#[tauri::command]
fn gateway_set_timezone(timezone: Option<String>) -> Result<gateway::server::Status, String> {
    with_status(gateway::server::set_timezone(timezone))
}

/// Lets a forward paused by the error breaker (or every one, with no id) work again now.
#[tauri::command]
fn gateway_reset_breaker(id: Option<String>) -> gateway::server::Status {
    gateway::server::reset_breaker(id.as_deref());
    gateway::server::status()
}

/// One small request through the running gateway, speaking `api` to it. It goes through
/// even while the forward is paused, and a success un-pauses it.
#[tauri::command]
async fn gateway_test(route: String, api: String, model: String) -> Result<net::TestResult, String> {
    blocking(move || {
        let st = gateway::server::status();
        if !st.running {
            anyhow::bail!("{}", i18n::l("The gateway is not running", "网关没有运行"));
        }
        let base = format!("http://127.0.0.1:{}/{route}/v1", st.port);
        Ok(net::test_call(&base, Some(gateway::server::test_key()), &api, model.trim()))
    })
    .await
}

/// Polled while the page waits for Codex; reads Codex's files (UNC paths in WSL mode).
#[tauri::command]
async fn codex_official_status() -> Result<official::FetchStatus, String> {
    blocking(|| Ok(official::status())).await
}

#[tauri::command]
async fn codex_official_start() -> Result<official::FetchStatus, String> {
    blocking_tx(official::start).await
}

#[tauri::command]
async fn codex_official_finish() -> Result<Vec<official::FetchModel>, String> {
    blocking_tx(official::finish).await
}

#[tauri::command]
async fn codex_official_cancel() -> Result<(), String> {
    blocking_tx(official::cancel).await
}

/// With a ChatGPT sign-in: the official list, downloaded by the installed Codex without a restart.
#[tauri::command]
async fn codex_official_auto() -> Result<Vec<official::FetchModel>, String> {
    blocking(official::fetch_signed_in).await
}

/// Codex's model catalog from the list built into the installed Codex (no sign-in needed).
#[tauri::command]
async fn codex_builtin_catalog() -> Result<Vec<official::FetchModel>, String> {
    blocking(official::from_codex).await
}

/// The provider dialog's refetch: reads Codex's built-in list into a temporary file only.
#[tauri::command]
async fn codex_builtin_preview() -> Result<Vec<official::FetchModel>, String> {
    blocking(official::builtin_preview).await
}

/// The provider dialog was saved: the list `codex_builtin_preview` read becomes the catalog.
#[tauri::command]
async fn codex_builtin_commit() -> Result<Vec<official::FetchModel>, String> {
    blocking(official::builtin_commit).await
}

/// The provider dialog was closed without saving: drops the list it read.
#[tauri::command]
fn codex_builtin_discard() {
    official::builtin_discard();
}

#[tauri::command]
fn library_list() -> Vec<library::LibEntry> {
    library::list()
}

/// No outer transaction: save reads the adopted key from the agent first (which can wake
/// WSL), then locks the store itself.
#[tauri::command]
async fn library_save(input: library::LibInput) -> Result<library::LibEntry, String> {
    blocking(move || library::save(input)).await
}

#[tauri::command]
fn library_delete(id: String) -> Result<(), String> {
    library::delete(&id).map_err(err)
}

/// UI language for backend text ("zh" / "en"); the frontend calls this first.
#[tauri::command]
fn set_locale(app: tauri::AppHandle, lang: String) {
    i18n::set(&lang);
    tray::relabel(&app);
    appmenu::relabel(&app);
}

/// Quit from the window's close button (the tray menu quits on its own).
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// Hide the window in the tray (macOS: the menu bar) from the close dialog / preference.
#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle) {
    tray::hide_main(&app);
}

#[tauri::command]
fn log_info() -> applog::LogInfo {
    applog::status()
}

#[tauri::command]
async fn log_set(enabled: bool, days: u32) -> Result<applog::LogInfo, String> {
    blocking(move || applog::set(enabled, days)).await
}

#[tauri::command]
async fn log_clear() -> Result<applog::LogInfo, String> {
    blocking(applog::clear).await
}

/// An entry from the page: a failed command, an uncaught error.
#[tauri::command]
fn log_client(level: String, source: String, message: String) {
    applog::client(&level, &source, &message);
}

/// Writes the scrubbed log, with a header about this machine, to one file; returns its path.
#[tauri::command]
async fn log_export(app: tauri::AppHandle) -> Result<String, String> {
    let version = app.package_info().version.to_string();
    blocking(move || {
        let mut header = format!(
            "AgentPlus {version} diagnostic log
exported: {}
os: {} {} ({})
language: {}
environment: {}
",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            os_version(),
            if i18n::is_en() { "en" } else { "zh" },
            env::id(),
        );
        header.push_str("agents:
");
        for d in adapters::detect_all().iter().filter(|d| d.app_found || d.config_found) {
            header.push_str(&format!(
                "  {}: {} running={} config={} custom_dir={}
",
                d.id,
                d.version.as_deref().unwrap_or("?"),
                d.running,
                d.config_found,
                d.custom_dir.is_some()
            ));
        }
        let p = applog::export(&header)?;
        Ok(p.to_string_lossy().to_string())
    })
    .await
}

/// "Windows 11 (26200)"-like text for the export header; empty when unknown.
fn os_version() -> String {
    sysinfo::System::long_os_version().unwrap_or_default()
}

#[tauri::command]
fn open_log_dir() -> Result<(), String> {
    let d = applog::dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    process::open_dir(&d.to_string_lossy()).map_err(err)
}

#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    let d = util::agentplus_dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    process::open_dir(&d.to_string_lossy()).map_err(err)
}

#[tauri::command]
async fn open_config_dir(agent: String) -> Result<(), String> {
    blocking(move || process::open_dir(&adapters::state(&agent)?.config_dir)).await
}

/// Checks each project folder and its config (UNC paths in WSL mode).
#[tauri::command]
async fn projects_list() -> Result<Vec<projects::ProjectEntry>, String> {
    blocking(|| Ok(projects::list())).await
}

#[tauri::command]
async fn project_open(path: String) -> Result<projects::ProjectEntry, String> {
    blocking_tx(move || projects::open(&path)).await
}

#[tauri::command]
fn project_forget(path: String) -> Result<(), String> {
    store::transaction(|| projects::forget(&path)).map_err(err)
}

/// Native folder picker, owned by the AgentPlus window.
#[tauri::command]
async fn pick_folder(window: tauri::WebviewWindow, start: Option<String>, purpose: Option<String>) -> Result<Option<String>, String> {
    #[cfg(windows)]
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(windows))]
    let owner = { let _ = window; 0 };
    // `purpose` only picks the dialog title: "sync" for the sync folder, else a project folder.
    let title = match purpose.as_deref() {
        Some("sync") => i18n::l("Choose sync folder", "选择同步文件夹"),
        Some("skill") => i18n::l("Choose a skill folder", "选择技能文件夹"),
        _ => i18n::l("Choose project folder", "选择项目文件夹"),
    };
    blocking(move || projects::pick_folder(owner, start.as_deref(), title)).await
}

/// Panics (a worker thread, the gateway) go to the diagnostic log before the default report.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        applog::error("panic", format!("thread {thread} at {at}: {msg}"));
        prev(info);
    }));
}

/// Import links: the one this launch was started with, macOS's "open URL" events, and the
/// registration of `agentplus://` (and of `ccswitch://`, when the user took it over).
fn setup_links(app: &tauri::App) {
    // Only queued: the page takes it once loaded, and the window shows itself then.
    for a in std::env::args().skip(1) {
        if deeplink::is_link(&a) {
            deeplink::queue(&a);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        let h = app.handle().clone();
        app.deep_link().on_open_url(move |e| {
            for u in e.urls() {
                deeplink::receive(&h, u.as_str());
            }
        });
        if let Ok(Some(urls)) = app.deep_link().get_current() {
            for u in urls {
                deeplink::queue(u.as_str());
            }
        }
    }
    #[cfg(windows)]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        // The installer registers agentplus://; a release build also points it back at itself
        // when it was moved (portable copy, another install folder). Dev builds leave it alone.
        let dl = app.deep_link();
        if !cfg!(debug_assertions) && !dl.is_registered(deeplink::SCHEME).unwrap_or(true) {
            if let Err(e) = dl.register(deeplink::SCHEME) {
                applog::warn("import", format!("Can't register {}://: {e}", deeplink::SCHEME));
            }
        }
        deeplink::refresh_ccswitch();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = app;
}

/// Import links that came in since the last call, each parsed (or why it can't be imported).
#[tauri::command]
fn take_imports() -> Vec<deeplink::ImportItem> {
    deeplink::take()
}

/// A pasted import link.
#[tauri::command]
fn parse_import_link(link: String) -> Result<deeplink::ImportItem, String> {
    deeplink::parse_link(&link).map_err(err)
}

#[tauri::command]
async fn ccswitch_link_status() -> Result<deeplink::LinkHandler, String> {
    blocking(|| Ok(deeplink::ccswitch_status())).await
}

#[tauri::command]
async fn set_ccswitch_link(on: bool) -> Result<deeplink::LinkHandler, String> {
    blocking(move || deeplink::set_ccswitch(on)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first: a second launch (e.g. while this one sits in the tray) exits
        // right away and brings this window forward instead of starting a second gateway.
        // A second launch may carry an import link (agentplus:// or ccswitch://).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            tray::show_main(app);
            deeplink::receive_args(app, args);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Pending::default())
        .setup(|app| {
            applog::init();
            sync::init(app.handle().clone());
            // Not fatal: the store retries creating the folder on its first write.
            if let Err(e) = util::ensure_private_dir(&util::agentplus_dir()) {
                applog::warn("app", format!("Can't create {}: {e}", util::agentplus_dir().display()));
            }
            applog::info("app", format!("AgentPlus {} started on {} {} ({})", app.package_info().version, std::env::consts::OS, std::env::consts::ARCH, os_version()));
            install_panic_hook();
            tray::setup(app.handle())?;
            appmenu::setup(app.handle())?;
            setup_links(app);
            // Off the startup path: binding the port and stopping an old listener can wait.
            std::thread::spawn(gateway::server::autostart);
            // macOS: ask the login shell for PATH now, before the first detection needs it.
            std::thread::spawn(process::search_path);
            // models.dev's catalog, for filling in new models' settings (refreshed weekly).
            std::thread::spawn(modelinfo::refresh_if_stale);
            // Sign-in tokens left in the logs of web UI servers that have stopped since.
            std::thread::spawn(process::forget_idle_tokens);
            // The window starts hidden and the page shows it after its first render, so the
            // WebView's blank white never flashes. Fallback in case the page never gets there.
            if let Some(w) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if !w.is_visible().unwrap_or(true) {
                        let _ = w.show();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_agents,
            get_agent,
            preview,
            apply,
            test_latency,
            restart_agent,
            stop_agent,
            cancel_restart,
            agent_running,
            open_config_dir,
            projects_list,
            project_open,
            project_forget,
            pick_folder,
            codex_sessions,
            codex_health,
            codex_cleanup_preview,
            codex_cleanup,
            codex_repair,
            codex_undo_repair,
            reveal_path,
            fetch_models,
            fetch_models_url,
            gateway_models,
            fetch_models_lib,
            guess_models,
            skills_list,
            skills_copy,
            skills_delete,
            skills_import,
            skills_set_enabled,
            mcp_list,
            mcp_library_save,
            mcp_library_delete,
            mcp_parse,
            mcp_probe,
            list_backups,
            backup_detail,
            restore_backup,
            sync_status,
            sync_set_folder,
            sync_export,
            sync_preview,
            sync_set_password,
            sync_generate_password,
            sync_save_key,
            sync_set_include_keys,
            sync_adopt_library,
            sync_adopt_mcp,
            sync_adopt_skills,
            sync_history,
            sync_delete_records,
            sync_set_options,
            sync_auto,
            open_path,
            open_url,
            open_web_ui,
            codex_dismiss_fixed_prompt,
            list_envs,
            set_env,
            library_list,
            library_save,
            library_delete,
            open_data_dir,
            set_locale,
            quit_app,
            hide_to_tray,
            detect_agents,
            test_provider,
            attribution_target,
            attribution_sample,
            attribution_history,
            attribution_history_add,
            attribution_history_delete,
            codex_official_status,
            codex_official_start,
            codex_official_finish,
            codex_official_cancel,
            codex_official_auto,
            codex_builtin_catalog,
            codex_builtin_preview,
            codex_builtin_commit,
            codex_builtin_discard,
            gateway_status,
            gateway_set,
            gateway_save_route,
            gateway_delete_route,
            gateway_set_breaker,
            gateway_set_timezone,
            gateway_reset_breaker,
            gateway_test,
            set_agent_dir,
            log_info,
            log_set,
            log_clear,
            log_client,
            log_export,
            open_log_dir,
            take_imports,
            parse_import_link,
            ccswitch_link_status,
            set_ccswitch_link,
            update::update_check,
            update::update_install
        ])
        .build(tauri::generate_context!())
        .expect("error while running AgentPlus")
        .run(|_app, event| {
            // macOS: a click on the Dock icon while the window sits hidden in the menu bar.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                tray::show_main(_app);
            }
            if let tauri::RunEvent::Exit = event {
                process::end_own_servers();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: prints what each adapter sees on this machine.
    /// `cargo test dump_states -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_states() {
        for a in adapters::ALL {
            let st = adapters::state(a).unwrap();
            println!("{}", serde_json::to_string_pretty(&st).unwrap());
        }
    }

    /// Read-only: Codex detection and how the running app was started.
    /// `cargo test dump_codex_launch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_codex_launch() {
        let t = std::time::Instant::now();
        let inst = process::detect("codex");
        println!("installed={} ver={:?} dir={:?} running={} ({:?})", inst.installed, inst.version, inst.dir, inst.running, t.elapsed());
        println!("{:?}", process::running("codex"));
    }

    /// Read-only: the WSL view (codex CLI) without touching the saved choice.
    /// `cargo test dump_wsl -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_wsl() {
        env::force(env::Target::Wsl { distro: "Ubuntu".into(), unix_home: "/home/klpz".into() });
        println!("home = {}", util::home().display());
        for a in adapters::ALL {
            let st = adapters::state(a).unwrap();
            println!("{} installed={} ver={:?} running={} readonly={} cur={:?} providers={:?} catalog={} settings={:?} notes={:?}",
                st.id, st.installed, st.version, st.running, st.readonly, st.current_provider,
                st.providers.iter().map(|p| (&p.id, &p.base_url, p.models.len())).collect::<Vec<_>>(),
                st.catalog.as_ref().map(|c| c.len()).unwrap_or(0),
                st.settings.iter().map(|s| &s.key).collect::<Vec<_>>(), st.notes);
        }
        let l = sessions::list().unwrap();
        println!("sessions={} current={} providers={:?} running={} writable={}", l.sessions.len(), l.current_provider, l.providers, l.codex_running, l.writable);
        let ops: Vec<Op> = serde_json::from_value(serde_json::json!([{"op": "set_setting", "key": "fixed_id", "value": true}])).unwrap();
        let (diff, written, _) = adapters::plan("codex", &ops, true).unwrap();
        assert!(written.is_empty());
        for g in diff.groups { println!("[{}] {:?}", g.file, g.lines.iter().map(|l| &l.text).collect::<Vec<_>>()); }
    }

    /// Read-only: OpenCode / Claude Code as AgentPlus sees them (keys never printed).
    /// `cargo test dump_new_agents -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_new_agents() {
        for a in ["opencode", "claude"] {
            let st = adapters::state(a).unwrap();
            println!("{} installed={} ver={:?} restartable={} readonly={} cur={:?}", st.id, st.installed, st.version, st.restartable, st.readonly, st.current_provider);
            for p in &st.providers {
                println!("   {} [{}] {} builtin={} enabled={} key={} fp={} models={:?}", p.id, p.api, p.base_url.as_deref().unwrap_or("-"), p.builtin, p.enabled, p.has_key, p.key_fp.is_some(), p.models.iter().map(|m| (&m.id, m.visible)).collect::<Vec<_>>());
            }
            println!("   settings={:?} notes={:?}", st.settings.iter().map(|s| (&s.key, &s.value)).collect::<Vec<_>>(), st.notes);
        }
        let ops: Vec<Op> = serde_json::from_value(serde_json::json!([
            {"op": "upsert_provider", "provider": {"name": "Relay CC", "baseUrl": "https://relay.example.com/v1", "api": "anthropic", "apiKey": "sk-ant-test-9999", "models": ["claude-x-1", "claude-y-2"]}}
        ])).unwrap();
        let (d, w, _) = adapters::plan("claude", &ops, true).unwrap();
        assert!(w.is_empty());
        for g in d.groups { println!("[{}] {:?}", g.file, g.lines.iter().map(|l| &l.text).collect::<Vec<_>>()); }
        // Create, switch to it and assign roles in one batch.
        let ops: Vec<Op> = serde_json::from_value(serde_json::json!([
            {"op": "upsert_provider", "provider": {"name": "Relay CC", "baseUrl": "http://127.0.0.1:18650/relay/v1", "api": "anthropic", "apiKey": "agentplus-gateway", "models": ["glm-5", "kimi-k3"]}},
            {"op": "set_current_provider", "provider": "relay-cc"},
            {"op": "set_model_roles", "provider": "relay-cc", "roles": {"default": "glm-5", "haiku": "kimi-k3"}}
        ])).unwrap();
        let (d, w, _) = adapters::plan("claude", &ops, true).unwrap();
        assert!(w.is_empty());
        for g in d.groups { println!("[{}] {:?}", g.file, g.lines.iter().map(|l| &l.text).collect::<Vec<_>>()); }
        let ops: Vec<Op> = serde_json::from_value(serde_json::json!([
            {"op": "upsert_provider", "provider": {"name": "New OC", "baseUrl": "https://n.example.com/v1", "api": "chat", "apiKey": "oc-key-7777", "models": ["m1"]}},
            {"op": "set_provider_enabled", "provider": "gemini-local", "enabled": false},
            {"op": "set_model_visible", "provider": "minew", "model": "mimo-v2.5", "visible": false}
        ])).unwrap();
        let (d, w, _) = adapters::plan("opencode", &ops, true).unwrap();
        assert!(w.is_empty());
        for g in d.groups { println!("[{}] {:?}", g.file, g.lines.iter().map(|l| &l.text).collect::<Vec<_>>()); }
    }

    /// Dry run only (nothing is written): prints the diff each adapter would produce.
    #[test]
    #[ignore]
    fn dry_run_plans() {
        let cases: [(&str, serde_json::Value); 5] = [
            ("codex", serde_json::json!([
                {"op": "set_provider_models", "provider": "klpz", "models": ["gpt-5.5", "my-relay-model"]}
            ])),
            ("codex", serde_json::json!([
                {"op": "set_provider_models", "provider": "klpz", "models": ["gpt-5.5", "my-relay-model"]},
                {"op": "set_current_provider", "provider": "klpz"}
            ])),
            ("codex", serde_json::json!([
                {"op": "set_setting", "key": "fixed_id", "value": true},
                {"op": "upsert_provider", "provider": {"name": "Test Relay", "baseUrl": "https://relay.example.com/v1", "api": "responses", "apiKey": "sk-test-abcd1234"}},
                {"op": "upsert_provider", "provider": {"id": "klpz", "name": "klpz", "baseUrl": "http://64.83.33.80:8080", "api": "responses", "apiKey": null}},
                {"op": "upsert_model", "provider": "*", "model": {"id": "my-model", "name": "My Model", "context": 131072}},
                {"op": "upsert_model", "provider": "*", "model": {"id": "gpt-5.5", "name": "GPT-5.5 (edited)", "context": null}},
                {"op": "set_model_visible", "provider": "*", "model": "gpt-reserve", "visible": true}
            ])),
            ("zcode", serde_json::json!([
                {"op": "import_provider", "fromAgent": "mimo", "provider": "opencode"},
                {"op": "upsert_provider", "provider": {"name": "New ZC", "baseUrl": "https://z.example.com/v1", "api": "chat", "apiKey": "zk-9999", "models": ["m1", "m2"]}},
                {"op": "upsert_provider", "provider": {"id": "minew", "name": "MiMo API", "baseUrl": "https://api.xiaomimimo.com/v1", "api": "chat"}},
                {"op": "upsert_model", "provider": "103-api", "model": {"id": "brand-new", "context": 200000}},
                {"op": "delete_model", "provider": "103-api", "model": "grok-4.3"},
                {"op": "delete_provider", "provider": "bf5a1eb0-a199-447f-9759-859bdc3e37ab"}
            ])),
            ("mimo", serde_json::json!([
                {"op": "upsert_provider", "provider": {"name": "Relay W", "baseUrl": "https://w.example.com/v1", "api": "chat", "apiKey": "wk-5678", "models": ["deepseek-v4-pro"]}},
                {"op": "upsert_model", "provider": "opencode", "model": {"id": "glm-5.3", "name": "GLM 5.3", "context": 200000}},
                {"op": "upsert_model", "provider": "opencode", "model": {"id": "kimi-k3"}},
                {"op": "delete_model", "provider": "opencode", "model": "glm-5.3-flash"}
            ])),
        ];
        for (agent, ops) in cases {
            let ops: Vec<Op> = serde_json::from_value(ops).unwrap();
            let (diff, written, backup) = adapters::plan(agent, &ops, true).unwrap();
            assert!(written.is_empty() && backup.is_none());
            println!("== {agent}");
            for g in diff.groups {
                println!("  [{}]", g.file);
                for l in g.lines {
                    println!("    {} {}", if l.add { "+" } else { "-" }, l.text);
                }
            }
        }
    }
}
