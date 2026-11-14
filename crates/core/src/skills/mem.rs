// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An in-memory [`SkillFs`] for tests, with folders, files and links at
//! absolute paths. Making a path makes its parent folders.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{normalize, Node, SkillFs};

/// How many links one lookup follows before it gives up, as Linux does.
const MAX_HOPS: usize = 40;

#[derive(Clone, Debug)]
enum Mem {
    Dir,
    File { bytes: Vec<u8>, executable: bool },
    Link(String),
}

/// Folders, files and links held in memory.
#[derive(Clone, Debug, Default)]
pub struct MemFs {
    nodes: BTreeMap<PathBuf, Mem>,
}

impl MemFs {
    /// An empty file system with only `/`.
    pub fn new() -> Self {
        let mut fs = Self::default();
        fs.nodes.insert(PathBuf::from("/"), Mem::Dir);
        fs
    }

    /// Makes a folder.
    pub fn dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.put(path.as_ref(), Mem::Dir)
    }

    /// Makes a file that isn't executable.
    pub fn file(&mut self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> &mut Self {
        let bytes = bytes.as_ref().to_vec();
        self.put(
            path.as_ref(),
            Mem::File {
                bytes,
                executable: false,
            },
        )
    }

    /// Makes an executable file.
    pub fn exec(&mut self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> &mut Self {
        let bytes = bytes.as_ref().to_vec();
        self.put(
            path.as_ref(),
            Mem::File {
                bytes,
                executable: true,
            },
        )
    }

    /// Makes a link at `path` pointing at `target`.
    pub fn link(&mut self, path: impl AsRef<Path>, target: &str) -> &mut Self {
        self.put(path.as_ref(), Mem::Link(target.to_owned()))
    }

    /// Removes `path` and everything under it.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path = path.as_ref();
        self.nodes.retain(|at, _| !at.starts_with(path));
        self
    }

    fn put(&mut self, path: &Path, node: Mem) -> &mut Self {
        let path = normalize(path);
        assert!(path.is_absolute(), "a MemFs path is absolute");
        let mut at = PathBuf::new();
        for part in path.parent().into_iter().flat_map(Path::components) {
            at.push(part);
            self.nodes.entry(at.clone()).or_insert(Mem::Dir);
        }
        self.nodes.insert(path, node);
        self
    }

    /// Where `path` leads with every link before the last part followed,
    /// and the last one too when `follow` is set.
    fn resolve(&self, path: &Path, follow: bool) -> io::Result<Option<PathBuf>> {
        let mut todo: Vec<String> = normalize(path)
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .rev()
            .collect();
        let mut at = PathBuf::from("/");
        let mut hops = 0;
        while let Some(part) = todo.pop() {
            let next = at.join(&part);
            match self.nodes.get(&next) {
                None => return Ok(None),
                Some(Mem::Link(target)) if follow || !todo.is_empty() => {
                    hops += 1;
                    if hops > MAX_HOPS {
                        return Err(io::Error::other("too many links"));
                    }
                    let to = normalize(&at.join(target));
                    for part in to.components().rev() {
                        if let Component::Normal(name) = part {
                            todo.push(name.to_string_lossy().into_owned());
                        }
                    }
                    at = PathBuf::from("/");
                }
                Some(Mem::File { .. }) if !todo.is_empty() => {
                    return Err(io::Error::other("not a folder"));
                }
                Some(_) => at = next,
            }
        }
        Ok(Some(at))
    }
}

impl SkillFs for MemFs {
    fn node(&self, path: &Path) -> io::Result<Option<Node>> {
        let Some(at) = self.resolve(path, false)? else {
            return Ok(None);
        };
        Ok(self.nodes.get(&at).map(|node| match node {
            Mem::Dir => Node::Dir,
            Mem::File { bytes, executable } => Node::File {
                size: bytes.len() as u64,
                executable: *executable,
            },
            Mem::Link(target) => Node::Link {
                target: target.clone(),
            },
        }))
    }

    fn real(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        self.resolve(path, true)
    }

    fn list(&self, path: &Path) -> io::Result<Vec<String>> {
        let at = self
            .resolve(path, true)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        if !matches!(self.nodes.get(&at), Some(Mem::Dir)) {
            return Err(io::Error::other("not a folder"));
        }
        Ok(self
            .nodes
            .range(at.clone()..)
            .skip(1)
            .take_while(|(child, _)| child.starts_with(&at))
            .filter(|(child, _)| child.parent() == Some(at.as_path()))
            .filter_map(|(child, _)| child.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect())
    }

    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        let at = self
            .resolve(path, true)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        match self.nodes.get(&at) {
            Some(Mem::File { bytes, .. }) => {
                let keep = bytes
                    .len()
                    .min(usize::try_from(limit).unwrap_or(usize::MAX) + 1);
                Ok(bytes[..keep].to_vec())
            }
            _ => Err(io::Error::other("not a file")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_followed_except_at_the_last_part_of_node() {
        let mut fs = MemFs::new();
        fs.file("/work/repo/.maxhq/skills/a/SKILL.md", "x")
            .link("/work/repo/.claude/skills/a", "../../.maxhq/skills/a");
        let link = Path::new("/work/repo/.claude/skills/a");
        assert_eq!(
            fs.node(link).unwrap(),
            Some(Node::Link {
                target: "../../.maxhq/skills/a".into()
            })
        );
        assert_eq!(
            fs.real(link).unwrap(),
            Some(PathBuf::from("/work/repo/.maxhq/skills/a"))
        );
        assert_eq!(fs.list(link).unwrap(), vec!["SKILL.md".to_owned()]);
        assert_eq!(fs.read(&link.join("SKILL.md"), 10).unwrap(), b"x");
        assert_eq!(fs.node(Path::new("/work/none")).unwrap(), None);
    }

    #[test]
    fn a_read_stops_one_byte_past_its_limit() {
        let mut fs = MemFs::new();
        fs.file("/f", "abcdef");
        assert_eq!(fs.read(Path::new("/f"), 2).unwrap(), b"abc");
    }

    #[test]
    fn a_link_loop_is_an_error() {
        let mut fs = MemFs::new();
        fs.link("/a", "b").link("/b", "a");
        assert!(fs.real(Path::new("/a")).is_err());
    }
}
