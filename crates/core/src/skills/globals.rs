// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tidying a device's global skills, for you to approve first. Your own
//! global skills move into the library, so you pick them per project. The
//! always set stays global as links into the device's store, from each
//! Claude config folder's `skills/` and from `~/.agents/skills`, and every
//! other copy of an always skill goes to the backup. That's also what ends
//! two CLIs reading two different cuts of one skill.
//!
//! A skill a repo you installed it from tracks, as `~/.agents/.skill-lock.json`
//! says, comes from that repo instead: the repo joins the library as a
//! third-party source and the copy goes to the backup. Same-named copies
//! that differ wait for you to pick one.
//!
//! Planning only reads. Claude's `synced/` and Codex's `.system/` are
//! never touched, and nor is anything that isn't a skill folder.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use maxhq_protocol::skills::{GlobalsConflict, GlobalsJoin, GlobalsPlan, TreeHash};

use super::{ignored, link_points_at, sha256_hex, tree, Node, SkillFs, SKILL_FILE};

/// The device's global skill folders, as `maxd` finds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalsInput {
    /// The home folder, for `~/.agents/skills`.
    pub home: PathBuf,
    /// Each Claude config folder on the device, the default first.
    pub claude_configs: Vec<PathBuf>,
    /// Each `CODEX_HOME` on the device.
    pub codex_homes: Vec<PathBuf>,
    /// The device's store, which always links point into.
    pub store: PathBuf,
    /// The always set, each with the tree MaxHQ already has for it. One
    /// without a tree takes the first global copy found.
    pub always: Vec<(String, Option<TreeHash>)>,
    /// The repo each skill came from, by name, for the skills a repo you
    /// installed them from tracks.
    pub tracked: BTreeMap<String, String>,
}

impl GlobalsInput {
    /// The folders that get the always set's links.
    pub fn link_folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self
            .claude_configs
            .iter()
            .map(|config| config.join("skills"))
            .collect();
        folders.push(self.home.join(".agents/skills"));
        folders
    }

    /// Every folder the tidy reads, in the order a copy is preferred.
    fn folders(&self) -> Vec<PathBuf> {
        let mut folders = self.link_folders();
        folders.extend(self.codex_homes.iter().map(|home| home.join("skills")));
        folders
    }
}

/// A global skill the tidy takes away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taken {
    /// Its name, its folder's name.
    pub name: String,
    /// The folder or link, as found.
    pub path: PathBuf,
    /// Its tree, when it hashes.
    pub tree: Option<TreeHash>,
}

/// A link into the store the tidy makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Where it goes.
    pub path: PathBuf,
    /// The stored tree it points at.
    pub target: PathBuf,
}

/// A repo that joins the library as a source, and the copies it stands
/// in for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Join {
    /// The repo.
    pub url: String,
    /// The copies, which go to the backup once it joins.
    pub taken: Vec<Taken>,
}

/// One skill's copies that differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The skill.
    pub name: String,
    /// Each copy, in the order a copy is preferred. Each has a tree.
    pub copies: Vec<Taken>,
}

/// What a tidy does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tidy {
    /// Skills that go into the library, then to the backup. Each has a
    /// tree.
    pub moves: Vec<Taken>,
    /// Copies that go to the backup only.
    pub backups: Vec<Taken>,
    /// Links that appear, after what was in their place went to the
    /// backup.
    pub links: Vec<Link>,
    /// Repos that join the library before their copies go to the backup.
    pub joins: Vec<Join>,
    /// Skills that stay where they are until you pick a copy.
    pub conflicts: Vec<Conflict>,
}

