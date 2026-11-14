// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What each CLI sees from a folder: every skill Claude Code, Codex,
//! Cursor and ACP agents find there, where it comes from, whether the CLI
//! offers it, and what's odd about it. This is the one place that knows
//! where each CLI looks and which copy wins; the app shows its marks and
//! never works them out.
//!
//! - Claude reads its managed folder, then your `skills/` in its config
//!   folder, then claude.ai's `skills/synced/`, then `.claude/skills` from
//!   the folder up to the repo's root, falling back to the main checkout's
//!   when a worktree has none. The first copy of a name wins, so a project
//!   skill under a global one is shadowed. Nested `.claude/skills` below
//!   the folder, added folders and plugins are prefixed with where they
//!   come from, so they never clash.
//! - Codex reads `.agents/skills` from the folder up to the repo's root,
//!   `$CODEX_HOME/skills`, `~/.agents/skills`, its admin folder and its
//!   own `.system` skills, and offers every copy of a name.
//! - Cursor reads `.agents`, `.cursor`, `.claude` and `.codex` skills in
//!   the folder and the first three and `.codex` under your home, and
//!   offers the first copy of a name. `~/.cursor` is
//!   never read, so its skills don't appear.
//! - An ACP agent that reads skills reads `.agents/skills` from the folder
//!   up to the repo's root and under your home, and offers the first copy.
//!
//! Every CLI loads a skill once however many links lead to it. The walk
//! keeps to these limits: 10 folders deep, 1 MB a skill file, and
//! 10,000 entries and 8 MB a scan.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use maxhq_protocol::acp::AcpSkills;
use maxhq_protocol::model::Vendor;
use maxhq_protocol::skills::{EffectiveSkill, SkillMark, SkillPlace, TreeHash};

use super::frontmatter::{self, CodexMeta, SkillDoc, CODEX_META};
use super::{ignored, tree, Node, SkillFs, SKILLS_DIR, SKILL_FILE};

/// How deep the walk goes below a skills folder.
pub const MAX_DEPTH: usize = 10;

/// The largest skill file read.
pub const MAX_SKILL_BYTES: u64 = 1_000_000;

/// The most entries one scan looks at.
pub const MAX_ENTRIES: usize = 10_000;

/// The most bytes one scan reads.
pub const MAX_BYTES: u64 = 8_000_000;

/// A plugin's skills, in `<root>/skills/<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    /// The plugin's name, which prefixes its skills.
    pub name: String,
    /// Its folder.
    pub root: PathBuf,
    /// The CLI that loads it.
    pub cli: Vendor,
}

/// Where to look, as the caller knows it for the agent's device and
/// account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanInput {
    /// The folder the agent runs in.
    pub cwd: PathBuf,
    /// The checkout's root, when the folder is in one.
    pub repo_root: Option<PathBuf>,
    /// The main checkout, when the folder is in a linked worktree.
    pub main_checkout: Option<PathBuf>,
    /// The home folder.
    pub home: PathBuf,
    /// Claude's config folder: `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub claude_config: PathBuf,
    /// Claude's managed skills folder, when the device has one.
    pub claude_managed: Option<PathBuf>,
    /// `$CODEX_HOME`, or `~/.codex`.
    pub codex_home: PathBuf,
    /// Codex's admin skills folder, `/etc/codex/skills`.
    pub codex_admin: Option<PathBuf>,
    /// Folders Claude was given with `--add-dir`.
    pub added_dirs: Vec<PathBuf>,
    /// Plugins each CLI loads.
    pub plugins: Vec<Plugin>,
    /// Whether ACP agents read `.agents/skills`.
    pub acp: AcpSkills,
    /// The skills MaxHQ keeps global on every device.
    pub always: Vec<String>,
    /// The device's skill store, so links into it count as MaxHQ's.
    pub store: Option<PathBuf>,
}

