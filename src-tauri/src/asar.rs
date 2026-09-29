//! Read-only access to an Electron `app.asar` archive, for metadata of packages an Electron
//! app ships inside it (the DeepSeek Harness desktop app's bundled plugins).
//!
//! Layout: a u32 pickle size (4), the header pickle's size, then the header pickle (payload
//! size, string length, JSON). The JSON is a tree of `{ "files": {…} }` folders and
//! `{ "size", "offset" }` files; file data starts right after the header pickle, and files
//! marked `unpacked` live in `<archive>.unpacked/` instead, and `{ "link" }` entries point at
//! another path of the archive.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    files: Option<BTreeMap<String, Node>>,
    #[serde(default)]
    size: u64,
    /// A decimal string (it can exceed JavaScript's safe integers).
    #[serde(default)]
    offset: Option<String>,
    #[serde(default)]
    unpacked: bool,
    /// A symlink: the archive-relative path it points to.
    #[serde(default)]
    link: Option<String>,
}

pub struct Archive {
    path: PathBuf,
    /// Where file data starts.
    base: u64,
    root: Node,
}

/// Files larger than this are refused (only small metadata files are read).
const MAX_READ: u64 = 16 << 20;
/// Links followed for one lookup before giving up (a loop).
const MAX_LINKS: u8 = 16;

impl Archive {
    pub fn open(path: &Path) -> Result<Archive> {
        let mut f = std::fs::File::open(path)?;
        let mut head = [0u8; 16];
        f.read_exact(&mut head)?;
        let word = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap()) as u64;
        let (header_size, json_len) = (word(4), word(12));
        if word(0) != 4 || json_len + 8 > header_size || 8 + header_size > f.metadata()?.len() {
            return Err(anyhow!(tr!("{} isn't an asar archive", "{} 不是 asar 归档", path.display())));
        }
        let mut json = vec![0u8; json_len as usize];
        f.read_exact(&mut json)?;
        let root: Node = serde_json::from_slice(&json)?;
        Ok(Archive { path: path.to_path_buf(), base: 8 + header_size, root })
    }

    /// The entry at `rel` (`/`-separated, relative to the archive root) with the path it really
    /// lives at, after following links.
    fn resolve(&self, rel: &str, links: u8) -> Option<(Vec<String>, &Node)> {
        let mut path: Vec<String> = vec![];
        let mut node = &self.root;
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            let child = node.files.as_ref()?.get(part)?;
            match &child.link {
                Some(target) => {
                    (path, node) = self.resolve(target, links.checked_sub(1)?)?;
                }
                None => {
                    path.push(part.to_string());
                    node = child;
                }
            }
        }
        Some((path, node))
    }

    fn node(&self, rel: &str) -> Option<&Node> {
        self.resolve(rel, MAX_LINKS).map(|(_, n)| n)
    }

    pub fn is_dir(&self, rel: &str) -> bool {
        self.node(rel).is_some_and(|n| n.files.is_some())
    }

    pub fn is_file(&self, rel: &str) -> bool {
        self.node(rel).is_some_and(|n| n.files.is_none())
    }

    /// Names in folder `rel`, sorted; empty when it isn't a folder.
    pub fn read_dir(&self, rel: &str) -> Vec<String> {
        self.node(rel).and_then(|n| n.files.as_ref()).map(|f| f.keys().cloned().collect()).unwrap_or_default()
    }

    /// The contents of file `rel`.
    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let (real, n) = self.resolve(rel, MAX_LINKS)?;
        if n.files.is_some() || n.size > MAX_READ {
            return None;
        }
        if n.unpacked {
            let mut p = self.path.as_os_str().to_owned();
            p.push(".unpacked");
            return std::fs::read(real.iter().fold(PathBuf::from(p), |d, s| d.join(s))).ok();
        }
        let offset: u64 = n.offset.as_deref()?.parse().ok()?;
        let mut f = std::fs::File::open(&self.path).ok()?;
        f.seek(SeekFrom::Start(self.base.checked_add(offset)?)).ok()?;
        let mut buf = vec![0u8; n.size as usize];
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    pub fn read_string(&self, rel: &str) -> Option<String> {
        String::from_utf8(self.read(rel)?).ok()
    }
}

