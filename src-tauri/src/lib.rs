#[macro_use]
mod i18n;
mod adapters;
mod cdp;
mod dotenv;
mod env;
mod gateway;
mod history;
mod library;
mod model;
mod mfields;
mod net;
mod official;
mod process;
mod projects;
mod sessions;
mod store;
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

#[tauri::command]
async fn list_agents() -> Result<Vec<AgentState>, String> {
    blocking(|| Ok(adapters::ALL.iter().filter_map(|a| adapters::state(a).ok()).collect())).await
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
/// it is up. Each step is reported on `on_progress` while it runs.
#[tauri::command]
async fn restart_agent(agent: String, on_progress: tauri::ipc::Channel<process::Progress>) -> Result<String, String> {
    process::reset_cancel();
    blocking(move || {
        let report = |p: process::Progress| {
            let _ = on_progress.send(p);
        };
        let patches = if agent == adapters::codex::ID { adapters::codex::ui_patches() } else { cdp::Patches::default() };
        let inject = patches.any();
        let args = if inject { format!("--remote-debugging-port={}", cdp::PORT) } else { String::new() };
        let mut steps = vec!["stop", "start"];
        if inject {
            steps.extend(["port", "patch"]);
        }
        report(process::Progress::Plan { steps });
        let r = process::restart(&agent, &args, &report)?;
        let mut msg = if inject {
            cdp::inject(cdp::PORT, patches, &report)?
        } else if r.was_running {
            i18n::l("已重启", "Restarted").into()
        } else {
            i18n::l("已启动", "Started").into()
        };
        if r.cli_sessions > 0 {
            let n = r.cli_sessions;
            msg.push_str(&tr!("；终端里的 {n} 个 CLI 会话要重新打开才会生效", "; reopen the {n} CLI session(s) in terminals for the change to apply"));
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

/// Whether an agent's app is running right now.
#[tauri::command]
async fn agent_running(agent: String) -> Result<bool, String> {
    blocking(move || Ok(process::running(&agent))).await
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

/// Model ids straight from a library entry's upstream (used while an agent goes through the gateway).
#[tauri::command]
async fn fetch_models_lib(id: String) -> Result<Vec<String>, String> {
    blocking(move || {
        let (base, key, api) = adapters::provider_endpoint(library::FROM, &id)?;
        net::list_models(&base, key.as_deref(), &api).map_err(anyhow::Error::msg)
    })
    .await
}

/// Model ids for a provider being added (key typed in the form).
#[tauri::command]
async fn fetch_models_url(base_url: String, api_key: Option<String>, api: String) -> Result<Vec<String>, String> {
    blocking(move || net::list_models(&base_url, api_key.as_deref(), &api).map_err(anyhow::Error::msg)).await
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
    blocking(move || history::restore(&id)).await
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
async fn sync_preview() -> Result<Vec<sync::Suggestion>, String> {
    blocking(sync::preview_import).await
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
        return Err(i18n::l("只能打开 https 链接", "Only https links can be opened").into());
    }
    process::open_dir(&url).map_err(err)
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
            anyhow::bail!("{}", i18n::l("网关没有运行", "The gateway is not running"));
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
}

/// Quit from the window's close button (the tray menu quits on its own).
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
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
async fn pick_folder(window: tauri::WebviewWindow, start: Option<String>) -> Result<Option<String>, String> {
    #[cfg(windows)]
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(windows))]
    let owner = { let _ = window; 0 };
    blocking(move || projects::pick_folder(owner, start.as_deref())).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first: a second launch (e.g. while this one sits in the tray) exits
        // right away and brings this window forward instead of starting a second gateway.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Pending::default())
        .setup(|app| {
            tray::setup(app.handle())?;
            // Off the startup path: binding the port and stopping an old listener can wait.
            std::thread::spawn(gateway::server::autostart);
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
            fetch_models_lib,
            list_backups,
            backup_detail,
            restore_backup,
            sync_status,
            sync_set_folder,
            sync_export,
            sync_preview,
            open_path,
            open_url,
            codex_dismiss_fixed_prompt,
            list_envs,
            set_env,
            library_list,
            library_save,
            library_delete,
            open_data_dir,
            set_locale,
            quit_app,
            detect_agents,
            test_provider,
            codex_official_status,
            codex_official_start,
            codex_official_finish,
            codex_official_cancel,
            gateway_status,
            gateway_set,
            gateway_save_route,
            gateway_delete_route,
            gateway_set_breaker,
            gateway_reset_breaker,
            gateway_test,
            set_agent_dir,
            update::update_check,
            update::update_install
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentPlus");
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