impl Tidy {
    /// Whether applying it changes nothing. Conflicts wait for a pick, so
    /// they don't count.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
            && self.backups.is_empty()
            && self.links.is_empty()
            && self.joins.is_empty()
    }

    /// Takes your picks, paths of conflicting copies: the picked copy
    /// moves into the library and the others go to the backup. A conflict
    /// with no pick stays. A path that isn't a copy in a conflict, or two
    /// picks for one skill, is refused.
    pub fn pick(&mut self, picks: &[String]) -> Result<(), String> {
        let is = |taken: &Taken, pick: &String| taken.path.as_os_str() == pick.as_str();
        if let Some(stray) = picks.iter().find(|pick| {
            !self
                .conflicts
                .iter()
                .any(|c| c.copies.iter().any(|t| is(t, pick)))
        }) {
            return Err(format!("{stray} isn't a copy you can pick."));
        }
        let mut left = Vec::new();
        for conflict in std::mem::take(&mut self.conflicts) {
            let picked: Vec<&Taken> = conflict
                .copies
                .iter()
                .filter(|t| picks.iter().any(|pick| is(t, pick)))
                .collect();
            match picked.as_slice() {
                [] => left.push(conflict),
                [one] => {
                    let tree = one.tree.clone();
                    for taken in conflict.copies {
                        if taken.tree == tree {
                            self.moves.push(taken);
                        } else {
                            self.backups.push(taken);
                        }
                    }
                }
                _ => return Err(format!("Pick one copy of {}.", conflict.name)),
            }
        }
        self.conflicts = left;
        Ok(())
    }

    /// Every folder or link the tidy takes away, each into the backup.
    pub fn taken(&self) -> impl Iterator<Item = &Taken> {
        self.moves
            .iter()
            .chain(&self.backups)
            .chain(self.joins.iter().flat_map(|join| &join.taken))
    }

    /// Names everything the tidy does, trees and targets too, so a plan
    /// you saw applies only while the folders are as they were.
    pub fn hash(&self) -> String {
        let mut text = String::new();
        let mut line = |kind: &str, path: &Path, what: &str| {
            text.push_str(&format!("{kind}\0{}\0{what}\n", path.display()));
        };
        let tree = |taken: &Taken| taken.tree.as_ref().map_or("-".to_owned(), |t| t.0.clone());
        for taken in &self.moves {
            line("move", &taken.path, &tree(taken));
        }
        for taken in &self.backups {
            line("backup", &taken.path, &tree(taken));
        }
        for link in &self.links {
            line("link", &link.path, &link.target.to_string_lossy());
        }
        for join in &self.joins {
            for taken in &join.taken {
                line(
                    "join",
                    &taken.path,
                    &format!("{}\0{}", tree(taken), join.url),
                );
            }
        }
        for conflict in &self.conflicts {
            for taken in &conflict.copies {
                line("conflict", &taken.path, &tree(taken));
            }
        }
        sha256_hex(text.as_bytes())
    }

    /// The plan the app shows.
    pub fn plan(&self) -> GlobalsPlan {
        let paths = |taken: &[Taken]| {
            taken
                .iter()
                .map(|t| t.path.to_string_lossy().into_owned())
                .collect()
        };
        GlobalsPlan {
            hash: self.hash(),
            moves: paths(&self.moves),
            links: self
                .links
                .iter()
                .map(|link| link.path.to_string_lossy().into_owned())
                .collect(),
            backups: paths(&self.backups),
            joins: self
                .joins
                .iter()
                .map(|join| GlobalsJoin {
                    name: source_name(&join.url),
                    url: join.url.clone(),
                    paths: paths(&join.taken),
                })
                .collect(),
            conflicts: self
                .conflicts
                .iter()
                .map(|conflict| GlobalsConflict {
                    name: conflict.name.clone(),
                    paths: paths(&conflict.copies),
                })
                .collect(),
            undo: false,
        }
    }
}

/// What a source from `url` is called: its owner and repo.
pub fn source_name(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let parts: Vec<&str> = url.rsplit(['/', ':']).take(2).collect();
    match parts.as_slice() {
        [repo, owner] if !owner.is_empty() => format!("{owner}/{repo}"),
        [repo, ..] => (*repo).to_owned(),
        [] => url.to_owned(),
    }
}

