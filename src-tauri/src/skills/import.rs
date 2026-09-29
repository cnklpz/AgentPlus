//! Importing skills into the skill library: from a folder (one skill, or a folder of skills),
//! a .zip file (unpacked with the system's `tar`, which refuses paths outside the target), or
//! a Git repository (a shallow clone with the `git` on PATH, prompts and exotic protocols off).

use super::write::{copy, Copied};
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
    let t0 = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let mut err = String::new();
            use std::io::Read;
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            bail!("{}", crate::util::clip(err.trim(), 600));
        }
        if t0.elapsed() > limit {
            let _ = child.kill();
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
    if let Some((repo, rest)) = s.split_once("/tree/").or_else(|| s.split_once("/-/tree/")) {
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
    let base = if let Some((url, branch, sub)) = git_source(source) {
        scratch = Scratch::new()?;
        let repo = scratch.0.join("repo");
        clone(&url, branch.as_deref(), &repo)?;
        match sub {
            Some(p) => repo.join(p),
            None => repo,
        }
    } else {
        let p = crate::env::resolve_path(source);
        if p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            scratch = Scratch::new()?;
            unzip(&p, &scratch.0)?;
            scratch.0.clone()
        } else if p.is_dir() {
            p
        } else {
            bail!("{}", tr!("Not a folder, .zip file or Git address: {source}", "不是文件夹、.zip 文件或 Git 地址：{source}"));
        }
    };
    let found = skills_in(&base);
    if found.is_empty() {
        bail!("{}", l("No skills found (folders with a SKILL.md)", "没有找到技能（带 SKILL.md 的文件夹）"));
    }
    let lib = display_path(&library_dir());
    let mut out = vec![];
    for dir in found {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let description = std::fs::read_to_string(dir.join("SKILL.md")).ok().and_then(|t| scan::frontmatter(&t).ok()).and_then(|(_, d)| d).unwrap_or_default();
        let result = copy(&display_path(&dir), &lib, replace.contains(&name))?;
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
    fn git_addresses() {
        assert_eq!(git_source("https://github.com/anthropics/skills"), Some(("https://github.com/anthropics/skills".into(), None, None)));
        assert_eq!(
            git_source("https://github.com/anthropics/skills/tree/main/document-skills/pdf/"),
            Some(("https://github.com/anthropics/skills.git".into(), Some("main".into()), Some("document-skills/pdf".into())))
        );
        assert_eq!(git_source("D:\\skills"), None);
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