impl ScanInput {
    /// The usual folders for an agent in `cwd` with home `home`, and
    /// nothing added.
    pub fn new(cwd: impl Into<PathBuf>, home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            cwd: cwd.into(),
            repo_root: None,
            main_checkout: None,
            claude_config: home.join(".claude"),
            claude_managed: None,
            codex_home: home.join(".codex"),
            codex_admin: Some(PathBuf::from("/etc/codex/skills")),
            added_dirs: Vec::new(),
            plugins: Vec::new(),
            acp: AcpSkills::Agents,
            always: Vec::new(),
            store: None,
            home,
        }
    }

    /// The folders from `cwd` up to the repo's root, nearest first.
    fn up_to_root(&self) -> Vec<PathBuf> {
        let Some(root) = self
            .repo_root
            .as_deref()
            .filter(|r| self.cwd.starts_with(r))
        else {
            return vec![self.cwd.clone()];
        };
        let mut dirs: Vec<PathBuf> = self
            .cwd
            .ancestors()
            .take_while(|dir| dir.starts_with(root))
            .map(Path::to_path_buf)
            .collect();
        if dirs.is_empty() {
            dirs.push(self.cwd.clone());
        }
        dirs
    }
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Each skill each CLI sees, by CLI and then in the order the CLI
    /// prefers them.
    pub skills: Vec<EffectiveSkill>,
    /// Whether a read failed or the scan hit its limits, so skills may be
    /// missing.
    pub incomplete: bool,
}

/// Finds what each CLI sees from `input.cwd`.
pub fn scan(fs: &dyn SkillFs, input: &ScanInput) -> Scan {
    let mut walk = Walk {
        fs,
        entries: MAX_ENTRIES,
        bytes: MAX_BYTES,
        incomplete: false,
        managed: managed_roots(fs, input),
        trees: BTreeMap::new(),
    };
    let mut skills = Vec::new();
    for cli in [Vendor::Claude, Vendor::Codex, Vendor::Cursor, Vendor::Acp] {
        if cli == Vendor::Acp && input.acp == AcpSkills::None {
            continue;
        }
        let mut found = Vec::new();
        for root in roots(fs, input, cli) {
            found.extend(walk.root(&root));
        }
        if cli == Vendor::Claude {
            found.extend(walk.nested(input));
        }
        skills.extend(walk.settle(cli, found, &input.always));
    }
    Scan {
        skills,
        incomplete: walk.incomplete,
    }
}

/// One folder a CLI reads skills from.
struct Root {
    dir: PathBuf,
    place: SkillPlace,
    scope: &'static str,
    /// What the skills' names are prefixed with.
    prefix: Option<String>,
    /// Whether skills sit below folders that aren't skills, as Codex and
    /// Cursor look for them, rather than only right inside.
    deep: bool,
    /// Names right inside it that aren't skills.
    skip: &'static [&'static str],
}

impl Root {
    fn new(dir: PathBuf, place: SkillPlace, scope: &'static str, deep: bool) -> Self {
        Self {
            dir,
            place,
            scope,
            prefix: None,
            deep,
            skip: &[],
        }
    }

    fn prefixed(mut self, prefix: String) -> Self {
        self.prefix = Some(prefix);
        self
    }

    fn skipping(mut self, skip: &'static [&'static str]) -> Self {
        self.skip = skip;
        self
    }
}

/// The folders `cli` reads, in the order it prefers them.
fn roots(fs: &dyn SkillFs, input: &ScanInput, cli: Vendor) -> Vec<Root> {
    use SkillPlace::{Global, Plugin as FromPlugin, Project, System};
    let home = &input.home;
    let project = |sub: &str, scope, deep| {
        input
            .up_to_root()
            .into_iter()
            .map(|dir| Root::new(dir.join(sub), Project, scope, deep))
            .collect::<Vec<_>>()
    };
    let plugins = |deep| {
        input.plugins.iter().filter(|p| p.cli == cli).map(move |p| {
            Root::new(p.root.join("skills"), FromPlugin, "plugin", deep).prefixed(p.name.clone())
        })
    };
    let mut roots = Vec::new();
    match cli {
        Vendor::Claude => {
            let config = input.claude_config.join("skills");
            if let Some(managed) = &input.claude_managed {
                roots.push(Root::new(managed.clone(), System, "enterprise", false));
            }
            roots.push(Root::new(config.clone(), Global, "personal", false).skipping(&["synced"]));
            roots.push(Root::new(config.join("synced"), System, "synced", false));
            let mut here = project(".claude/skills", "project", false);
            let none = here
                .iter()
                .all(|root| !matches!(fs.node(&root.dir), Ok(Some(_))));
            if let (true, Some(main), Some(repo)) = (none, &input.main_checkout, &input.repo_root) {
                // A worktree with no skills of its own gets the main
                // checkout's.
                for root in &mut here {
                    if let Ok(rest) = root.dir.strip_prefix(repo) {
                        root.dir = main.join(rest);
                    }
                }
            }
            roots.extend(here);
            for dir in &input.added_dirs {
                let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
                let root = Root::new(dir.join(".claude/skills"), Project, "added", false);
                roots.push(match name {
                    Some(name) => root.prefixed(name),
                    None => root,
                });
            }
            roots.extend(plugins(false));
        }
        Vendor::Codex => {
            let own = input.codex_home.join("skills");
            roots.extend(project(".agents/skills", "repo", true));
            roots.push(Root::new(own.clone(), Global, "user", true));
            roots.push(Root::new(home.join(".agents/skills"), Global, "user", true));
            if let Some(admin) = &input.codex_admin {
                roots.push(Root::new(admin.clone(), System, "admin", true));
            }
            roots.push(Root::new(own.join(".system"), System, "system", true));
        }
        Vendor::Cursor => {
            for sub in [".agents", ".cursor", ".claude", ".codex"] {
                let dir = input.cwd.join(sub).join("skills");
                roots.push(Root::new(dir, Project, "project", true));
            }
            for sub in [".agents", ".claude", ".codex"] {
                let dir = home.join(sub).join("skills");
                roots.push(Root::new(dir, Global, "user", true).skipping(&["synced"]));
            }
            roots.extend(plugins(true));
        }
        Vendor::Acp => {
            roots.extend(project(".agents/skills", "project", true));
            roots.push(Root::new(home.join(".agents/skills"), Global, "user", true));
        }
    }
    roots
}