/// What's at one name in a global folder.
enum Found {
    /// A skill folder, or a link to one.
    Skill(Taken),
    /// A link into the store, to the tree named, and whether that tree is
    /// there.
    Stored(TreeHash, bool),
    /// Anything else, which stays.
    Other,
}

/// Works out what tidying `input`'s folders would do.
pub fn plan(fs: &dyn SkillFs, input: &GlobalsInput) -> Tidy {
    let stores: Vec<PathBuf> = [
        Some(input.store.clone()),
        fs.real(&input.store).ok().flatten(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let is_always = |name: &str| input.always.iter().any(|(n, _)| n == name);

    // Every skill in the folders, and each always link already there.
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut skills: Vec<Taken> = Vec::new();
    let mut stored: Vec<(Taken, bool)> = Vec::new();
    for folder in input.folders() {
        // A folder that's a link to another one is read once.
        let Ok(Some(real)) = fs.real(&folder) else {
            continue;
        };
        if seen.contains(&real) {
            continue;
        }
        seen.push(real);
        let mut names = fs.list(&folder).unwrap_or_default();
        names.sort_unstable();
        for name in names {
            if name.starts_with('.') || ignored(&name) || name == "synced" {
                continue;
            }
            let path = folder.join(&name);
            match found(fs, &stores, &name, &path) {
                Found::Skill(taken) => skills.push(taken),
                Found::Stored(tree, there) if is_always(&name) => stored.push((
                    Taken {
                        name,
                        path,
                        tree: Some(tree),
                    },
                    there,
                )),
                Found::Stored(..) | Found::Other => {}
            }
        }
    }

    // Each always skill's tree: MaxHQ's own, else one a link already
    // points at, else the first global copy, which then goes into the
    // library.
    let mut tidy = Tidy::default();
    let mut trees: Vec<(String, TreeHash)> = Vec::new();
    for (name, tree) in &input.always {
        let linked = || {
            stored
                .iter()
                .find(|(t, there)| &t.name == name && *there)
                .and_then(|(t, _)| t.tree.clone())
        };
        let tree = tree.clone().or_else(linked).or_else(|| {
            let at = skills
                .iter()
                .position(|s| &s.name == name && s.tree.is_some())?;
            let taken = skills.remove(at);
            let tree = taken.tree.clone();
            tidy.moves.push(taken);
            tree
        });
        if let Some(tree) = tree {
            trees.push((name.clone(), tree));
        }
    }
    let wanted = |name: &str| trees.iter().find(|(n, _)| n == name).map(|(_, t)| t);

    // Your own skills: a tracked one comes from its repo, and copies that
    // differ wait for a pick.
    let mut trees_of: BTreeMap<&str, BTreeSet<&TreeHash>> = BTreeMap::new();
    for taken in &skills {
        if let Some(tree) = &taken.tree {
            trees_of.entry(&taken.name).or_default().insert(tree);
        }
    }
    let differ: BTreeSet<String> = trees_of
        .into_iter()
        .filter(|(name, trees)| trees.len() > 1 && !input.tracked.contains_key(*name))
        .map(|(name, _)| name.to_owned())
        .collect();
    for taken in skills {
        if is_always(&taken.name) {
            tidy.backups.push(taken);
            continue;
        }
        if taken.tree.is_none() {
            continue;
        }
        if let Some(url) = input.tracked.get(&taken.name) {
            match tidy.joins.iter_mut().find(|join| &join.url == url) {
                Some(join) => join.taken.push(taken),
                None => tidy.joins.push(Join {
                    url: url.clone(),
                    taken: vec![taken],
                }),
            }
        } else if differ.contains(&taken.name) {
            match tidy.conflicts.iter_mut().find(|c| c.name == taken.name) {
                Some(conflict) => conflict.copies.push(taken),
                None => tidy.conflicts.push(Conflict {
                    name: taken.name.clone(),
                    copies: vec![taken],
                }),
            }
        } else {
            tidy.moves.push(taken);
        }
    }
    for (taken, _) in stored {
        if taken.tree.as_ref() != wanted(&taken.name) {
            tidy.backups.push(taken);
        }
    }

    for folder in input.link_folders() {
        for (name, tree) in &trees {
            let path = folder.join(name);
            let correct = matches!(
                found(fs, &stores, name, &path),
                Found::Stored(at, _) if &at == tree
            );
            let free = matches!(fs.node(&path), Ok(None)) || tidy.taken().any(|t| t.path == path);
            let link = Link {
                path,
                target: input.store.join(&tree.0),
            };
            if !correct && free && !tidy.links.contains(&link) {
                tidy.links.push(link);
            }
        }
    }
    tidy
}

/// What's at `path`, named `name`, in a global folder. `stores` is the
/// store's folder as given and with links resolved.
fn found(fs: &dyn SkillFs, stores: &[PathBuf], name: &str, path: &Path) -> Found {
    let Ok(Some(node)) = fs.node(path) else {
        return Found::Other;
    };
    if let Node::Link { target } = &node {
        let to = link_points_at(path, target);
        if let (Some(parent), Some(tree)) = (to.parent(), to.file_name())
            && stores.iter().any(|store| store == parent)
        {
            let there = matches!(fs.node(&to), Ok(Some(Node::Dir)));
            return Found::Stored(TreeHash(tree.to_string_lossy().into_owned()), there);
        }
    }
    let Ok(Some(real)) = fs.real(path) else {
        return Found::Other;
    };
    if !matches!(fs.node(&real), Ok(Some(Node::Dir)))
        || !matches!(fs.node(&real.join(SKILL_FILE)), Ok(Some(_)))
    {
        return Found::Other;
    }
    Found::Skill(Taken {
        name: name.to_owned(),
        path: path.to_path_buf(),
        tree: tree::of(fs, &real).ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::mem::MemFs;

    const STORE: &str = "/data/skills/store";

    fn skill(fs: &mut MemFs, dir: &str, body: &str) {
        fs.file(
            format!("{dir}/SKILL.md"),
            format!("---\nname: x\ndescription: {body}\n---\n"),
        );
    }

    fn input(propose: Option<TreeHash>) -> GlobalsInput {
        GlobalsInput {
            home: "/home/dev".into(),
            claude_configs: vec!["/home/dev/.claude".into()],
            codex_homes: vec!["/home/dev/.codex".into()],
            store: STORE.into(),
            always: vec![
                ("unslop".into(), None),
                ("propose-skill-change".into(), propose),
            ],
            tracked: BTreeMap::new(),
        }
    }

    /// The layout the plan found on the Mac, with placeholder paths.
    fn today() -> MemFs {
        let mut fs = MemFs::new();
        skill(&mut fs, "/home/dev/.claude/skills/git-flow", "Branches.");
        skill(&mut fs, "/home/dev/.claude/skills/unslop", "Cut AI tells.");
        skill(&mut fs, "/home/dev/.claude/skills/synced/notes", "Synced.");
        skill(&mut fs, "/home/dev/.agents/skills/git-flow", "Branches.");
        fs.file("/home/dev/.agents/skills/.DS_Store", "x");
        skill(
            &mut fs,
            "/home/dev/.codex/skills/.system/imagegen",
            "Images.",
        );
        skill(&mut fs, "/home/dev/.codex/skills/unslop", "An older cut.");
        fs.dir("/home/dev/.codex/skills/not-a-skill");
        fs.dir(STORE);
        fs
    }

    /// `path` with `/` between its parts, as the tests spell them: a
    /// Windows host joins with `\`.
    fn slash(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn paths(taken: &[Taken]) -> Vec<String> {
        taken.iter().map(|t| slash(&t.path)).collect()
    }

    fn propose() -> TreeHash {
        TreeHash("b".repeat(64))
    }

    #[test]
    fn the_plan_moves_your_skills_and_keeps_one_unslop() {
        let fs = today();
        let tidy = plan(&fs, &input(Some(propose())));
        assert_eq!(
            paths(&tidy.moves),
            [
                "/home/dev/.claude/skills/unslop",
                "/home/dev/.claude/skills/git-flow",
                "/home/dev/.agents/skills/git-flow",
            ]
        );
        assert_eq!(paths(&tidy.backups), ["/home/dev/.codex/skills/unslop"]);
        let unslop = tidy.moves[0].tree.clone().unwrap();
        let links: Vec<(String, PathBuf)> = tidy
            .links
            .iter()
            .map(|l| (slash(&l.path), l.target.clone()))
            .collect();
        let links: Vec<(&str, PathBuf)> = links
            .iter()
            .map(|(path, target)| (path.as_str(), target.clone()))
            .collect();
        let at = |tree: &TreeHash| Path::new(STORE).join(&tree.0);
        assert_eq!(
            links,
            [
                ("/home/dev/.claude/skills/unslop", at(&unslop)),
                (
                    "/home/dev/.claude/skills/propose-skill-change",
                    at(&propose())
                ),
                ("/home/dev/.agents/skills/unslop", at(&unslop)),
                (
                    "/home/dev/.agents/skills/propose-skill-change",
                    at(&propose())
                ),
            ]
        );
        let shown = tidy.plan();
        assert_eq!(shown.hash, tidy.hash());
        assert_eq!(shown.backups, ["/home/dev/.codex/skills/unslop"]);
        assert_eq!(shown.links.len(), 4);
    }

    #[test]
    fn a_tidied_device_plans_nothing() {
        let mut fs = today();
        let tidy = plan(&fs, &input(Some(propose())));
        for taken in tidy.moves.iter().chain(&tidy.backups) {
            fs.remove(&taken.path);
        }
        for link in &tidy.links {
            fs.dir(&link.target);
            fs.link(&link.path, link.target.to_str().unwrap());
        }
        // Without the library's word, the link names unslop's tree.
        let again = plan(&fs, &input(Some(propose())));
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn a_stale_link_is_replaced_and_what_isnt_a_skill_stays() {
        let mut fs = today();
        let old = format!("{STORE}/{}", "c".repeat(64));
        fs.dir(&old);
        fs.link("/home/dev/.agents/skills/propose-skill-change", &old);
        fs.file("/home/dev/.agents/skills/unslop", "a file, not a skill");
        let tidy = plan(&fs, &input(Some(propose())));
        assert!(paths(&tidy.backups)
            .iter()
            .any(|p| p == "/home/dev/.agents/skills/propose-skill-change"));
        let linked: Vec<String> = tidy.links.iter().map(|l| slash(&l.path)).collect();
        assert!(linked
            .iter()
            .any(|p| p == "/home/dev/.agents/skills/propose-skill-change"));
        assert!(!linked
            .iter()
            .any(|p| p == "/home/dev/.agents/skills/unslop"));
        assert!(!paths(&tidy.moves).iter().any(|p| p.contains("not-a-skill")));
        assert!(!paths(&tidy.moves).iter().any(|p| p.contains("synced")));
    }

    #[test]
    fn the_library_tree_wins_and_every_copy_goes_to_the_backup() {
        let fs = today();
        let mut given = input(Some(propose()));
        given.always[0].1 = Some(TreeHash("d".repeat(64)));
        let tidy = plan(&fs, &given);
        assert!(!paths(&tidy.moves).iter().any(|p| p.ends_with("unslop")));
        assert_eq!(
            paths(&tidy.backups),
            [
                "/home/dev/.claude/skills/unslop",
                "/home/dev/.codex/skills/unslop"
            ]
        );
    }

    #[test]
    fn the_hash_changes_with_a_tree() {
        let mut fs = today();
        let before = plan(&fs, &input(Some(propose()))).hash();
        skill(
            &mut fs,
            "/home/dev/.claude/skills/git-flow",
            "Branches, changed.",
        );
        assert_ne!(plan(&fs, &input(Some(propose()))).hash(), before);
    }

    #[test]
    fn a_folder_linked_twice_is_read_once() {
        let mut fs = today();
        fs.remove("/home/dev/.agents/skills");
        fs.link("/home/dev/.agents/skills", "/home/dev/.claude/skills");
        let tidy = plan(&fs, &input(Some(propose())));
        assert_eq!(tidy.moves.len(), 2, "{:?}", tidy.moves);
    }

    #[test]
    fn a_tracked_skill_joins_as_its_repo() {
        let mut fs = today();
        skill(&mut fs, "/home/dev/.agents/skills/pdf", "PDFs.");
        skill(
            &mut fs,
            "/home/dev/.claude/skills/pdf",
            "PDFs, linked once.",
        );
        let mut given = input(Some(propose()));
        let url = "https://github.com/owner/skills.git";
        given.tracked.insert("pdf".into(), url.into());
        let tidy = plan(&fs, &given);
        assert_eq!(tidy.joins.len(), 1);
        assert_eq!(tidy.joins[0].url, url);
        assert_eq!(
            paths(&tidy.joins[0].taken),
            [
                "/home/dev/.claude/skills/pdf",
                "/home/dev/.agents/skills/pdf"
            ]
        );
        assert!(
            tidy.conflicts.is_empty(),
            "differing copies of a tracked skill join"
        );
        assert!(!paths(&tidy.moves).iter().any(|p| p.ends_with("pdf")));
        let shown = tidy.plan();
        assert_eq!(shown.joins[0].name, "owner/skills");
        assert_eq!(shown.joins[0].paths.len(), 2);
        assert_ne!(shown.hash, plan(&fs, &input(Some(propose()))).hash());
    }

    // A pick matches the plan's own path text, which a Windows host writes
    // with `\`, so this test's `/` picks only match on Unix.
    #[cfg(unix)]
    #[test]
    fn copies_that_differ_wait_for_a_pick() {
        let mut fs = today();
        skill(&mut fs, "/home/dev/.claude/skills/grill-me", "Questions.");
        skill(
            &mut fs,
            "/home/dev/.agents/skills/grill-me",
            "More questions.",
        );
        skill(&mut fs, "/home/dev/.codex/skills/grill-me", "Questions.");
        let mut tidy = plan(&fs, &input(Some(propose())));
        assert!(!paths(&tidy.moves).iter().any(|p| p.ends_with("grill-me")));
        assert_eq!(tidy.conflicts.len(), 1);
        assert_eq!(tidy.conflicts[0].name, "grill-me");
        assert_eq!(tidy.plan().conflicts[0].paths.len(), 3);
        let before = tidy.clone();

        let stray = tidy.pick(&["/home/dev/.claude/skills/git-flow".into()]);
        assert!(stray.unwrap_err().contains("isn't a copy you can pick"));
        let two = tidy.pick(&[
            "/home/dev/.claude/skills/grill-me".into(),
            "/home/dev/.agents/skills/grill-me".into(),
        ]);
        assert_eq!(two.unwrap_err(), "Pick one copy of grill-me.");
        tidy = before.clone();
        tidy.pick(&[]).unwrap();
        assert_eq!(tidy, before, "with no pick it stays");

        tidy.pick(&["/home/dev/.codex/skills/grill-me".into()])
            .unwrap();
        assert!(tidy.conflicts.is_empty());
        // The same tree as the pick goes into the library too.
        let moved = paths(&tidy.moves);
        assert!(moved
            .iter()
            .any(|p| p == "/home/dev/.claude/skills/grill-me"));
        assert!(moved
            .iter()
            .any(|p| p == "/home/dev/.codex/skills/grill-me"));
        assert!(paths(&tidy.backups)
            .iter()
            .any(|p| p == "/home/dev/.agents/skills/grill-me"));
    }

    #[test]
    fn a_source_is_named_by_its_owner_and_repo() {
        assert_eq!(
            source_name("https://github.com/owner/repo.git"),
            "owner/repo"
        );
        assert_eq!(source_name("git@github.com:owner/repo"), "owner/repo");
        assert_eq!(source_name("/tmp/skills/"), "tmp/skills");
    }
}
