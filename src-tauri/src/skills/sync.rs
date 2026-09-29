//! The skill library in the sync file: each skill as its files (base64), with its fingerprint
//! to compare against this device's copy. Skills over `MAX_BYTES` stay out of the file.

use super::import::Scratch;
use super::write::{copy, Copied};
use super::{content, library_dir, scan};
use crate::i18n::l;
use crate::util::display_path;
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path};

/// A skill bigger than this isn't synced (a sync file should stay small enough to upload).
pub const MAX_BYTES: u64 = 5 << 20;
const SIG_VERSION: u64 = 2;
type Files = BTreeMap<String, Vec<u8>>;

/// Read one bounded snapshot. Its fingerprint covers exactly the bytes being exported.
fn files(dir: &Path) -> Result<Option<Files>> {
    let mut out = Files::new();
    let mut bytes = 0;
    for rel in scan::files(dir)? {
        let mut data = vec![];
        fs::File::open(dir.join(&rel))?.take(MAX_BYTES - bytes + 1).read_to_end(&mut data)?;
        bytes += data.len() as u64;
        if bytes > MAX_BYTES {
            return Ok(None);
        }
        out.insert(rel, data);
    }
    Ok(Some(out))
}

/// Version 1 covered at most 2000 files and used only the length of files over 8 MiB.
/// Used only to validate old snapshots; comparisons always use the full fingerprint.
fn digest(files: &Files, legacy: bool) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for (path, data) in files.iter().take(if legacy { 2000 } else { usize::MAX }) {
        ctx.update(path.as_bytes());
        ctx.update(&[0]);
        if legacy && data.len() > 8 << 20 {
            ctx.update(&(data.len() as u64).to_le_bytes());
        } else {
            ctx.update(data);
        }
        ctx.update(&[0]);
    }
    ctx.finish().as_ref().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// The library for the sync file: (entries, names left out for their size). Any read failure
/// aborts the export so it cannot overwrite a complete snapshot with a partial library.
pub fn export() -> Result<(Vec<Value>, Vec<String>)> {
    let (mut out, mut skipped) = (vec![], vec![]);
    let root = library_dir();
    let rd = match fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((out, skipped)),
        Err(e) => return Err(e).with_context(|| tr!("Can't read the skill library: {}", "无法读取技能库：{}", display_path(&root))),
    };
    let mut dirs = vec![];
    for entry in rd {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with('.') && fs::metadata(entry.path())?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    for dir in dirs {
        match fs::metadata(dir.join("SKILL.md")) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| tr!("Can't read skill {}", "无法读取技能 {}", display_path(&dir))),
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Some(files) = files(&dir).with_context(|| tr!("Can't read skill \"{name}\"", "无法读取技能「{name}」"))? else {
            skipped.push(name);
            continue;
        };
        let md = files.iter().find(|(path, _)| path.as_str() == "SKILL.md" || cfg!(windows) && path.eq_ignore_ascii_case("SKILL.md"))
            .map(|(_, data)| data).ok_or_else(|| anyhow!(tr!("Skill \"{name}\" has no readable SKILL.md", "技能「{name}」缺少可读取的 SKILL.md")))?;
        let description = std::str::from_utf8(md).ok().and_then(|s| scan::frontmatter(s).ok()).and_then(|(_, d)| d).unwrap_or_default();
        let sig = digest(&files, false);
        let list: Vec<_> = files.into_iter().map(|(path, data)| json!({ "path": path, "data": B64.encode(data) })).collect();
        out.push(json!({ "name": name, "description": crate::util::clip(&description, 600), "sig": sig, "sigVersion": SIG_VERSION, "files": list }));
    }
    Ok((out, skipped))
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

/// Normalize separators before validation: backslashes are ordinary filename characters
/// on Unix, but become path traversal or an absolute path when converted afterwards.
fn relative_path(raw: &str) -> Option<String> {
    let rel = raw.replace('\\', "/");
    safe(&rel).then_some(rel)
}