/// Where MaxHQ's own trees resolve to: the checkout's `.maxhq/skills`,
/// which may be a link to the mount, and the device's store.
fn managed_roots(fs: &dyn SkillFs, input: &ScanInput) -> Vec<PathBuf> {
    let checkout = input.repo_root.as_ref().unwrap_or(&input.cwd);
    [Some(checkout.join(SKILLS_DIR)), input.store.clone()]
        .into_iter()
        .flatten()
        .filter_map(|dir| fs.real(&dir).ok().flatten())
        .collect()
}

/// A skill folder as the walk found it.
struct Found {
    /// The folder's name, the skill's name in Claude and Cursor.
    folder: String,
    /// The folder as found.
    dir: PathBuf,
    /// The folder with links resolved.
    real: PathBuf,
    /// Its `SKILL.md`, or why it wouldn't read.
    doc: Option<Result<SkillDoc, frontmatter::ParseError>>,
    /// Codex's metadata, for Codex.
    codex: Option<CodexMeta>,
    place: SkillPlace,
    scope: &'static str,
    prefix: Option<String>,
    /// Whether another installer's link brings it in.
    vendored: bool,
}

struct Walk<'a> {
    fs: &'a dyn SkillFs,
    entries: usize,
    bytes: u64,
    incomplete: bool,
    managed: Vec<PathBuf>,
    trees: BTreeMap<PathBuf, Option<TreeHash>>,
}