/// Writes an archive holding `files` (path, contents) and `links` (path, target); paths listed
/// in `unpacked` go to the `.unpacked` folder next to it instead, as electron-builder does.
#[cfg(test)]
pub(crate) fn write_test_archive(path: &Path, files: &[(&str, &str)], links: &[(&str, &str)], unpacked: &[&str]) {
    use serde_json::{json, Value as J};
    fn slot<'a>(root: &'a mut J, parts: &[&str]) -> &'a mut J {
        let mut dir = root;
        for p in &parts[..parts.len() - 1] {
            dir = dir["files"].as_object_mut().unwrap().entry(p.to_string()).or_insert_with(|| json!({ "files": {} }));
        }
        dir
    }
    let mut root = json!({ "files": {} });
    let mut data: Vec<u8> = vec![];
    for (rel, target) in links {
        let parts: Vec<&str> = rel.split('/').collect();
        slot(&mut root, &parts)["files"][parts[parts.len() - 1]] = json!({ "link": target });
    }
    for (rel, text) in files {
        let parts: Vec<&str> = rel.split('/').collect();
        let dir = slot(&mut root, &parts);
        let entry: J = if unpacked.contains(rel) {
            let out = parts.iter().fold(PathBuf::from(format!("{}.unpacked", path.display())), |d, s| d.join(s));
            std::fs::create_dir_all(out.parent().unwrap()).unwrap();
            std::fs::write(out, text).unwrap();
            json!({ "size": text.len(), "unpacked": true })
        } else {
            let e = json!({ "size": text.len(), "offset": data.len().to_string() });
            data.extend_from_slice(text.as_bytes());
            e
        };
        dir["files"][parts[parts.len() - 1]] = entry;
    }
    let json = serde_json::to_vec(&root).unwrap();
    // The pickle pads the string to 4 bytes.
    let padded = json.len().div_ceil(4) * 4;
    let mut out: Vec<u8> = vec![];
    for w in [4, padded + 8, padded + 4, json.len()] {
        out.extend_from_slice(&(w as u32).to_le_bytes());
    }
    out.extend_from_slice(&json);
    out.resize(16 + padded, 0);
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_packed_and_unpacked_files() {
        let home = crate::util::TestHome::new("asar");
        let path = home.0.join("app.asar");
        write_test_archive(&path, &[("dsh/package.json", "{\"name\":\"x\"}"), ("dsh/a/b.txt", "héllo"), ("dsh/native.node", "bin")], &[], &["dsh/native.node"]);
        let a = Archive::open(&path).unwrap();
        assert_eq!(a.read_dir("dsh"), vec!["a", "native.node", "package.json"]);
        assert!(a.is_dir("dsh/a") && !a.is_file("dsh/a"));
        assert!(a.is_file("dsh/a/b.txt") && !a.is_dir("dsh/a/b.txt"));
        assert_eq!(a.read_string("dsh/package.json").as_deref(), Some("{\"name\":\"x\"}"));
        assert_eq!(a.read_string("dsh/a/b.txt").as_deref(), Some("héllo"));
        assert_eq!(a.read_string("dsh/native.node").as_deref(), Some("bin"));
        // Missing paths and folders read as nothing.
        assert!(a.read("dsh/missing").is_none() && a.read("dsh/a").is_none());
        assert!(a.read_dir("dsh/missing").is_empty() && a.read_dir("dsh/package.json").is_empty());
    }

    #[test]
    fn follows_links() {
        let home = crate::util::TestHome::new("asar-links");
        let path = home.0.join("app.asar");
        write_test_archive(
            &path,
            &[("store/pkg/package.json", "{}"), ("store/pkg/data.bin", "raw"), ("store/pkg/sub/x.txt", "x")],
            &[("dsh/node_modules/pkg", "store/pkg"), ("loop/a", "loop/b"), ("loop/b", "loop/a")],
            &["store/pkg/data.bin"],
        );
        let a = Archive::open(&path).unwrap();
        // A linked folder lists, reads and nests like its target, packed or unpacked.
        assert!(a.is_dir("dsh/node_modules/pkg") && !a.is_file("dsh/node_modules/pkg"));
        assert_eq!(a.read_dir("dsh/node_modules/pkg"), vec!["data.bin", "package.json", "sub"]);
        assert_eq!(a.read_string("dsh/node_modules/pkg/package.json").as_deref(), Some("{}"));
        assert_eq!(a.read_string("dsh/node_modules/pkg/data.bin").as_deref(), Some("raw"));
        assert!(a.is_file("dsh/node_modules/pkg/sub/x.txt"));
        // Dangling and looping links read as nothing.
        assert!(a.node("loop/a").is_none() && !a.is_file("loop/a") && !a.is_dir("loop/a"));
    }

    #[test]
    fn refuses_other_files() {
        let home = crate::util::TestHome::new("asar-bad");
        let path = home.0.join("x.asar");
        std::fs::write(&path, b"PK\x03\x04 not an archive at all").unwrap();
        assert!(Archive::open(&path).is_err());
        std::fs::write(&path, b"\x04\0\0").unwrap();
        assert!(Archive::open(&path).is_err());
    }
}
