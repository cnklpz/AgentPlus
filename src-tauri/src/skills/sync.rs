//! The skill library in the sync file: each skill as its files (base64), with its fingerprint
//! to compare against this device's copy. Skills over `MAX_BYTES` stay out of the file.

use super::import::Scratch;
use super::write::{copy, Copied};
use super::{content, library_dir, scan};
use crate::i18n::l;
use crate::util::display_path;
use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path};

/// A skill bigger than this isn't synced (a sync file should stay small enough to upload).
pub const MAX_BYTES: u64 = 5 << 20;

fn files(dir: &Path, root: &Path, out: &mut Vec<Value>) -> Result<()> {
    for e in fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, root, out)?;
        } else {
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            out.push(json!({ "path": rel, "data": B64.encode(fs::read(&p)?) }));
        }
    }
    Ok(())
}

/// The library for the sync file: (entries, names left out for their size).
pub fn export() -> (Vec<Value>, Vec<String>) {
    let (mut out, mut skipped) = (vec![], vec![]);
    for s in scan::scan(&library_dir(), 1) {
        let dir = library_dir().join(&s.id);
        if s.bytes > MAX_BYTES {
            skipped.push(s.id);
            continue;
        }
        let mut list = vec![];
        if files(&dir, &dir, &mut list).is_ok() {
            out.push(json!({ "name": s.id, "description": s.description, "sig": s.sig, "files": list }));
        }
    }
    (out, skipped)
}

/// This device's library: (folder name, fingerprint).
pub fn local() -> Vec<(String, String)> {
    scan::scan(&library_dir(), 1).into_iter().map(|s| (s.id, s.sig)).collect()
}

/// A relative path inside the skill, never outside it.
fn safe(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Writes one exported skill into the library (another version is backed up and replaced).
pub fn write(entry: &Value) -> Result<Copied> {
    let name = entry["name"].as_str().filter(|n| safe(n) && !n.contains('/')).ok_or_else(|| anyhow!(l("The sync file's skill entry is damaged", "同步文件里的技能条目已损坏")))?;
    let scratch = Scratch::new()?;
    let dir = scratch.0.join(name);
    for f in entry["files"].as_array().into_iter().flatten() {
        let rel = f["path"].as_str().filter(|r| safe(r)).ok_or_else(|| anyhow!(l("The sync file's skill entry is damaged", "同步文件里的技能条目已损坏")))?;
        let data = B64.decode(f["data"].as_str().unwrap_or_default())?;
        let to = dir.join(rel);
        fs::create_dir_all(to.parent().unwrap())?;
        fs::write(&to, data)?;
    }
    if !dir.join("SKILL.md").is_file() {
        bail!("{}", tr!("The synced skill \"{name}\" has no SKILL.md", "同步来的技能「{name}」缺少 SKILL.md"));
    }
    // What arrives is what was exported.
    if entry["sig"].as_str().is_some_and(|s| s != content(&dir).2) {
        bail!("{}", tr!("The synced skill \"{name}\" doesn't match its fingerprint", "同步来的技能「{name}」和它的指纹对不上"));
    }
    copy(&display_path(&dir), &display_path(&library_dir()), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    #[test]
    fn library_skills_round_trip_through_the_sync_file() {
        let _h = TestHome::new("skills-sync");
        let d = library_dir().join("pdf");
        fs::create_dir_all(d.join("scripts")).unwrap();
        fs::write(d.join("SKILL.md"), "---\nname: pdf\ndescription: PDFs\n---\n").unwrap();
        fs::write(d.join("scripts/run.py"), [0u8, 159, 146, 150]).unwrap();
        let (out, skipped) = export();
        assert!(skipped.is_empty());
        assert_eq!(out[0]["files"].as_array().unwrap().len(), 2);
        fs::remove_dir_all(&d).unwrap();
        assert_eq!(write(&out[0]).unwrap(), Copied::Added);
        assert_eq!(fs::read(d.join("scripts/run.py")).unwrap(), [0u8, 159, 146, 150]);
        assert_eq!(local(), [("pdf".to_string(), out[0]["sig"].as_str().unwrap().to_string())]);
        // Paths that climb out, or a tampered file, are refused.
        let mut bad = out[0].clone();
        bad["files"][0]["path"] = json!("../evil.txt");
        assert!(write(&bad).is_err());
        let mut tampered = out[0].clone();
        tampered["files"][1]["data"] = json!(B64.encode("changed"));
        assert!(write(&tampered).is_err());
    }
}
