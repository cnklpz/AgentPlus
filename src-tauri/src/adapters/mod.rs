pub mod codex;
pub mod mimo;
pub mod zcode;

use crate::model::{AgentState, Diff, Op};
use crate::process;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub const ALL: [&str; 3] = [codex::ID, zcode::ID, mimo::ID];

pub fn state(agent: &str) -> Result<AgentState> {
    let inst = process::detect(agent);
    Ok(match agent {
        codex::ID => codex::state(&inst),
        zcode::ID => zcode::state(&inst),
        mimo::ID => mimo::state(&inst),
        _ => return Err(anyhow!("未知 Agent {agent}")),
    })
}

pub fn plan(agent: &str, ops: &[Op], dry_run: bool) -> Result<(Diff, Vec<PathBuf>, Option<PathBuf>)> {
    match agent {
        codex::ID => codex::plan(ops, dry_run),
        zcode::ID => zcode::plan(ops, dry_run),
        mimo::ID => mimo::plan(ops, dry_run),
        _ => Err(anyhow!("未知 Agent {agent}")),
    }
}
