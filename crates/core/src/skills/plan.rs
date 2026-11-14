// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a checkout needs to match its lock: the skill trees in
//! `.maxhq/skills`, as copies or as a link to the workspace's mount, and a
//! relative link per skill in each of [`LINK_DIRS`]. A plan never replaces
//! a real file or folder, or a link another installer made; it reports
//! those as clashes. `.maxhq` is MaxHQ's, so anything there may go. Git
//! commits links, so a plan never writes through one that a repo could
//! have aimed out of the checkout.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_protocol::skills::{SkillLock, TreeHash};

use super::{link_points_at, Node, SkillFs, LINK_DIRS, MOUNT, PROPOSALS_DIR, SKILLS_DIR};

/// How a checkout holds its trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `.maxhq/skills/<name>` are read-only copies, on the Mac and SSH
    /// hosts.
    Copies,
    /// `.maxhq/skills` links to the workspace's read-only [`MOUNT`], in an
    /// Incus workspace.
    Mount,
}

/// One change a plan makes, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make a folder and any parents it lacks.
    MakeDir(PathBuf),
    /// Remove what's at a path in `.maxhq`, folder or link, before
    /// something else takes its place.
    Clear(PathBuf),
    /// Write the tree to `.maxhq/skills/<name>`, replacing a copy that's
    /// there.
    Copy {
        /// The skill.
        name: String,
        /// Its tree.
        tree: TreeHash,
    },
    /// Remove the copy of a skill the lock no longer has.
    RemoveCopy(String),
    /// Make a link.
    Link {
        /// Where the link goes.
        path: PathBuf,
        /// What it points at, as written.
        target: String,
    },
    /// Replace a MaxHQ link that points somewhere else.
    Relink {
        /// The link.
        path: PathBuf,
        /// What it should point at.
        target: String,
    },
    /// Remove a MaxHQ link to a skill the lock no longer has.
    Unlink(PathBuf),
}

/// Something in the way that a plan leaves alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clash {
    /// A real file or folder where a link should go, such as a skill the
    /// project commits.
    Taken(PathBuf),
    /// A link another installer made, such as devflow.
    Vendored {
        /// The link.
        path: PathBuf,
        /// What it points at, as written.
        target: String,
    },
    /// A link on the way to a folder MaxHQ writes in that leads out of the
    /// checkout, or into its `.git`. Nothing goes through it.
    Outside(PathBuf),
}

/// What a checkout needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The changes, in the order to make them.
    pub steps: Vec<Step>,
    /// What's in the way.
    pub clashes: Vec<Clash>,
    /// The lines `.git/info/exclude` needs, for [`merge_exclude`].
    pub exclude: Vec<String>,
}

