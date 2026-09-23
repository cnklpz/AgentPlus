mod adapters;
mod cdp;
mod model;
mod net;
mod process;
mod store;
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

    /// Dry run only (nothing is written): prints the diff each adapter would produce.
    #[test]
    #[ignore]
    fn dry_run_plans() {
        let cases: [(&str, serde_json::Value); 3] = [
            ("codex", serde_json::json!([
                {"op": "set_current_provider", "provider": "klpz"},
                {"op": "set_setting", "key": "fast_default", "value": true},
                {"op": "set_setting", "key": "fast_inject", "value": true},
                {"op": "set_setting", "key": "fast_cli", "value": true},
                {"op": "set_setting", "key": "efforts", "value": ["low", "medium", "high", "xhigh"]},
                {"op": "set_model_visible", "provider": "*", "model": "gpt-reserve", "visible": true}
            ])),
            ("zcode", serde_json::json!([
                {"op": "set_provider_enabled", "provider": "326b8058-b38d-41da-bac9-757f74f44106", "enabled": true},
                {"op": "set_model_visible", "provider": "103-api", "model": "grok-4.3", "visible": false},
                {"op": "set_setting", "key": "memoryEnabled", "value": false}
            ])),
            ("mimo", serde_json::json!([
                {"op": "set_model_visible", "provider": "opencode", "model": "glm-5.3", "visible": false},
                {"op": "set_provider_enabled", "provider": "opencode", "enabled": false},
                {"op": "set_setting", "key": "skills", "value": ["~/.agents", "~/.codex"]}
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
            open_config_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentPlus");
}