impl Walk<'_> {
    /// Takes one entry from the scan's limit, saying whether there was one.
    fn spend(&mut self) -> bool {
        if self.entries == 0 {
            self.incomplete = true;
            return false;
        }
        self.entries -= 1;
        true
    }

    fn real(&mut self, path: &Path) -> Option<PathBuf> {
        match self.fs.real(path) {
            Ok(real) => real,
            Err(_) => {
                self.incomplete = true;
                None
            }
        }
    }

    fn is_dir(&mut self, real: &Path) -> bool {
        matches!(self.fs.node(real), Ok(Some(Node::Dir)))
    }

    fn list(&mut self, dir: &Path) -> Vec<String> {
        match self.fs.list(dir) {
            Ok(mut names) => {
                names.sort_unstable();
                names
            }
            Err(_) => {
                self.incomplete = true;
                Vec::new()
            }
        }
    }

    /// Reads a small file inside the folder `real`, within the limits.
    fn read(&mut self, real: &Path, name: &str) -> Option<String> {
        let path = real.join(name);
        let at = self.real(&path)?;
        let Ok(Some(Node::File { size, .. })) = self.fs.node(&at) else {
            return None;
        };
        if size > MAX_SKILL_BYTES || size > self.bytes {
            self.incomplete |= size > self.bytes;
            return None;
        }
        self.bytes -= size;
        match self.fs.read(&at, size) {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(_) => {
                self.incomplete = true;
                None
            }
        }
    }

    /// Whether a link in `link`'s path that leads outside MaxHQ's trees
    /// brings it in.
    fn vendored(&mut self, links: &[&Path], real: &Path) -> bool {
        let linked = links
            .iter()
            .any(|path| matches!(self.fs.node(path), Ok(Some(Node::Link { .. }))));
        linked && !self.managed.iter().any(|root| real.starts_with(root))
    }

    /// The skills in one root.
    fn root(&mut self, root: &Root) -> Vec<Found> {
        let mut found = Vec::new();
        let Some(real_root) = self.real(&root.dir) else {
            return found;
        };
        if !self.is_dir(&real_root) {
            return found;
        }
        let mut todo: Vec<(PathBuf, usize)> = vec![(root.dir.clone(), 0)];
        while let Some((dir, depth)) = todo.pop() {
            let mut children = Vec::new();
            for name in self.list(&dir) {
                if name.starts_with('.') || ignored(&name) || name == "node_modules" {
                    continue;
                }
                if depth == 0 && root.skip.contains(&name.as_str()) {
                    continue;
                }
                if !self.spend() {
                    return found;
                }
                let child = dir.join(&name);
                let Some(real) = self.real(&child) else {
                    continue;
                };
                if !self.is_dir(&real) {
                    continue;
                }
                if matches!(self.fs.node(&real.join(SKILL_FILE)), Ok(Some(_))) {
                    let doc = self.read(&real, SKILL_FILE).map(|t| frontmatter::parse(&t));
                    let codex = self
                        .read(&real, CODEX_META)
                        .and_then(|t| frontmatter::codex_meta(&t).ok());
                    let vendored = root.place == SkillPlace::Project
                        && self.vendored(&[&root.dir, &child], &real);
                    found.push(Found {
                        folder: name,
                        dir: child,
                        real,
                        doc,
                        codex,
                        place: root.place,
                        scope: root.scope,
                        prefix: root.prefix.clone(),
                        vendored,
                    });
                } else if root.deep && depth < MAX_DEPTH && real.starts_with(&real_root) {
                    // A link out of the root shows only its own skill.
                    children.push((child, depth + 1));
                }
            }
            todo.extend(children.into_iter().rev());
        }
        found
    }

    /// Claude's nested `.claude/skills` below the folder, each prefixed
    /// with its path from there.
    fn nested(&mut self, input: &ScanInput) -> Vec<Found> {
        let mut found = Vec::new();
        let mut todo: Vec<(PathBuf, usize)> = vec![(input.cwd.clone(), 0)];
        while let Some((dir, depth)) = todo.pop() {
            let mut children = Vec::new();
            for name in self.list(&dir) {
                if name.starts_with('.') || ignored(&name) || name == "node_modules" {
                    continue;
                }
                if !self.spend() {
                    return found;
                }
                let child = dir.join(&name);
                if !matches!(self.fs.node(&child), Ok(Some(Node::Dir))) {
                    continue;
                }
                let skills = child.join(".claude/skills");
                if matches!(self.fs.node(&skills), Ok(Some(_))) {
                    let prefix = child
                        .strip_prefix(&input.cwd)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(name);
                    let root =
                        Root::new(skills, SkillPlace::Project, "nested", false).prefixed(prefix);
                    found.extend(self.root(&root));
                }
                if depth < MAX_DEPTH {
                    children.push((child, depth + 1));
                }
            }
            todo.extend(children.into_iter().rev());
        }
        found
    }

    /// The folder's tree, once per folder, or `None` when it can't be
    /// hashed.
    fn tree(&mut self, real: &Path) -> Option<TreeHash> {
        if let Some(tree) = self.trees.get(real) {
            return tree.clone();
        }
        let tree = tree::of(self.fs, real).ok();
        self.trees.insert(real.to_owned(), tree.clone());
        tree
    }

    /// Turns what `cli` found, in the order it prefers, into what it sees:
    /// one skill per folder, the first copy of a name winning except in
    /// Codex, and the marks.
    fn settle(&mut self, cli: Vendor, found: Vec<Found>, always: &[String]) -> Vec<EffectiveSkill> {
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut skills: Vec<(EffectiveSkill, PathBuf, bool)> = Vec::new();
        for found in found {
            if seen.contains(&found.real) {
                continue;
            }
            seen.push(found.real.clone());
            if let Some(skill) = effective(cli, &found, always) {
                skills.push((skill, found.real, found.vendored));
            }
        }
        let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (at, (skill, _, _)) in skills.iter().enumerate() {
            names.entry(skill.name.clone()).or_default().push(at);
        }
        for copies in names.values().filter(|copies| copies.len() > 1) {
            let first = self.tree(&skills[copies[0]].1);
            let mut same = first.is_some();
            for &at in &copies[1..] {
                same &= self.tree(&skills[at].1) == first;
            }
            let mark = if same {
                SkillMark::Same
            } else {
                SkillMark::Conflict
            };
            let winner = skills[copies[0]].0.place;
            for (index, &at) in copies.iter().enumerate() {
                let skill = &mut skills[at].0;
                skill.mark = Some(mark);
                if index > 0 && cli != Vendor::Codex {
                    skill.enabled = false;
                    if skill.place == SkillPlace::Project && winner != SkillPlace::Project {
                        skill.mark = Some(SkillMark::Shadowed);
                    }
                }
            }
        }
        skills
            .into_iter()
            .map(|(mut skill, _, vendored)| {
                if vendored && matches!(skill.mark, None | Some(SkillMark::Same)) {
                    skill.mark = Some(SkillMark::Vendored);
                }
                skill
            })
            .collect()
    }
}