impl Plan {
    /// Whether the checkout already matches.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// The plan for the checkout at `root` to match `lock`. `copies` is the
/// tree each copy in `.maxhq/skills` holds, as the caller last checked;
/// a copy it doesn't name, or names at another tree, is written again.
/// It's ignored for [`Layout::Mount`].
pub fn checkout(
    fs: &dyn SkillFs,
    root: &Path,
    lock: &SkillLock,
    layout: Layout,
    copies: &BTreeMap<String, TreeHash>,
) -> io::Result<Plan> {
    if let Some(link) = store_link(fs, root)? {
        return Ok(Plan {
            clashes: vec![Clash::Outside(link)],
            ..Plan::default()
        });
    }
    let mut plan = Plan {
        exclude: exclude_lines(lock),
        ..Plan::default()
    };
    let store = root.join(SKILLS_DIR);
    let here = fs.node(&store)?;
    match layout {
        Layout::Copies => {
            match here {
                Some(Node::Dir) => {}
                Some(_) => {
                    plan.steps.push(Step::Clear(store.clone()));
                    plan.steps.push(Step::MakeDir(store.clone()));
                }
                None => plan.steps.push(Step::MakeDir(store.clone())),
            }
            let fresh = here != Some(Node::Dir);
            for skill in &lock.skills {
                let copied = !fresh
                    && copies.get(&skill.name) == Some(&skill.tree)
                    && fs.node(&store.join(&skill.name))? == Some(Node::Dir);
                if !copied {
                    plan.steps.push(Step::Copy {
                        name: skill.name.clone(),
                        tree: skill.tree.clone(),
                    });
                }
            }
            if !fresh {
                let mut names = fs.list(&store)?;
                names.sort_unstable();
                for name in names {
                    if !lock.skills.iter().any(|s| s.name == name) {
                        plan.steps.push(Step::RemoveCopy(name));
                    }
                }
            }
        }
        Layout::Mount => match here {
            Some(Node::Link { target }) if target == MOUNT => {}
            Some(Node::Link { .. }) => plan.steps.push(Step::Relink {
                path: store.clone(),
                target: MOUNT.to_owned(),
            }),
            other => {
                if other.is_some() {
                    plan.steps.push(Step::Clear(store.clone()));
                } else if let Some(parent) = store.parent()
                    && fs.node(parent)?.is_none()
                {
                    plan.steps.push(Step::MakeDir(parent.to_owned()));
                }
                plan.steps.push(Step::Link {
                    path: store.clone(),
                    target: MOUNT.to_owned(),
                });
            }
        },
    }
    let ours = |to: &Path| to.starts_with(&store);
    for dir in LINK_DIRS {
        if let Some(link) = leads_out(fs, root, dir)? {
            plan.clashes.push(Clash::Outside(link));
            continue;
        }
        let wanted: BTreeMap<String, String> = lock
            .skills
            .iter()
            .map(|s| (s.name.clone(), format!("../../{SKILLS_DIR}/{}", s.name)))
            .collect();
        let (steps, clashes) = links(fs, &root.join(dir), &wanted, &ours)?;
        plan.steps.extend(steps);
        plan.clashes.extend(clashes);
    }
    Ok(plan)
}

/// The link at `.maxhq` or `.maxhq/proposals` in the checkout at `root`, if
/// there is one. MaxHQ moves and removes folders in there, so it stops at
/// any link, even one that stays in the checkout, since that could be aimed
/// at `.git`. A link at `.maxhq/skills` isn't one: a plan replaces it without
/// following it.
pub fn store_link(fs: &dyn SkillFs, root: &Path) -> io::Result<Option<PathBuf>> {
    let mut path = root.to_owned();
    for part in Path::new(PROPOSALS_DIR).components() {
        path.push(part);
        if matches!(fs.node(&path)?, Some(Node::Link { .. })) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The first link on the way from `root` to its folder `dir` that ends up
/// outside the checkout, in its `.git`, or nowhere. A project may link
/// `.claude` to `.agents`, so a link that stays inside is fine.
fn leads_out(fs: &dyn SkillFs, root: &Path, dir: &str) -> io::Result<Option<PathBuf>> {
    let home = fs.real(root)?.unwrap_or_else(|| root.to_owned());
    let mut path = root.to_owned();
    for part in Path::new(dir).components() {
        path.push(part);
        if !matches!(fs.node(&path)?, Some(Node::Link { .. })) {
            continue;
        }
        let stays = fs.real(&path).ok().flatten().is_some_and(|to| {
            to.strip_prefix(&home).is_ok_and(|rest| {
                !rest
                    .components()
                    .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
            })
        });
        if !stays {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The steps that make the folder `dir` hold a link per entry of `wanted`,
/// a name and the target it points at, and drop the links to names
/// `wanted` lacks. A link is MaxHQ's when `ours` says so of where it
/// points; any other link or a real file is a clash and stays. A missing
/// `dir` is made, since Claude only watches a skills folder that's there
/// when it starts. Globals use it too, with their own `ours`.
pub fn links(
    fs: &dyn SkillFs,
    dir: &Path,
    wanted: &BTreeMap<String, String>,
    ours: &dyn Fn(&Path) -> bool,
) -> io::Result<(Vec<Step>, Vec<Clash>)> {
    let mut steps = Vec::new();
    let mut clashes = Vec::new();
    let existing = match fs.node(dir)? {
        Some(Node::Dir) => {
            let mut names = fs.list(dir)?;
            names.sort_unstable();
            names
        }
        Some(_) => {
            clashes.push(Clash::Taken(dir.to_owned()));
            return Ok((steps, clashes));
        }
        None => {
            steps.push(Step::MakeDir(dir.to_owned()));
            Vec::new()
        }
    };
    for (name, target) in wanted {
        let path = dir.join(name);
        match fs.node(&path)? {
            None => steps.push(Step::Link {
                path,
                target: target.clone(),
            }),
            Some(Node::Link { target: now }) if now == *target => {}
            Some(Node::Link { target: now }) if ours(&link_points_at(&path, &now)) => {
                steps.push(Step::Relink {
                    path,
                    target: target.clone(),
                });
            }
            Some(Node::Link { target: now }) => {
                clashes.push(Clash::Vendored { path, target: now });
            }
            Some(_) => clashes.push(Clash::Taken(path)),
        }
    }
    for name in existing {
        if wanted.contains_key(&name) {
            continue;
        }
        let path = dir.join(&name);
        if let Some(Node::Link { target }) = fs.node(&path)?
            && ours(&link_points_at(&path, &target))
        {
            steps.push(Step::Unlink(path));
        }
    }
    Ok((steps, clashes))
}

/// The lines `.git/info/exclude` needs for `lock`, as paths from the
/// repo's root. Excluded copies leave out all of `.maxhq` and each link,
/// not all of `.claude`, so a project can still commit its own skills.
/// Committed copies leave out only the proposals.
pub fn exclude_lines(lock: &SkillLock) -> Vec<String> {
    if lock.committed {
        return vec![format!("/{}/", super::PROPOSALS_DIR)];
    }
    let mut lines = vec!["/.maxhq/".to_owned()];
    for dir in LINK_DIRS {
        for skill in &lock.skills {
            lines.push(format!("/{dir}/{}", skill.name));
        }
    }
    lines
}

/// The heading MaxHQ puts above the lines it adds to an exclude file.
pub const EXCLUDE_HEADING: &str = "# MaxHQ skills";

/// `existing`, an exclude file's text, with the lines of `wanted` it
/// lacks added at the end, or `None` when it has them all. Lines already
/// there are never removed or repeated.
pub fn merge_exclude(existing: &str, wanted: &[String]) -> Option<String> {
    let have: Vec<&str> = existing.lines().map(str::trim).collect();
    let add: Vec<&String> = wanted
        .iter()
        .filter(|line| !have.contains(&line.as_str()))
        .collect();
    if add.is_empty() {
        return None;
    }
    let mut text = existing.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !have.contains(&EXCLUDE_HEADING) {
        text.push_str(EXCLUDE_HEADING);
        text.push('\n');
    }
    for line in add {
        text.push_str(line);
        text.push('\n');
    }
    Some(text)
}

/// `existing`, an exclude file's text, without the lines of `lines` that
/// [`merge_exclude`] put under its heading, or `None` when it has none of
/// them. Lines above the heading are yours, so they stay. The heading goes
/// once nothing is left under it.
pub fn unmerge_exclude(existing: &str, lines: &[String]) -> Option<String> {
    let all: Vec<&str> = existing.lines().collect();
    let heading = all.iter().position(|line| line.trim() == EXCLUDE_HEADING)?;
    let (above, below) = all.split_at(heading + 1);
    let kept: Vec<&str> = below
        .iter()
        .copied()
        .filter(|line| !lines.iter().any(|drop| drop == line.trim()))
        .collect();
    if kept.len() == below.len() {
        return None;
    }
    let alone = kept.iter().all(|line| line.trim().is_empty());
    let mut text = String::new();
    for line in &above[..heading] {
        text.push_str(line);
        text.push('\n');
    }
    if !alone {
        text.push_str(above[heading]);
        text.push('\n');
        for line in kept {
            text.push_str(line);
            text.push('\n');
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::SkillSourceId;
    use maxhq_protocol::skills::LockedSkill;

    use super::super::mem::MemFs;
    use super::*;

    const ROOT: &str = "/work/repo";

    fn lock(names: &[&str]) -> SkillLock {
        SkillLock {
            skills: names
                .iter()
                .map(|name| LockedSkill {
                    name: (*name).into(),
                    source: SkillSourceId::derive("example/agent-skills"),
                    path: format!("skills/{name}"),
                    commit: None,
                    tree: TreeHash(format!("tree-{name}")),
                    pinned: false,
                })
                .collect(),
            committed: false,
            pack: None,
        }
    }

    fn at(path: &str) -> PathBuf {
        Path::new(ROOT).join(path)
    }

    fn copied(names: &[&str]) -> BTreeMap<String, TreeHash> {
        names
            .iter()
            .map(|n| ((*n).to_owned(), TreeHash(format!("tree-{n}"))))
            .collect()
    }

    fn link_step(dir: &str, name: &str) -> Step {
        Step::Link {
            path: at(&format!("{dir}/{name}")),
            target: format!("../../.maxhq/skills/{name}"),
        }
    }

    #[test]
    fn a_bare_checkout_gets_copies_folders_and_links() {
        let mut fs = MemFs::new();
        fs.dir(ROOT);
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["unslop"]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::MakeDir(at(".maxhq/skills")),
                Step::Copy {
                    name: "unslop".into(),
                    tree: TreeHash("tree-unslop".into())
                },
                Step::MakeDir(at(".claude/skills")),
                link_step(".claude/skills", "unslop"),
                Step::MakeDir(at(".agents/skills")),
                link_step(".agents/skills", "unslop"),
            ]
        );
        assert!(plan.clashes.is_empty());
    }

    #[test]
    fn an_empty_lock_still_makes_the_claude_folder() {
        let mut fs = MemFs::new();
        fs.dir(ROOT);
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&[]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(plan.steps.contains(&Step::MakeDir(at(".claude/skills"))));
    }

    #[test]
    fn a_matching_checkout_needs_nothing() {
        let mut fs = MemFs::new();
        for dir in LINK_DIRS {
            fs.link(at(&format!("{dir}/a")), "../../.maxhq/skills/a");
        }
        fs.file(at(".maxhq/skills/a/SKILL.md"), "x");
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["a"]),
            Layout::Copies,
            &copied(&["a"]),
        )
        .unwrap();
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn a_changed_or_unknown_copy_is_written_again_and_an_old_one_removed() {
        let mut fs = MemFs::new();
        fs.file(at(".maxhq/skills/a/SKILL.md"), "x")
            .file(at(".maxhq/skills/b/SKILL.md"), "x")
            .file(at(".maxhq/skills/old/SKILL.md"), "x");
        let mut known = copied(&["a"]);
        known.insert("b".into(), TreeHash("drifted".into()));
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["a", "b", "c"]),
            Layout::Copies,
            &known,
        )
        .unwrap();
        let copies: Vec<&Step> = plan
            .steps
            .iter()
            .filter(|s| matches!(s, Step::Copy { .. } | Step::RemoveCopy(_)))
            .collect();
        assert_eq!(
            copies,
            [
                &Step::Copy {
                    name: "b".into(),
                    tree: TreeHash("tree-b".into())
                },
                &Step::Copy {
                    name: "c".into(),
                    tree: TreeHash("tree-c".into())
                },
                &Step::RemoveCopy("old".into()),
            ]
        );
    }

    #[test]
    fn a_real_folder_is_never_replaced() {
        let mut fs = MemFs::new();
        fs.file(at(".claude/skills/deploy/SKILL.md"), "the project's own");
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["deploy"]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            plan.clashes,
            vec![Clash::Taken(at(".claude/skills/deploy"))]
        );
        assert!(!plan.steps.iter().any(|s| matches!(
            s,
            Step::Link { path, .. } | Step::Relink { path, .. } if path == &at(".claude/skills/deploy")
        )));
    }

    #[test]
    fn a_stale_maxhq_link_is_replaced_and_a_dropped_one_removed() {
        let mut fs = MemFs::new();
        fs.dir(at(".maxhq/skills"))
            .link(at(".claude/skills/a"), "../../.maxhq/skills/elsewhere")
            .link(at(".claude/skills/gone"), "../../.maxhq/skills/gone");
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["a"]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(plan.steps.contains(&Step::Relink {
            path: at(".claude/skills/a"),
            target: "../../.maxhq/skills/a".into()
        }));
        assert!(plan
            .steps
            .contains(&Step::Unlink(at(".claude/skills/gone"))));
    }

    #[test]
    fn a_vendored_link_is_left() {
        let mut fs = MemFs::new();
        fs.link(
            at(".claude/skills/unslop"),
            "../../.devflow-vendor/skills/unslop",
        )
        .link(
            at(".claude/skills/df-plan"),
            "../../.devflow-vendor/skills/df-plan",
        );
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["unslop"]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            plan.clashes,
            vec![Clash::Vendored {
                path: at(".claude/skills/unslop"),
                target: "../../.devflow-vendor/skills/unslop".into()
            }]
        );
        assert!(!plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::Unlink(_) | Step::Relink { .. })));
    }

    fn copies_plan(fs: &MemFs) -> Plan {
        checkout(
            fs,
            Path::new(ROOT),
            &lock(&["a"]),
            Layout::Copies,
            &BTreeMap::new(),
        )
        .unwrap()
    }

    fn writes_in(plan: &Plan, dir: &str) -> bool {
        plan.steps.iter().any(|step| match step {
            Step::MakeDir(path)
            | Step::Clear(path)
            | Step::Unlink(path)
            | Step::Link { path, .. }
            | Step::Relink { path, .. } => path.starts_with(at(dir)),
            Step::Copy { .. } | Step::RemoveCopy(_) => false,
        })
    }

    #[test]
    fn a_skills_folder_linked_inside_the_checkout_is_only_taken() {
        let mut fs = MemFs::new();
        fs.dir(at("shared")).link(at(".agents/skills"), "../shared");
        let plan = copies_plan(&fs);
        assert!(plan.clashes.contains(&Clash::Taken(at(".agents/skills"))));
        assert!(!writes_in(&plan, ".agents"));
        assert!(writes_in(&plan, ".claude/skills"));
    }

    #[test]
    fn a_cli_folder_linked_out_of_the_checkout_gets_nothing_and_the_rest_lands() {
        for (link, target) in [
            (".claude", "/elsewhere"),
            (".claude", "../.."),
            (".claude/skills", "/elsewhere"),
            (".claude/skills", "/elsewhere/gone"),
            (".claude", ".git"),
            (".claude", ".GIT/hooks"),
        ] {
            let mut fs = MemFs::new();
            fs.dir("/elsewhere")
                .dir(at(".git/hooks"))
                .dir(at(".GIT/hooks"))
                .dir(at(".claude"))
                .link(at(link), target);
            let plan = copies_plan(&fs);
            assert_eq!(
                plan.clashes,
                [Clash::Outside(at(link))],
                "{link} -> {target}"
            );
            assert!(!writes_in(&plan, ".claude"), "{link} -> {target}");
            assert!(writes_in(&plan, ".agents/skills"), "{link} -> {target}");
            assert!(plan.steps.contains(&Step::Copy {
                name: "a".into(),
                tree: TreeHash("tree-a".into()),
            }));
        }
    }

    #[test]
    fn a_cli_folder_linked_inside_the_checkout_keeps_working() {
        let mut fs = MemFs::new();
        fs.dir(at("shared")).link(at(".claude"), "shared");
        let plan = copies_plan(&fs);
        assert!(plan.clashes.is_empty(), "{:?}", plan.clashes);
        assert!(writes_in(&plan, ".claude/skills"));
    }

    #[test]
    fn any_link_on_the_way_to_our_own_folder_stops_the_whole_checkout() {
        for (link, target) in [
            (".maxhq", "/elsewhere"),
            (".maxhq", "stays-inside"),
            (".maxhq/proposals", "/elsewhere"),
            (".maxhq/proposals", "../.git"),
        ] {
            for layout in [Layout::Copies, Layout::Mount] {
                let mut fs = MemFs::new();
                fs.dir("/elsewhere")
                    .dir(at("stays-inside"))
                    .dir(at(".git"))
                    .dir(at(".maxhq"))
                    .link(at(link), target);
                let plan = checkout(
                    &fs,
                    Path::new(ROOT),
                    &lock(&["a"]),
                    layout,
                    &BTreeMap::new(),
                )
                .unwrap();
                assert_eq!(
                    plan.clashes,
                    [Clash::Outside(at(link))],
                    "{link} -> {target}"
                );
                assert!(plan.steps.is_empty(), "{link} -> {target}");
                assert!(plan.exclude.is_empty(), "{link} -> {target}");
            }
        }
    }

    #[test]
    fn a_link_at_the_skills_store_is_still_replaced_not_followed() {
        let mut fs = MemFs::new();
        fs.dir("/elsewhere")
            .dir(at(".maxhq"))
            .link(at(".maxhq/skills"), "/elsewhere");
        let plan = copies_plan(&fs);
        assert!(plan.clashes.is_empty(), "{:?}", plan.clashes);
        assert_eq!(plan.steps[0], Step::Clear(at(".maxhq/skills")));
        assert_eq!(plan.steps[1], Step::MakeDir(at(".maxhq/skills")));
    }

    #[test]
    fn a_workspace_links_its_set_to_the_mount() {
        let mut fs = MemFs::new();
        fs.dir(ROOT);
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["a"]),
            Layout::Mount,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            plan.steps[..2],
            [
                Step::MakeDir(at(".maxhq")),
                Step::Link {
                    path: at(".maxhq/skills"),
                    target: MOUNT.into()
                },
            ]
        );
        assert!(!plan.steps.iter().any(|s| matches!(s, Step::Copy { .. })));
        assert!(plan.steps.contains(&link_step(".claude/skills", "a")));
    }

    #[test]
    fn copies_give_way_to_the_mount() {
        let mut fs = MemFs::new();
        fs.file(at(".maxhq/skills/a/SKILL.md"), "x");
        let plan = checkout(
            &fs,
            Path::new(ROOT),
            &lock(&["a"]),
            Layout::Mount,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(plan.steps[0], Step::Clear(at(".maxhq/skills")));
    }

    #[test]
    fn exclude_lines_cover_maxhq_and_each_link() {
        assert_eq!(
            exclude_lines(&lock(&["a"])),
            ["/.maxhq/", "/.claude/skills/a", "/.agents/skills/a"]
        );
        let mut committed = lock(&["a"]);
        committed.committed = true;
        assert_eq!(exclude_lines(&committed), ["/.maxhq/proposals/"]);
    }

    #[test]
    fn exclude_entries_are_added_once() {
        let wanted = exclude_lines(&lock(&["a"]));
        let first = merge_exclude("# git's own\n*.log", &wanted).unwrap();
        assert_eq!(
            first,
            "# git's own\n*.log\n# MaxHQ skills\n/.maxhq/\n/.claude/skills/a\n/.agents/skills/a\n"
        );
        assert_eq!(merge_exclude(&first, &wanted), None);
        let more = merge_exclude(&first, &exclude_lines(&lock(&["a", "b"]))).unwrap();
        assert_eq!(more.matches(EXCLUDE_HEADING).count(), 1);
        assert!(more.ends_with("/.claude/skills/b\n/.agents/skills/b\n"));
    }

    #[test]
    fn unmerging_takes_out_only_the_lines_under_the_heading() {
        let pack = ["/CLAUDE.md".to_owned(), "/.claude/".to_owned()];
        let text = "/CLAUDE.md\n*.log\n# MaxHQ skills\n/.maxhq/\n/CLAUDE.md\n/.claude/\n";
        assert_eq!(
            unmerge_exclude(text, &pack).as_deref(),
            Some("/CLAUDE.md\n*.log\n# MaxHQ skills\n/.maxhq/\n")
        );
        let only = "*.log\n# MaxHQ skills\n/.claude/\n";
        assert_eq!(unmerge_exclude(only, &pack).as_deref(), Some("*.log\n"));
        assert_eq!(unmerge_exclude("*.log\n/CLAUDE.md\n", &pack), None);
        assert_eq!(unmerge_exclude("# MaxHQ skills\n/.maxhq/\n", &pack), None);
    }
}
