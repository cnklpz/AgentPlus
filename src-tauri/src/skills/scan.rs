//! Finding skills in a folder: every directory holding a `SKILL.md` (Agent Skills format:
//! YAML frontmatter with `name` and `description`) is one skill; its subfolders are part of it.

use crate::util::display_path;
use serde::Serialize;
use std::fs;
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
    /// Fingerprint of every file's path and content: equal means the same skill.
    pub sig: String,
    /// What's wrong with its SKILL.md, if anything.
    pub problem: Option<String>,
}

/// Scanning stops after this many folders (a skills root is not a disk).
const MAX_DIRS: usize = 3000;
/// Files counted and hashed per skill; a bigger folder is still listed.
const MAX_FILES: usize = 2000;
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
    let (files, bytes, sig) = content(dir);
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
pub fn content(dir: &Path) -> (usize, u64, String) {
    let mut files = vec![];
    collect(dir, dir, &mut files);
    files.sort();
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut bytes = 0u64;
    for rel in files.iter().take(MAX_FILES) {
        let p = dir.join(rel);
        ctx.update(rel.as_bytes());
        ctx.update(&[0]);
        let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        bytes += len;
        // Big files (images, archives) count by size, not by content.
        if len <= 8 << 20 {
            if let Ok(b) = fs::read(&p) {
                ctx.update(&b);
            }
        } else {
            ctx.update(&len.to_le_bytes());
        }
        ctx.update(&[0]);
    }
    let hex: String = ctx.finish().as_ref().iter().take(6).map(|b| format!("{b:02x}")).collect();
    (files.len(), bytes, hex)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_FILES * 2 {
            return;
        }
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
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
        let (a, b) = (content(&h.0.join("x/a")), content(&h.0.join("y/a")));
        assert_eq!(a, b);
        fs::write(h.0.join("y/a/extra.md"), "more").unwrap();
        assert_ne!(content(&h.0.join("y/a")).2, a.2);
    }
}
