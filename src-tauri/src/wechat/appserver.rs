//! A `codex app-server` child speaking JSON-RPC over stdio: one JSON object per line, no
//! `"jsonrpc"` field. Requests we send get responses by id; the server's notifications and
//! its own requests (approvals, questions) go to the bridge's event channel.

use super::Event;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

type Pending = Arc<Mutex<HashMap<i64, mpsc::Sender<Result<Value, String>>>>>;

pub struct AppServer {
    child: Child,
    stdin: Mutex<ChildStdin>,
    next: AtomicI64,
    pending: Pending,
    alive: Arc<AtomicBool>,
}

/// What a line from the server is.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Response { id: i64, result: Result<Value, String> },
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
}

pub fn classify(line: &str) -> Option<Incoming> {
    let v: Value = serde_json::from_str(line).ok()?;
    let method = v["method"].as_str().map(String::from);
    match (method, v.get("id")) {
        (Some(method), Some(id)) => Some(Incoming::Request { id: id.clone(), method, params: v["params"].clone() }),
        (Some(method), None) => Some(Incoming::Notification { method, params: v["params"].clone() }),
        (None, Some(id)) => {
            let id = id.as_i64()?;
            let result = match v.get("error") {
                Some(e) => Err(e["message"].as_str().map(String::from).unwrap_or_else(|| e.to_string())),
                None => Ok(v["result"].clone()),
            };
            Some(Incoming::Response { id, result })
        }
        (None, None) => None,
    }
}