fn checked_files(entry: &Value) -> Result<(Files, String)> {
    let damaged = || anyhow!(l("The sync file's skill entry is damaged", "同步文件里的技能条目已损坏"));
    let mut files = Files::new();
    for f in entry["files"].as_array().ok_or_else(damaged)? {
        let rel = f["path"].as_str().and_then(relative_path).ok_or_else(damaged)?;
        let data = B64.decode(f["data"].as_str().ok_or_else(damaged)?)?;
        if files.insert(rel, data).is_some() {
            return Err(damaged());
        }
    }
    let sig = digest(&files, false);
    let expected = entry["sig"].as_str().ok_or_else(damaged)?;
    let valid = match entry.get("sigVersion") {
        // Unversioned entries include both the old truncated algorithm and early full hashes.
        None => expected == sig || expected == digest(&files, true),
        Some(v) if v.as_u64() == Some(SIG_VERSION) => expected == sig,
        _ => bail!("{}", l("Unsupported skill fingerprint version", "不支持的技能指纹版本")),
    };
    if !valid {
        let name = entry["name"].as_str().unwrap_or_default();
        bail!("{}", tr!("The synced skill \"{name}\" doesn't match its fingerprint", "同步来的技能「{name}」和它的指纹对不上"));
    }
    Ok((files, sig))
}

/// Full fingerprint of a validated snapshot, including snapshots written with the old hash.
pub fn fingerprint(entry: &Value) -> Result<String> {
    checked_files(entry).map(|(_, sig)| sig)
}

