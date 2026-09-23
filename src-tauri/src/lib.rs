mod adapters;
mod cdp;
mod env;
mod history;
mod library;
mod model;
mod net;
mod process;
mod sessions;
mod store;
mod sync;
mod util;

use model::*;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn list_agents() -> Vec<AgentState> {
    adapters::ALL.iter().filter_map(|a| adapters::state(a).ok()).collect()
}

#[tauri::command]
fn get_agent(agent: String) -> Result<AgentState, String> {
    adapters::state(&agent).map_err(err)
}

#[tauri::command]
fn preview(agent: String, ops: Vec<Op>) -> Result<Vec<DiffGroup>, String> {
    adapters::plan(&agent, &ops, true).map(|(d, ..)| d.groups).map_err(err)
}

#[tauri::command]
fn apply(agent: String, ops: Vec<Op>) -> Result<ApplyResult, String> {
    let (_, files, backup) = adapters::plan(&agent, &ops, false).map_err(err)?;
    Ok(ApplyResult {
        state: adapters::state(&agent).map_err(err)?,
        files: files.iter().map(|f| util::display_path(f)).collect(),
        backup_dir: backup.map(|b| b.to_string_lossy().to_string()),
    })
}

#[tauri::command]
async fn test_latency(url: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || net::latency(&url))
        .await
        .map_err(|e| e.to_string())?
}

/// Restarts an agent. For Codex with Fast injection on, starts it with a local
/// DevTools port and patches the UI once it is up.
#[tauri::command]
async fn restart_agent(agent: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let inject = agent == adapters::codex::ID && adapters::codex::fast_inject_enabled();
        let args = if inject { format!("--remote-debugging-port={}", cdp::PORT) } else { String::new() };
        process::restart(&agent, &args).map_err(err)?;
        if inject {
            cdp::inject(cdp::PORT).map_err(err)
        } else {
            Ok("已重启".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Runs a blocking closure off the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(err)
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
    tauri::async_runtime::spawn_blocking(move || {
        let (base, key, api) = adapters::provider_endpoint(&agent, &provider).map_err(err)?;
        net::list_models(&base, key.as_deref(), &api)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Model ids for a provider being added (key typed in the form).
#[tauri::command]
async fn fetch_models_url(base_url: String, api_key: Option<String>, api: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || net::list_models(&base_url, api_key.as_deref(), &api))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_backups() -> Result<Vec<history::BackupEntry>, String> {
    blocking(history::list).await
}

#[tauri::command]
async fn restore_backup(id: String) -> Result<String, String> {
    blocking(move || history::restore(&id)).await
}

#[tauri::command]
fn sync_status() -> sync::SyncStatus {
    sync::status()
}

#[tauri::command]
fn sync_set_folder(path: String) -> Result<(), String> {
    sync::set_folder(&path).map_err(err)
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
    adapters::codex::dismiss_fixed_prompt().map_err(err)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    process::open_dir(&path).map_err(err)
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
    tauri::async_runtime::spawn_blocking(move || env::set(&id)).await.map_err(|e| e.to_string())?.map_err(err)
}

#[tauri::command]
async fn detect_agents() -> Vec<adapters::Detect> {
    tauri::async_runtime::spawn_blocking(adapters::detect_all).await.unwrap_or_default()
}

#[tauri::command]
fn set_agent_dir(agent: String, path: Option<String>) -> Result<(), String> {
    adapters::set_dir(&agent, path.as_deref()).map_err(err)
}

#[tauri::command]
fn library_list() -> Vec<library::LibEntry> {
    library::list()
}

#[tauri::command]
fn library_save(input: library::LibInput) -> Result<library::LibEntry, String> {
    library::save(input).map_err(err)
}

#[tauri::command]
fn library_delete(id: String) -> Result<(), String> {
    library::delete(&id).map_err(err)
}

#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    let d = util::agentplus_dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    process::open_dir(&d.to_string_lossy()).map_err(err)
}

#[tauri::command]
fn open_config_dir(agent: String) -> Result<(), String> {
    let st = adapters::state(&agent).map_err(err)?;
    process::open_dir(&st.config_dir).map_err(err)
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

    /// Dry run only (nothing is written): prints the diff each adapter would produce.
    #[test]
    #[ignore]
    fn dry_run_plans() {
        let cases: [(&str, serde_json::Value); 3] = [
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_agents,
            get_agent,
            preview,
            apply,
            test_latency,
            restart_agent,
            open_config_dir,
            codex_sessions,
            codex_health,
            codex_cleanup_preview,
            codex_cleanup,
            codex_repair,
            codex_undo_repair,
            reveal_path,
            fetch_models,
            fetch_models_url,
            list_backups,
            restore_backup,
            sync_status,
            sync_set_folder,
            sync_export,
            sync_preview,
            open_path,
            codex_dismiss_fixed_prompt,
            list_envs,
            set_env,
            library_list,
            library_save,
            library_delete,
            open_data_dir,
            detect_agents,
            set_agent_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentPlus");
}