/// What `cli` makes of one folder, or `None` when it doesn't take it as a
/// skill: Codex needs a name and a description, and Cursor skips a file it
/// can't read or one kept off its surface.
fn effective(cli: Vendor, found: &Found, always: &[String]) -> Option<EffectiveSkill> {
    let doc = match &found.doc {
        Some(Ok(doc)) => Some(doc),
        Some(Err(_)) | None if matches!(cli, Vendor::Codex | Vendor::Cursor) => return None,
        _ => None,
    };
    if cli == Vendor::Cursor && !doc.is_some_and(|d| d.on_surface("cli")) {
        return None;
    }
    let base = match cli {
        Vendor::Codex => {
            let doc = doc?;
            doc.description()?;
            doc.name()?.to_owned()
        }
        _ => found.folder.clone(),
    };
    let name = match &found.prefix {
        Some(prefix) => format!("{prefix}:{base}"),
        None => base.clone(),
    };
    let place = if found.place == SkillPlace::Global && always.contains(&base) {
        SkillPlace::Always
    } else {
        found.place
    };
    let mut skill = EffectiveSkill {
        name,
        description: doc.and_then(SkillDoc::description).map(str::to_owned),
        path: found.dir.join(SKILL_FILE).to_string_lossy().into_owned(),
        scope: Some(found.scope.to_owned()),
        enabled: true,
        display_name: None,
        short_description: None,
        user_invocation_only: None,
        user_invocable: None,
        place,
        cli,
        mark: None,
    };
    if cli == Vendor::Codex {
        let meta = found.codex.clone().unwrap_or_default();
        skill.display_name = meta.display_name;
        skill.short_description = meta.short_description;
        if meta.allow_implicit_invocation == Some(false) {
            skill.user_invocation_only = Some(true);
        }
    } else if let Some(doc) = doc {
        skill.display_name = doc
            .name()
            .filter(|name| *name != found.folder)
            .map(str::to_owned);
        if doc.user_invocation_only() {
            skill.user_invocation_only = Some(true);
        }
        if !doc.user_invocable() {
            skill.user_invocable = Some(false);
        }
    }
    Some(skill)
}

#[cfg(test)]
mod tests {
    use super::super::mem::MemFs;
    use super::*;

    const HOME: &str = "/home/me";
    const REPO: &str = "/work/repo";

