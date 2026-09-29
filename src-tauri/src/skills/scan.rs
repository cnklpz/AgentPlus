//! Finding skills in a folder: every directory holding a `SKILL.md` (Agent Skills format:
//! YAML frontmatter with `name` and `description`) is one skill; its subfolders are part of it.

use crate::util::display_path;
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::Path;

/// One skill folder.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillCopy {
    /// Its path inside the root, `/`-separated ("frontend-design", "creative/ascii-art").
    pub id: String,
    /// The frontmatter `name`, else the folder name.
    pub name: String,
    pub description: String,
    /// Where it is (display path).
    pub dir: String,
    pub files: usize,
    pub bytes: u64,
    /// Fingerprint of every file's path and content; empty if files couldn't be read.
    pub sig: String,
    /// What's wrong with its SKILL.md or files, if anything.
    pub problem: Option<String>,
}

/// Scanning stops after this many folders (a skills root is not a disk).
const MAX_DIRS: usize = 3000;
const MAX_SKILL_MD: u64 = 1 << 20;

/// The skills under `root`, looking `depth` levels down (Hermes keeps them in category
/// folders). Hidden folders are skipped: they are caches and system copies.
pub fn scan(root: &Path, depth: usize) -> Vec<SkillCopy> {
    let mut out = vec![];
    let mut seen = 0usize;
    walk(root, root, depth, &mut out, &mut seen);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<SkillCopy>, seen: &mut usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        *seen += 1;
        if *seen > MAX_DIRS {
            return;
        }
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        // Follows links (skills are often linked in), but not into hidden folders.
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file() {
            out.push(read(root, &path));
        } else if depth > 1 {
            walk(root, &path, depth - 1, out, seen);
        }
    }
}

/// `name` and `description` from a SKILL.md's frontmatter.
pub fn frontmatter(text: &str) -> Result<(Option<String>, Option<String>), String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("no-frontmatter".into());
    }
    let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim_end() != "---").collect();
    let y: serde_yaml::Value = serde_yaml::from_str(&body.join("\n")).map_err(|e| e.to_string())?;
    let s = |k: &str| y.get(k).and_then(|v| v.as_str()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Ok((s("name"), s("description")))
}

fn read(root: &Path, dir: &Path) -> SkillCopy {
    let id = dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let md = dir.join("SKILL.md");
    let (mut name, mut description, mut problem) = (None, None, None);
    match fs::metadata(&md).map(|m| m.len()) {
        Ok(n) if n > MAX_SKILL_MD => problem = Some(crate::i18n::l("SKILL.md is too big to read", "SKILL.md 太大，没有读取").to_string()),
        _ => match fs::read_to_string(&md) {
            Err(e) => problem = Some(tr!("Can't read SKILL.md: {e}", "读不了 SKILL.md：{e}")),
            Ok(text) => match frontmatter(&text) {
                Ok((n, d)) => {
                    (name, description) = (n, d);
                    if description.is_none() {
                        problem = Some(crate::i18n::l("SKILL.md has no description", "SKILL.md 缺少 description").to_string());
                    }
                }
                Err(e) if e == "no-frontmatter" => problem = Some(crate::i18n::l("SKILL.md has no frontmatter (---)", "SKILL.md 缺少开头的 frontmatter（---）").to_string()),
                Err(e) => problem = Some(tr!("SKILL.md's frontmatter can't be read: {e}", "SKILL.md 的 frontmatter 无法解析：{e}")),
            },
        },
    }
    let (files, bytes, sig) = content(dir).unwrap_or_else(|e| {
        problem = Some(tr!("Can't read skill files: {e}", "读不了技能文件：{e}"));
        (0, 0, String::new())
    });
    SkillCopy {
        id,
        name: name.unwrap_or(folder),
        description: crate::util::clip(&description.unwrap_or_default(), 600),
        dir: display_path(dir),
        files,
        bytes,
        sig,
        problem,
    }
}

/// (files, bytes, fingerprint) of a skill folder: every file's relative path and content, in
/// path order, so two copies compare equal wherever they are.
pub fn content(dir: &Path) -> Result<(usize, u64, String)> {
    let files = files(dir)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut bytes = 0u64;
    let mut buf = [0u8; 64 * 1024];
    for rel in &files {
        let p = dir.join(rel);
        ctx.update(rel.as_bytes());
        ctx.update(&[0]);
        let mut file = fs::File::open(&p).with_context(|| tr!("Failed to read {}", "读取 {} 失败", p.display()))?;
        loop {
            let n = file.read(&mut buf).with_context(|| tr!("Failed to read {}", "读取 {} 失败", p.display()))?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            ctx.update(&buf[..n]);
        }
        ctx.update(&[0]);
    }
    let hex: String = ctx.finish().as_ref().iter().take(6).map(|b| format!("{b:02x}")).collect();
    Ok((files.len(), bytes, hex))
}