impl AppServer {
    /// Starts `exe app-server`, completes the handshake and forwards what it sends to `events`.
    pub fn spawn(exe: &Path, events: mpsc::Sender<Event>, generation: u64) -> Result<AppServer> {
        let mut cmd = Command::new(exe);
        cmd.arg("app-server")
            .env("CODEX_HOME", crate::adapters::codex::codex_home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::with_login_path(crate::process::no_window(&mut cmd));
        let mut child = cmd.spawn().map_err(|e| anyhow!(tr!("Couldn't start Codex ({e})", "无法启动 Codex（{e}）")))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        // Drained so a chatty server can't fill the pipe and stall.
        std::thread::spawn(move || {
            let mut sink = [0u8; 4096];
            while matches!(stderr.read(&mut sink), Ok(n) if n > 0) {}
        });
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (pending, alive) = (pending.clone(), alive.clone());
            std::thread::Builder::new().name("agentplus-wechat-codex".into()).spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match classify(&line) {
                        Some(Incoming::Response { id, result }) => {
                            if let Some(tx) = crate::util::lock(&pending).remove(&id) {
                                let _ = tx.send(result);
                            }
                        }
                        Some(Incoming::Request { id, method, params }) => {
                            let _ = events.send(Event::CodexRequest { generation, id, method, params });
                        }
                        Some(Incoming::Notification { method, params }) => {
                            let _ = events.send(Event::CodexNotify { generation, method, params });
                        }
                        None => {}
                    }
                }
                alive.store(false, Ordering::SeqCst);
                // Wake whoever still waits for an answer instead of leaving the bridge blocked
                // until each request's timeout.
                let waiting = std::mem::take(&mut *crate::util::lock(&pending));
                for tx in waiting.into_values() {
                    let _ = tx.send(Err(crate::i18n::l("Codex stopped", "Codex 已退出").into()));
                }
                let _ = events.send(Event::CodexExited { generation });
            })?;
        }
        let server = AppServer { child, stdin: Mutex::new(stdin), next: AtomicI64::new(1), pending, alive };
        server.request(
            "initialize",
            json!({
                "clientInfo": { "name": "agentplus", "title": "AgentPlus", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": false, "requestAttestation": false },
            }),
            Duration::from_secs(30),
        )?;
        server.write(&json!({ "method": "initialized" }))?;
        Ok(server)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn write(&self, v: &Value) -> Result<()> {
        let mut w = crate::util::lock(&self.stdin);
        w.write_all(format!("{v}\n").as_bytes())?;
        w.flush()?;
        Ok(())
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.alive() {
            bail!("{}", crate::i18n::l("Codex stopped", "Codex 已退出"));
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        crate::util::lock(&self.pending).insert(id, tx);
        if let Err(e) = self.write(&json!({ "id": id, "method": method, "params": params })) {
            crate::util::lock(&self.pending).remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => bail!("{method}: {e}"),
            Err(_) => {
                crate::util::lock(&self.pending).remove(&id);
                bail!("{}", tr!("{method}: no answer from Codex", "{method}：Codex 没有响应"))
            }
        }
    }

    pub fn respond(&self, id: &Value, result: Value) -> Result<()> {
        self.write(&json!({ "id": id, "result": result }))
    }

    pub fn respond_error(&self, id: &Value, message: &str) -> Result<()> {
        self.write(&json!({ "id": id, "error": { "code": -32601, "message": message } }))
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_lines() {
        assert_eq!(classify(r#"{"id":1,"result":{"ok":true}}"#), Some(Incoming::Response { id: 1, result: Ok(json!({ "ok": true })) }));
        assert_eq!(classify(r#"{"id":2,"error":{"code":1,"message":"bad"}}"#), Some(Incoming::Response { id: 2, result: Err("bad".into()) }));
        assert_eq!(
            classify(r#"{"id":"r1","method":"item/commandExecution/requestApproval","params":{"threadId":"t"}}"#),
            Some(Incoming::Request { id: json!("r1"), method: "item/commandExecution/requestApproval".into(), params: json!({ "threadId": "t" }) })
        );
        assert_eq!(
            classify(r#"{"method":"turn/started","params":{"threadId":"t"}}"#),
            Some(Incoming::Notification { method: "turn/started".into(), params: json!({ "threadId": "t" }) })
        );
        assert_eq!(classify("not json"), None);
        assert_eq!(classify("{}"), None);
    }

    /// Checks local CLI detection and the real bridge handshake without a model request:
    /// `cargo test real_codex_start -- --ignored`.
    #[test]
    #[ignore]
    fn real_codex_start() {
        let _home = crate::util::TestHome::new("codex-start");
        std::fs::create_dir_all(crate::adapters::codex::codex_home()).unwrap();
        let exes = crate::process::codex_clis();
        assert!(!exes.is_empty(), "no installed Codex CLI found");
        let (tx, _rx) = mpsc::channel();
        let mut failures = vec![];
        let (exe, s) = exes
            .iter()
            .find_map(|exe| match AppServer::spawn(exe, tx.clone(), 1) {
                Ok(s) => Some((exe, s)),
                Err(e) => {
                    let failure = format!("{}: {e:#}", exe.display());
                    println!("handshake failed via {failure}");
                    failures.push(failure);
                    None
                }
            })
            .unwrap_or_else(|| panic!("no Codex CLI completed the handshake: {}", failures.join("; ")));
        println!("Codex app-server via {}", exe.display());
        let threads = s.request("thread/list", json!({ "limit": 1 }), Duration::from_secs(30)).unwrap();
        assert!(threads["data"].as_array().is_some_and(Vec::is_empty), "expected no sessions in the isolated home");
        assert!(s.alive());
    }

    /// Runs one real turn (one model request) in a throwaway session:
    /// `CODEX_EXE=<codex.exe> cargo test real_codex_turn -- --ignored`.
    #[test]
    #[ignore]
    fn real_codex_turn() {
        let exe = std::env::var("CODEX_EXE").expect("CODEX_EXE");
        let (tx, rx) = mpsc::channel();
        let s = AppServer::spawn(Path::new(&exe), tx, 1).unwrap();
        let dir = std::env::temp_dir();
        let t = s.request("thread/start", json!({ "cwd": dir, "ephemeral": true }), Duration::from_secs(60)).unwrap();
        let thread = t["thread"]["id"].as_str().unwrap().to_string();
        let input = json!([{ "type": "text", "text": "Reply with exactly: pong", "text_elements": [] }]);
        s.request("turn/start", json!({ "threadId": thread, "input": input }), Duration::from_secs(60)).unwrap();
        let mut answer = String::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(180)).unwrap() {
                Event::CodexNotify { method, params, .. } if method == "item/completed" && params["item"]["type"] == "agentMessage" => {
                    answer = params["item"]["text"].as_str().unwrap_or("").to_string();
                }
                Event::CodexNotify { method, params, .. } if method == "turn/completed" => {
                    assert_eq!(params["turn"]["status"], "completed", "{params}");
                    break;
                }
                Event::CodexRequest { method, .. } => panic!("unexpected request {method}"),
                Event::CodexExited { .. } => panic!("codex exited"),
                _ => {}
            }
        }
        assert!(answer.to_lowercase().contains("pong"), "{answer}");
    }
}