    fn skill_md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\nBody of {name}.\n")
    }

    fn put(fs: &mut MemFs, dir: &str, name: &str, description: &str) {
        fs.file(
            format!("{dir}/{name}/SKILL.md"),
            skill_md(name, description),
        );
    }

    fn input() -> ScanInput {
        let mut input = ScanInput::new(REPO, HOME);
        input.repo_root = Some(REPO.into());
        input.codex_admin = None;
        input
    }

    fn of(scan: &Scan, cli: Vendor) -> Vec<&EffectiveSkill> {
        scan.skills.iter().filter(|s| s.cli == cli).collect()
    }

    fn named<'a>(scan: &'a Scan, cli: Vendor, name: &str) -> Vec<&'a EffectiveSkill> {
        of(scan, cli)
            .into_iter()
            .filter(|s| s.name == name)
            .collect()
    }

    #[test]
    fn claude_runs_a_personal_skill_over_a_project_one() {
        let mut fs = MemFs::new();
        put(&mut fs, "/home/me/.claude/skills", "deploy", "mine");
        put(
            &mut fs,
            "/work/repo/.claude/skills",
            "deploy",
            "the project's",
        );
        let scan = scan(&fs, &input());
        let copies = named(&scan, Vendor::Claude, "deploy");
        assert_eq!(copies.len(), 2);
        assert_eq!(copies[0].place, SkillPlace::Global);
        assert!(copies[0].enabled);
        assert_eq!(copies[0].mark, Some(SkillMark::Conflict));
        assert_eq!(copies[1].place, SkillPlace::Project);
        assert!(!copies[1].enabled);
        assert_eq!(copies[1].mark, Some(SkillMark::Shadowed));
        assert!(!scan.incomplete);
    }

    #[test]
    fn codex_shows_both_copies_and_tells_same_from_conflict() {
        let mut fs = MemFs::new();
        put(&mut fs, "/home/me/.codex/skills", "git-flow", "same");
        put(&mut fs, "/home/me/.agents/skills", "git-flow", "same");
        put(&mut fs, "/home/me/.codex/skills", "unslop", "old cut");
        put(&mut fs, "/home/me/.agents/skills", "unslop", "new cut");
        let scan = scan(&fs, &input());
        for (name, mark) in [
            ("git-flow", SkillMark::Same),
            ("unslop", SkillMark::Conflict),
        ] {
            let copies = named(&scan, Vendor::Codex, name);
            assert_eq!(copies.len(), 2, "{name}");
            assert!(
                copies.iter().all(|s| s.enabled && s.mark == Some(mark)),
                "{name}"
            );
        }
    }

    #[test]
    fn codex_reads_its_system_skills_and_metadata() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/home/me/.codex/skills/.system",
            "imagegen",
            "Make images.",
        );
        put(&mut fs, "/work/repo/.agents/skills", "unslop", "Cut tells.");
        fs.file(
            "/work/repo/.agents/skills/unslop/agents/openai.yaml",
            "interface:\n  display_name: Unslop\npolicy:\n  allow_implicit_invocation: false\n",
        );
        let scan = scan(&fs, &input());
        let system = named(&scan, Vendor::Codex, "imagegen");
        assert_eq!(system[0].place, SkillPlace::System);
        assert_eq!(system[0].scope.as_deref(), Some("system"));
        let repo = named(&scan, Vendor::Codex, "unslop");
        assert_eq!(repo[0].place, SkillPlace::Project);
        assert_eq!(repo[0].display_name.as_deref(), Some("Unslop"));
        assert_eq!(repo[0].user_invocation_only, Some(true));
        // Codex's `.system` is not one of your skills to Claude or Cursor.
        assert!(named(&scan, Vendor::Cursor, "imagegen").is_empty());
    }

    #[test]
    fn codex_skips_a_skill_with_no_name_or_description() {
        let mut fs = MemFs::new();
        fs.file(
            "/work/repo/.agents/skills/bare/SKILL.md",
            "# No frontmatter\n",
        );
        let scan = scan(&fs, &input());
        assert!(named(&scan, Vendor::Codex, "bare").is_empty());
        assert_eq!(named(&scan, Vendor::Acp, "bare")[0].description, None);
    }

    #[test]
    fn cursor_reads_four_project_folders_and_takes_the_first_copy() {
        let mut fs = MemFs::new();
        put(&mut fs, "/work/repo/.agents/skills", "a", "agents");
        put(&mut fs, "/work/repo/.cursor/skills", "b", "cursor");
        put(&mut fs, "/work/repo/.claude/skills", "c", "claude");
        put(&mut fs, "/work/repo/.codex/skills", "d", "codex");
        put(&mut fs, "/work/repo/.claude/skills", "a", "another a");
        let scan = scan(&fs, &input());
        let names: Vec<(&str, bool)> = of(&scan, Vendor::Cursor)
            .iter()
            .map(|s| (s.name.as_str(), s.enabled))
            .collect();
        assert_eq!(
            names,
            [
                ("a", true),
                ("b", true),
                ("a", false),
                ("c", true),
                ("d", true)
            ]
        );
        assert_eq!(
            named(&scan, Vendor::Cursor, "a")[1].mark,
            Some(SkillMark::Conflict)
        );
    }

    #[test]
    fn cursor_never_reads_its_own_home_folder() {
        let mut fs = MemFs::new();
        put(&mut fs, "/home/me/.cursor/skills", "secret", "never read");
        put(
            &mut fs,
            "/home/me/.cursor/skills-cursor",
            "builtin",
            "never read",
        );
        let scan = scan(&fs, &input());
        assert!(scan.skills.is_empty());
    }

    #[test]
    fn cursor_hides_broken_and_off_surface_skills() {
        let mut fs = MemFs::new();
        fs.file(
            "/work/repo/.agents/skills/broken/SKILL.md",
            "---\ndescription: Use when: x\n---\n",
        )
        .file(
            "/work/repo/.agents/skills/web/SKILL.md",
            "---\nname: web\ndescription: d\nmetadata:\n  surfaces: [app]\n---\n",
        );
        let scan = scan(&fs, &input());
        assert!(of(&scan, Vendor::Cursor).is_empty());
        let claude: Vec<_> = of(&scan, Vendor::Acp)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(claude, ["broken", "web"]);
    }

    #[test]
    fn one_tree_through_two_links_loads_once() {
        let mut fs = MemFs::new();
        put(&mut fs, "/work/repo/.maxhq/skills", "unslop", "Cut tells.");
        for dir in [".claude", ".agents"] {
            fs.link(
                format!("/work/repo/{dir}/skills/unslop"),
                "../../.maxhq/skills/unslop",
            );
        }
        let scan = scan(&fs, &input());
        let cursor = named(&scan, Vendor::Cursor, "unslop");
        assert_eq!(cursor.len(), 1);
        assert_eq!(cursor[0].mark, None);
        assert_eq!(named(&scan, Vendor::Claude, "unslop")[0].mark, None);
    }

    #[test]
    fn devflow_links_are_marked_vendored() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/work/repo/.devflow-vendor/skills",
            "df-plan",
            "Plan.",
        );
        fs.link(
            "/work/repo/.claude/skills/df-plan",
            "../../.devflow-vendor/skills/df-plan",
        );
        let scan = scan(&fs, &input());
        let found = named(&scan, Vendor::Claude, "df-plan");
        assert_eq!(found[0].mark, Some(SkillMark::Vendored));
        assert_eq!(found[0].place, SkillPlace::Project);
    }

    #[test]
    fn a_mounted_set_is_not_vendored() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/opt/maxhq/skills/sets/abc",
            "unslop",
            "Cut tells.",
        );
        fs.link("/opt/maxhq/skills/current", "sets/abc")
            .link("/work/repo/.maxhq/skills", "/opt/maxhq/skills/current")
            .link(
                "/work/repo/.claude/skills/unslop",
                "../../.maxhq/skills/unslop",
            );
        let scan = scan(&fs, &input());
        let found = named(&scan, Vendor::Claude, "unslop");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mark, None);
    }

    #[test]
    fn nested_and_added_skills_are_prefixed() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/work/repo/packages/web/.claude/skills",
            "deploy",
            "web",
        );
        put(&mut fs, "/work/other/.claude/skills", "deploy", "other");
        put(
            &mut fs,
            "/work/repo/node_modules/x/.claude/skills",
            "junk",
            "skip",
        );
        let mut input = input();
        input.added_dirs.push("/work/other".into());
        let scan = scan(&fs, &input);
        let names: Vec<_> = of(&scan, Vendor::Claude)
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    s.scope.clone().unwrap_or_default(),
                    s.enabled,
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("other:deploy".to_owned(), "added".to_owned(), true),
                ("packages/web:deploy".to_owned(), "nested".to_owned(), true),
            ]
        );
    }

    #[test]
    fn plugin_skills_are_namespaced_for_their_cli() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/home/me/.claude/plugins/fd/skills",
            "frontend-design",
            "UI.",
        );
        let mut input = input();
        input.plugins.push(Plugin {
            name: "frontend-design".into(),
            root: "/home/me/.claude/plugins/fd".into(),
            cli: Vendor::Claude,
        });
        let scan = scan(&fs, &input);
        let found = named(&scan, Vendor::Claude, "frontend-design:frontend-design");
        assert_eq!(found[0].place, SkillPlace::Plugin);
        assert!(of(&scan, Vendor::Cursor).is_empty());
    }

    #[test]
    fn synced_and_managed_skills_are_marked_as_not_yours() {
        let mut fs = MemFs::new();
        put(
            &mut fs,
            "/home/me/.claude/skills/synced",
            "notes",
            "From claude.ai.",
        );
        put(&mut fs, "/etc/claude/skills", "policy", "Company.");
        put(&mut fs, "/home/me/.claude/skills", "policy", "Mine.");
        let mut input = input();
        input.claude_managed = Some("/etc/claude/skills".into());
        let scan = scan(&fs, &input);
        let synced = named(&scan, Vendor::Claude, "notes");
        assert_eq!(synced.len(), 1);
        assert_eq!(
            (synced[0].place, synced[0].scope.as_deref()),
            (SkillPlace::System, Some("synced"))
        );
        let policy = named(&scan, Vendor::Claude, "policy");
        assert_eq!(policy[0].scope.as_deref(), Some("enterprise"));
        assert!(policy[0].enabled);
        assert!(!policy[1].enabled);
        assert_eq!(policy[1].mark, Some(SkillMark::Conflict));
        assert!(named(&scan, Vendor::Cursor, "notes").is_empty());
    }

    #[test]
    fn a_worktree_with_no_skills_falls_back_to_the_main_checkout() {
        let mut fs = MemFs::new();
        put(&mut fs, "/work/repo/.claude/skills", "deploy", "main's");
        fs.dir("/work/repo/.worktrees/feature");
        let mut input = ScanInput::new("/work/repo/.worktrees/feature", HOME);
        input.repo_root = Some("/work/repo/.worktrees/feature".into());
        input.main_checkout = Some(REPO.into());
        let scan = scan(&fs, &input);
        assert_eq!(named(&scan, Vendor::Claude, "deploy").len(), 1);
        assert!(named(&scan, Vendor::Codex, "deploy").is_empty());
    }

    #[test]
    fn project_folders_are_read_from_the_cwd_up_to_the_root() {
        let mut fs = MemFs::new();
        put(&mut fs, "/work/repo/.claude/skills", "root-skill", "d");
        put(&mut fs, "/work/repo/app/.claude/skills", "app-skill", "d");
        put(&mut fs, "/work/.claude/skills", "outside", "d");
        let mut input = input();
        input.cwd = "/work/repo/app".into();
        let scan = scan(&fs, &input);
        let names: Vec<_> = of(&scan, Vendor::Claude)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(names, ["app-skill", "root-skill"]);
    }

    #[test]
    fn the_always_set_and_claude_fields_come_through() {
        let mut fs = MemFs::new();
        fs.file(
            "/home/me/.claude/skills/unslop/SKILL.md",
            "---\nname: Unslop\ndescription: d\ndisable-model-invocation: true\nuser-invocable: false\n---\n",
        );
        let mut input = input();
        input.always = vec!["unslop".into()];
        let scan = scan(&fs, &input);
        let found = named(&scan, Vendor::Claude, "unslop");
        assert_eq!(found[0].place, SkillPlace::Always);
        assert_eq!(found[0].display_name.as_deref(), Some("Unslop"));
        assert_eq!(found[0].user_invocation_only, Some(true));
        assert_eq!(found[0].user_invocable, Some(false));
        assert_eq!(found[0].path, "/home/me/.claude/skills/unslop/SKILL.md");
    }

    #[test]
    fn acp_agents_that_read_no_skills_see_none() {
        let mut fs = MemFs::new();
        put(&mut fs, "/work/repo/.agents/skills", "a", "d");
        let mut input = input();
        assert_eq!(of(&scan(&fs, &input), Vendor::Acp).len(), 1);
        input.acp = AcpSkills::None;
        assert!(of(&scan(&fs, &input), Vendor::Acp).is_empty());
    }

    #[test]
    fn deep_roots_find_grouped_skills() {
        let mut fs = MemFs::new();
        put(&mut fs, "/home/me/.agents/skills/group", "inner", "d");
        let scan = scan(&fs, &input());
        assert_eq!(named(&scan, Vendor::Codex, "inner").len(), 1);
        assert!(named(&scan, Vendor::Claude, "inner").is_empty());
    }

    #[test]
    fn a_scan_past_its_limit_says_it_is_incomplete() {
        let mut fs = MemFs::new();
        for n in 0..=MAX_ENTRIES {
            fs.dir(format!("/work/repo/.agents/skills/empty-{n}"));
        }
        assert!(scan(&fs, &input()).incomplete);
    }
}