/// The files of a skill folder (relative, `/`-separated, sorted): what copying, fingerprinting
/// and syncing a skill take. Links are not followed out of the folder: a linked file counts
/// only when it points inside the skill, and linked folders are skipped (they could point
/// anywhere, or loop). Otherwise a skill from the web linking to `~/.ssh/id_ed25519` would
/// carry that file into the library and the sync folder.
pub fn files(dir: &Path) -> Result<Vec<String>> {
    let mut out = vec![];
    let root = dir.canonicalize()?;
    collect(dir, &root, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect(root: &Path, real_root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let rd = fs::read_dir(dir).with_context(|| tr!("Failed to read {}", "读取 {} 失败", dir.display()))?;
    for e in rd {
        let e = e?;
        let ft = e.file_type()?;
        let p = e.path();
        let file = if ft.is_symlink() {
            // A link to a file inside the skill; anything else is left out.
            p.canonicalize().is_ok_and(|t| t.starts_with(real_root) && t.is_file())
        } else if ft.is_dir() {
            collect(root, real_root, &p, out)?;
            false
        } else {
            ft.is_file()
        };
        if file {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

/// Copies skill folder `from` to `to` (created), file by file as [`files`] lists them.
pub fn copy_skill(from: &Path, to: &Path) -> anyhow::Result<()> {
    let files = files(from)?;
    fs::create_dir_all(to)?;
    for rel in files {
        let (src, dst) = (from.join(&rel), to.join(&rel));
        if let Some(d) = dst.parent() {
            fs::create_dir_all(d)?;
        }
        fs::copy(&src, &dst).with_context(|| tr!("Failed to copy {}", "复制 {} 失败", src.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;

    fn skill(root: &Path, id: &str, md: &str) {
        let d = root.join(id);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), md).unwrap();
    }

    #[test]
    fn finds_skills_and_reads_their_frontmatter() {
        let h = TestHome::new("skills-scan");
        let root = h.0.join("skills");
        skill(&root, "pdf", "---\nname: pdf\ndescription: Read and write PDFs\n---\n# PDF\n");
        fs::write(root.join("pdf/helper.py"), "print(1)").unwrap();
        skill(&root, "creative/ascii-art", "---\nname: ascii-art\ndescription: >\n  Draw with\n  letters\n---\n");
        skill(&root, "bare", "# no frontmatter\n");
        skill(&root, ".system/hidden", "---\nname: hidden\ndescription: x\n---\n");
        fs::create_dir_all(root.join("empty")).unwrap();
        let found = scan(&root, 2);
        let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["bare", "creative/ascii-art", "pdf"]);
        let pdf = &found[2];
        assert_eq!((pdf.name.as_str(), pdf.description.as_str(), pdf.files, pdf.problem.is_none()), ("pdf", "Read and write PDFs", 2, true));
        assert_eq!(found[1].description, "Draw with letters");
        assert_eq!((found[0].name.as_str(), found[0].problem.as_deref()), ("bare", Some("SKILL.md 缺少开头的 frontmatter（---）")));
        // One level down only: the category folder hides it.
        assert_eq!(scan(&root, 1).len(), 2);
    }

    #[test]
    fn copies_with_the_same_files_share_a_fingerprint() {
        let h = TestHome::new("skills-sig");
        let md = "---\nname: a\ndescription: d\n---\n";
        skill(&h.0.join("x"), "a", md);
        skill(&h.0.join("y"), "a", md);
        let (a, b) = (content(&h.0.join("x/a")).unwrap(), content(&h.0.join("y/a")).unwrap());
        assert_eq!(a, b);
        fs::write(h.0.join("y/a/extra.md"), "more").unwrap();
        assert_ne!(content(&h.0.join("y/a")).unwrap().2, a.2);
    }

    #[test]
    fn all_files_are_copied_and_hashed_in_large_skills() {
        let h = TestHome::new("skills-many-files");
        let dir = h.0.join("src");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..4002 {
            fs::write(dir.join(format!("{i:04}.txt")), "a").unwrap();
        }
        let before = content(&dir).unwrap();
        assert_eq!((before.0, before.1), (4002, 4002));
        let to = h.0.join("copy");
        copy_skill(&dir, &to).unwrap();
        assert_eq!(content(&to).unwrap(), before);
        fs::write(dir.join("4001.txt"), "b").unwrap();
        assert_ne!(content(&dir).unwrap().2, before.2);
    }

    #[test]
    fn large_files_are_hashed_by_content_and_read_failures_are_errors() {
        let h = TestHome::new("skills-large-file");
        let mut data = vec![0; (8 << 20) + 1];
        let path = h.0.join("large.bin");
        fs::write(&path, &data).unwrap();
        let before = content(&h.0).unwrap();
        data[8 << 20] = 1;
        fs::write(&path, &data).unwrap();
        let after = content(&h.0).unwrap();
        assert_eq!(before.1, after.1);
        assert_ne!(before.2, after.2);
        assert!(content(&path).is_err());
        assert!(files(&h.0.join("missing")).is_err());
        assert!(copy_skill(&h.0.join("missing"), &h.0.join("copy")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_files_never_produce_a_successful_fingerprint_or_copy() {
        use std::os::windows::fs::OpenOptionsExt;
        let h = TestHome::new("skills-read-error");
        skill(&h.0, "pdf", "---\nname: pdf\ndescription: PDF\n---\n");
        let dir = h.0.join("pdf");
        fs::write(dir.join("locked.txt"), "private").unwrap();
        let _lock = fs::OpenOptions::new().read(true).share_mode(0).open(dir.join("locked.txt")).unwrap();
        assert!(content(&dir).is_err());
        let found = scan(&h.0, 1);
        assert!(found[0].sig.is_empty() && found[0].problem.is_some());
        assert!(copy_skill(&dir, &h.0.join("copy")).is_err());
    }

    #[cfg(unix)]
    fn link(target: &Path, at: &Path, _dir: bool) -> bool {
        std::os::unix::fs::symlink(target, at).is_ok()
    }

    /// Windows needs Developer Mode (or admin) for symbolic links; a folder falls back to a
    /// junction, which any user can make (and which reads as a link too).
    #[cfg(windows)]
    fn link(target: &Path, at: &Path, dir: bool) -> bool {
        if !dir {
            return std::os::windows::fs::symlink_file(target, at).is_ok();
        }
        let win = |p: &Path| p.to_string_lossy().replace('/', "\\");
        std::os::windows::fs::symlink_dir(target, at).is_ok()
            || std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(win(at)).arg(win(target)).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// Backups and moves (`util::copy_dir`) don't follow a linked folder either.
    #[test]
    fn backups_leave_linked_folders_out() {
        let h = TestHome::new("copy-dir-links");
        let big = h.0.join("big");
        fs::create_dir_all(&big).unwrap();
        fs::write(big.join("data.bin"), "x").unwrap();
        let d = h.0.join("skill");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), "s").unwrap();
        if !link(&big, &d.join("cache"), true) {
            return;
        }
        let out = h.0.join("copy");
        crate::util::copy_dir(&d, &out).unwrap();
        assert!(out.join("SKILL.md").is_file());
        assert!(!out.join("cache").exists(), "the linked folder isn't copied");
    }

    #[test]
    fn linked_folders_out_of_a_skill_are_not_followed() {
        let h = TestHome::new("skills-dir-links");
        let secret = h.0.join(".ssh");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("id_ed25519"), "PRIVATE").unwrap();
        skill(&h.0.join("repo"), "evil", "---\nname: evil\ndescription: x\n---\n");
        let d = h.0.join("repo/evil");
        assert!(link(&secret, &d.join("ssh"), true));
        assert!(d.join("ssh/id_ed25519").is_file(), "the link works");
        assert_eq!(files(&d).unwrap(), ["SKILL.md"]);
        copy_skill(&d, &h.0.join("lib/evil")).unwrap();
        assert!(!h.0.join("lib/evil/ssh").exists());
    }

    #[test]
    fn links_out_of_a_skill_are_not_followed() {
        let h = TestHome::new("skills-links");
        let secret = h.0.join(".ssh");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("id_ed25519"), "PRIVATE").unwrap();
        let d = h.0.join("repo/evil");
        skill(&h.0.join("repo"), "evil", "---\nname: evil\ndescription: x\n---\n");
        fs::write(d.join("inside.md"), "ok").unwrap();
        if !link(&secret.join("id_ed25519"), &d.join("key"), false) {
            return;
        }
        assert!(link(&secret, &d.join("ssh"), true));
        assert!(link(&d.join("inside.md"), &d.join("alias.md"), false));
        assert_eq!(files(&d).unwrap(), ["SKILL.md", "alias.md", "inside.md"]);
        let to = h.0.join("lib/evil");
        copy_skill(&d, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("alias.md")).unwrap(), "ok");
        assert!(!to.join("key").exists() && !to.join("ssh").exists());
        assert_eq!(content(&d).unwrap(), content(&to).unwrap(), "the fingerprint covers what a copy holds");
    }
}