/// Writes one exported skill into the library (another version is backed up and replaced).
pub fn write(entry: &Value) -> Result<Copied> {
    let name = entry["name"].as_str().filter(|n| safe(n) && !n.contains(['/', '\\'])).ok_or_else(|| anyhow!(l("The sync file's skill entry is damaged", "同步文件里的技能条目已损坏")))?;
    let (files, sig) = checked_files(entry)?;
    let scratch = Scratch::new()?;
    let dir = scratch.0.join(name);
    for (rel, data) in files {
        let to = dir.join(rel);
        if !to.starts_with(&dir) {
            bail!("{}", l("The sync file's skill entry is damaged", "同步文件里的技能条目已损坏"));
        }
        fs::create_dir_all(to.parent().unwrap())?;
        fs::write(&to, data)?;
    }
    if !dir.join("SKILL.md").is_file() {
        bail!("{}", tr!("The synced skill \"{name}\" has no SKILL.md", "同步来的技能「{name}」缺少 SKILL.md"));
    }
    // What arrives is what was exported.
    if content(&dir)?.2 != sig {
        bail!("{}", tr!("The synced skill \"{name}\" doesn't match its fingerprint", "同步来的技能「{name}」和它的指纹对不上"));
    }
    copy(&display_path(&dir), &display_path(&library_dir()), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    #[test]
    fn paths_are_validated_after_normalizing_separators() {
        for raw in ["", ".", "..", "../outside.txt", r"..\outside.txt", r"scripts\..\..\outside.txt", "/outside.txt", r"\outside.txt", r"\\server\share\outside.txt"] {
            assert_eq!(relative_path(raw), None, "{raw:?}");
        }
        assert_eq!(relative_path(r"scripts\run.py").as_deref(), Some("scripts/run.py"));
        assert_eq!(relative_path("scripts/run.py").as_deref(), Some("scripts/run.py"));
    }

    #[test]
    fn invalid_snapshot_paths_cannot_write_outside_the_skill() {
        let _h = TestHome::new("skills-sync-traversal");
        let root = crate::util::agentplus_dir();
        fs::create_dir_all(&root).unwrap();
        let marker = root.join("outside.txt");
        fs::write(&marker, "keep").unwrap();
        let absolute = marker.to_string_lossy().replace('/', "\\");
        for raw in [r"..\..\..\outside.txt", r"scripts\..\..\..\..\outside.txt", absolute.as_str()] {
            // Give the attack a matching fingerprint: rejection must come from the path
            // check, before staging any bytes, rather than the final on-disk hash check.
            let files = Files::from([
                ("SKILL.md".into(), b"---\nname: pdf\ndescription: PDF\n---\n".to_vec()),
                (raw.replace('\\', "/"), b"overwrite".to_vec()),
            ]);
            let list: Vec<_> = files.iter().map(|(p, data)| json!({ "path": if p == "SKILL.md" { p.as_str() } else { raw }, "data": B64.encode(data) })).collect();
            let entry = json!({ "name": "pdf", "sigVersion": 2, "sig": digest(&files, false), "files": list });
            assert!(write(&entry).is_err(), "{raw:?}");
            assert_eq!(fs::read_to_string(&marker).unwrap(), "keep");
            assert!(!root.join("tmp").exists(), "invalid entries must fail before staging");
            assert!(!library_dir().exists());
        }
    }

    #[test]
    fn valid_windows_separators_round_trip_and_aliases_are_rejected() {
        let _h = TestHome::new("skills-sync-separators");
        let files = Files::from([
            ("SKILL.md".into(), b"---\nname: pdf\ndescription: PDF\n---\n".to_vec()),
            ("scripts/run.py".into(), b"print(1)".to_vec()),
        ]);
        let list: Vec<_> = files.iter().map(|(path, data)| json!({ "path": path.replace('/', "\\"), "data": B64.encode(data) })).collect();
        let mut entry = json!({ "name": "pdf", "sigVersion": 2, "sig": digest(&files, false), "files": list });
        assert_eq!(write(&entry).unwrap(), Copied::Added);
        assert_eq!(fs::read_to_string(library_dir().join("pdf/scripts/run.py")).unwrap(), "print(1)");
        for name in [r"nested\pdf", "nested/pdf"] {
            let mut bad = entry.clone();
            bad["name"] = json!(name);
            assert!(write(&bad).is_err());
        }
        entry["files"].as_array_mut().unwrap().push(json!({ "path": "scripts/run.py", "data": B64.encode("print(1)") }));
        assert!(write(&entry).is_err());
    }

    #[test]
    fn library_skills_round_trip_through_the_sync_file() {
        let _h = TestHome::new("skills-sync");
        let d = library_dir().join("pdf");
        fs::create_dir_all(d.join("scripts")).unwrap();
        fs::write(d.join("SKILL.md"), "---\nname: pdf\ndescription: PDFs\n---\n").unwrap();
        fs::write(d.join("scripts/run.py"), [0u8, 159, 146, 150]).unwrap();
        let (out, skipped) = export().unwrap();
        assert!(skipped.is_empty());
        assert_eq!(out[0]["files"].as_array().unwrap().len(), 2);
        assert_eq!(out[0]["sigVersion"], 2);
        assert_eq!(out[0]["sig"], content(&d).unwrap().2);
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
        let mut unversioned = out[0].clone();
        unversioned.as_object_mut().unwrap().shift_remove("sigVersion");
        assert_eq!(write(&unversioned).unwrap(), Copied::Same);
        let mut unsupported = out[0].clone();
        unsupported["sigVersion"] = json!(3);
        assert!(write(&unsupported).is_err());
        let mut duplicate = out[0].clone();
        duplicate["files"].as_array_mut().unwrap().push(out[0]["files"][0].clone());
        assert!(write(&duplicate).is_err());
    }

    #[test]
    fn a_missing_library_is_empty_but_an_unreadable_library_is_an_error() {
        let _h = TestHome::new("skills-sync-library-error");
        let root = library_dir();
        assert!(export().unwrap().0.is_empty());
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "not a directory").unwrap();
        assert!(export().is_err());
    }

    #[test]
    fn oversized_skills_are_reported_without_exporting_partial_files() {
        let _h = TestHome::new("skills-sync-size");
        let dir = library_dir().join("large");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: large\ndescription: Large\n---\n").unwrap();
        fs::File::create(dir.join("large.bin")).unwrap().set_len(MAX_BYTES).unwrap();
        let (out, skipped) = export().unwrap();
        assert!(out.is_empty());
        assert_eq!(skipped, ["large"]);
    }

    #[cfg(windows)]
    #[test]
    fn skill_markdown_keeps_its_original_filename_case() {
        let _h = TestHome::new("skills-sync-filename-case");
        let dir = library_dir().join("pdf");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("skill.md"), "---\nname: pdf\ndescription: PDF tools\n---\n").unwrap();
        let (out, _) = export().unwrap();
        assert_eq!(out[0]["description"], "PDF tools");
        assert_eq!(out[0]["files"][0]["path"], "skill.md");
        assert_eq!(write(&out[0]).unwrap(), Copied::Same);
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_skills_abort_export() {
        use std::os::windows::fs::OpenOptionsExt;
        let _h = TestHome::new("skills-sync-read-error");
        let d = library_dir().join("pdf");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), "---\nname: pdf\ndescription: PDF\n---\n").unwrap();
        fs::write(d.join("locked.txt"), "private").unwrap();
        let _lock = fs::OpenOptions::new().read(true).share_mode(0).open(d.join("locked.txt")).unwrap();
        assert!(export().is_err());
        assert!(local()[0].1.is_empty());
    }
}
