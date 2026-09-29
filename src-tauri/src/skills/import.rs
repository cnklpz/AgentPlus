//! Importing skills into the skill library: from a folder (one skill, or a folder of skills),
//! a .zip file (unpacked with the system's `tar`, which refuses paths outside the target), or
//! a Git repository (a shallow clone with the `git` on PATH, prompts and exotic protocols off).

use super::write::{copy_named, safe_name, Copied};
use super::{library_dir, scan};
use crate::i18n::l;
use crate::util::{agentplus_dir, display_path};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// The skill's folder name (its name in the library).
    pub name: String,
    pub description: String,
    pub result: Copied,
}

/// At most this many skills per import.
const MAX_SKILLS: usize = 200;
const CLONE_TIMEOUT: Duration = Duration::from_secs(180);

/// A scratch folder under `~/.agentplus/tmp`, removed when dropped.
pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    pub(super) fn new() -> Result<Scratch> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let p = agentplus_dir().join("tmp").join(format!("skill-import-{nanos}"));
        std::fs::create_dir_all(&p)?;
        Ok(Scratch(p))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs a command to the end, or kills it after `limit`. Its stderr on failure.
fn run(mut cmd: Command, limit: Duration) -> Result<()> {
    crate::process::no_window(&mut cmd);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    // Read while it runs: a child writing more than the pipe holds would block until killed.
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut err = Vec::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_end(&mut err);
        }
        String::from_utf8_lossy(&err).into_owned()
    });
    let t0 = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let err = reader.join().unwrap_or_default();
            bail!("{}", crate::util::clip(err.trim(), 600));
        }
        if t0.elapsed() > limit {
            // With what it started (git runs git-remote-https), so nothing keeps the scratch folder.
            crate::process::kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            bail!("{}", l("It took too long and was stopped", "耗时太久，已停止"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A Git address: plain, or a GitHub / GitLab page URL with `/tree/<branch>/<path>`.
/// (clone URL, branch, path inside the repository)
fn git_source(s: &str) -> Option<(String, Option<String>, Option<String>)> {
    let s = s.trim().trim_end_matches('/');
    let is_git = s.starts_with("https://") || s.starts_with("http://") || s.starts_with("git@") || s.starts_with("ssh://");
    if !is_git {
        return None;
    }
    if let Some((repo, rest)) = s.split_once("/-/tree/").or_else(|| s.split_once("/tree/")) {
        let mut parts = rest.splitn(2, '/');
        let branch = parts.next().map(String::from).filter(|b| !b.is_empty());
        let path = parts.next().map(String::from).filter(|p| !p.is_empty());
        return Some((format!("{}.git", repo.trim_end_matches(".git")), branch, path));
    }
    Some((s.to_string(), None, None))
}

fn clone(url: &str, branch: Option<&str>, to: &Path) -> Result<()> {
    let git = crate::process::on_path(&["git.exe"]).ok_or_else(|| anyhow!(l("Git isn't installed (or not on PATH); download the repository as a .zip instead", "没有找到 Git（或不在 PATH 里），可以改为下载仓库的 .zip 导入")))?;
    let mut cmd = Command::new(git);
    cmd.args(["clone", "--depth", "1", "--quiet"]);
    if let Some(b) = branch {
        cmd.args(["--branch", b]);
    }
    cmd.arg("--").arg(url).arg(to);
    // Never wait for a password prompt; no ext:: or file:: transports.
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_ALLOW_PROTOCOL", "https:http:ssh:git").env("GCM_INTERACTIVE", "never");
    run(cmd, CLONE_TIMEOUT).map_err(|e| anyhow!(tr!("Couldn't clone {url}: {e}", "克隆 {url} 失败：{e}")))
}

fn unzip(zip: &Path, to: &Path) -> Result<()> {
    // Windows 10+ ships bsdtar as System32\tar.exe; Git's GNU tar can't read zips.
    let tar = if cfg!(windows) { std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe")).unwrap_or_else(|| PathBuf::from("tar.exe")) } else { PathBuf::from("tar") };
    let mut cmd = Command::new(tar);
    cmd.arg("-xf").arg(zip).arg("-C").arg(to);
    run(cmd, Duration::from_secs(120)).map_err(|e| anyhow!(tr!("Couldn't unpack {}: {e}", "解压 {} 失败：{e}", display_path(zip))))
}

/// The skill folders in `base`: itself when it is one, else the ones below it.
fn skills_in(base: &Path) -> Vec<PathBuf> {
    if base.join("SKILL.md").is_file() {
        return vec![base.to_path_buf()];
    }
    scan::scan(base, 4).into_iter().map(|s| crate::env::resolve_path(&s.dir)).take(MAX_SKILLS).collect()
}

/// Imports the skills at `source` (a folder, a .zip or a Git address) into the library.
/// Skills whose names are in `replace` overwrite another version already there.
pub fn import(source: &str, replace: &[String]) -> Result<Vec<Imported>> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() {
        bail!("{}", l("Enter a folder, a .zip file or a Git address", "请填写文件夹、.zip 文件或 Git 地址"));
    }
    let scratch;
    let mut root_name = None;
    let base = if let Some((url, branch, sub)) = git_source(source) {
        scratch = Scratch::new()?;
        let repo = scratch.0.join("repo");
        clone(&url, branch.as_deref(), &repo)?;
        // Keep clone metadata outside the skill; its contents change on each import.
        // Moving also avoids deleting read-only Git object files before the import.
        std::fs::rename(repo.join(".git"), scratch.0.join("git"))?;
        match sub {
            Some(p) => repo.join(p),
            None => {
                root_name = Some(url.rsplit(['/', ':']).next().unwrap_or_default().trim_end_matches(".git").to_string());
                repo
            }
        }
    } else {
        let p = crate::env::resolve_path(source);
        if p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            root_name = p.file_stem().map(|n| n.to_string_lossy().to_string());
            scratch = Scratch::new()?;
            unzip(&p, &scratch.0)?;
            scratch.0.clone()
        } else if p.is_dir() {
            p
        } else {
            bail!("{}", tr!("Not a folder, .zip file or Git address: {source}", "不是文件夹、.zip 文件或 Git 地址：{source}"));
        }
    };
    import_base(&base, root_name.as_deref(), replace)
}

fn import_base(base: &Path, root_name: Option<&str>, replace: &[String]) -> Result<Vec<Imported>> {
    let found = skills_in(base);
    if found.is_empty() {
        bail!("{}", l("No skills found (folders with a SKILL.md)", "没有找到技能（带 SKILL.md 的文件夹）"));
    }
    let lib = display_path(&library_dir());
    let mut out = vec![];
    for dir in found {
        let (declared, description) = std::fs::read_to_string(dir.join("SKILL.md")).ok().and_then(|t| scan::frontmatter(&t).ok()).unwrap_or_default();
        let named = if dir == base {
            root_name.map(|fallback| declared.as_deref().filter(|n| safe_name(n)).unwrap_or(fallback))
        } else {
            None
        };
        let name = named.map(String::from).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let description = description.unwrap_or_default();
        let result = copy_named(&display_path(&dir), &lib, replace.contains(&name), named)?;
        out.push(Imported { name, description: crate::util::clip(&description, 300), result });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TestHome;
    use std::fs;

    fn skill(dir: &Path, name: &str, body: &str) {
        let d = dir.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), format!("---\nname: {name}\ndescription: {body}\n---\n")).unwrap();
    }

    #[test]
    fn a_child_with_lots_of_errors_does_not_hang() {
        let h = TestHome::new("skills-import-stderr");
        let big = h.0.join("big.txt");
        fs::write(&big, "error line\n".repeat(20_000)).unwrap();
        #[cfg(windows)]
        let cmd = {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.raw_arg(format!("/c type \"{}\" 1>&2 & exit /b 3", big.display()));
            c
        };
        #[cfg(not(windows))]
        let cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(format!("cat '{}' >&2; exit 3", big.display()));
            c
        };
        let t0 = Instant::now();
        let err = run(cmd, Duration::from_secs(20)).unwrap_err().to_string();
        assert!(t0.elapsed() < Duration::from_secs(15), "blocked on a full pipe");
        assert!(err.starts_with("error line"), "{err}");
    }

    #[test]
    fn git_addresses() {
        assert_eq!(git_source("https://github.com/anthropics/skills"), Some(("https://github.com/anthropics/skills".into(), None, None)));
        assert_eq!(
            git_source("https://github.com/anthropics/skills/tree/main/document-skills/pdf/"),
            Some(("https://github.com/anthropics/skills.git".into(), Some("main".into()), Some("document-skills/pdf".into())))
        );
        assert_eq!(git_source("D:\\skills"), None);
        assert_eq!(git_source("https://gitlab.com/team/skills/-/tree/main/pdf/"), Some(("https://gitlab.com/team/skills.git".into(), Some("main".into()), Some("pdf".into()))));
        assert_eq!(git_source("https://gitlab.com/team/nested/skills/-/tree/main"), Some(("https://gitlab.com/team/nested/skills.git".into(), Some("main".into()), None)));
    }

    #[test]
    fn root_repository_skills_have_stable_safe_names() {
        let h = TestHome::new("skills-root-repo");
        let repo = h.0.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("SKILL.md"), "---\nname: pdf\ndescription: PDF\n---\n").unwrap();
        let got = import_base(&repo, Some("document-tools"), &[]).unwrap();
        assert_eq!((got[0].name.as_str(), got[0].result), ("pdf", Copied::Added));
        assert_eq!(import_base(&repo, Some("document-tools"), &[]).unwrap()[0].result, Copied::Same);
        for name in ["../escape", "a/b", "a\\b", "C:escape", "NUL", "con.txt", "LPT1", "COM¹", ".hidden", "trailing."] {
            assert!(!safe_name(name), "{name}");
            fs::write(repo.join("SKILL.md"), format!("---\nname: '{name}'\ndescription: PDF\n---\n")).unwrap();
            assert_eq!(import_base(&repo, Some("document-tools"), &[]).unwrap()[0].name, "document-tools");
            assert!(import_base(&repo, Some("../unsafe"), &[]).is_err());
        }
        fs::write(repo.join("SKILL.md"), "# No frontmatter\n").unwrap();
        assert_eq!(import_base(&repo, Some("second-repository"), &[]).unwrap()[0].name, "second-repository");
        assert!(!library_dir().join("repo").exists());
        assert!(!h.0.join("escape").exists());
        // Ordinary source folders retain their existing names.
        assert_eq!(import_base(&repo, None, &[]).unwrap()[0].name, "repo");
    }

    #[cfg(windows)]
    #[test]
    fn root_zip_imports_deduplicate_and_replace_by_the_stable_name() {
        let h = TestHome::new("skills-root-zip");
        let src = h.0.join("src");
        fs::create_dir_all(&src).unwrap();
        let zip = h.0.join("archive-name.zip");
        let pack = |body: &str| {
            fs::write(src.join("SKILL.md"), body).unwrap();
            let tar = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/tar.exe");
            assert!(Command::new(tar).arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&src).arg("SKILL.md").status().unwrap().success());
        };
        pack("---\nname: pdf\ndescription: v1\n---\n");
        let got = import(&zip.to_string_lossy(), &[]).unwrap();
        assert_eq!((got[0].name.as_str(), got[0].result), ("pdf", Copied::Added));
        assert_eq!(import(&zip.to_string_lossy(), &[]).unwrap()[0].result, Copied::Same);
        pack("---\nname: pdf\ndescription: v2\n---\n");
        assert_eq!(import(&zip.to_string_lossy(), &[]).unwrap()[0].result, Copied::Exists);
        assert_eq!(import(&zip.to_string_lossy(), &["pdf".into()]).unwrap()[0].result, Copied::Replaced);
        assert_eq!(fs::read_dir(library_dir()).unwrap().count(), 1);
        pack("# No name\n");
        assert_eq!(import(&zip.to_string_lossy(), &[]).unwrap()[0].name, "archive-name");
    }

    #[test]
    fn folders_import_one_or_many_and_ask_before_replacing() {
        let h = TestHome::new("skills-import");
        let src = h.0.join("download");
        skill(&src, "pdf", "PDFs");
        skill(&src.join("more"), "xlsx", "Sheets");
        let got = import(&src.to_string_lossy(), &[]).unwrap();
        // Sorted by path: "more/xlsx" before "pdf".
        assert_eq!(got.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["xlsx", "pdf"]);
        assert!(got.iter().all(|i| i.result == Copied::Added));
        assert_eq!(got[1].description, "PDFs");
        assert!(library_dir().join("pdf/SKILL.md").is_file() && library_dir().join("xlsx/SKILL.md").is_file());
        // Once more: the same; then a changed one waits for a yes.
        assert!(import(&src.join("pdf").to_string_lossy(), &[]).unwrap().iter().all(|i| i.result == Copied::Same));
        skill(&src, "pdf", "PDFs v2");
        assert_eq!(import(&src.join("pdf").to_string_lossy(), &[]).unwrap()[0].result, Copied::Exists);
        assert_eq!(import(&src.join("pdf").to_string_lossy(), &["pdf".into()]).unwrap()[0].result, Copied::Replaced);
        assert_eq!(import(&h.0.join("nothing").to_string_lossy(), &[]).unwrap_err().to_string(), format!("不是文件夹、.zip 文件或 Git 地址：{}", h.0.join("nothing").to_string_lossy()));
        fs::create_dir_all(h.0.join("empty")).unwrap();
        assert_eq!(import(&h.0.join("empty").to_string_lossy(), &[]).unwrap_err().to_string(), "没有找到技能（带 SKILL.md 的文件夹）");
    }

    #[cfg(windows)]
    #[test]
    fn zip_files_unpack() {
        let h = TestHome::new("skills-import-zip");
        let src = h.0.join("src");
        skill(&src, "notes", "Notes");
        let zip = h.0.join("notes.zip");
        // Built with the same tar: `-a` picks the format from the name.
        let tar = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("tar.exe");
        let ok = Command::new(tar).arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&src).arg("notes").status().unwrap().success();
        assert!(ok);
        let got = import(&zip.to_string_lossy(), &[]).unwrap();
        assert_eq!((got[0].name.as_str(), &got[0].result), ("notes", &Copied::Added));
        assert!(library_dir().join("notes/SKILL.md").is_file());
    }
}
