// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the resolver for
// how a CLI updates, and the Claude and Codex drivers' native update rules.
// MaxHQ only runs updates you own the files for: a prefix you can't write,
// or one under nix, mise or another package's node_modules, gets the
// command to run yourself.

//! How an agent CLI on the device was installed, and how to update it.

use std::path::Path;

use maxhq_core::shell::word;
use maxhq_protocol::model::Tool;

use super::defs::Def;

/// The file facts resolving needs, so tests can fake them.
pub trait Probe {
    /// Whether `path` exists.
    fn exists(&self, path: &str) -> bool;
    /// Whether this user can write in the folder `path`.
    fn writable(&self, path: &str) -> bool;
    /// The start of a small file, or none if it's large or unreadable.
    fn small_file(&self, path: &str) -> Option<String>;
    /// What `program args` prints, trimmed, or none if it fails.
    fn output(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// The command an update runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The program, as a path or a name on PATH.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    /// Updates with the same lock never run at once.
    pub lock: String,
}

impl Command {
    /// The command as you'd type it.
    pub fn display(&self) -> String {
        let program = Path::new(&self.program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.clone());
        std::iter::once(word(&program))
            .chain(self.args.iter().map(|arg| word(arg)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How to update one install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// MaxHQ can run this.
    Run(Command),
    /// You have to. `command` is what to run, or how to go about it.
    Manual {
        /// What to run.
        command: String,
    },
    /// Nobody through MaxHQ: it never updates this CLI.
    Never,
}

/// What resolving found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// How it was installed, such as `native` or `npm`.
    pub method: &'static str,
    /// How to update it.
    pub action: Action,
}

/// Resolves the install at `resolved` (the path PATH gives) whose real
/// path is `real`.
pub fn resolve(def: &Def, resolved: &str, real: &str, probe: &dyn Probe) -> Resolved {
    let Some(package) = def.package else {
        return Resolved {
            method: "native",
            action: Action::Never,
        };
    };
    let paths = [resolved, real];
    let any = |test: fn(&str) -> bool| paths.iter().any(|path| test(&normalize(path)));
    let manual = |method: &'static str, command: String| Resolved {
        method,
        action: Action::Manual { command },
    };
    let elsewhere = || "Update it with the app or tool that installed it.".to_owned();

    let native_matches = paths.iter().any(|path| native_path(def.tool, path));
    let mise_wrapper = is_mise_wrapper(real, probe);
    let native = native_command(def, resolved, real).map(|command| {
        let folder = parent(real);
        if probe.writable(&folder) {
            Resolved {
                method: "native",
                action: Action::Run(command),
            }
        } else {
            manual("native", command.display())
        }
    });
    if native_matches
        && !any(is_mise)
        && !mise_wrapper
        && let Some(native) = native.clone()
    {
        return native;
    }
    // A node_modules path not proven below belongs to another package or a
    // project, so the CLI's own updater could act on the wrong install.
    let requires_path = def.tool == Tool::Codex;
    let fallback = if requires_path || any(|path| path.contains("/node_modules/")) {
        manual("unknown", elsewhere())
    } else {
        native.unwrap_or_else(|| manual("unknown", elsewhere()))
    };

    if any(|path| path.contains("/nix/store/")) {
        return manual("nix", "Update it through Nix.".to_owned());
    }
    if any(is_vite_plus) {
        return manual("vite+", format!("vp i -g {package}"));
    }
    if any(|path| path.contains("/.bun/bin/")) {
        return run_or_manual(
            "bun",
            &parent(real),
            command(
                "bun",
                &["i", "-g", &format!("{package}@latest")],
                "bun-global",
            ),
            probe,
        );
    }
    if any(is_pnpm) {
        // pnpm 10 skips unapproved scripts, and the approval flag needs 10.4.
        let allow = probe
            .output("pnpm", &["--version"])
            .and_then(|text| crate::recipe::steps::Version::parse(text.trim()))
            .is_some_and(|version| {
                version.at_least(crate::recipe::steps::Version {
                    major: 10,
                    minor: 4,
                    patch: 0,
                    pre: false,
                })
            });
        let allow_build = format!("--allow-build={package}");
        let latest = format!("{package}@latest");
        let mut args = vec!["add", "-g"];
        if allow {
            args.push(&allow_build);
        }
        args.push(&latest);
        return run_or_manual(
            "pnpm",
            &parent(real),
            command("pnpm", &args, "pnpm-global"),
            probe,
        );
    }
    if any(|path| {
        path.contains("/yarn/global/node_modules/")
            || path.contains("/yarn/data/global/node_modules/")
    }) {
        return manual("yarn", format!("yarn global add {package}@latest"));
    }
    if Path::new(&normalize(real))
        .file_name()
        .is_some_and(|name| name == "volta-shim")
    {
        return manual("volta", format!("volta install {package}@latest"));
    }

    // npm proof outranks a containing Homebrew keg.
    if let Some(prefix) = npm_prefix(real, package) {
        let npm = command(
            "npm",
            &[
                "install",
                "-g",
                "--prefix",
                &prefix,
                &format!("--allow-scripts={package}"),
                &format!("{package}@latest"),
            ],
            &format!("npm-global:{}", normalize(&prefix)),
        );
        let modules = format!("{}/lib/node_modules", prefix.trim_end_matches('/'));
        return run_or_manual("npm", &modules, npm, probe);
    }

    let brew = homebrew(real);
    if any(is_mise)
        || brew
            .as_ref()
            .is_some_and(|brew| !brew.cask && brew.name.eq_ignore_ascii_case("mise"))
        || mise_wrapper
    {
        return manual("mise", "Update it through mise.".to_owned());
    }
    if let Some(brew) = brew {
        let keg_brew = format!("{}/bin/brew", brew.prefix);
        if probe.exists(&keg_brew)
            && probe
                .output(&keg_brew, &["--prefix"])
                .is_some_and(|prefix| normalize(prefix.trim()) == normalize(&brew.prefix))
        {
            let mut args = vec!["upgrade"];
            if brew.cask {
                args.push("--cask");
            }
            args.push(&brew.name);
            return run_or_manual(
                "brew",
                &brew.prefix,
                command(&keg_brew, &args, "homebrew"),
                probe,
            );
        }
    }
    fallback
}

fn command(program: &str, args: &[&str], lock: &str) -> Command {
    Command {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: Vec::new(),
        lock: lock.to_owned(),
    }
}

/// Runs `command` when this user can write `folder`, and otherwise shows it.
fn run_or_manual(
    method: &'static str,
    folder: &str,
    command: Command,
    probe: &dyn Probe,
) -> Resolved {
    let action = if probe.writable(folder) {
        Action::Run(command)
    } else {
        Action::Manual {
            command: command.display(),
        }
    };
    Resolved { method, action }
}

/// The CLI's own updater, when its path is one it manages.
fn native_command(def: &Def, resolved: &str, real: &str) -> Option<Command> {
    let args = def.self_update?;
    let mut command = command(resolved, args, &format!("{}-native", def.binary));
    if def.tool == Tool::Codex {
        // Standalone updates find their install through CODEX_HOME.
        let home = codex_standalone_home(real).or_else(|| codex_standalone_home(resolved))?;
        command.env.push(("CODEX_HOME".to_owned(), home));
    }
    Some(command)
}

fn native_path(tool: Tool, path: &str) -> bool {
    match tool {
        Tool::Claude => {
            let path = normalize(path);
            path.ends_with("/.local/bin/claude") || path.contains("/.local/share/claude/")
        }
        Tool::Codex => codex_standalone_home(path).is_some(),
        Tool::Cursor => false,
    }
}

/// The Codex home that holds a standalone install's `packages/standalone`.
fn codex_standalone_home(path: &str) -> Option<String> {
    let slashed = path.replace('\\', "/");
    let index = slashed.to_lowercase().find("/packages/standalone/")?;
    let home = &slashed[..index];
    Some(if home.is_empty() {
        "/".to_owned()
    } else {
        home.to_owned()
    })
}

/// Lowercase with forward slashes, for matching.
pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn is_mise(path: &str) -> bool {
    path.contains("/mise/installs/") || path.contains("/mise/shims/")
}

fn is_vite_plus(path: &str) -> bool {
    path.contains("/.vite-plus/bin/") || path.contains("/.local/share/vite-plus/bin/")
}

fn is_pnpm(path: &str) -> bool {
    [
        "/.local/share/pnpm/",
        "/library/pnpm/",
        "/local/share/pnpm/",
        "/pnpm/global/",
    ]
    .iter()
    .any(|part| path.contains(part))
}

/// A script that runs the CLI through `mise x` or `mise exec`.
fn is_mise_wrapper(real: &str, probe: &dyn Probe) -> bool {
    let Some(script) = probe.small_file(real) else {
        return false;
    };
    script.starts_with("#!")
        && script
            .split(|c: char| !c.is_alphanumeric())
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[0] == "mise" && (pair[1] == "x" || pair[1] == "exec"))
}

/// The npm prefix a global install's real path sits under.
pub fn npm_prefix(real: &str, package: &str) -> Option<String> {
    let slashed = real.replace('\\', "/");
    let lower = slashed.to_lowercase();
    let segment = format!("/lib/node_modules/{}/", package.to_lowercase());
    let index = lower.rfind(&segment)?;
    if lower[..index].contains("/node_modules/") {
        return None;
    }
    // Mise tool layouts look like npm globals. Only those under its Node
    // belong to npm.
    let before = &lower[..index];
    if let Some(at) = before.rfind("/mise/installs/") {
        let tool = before[at + "/mise/installs/".len()..].split('/').next();
        if tool.is_some_and(|tool| tool != "node") {
            return None;
        }
    }
    Some(if index == 0 {
        "/".to_owned()
    } else {
        slashed[..index].to_owned()
    })
}

struct Homebrew {
    cask: bool,
    name: String,
    prefix: String,
}

/// The Homebrew keg a real path sits in: `<prefix>/Cellar/<name>/<version>/`.
fn homebrew(real: &str) -> Option<Homebrew> {
    let slashed = real.replace('\\', "/");
    let parts: Vec<&str> = slashed.split('/').collect();
    // The last keg folder wins, as the greedy pattern does.
    let at = (0..parts.len().saturating_sub(3)).rev().find(|&i| {
        let part = parts[i].to_lowercase();
        (part == "cellar" || part == "caskroom")
            && !parts[i + 1].is_empty()
            && !parts[i + 2].is_empty()
    })?;
    Some(Homebrew {
        cask: parts[at].eq_ignore_ascii_case("caskroom"),
        name: parts[at + 1].to_owned(),
        prefix: parts[..at].join("/"),
    })
}

fn parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::super::defs::{CLAUDE, CODEX, CURSOR};
    use super::*;

    /// Writable folders, files and command output, all made up.
    #[derive(Default)]
    struct Fake {
        writable: HashSet<String>,
        files: HashMap<String, String>,
        outputs: HashMap<String, String>,
    }

    impl Fake {
        fn writable(paths: &[&str]) -> Self {
            Self {
                writable: paths.iter().map(|path| (*path).to_owned()).collect(),
                ..Self::default()
            }
        }
    }

    impl Probe for Fake {
        fn exists(&self, path: &str) -> bool {
            self.files.contains_key(path) || self.outputs.keys().any(|key| key.starts_with(path))
        }
        fn writable(&self, path: &str) -> bool {
            self.writable.contains(path)
        }
        fn small_file(&self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn output(&self, program: &str, args: &[&str]) -> Option<String> {
            self.outputs
                .get(&format!("{program} {}", args.join(" ")))
                .cloned()
        }
    }

    fn run(resolved: &Resolved) -> &Command {
        match &resolved.action {
            Action::Run(command) => command,
            other => panic!("doesn't run: {other:?}"),
        }
    }

    fn manual(resolved: &Resolved) -> &str {
        match &resolved.action {
            Action::Manual { command } => command,
            other => panic!("isn't manual: {other:?}"),
        }
    }

    #[test]
    fn claude_s_native_install_updates_itself() {
        let path = "/home/dev/.local/bin/claude";
        let real = "/home/dev/.local/share/claude/versions/2.1.211";
        let probe = Fake::writable(&["/home/dev/.local/share/claude/versions"]);
        let resolved = resolve(&CLAUDE, path, real, &probe);
        assert_eq!(resolved.method, "native");
        let command = run(&resolved);
        assert_eq!(command.program, path);
        assert_eq!(command.args, ["update"]);
        assert_eq!(command.lock, "claude-native");
    }

    #[test]
    fn codex_s_standalone_install_updates_with_its_home() {
        let real = "/home/dev/.codex/packages/standalone/0.164.0/bin/codex";
        let probe = Fake::writable(&["/home/dev/.codex/packages/standalone/0.164.0/bin"]);
        let resolved = resolve(&CODEX, "/home/dev/.local/bin/codex", real, &probe);
        assert_eq!(resolved.method, "native");
        let command = run(&resolved);
        assert_eq!(
            command.env,
            [("CODEX_HOME".to_owned(), "/home/dev/.codex".to_owned())]
        );
    }

    #[test]
    fn a_writable_npm_prefix_updates_through_npm() {
        let real = "/home/dev/.npm-global/lib/node_modules/@openai/codex/bin/codex.js";
        let probe = Fake::writable(&["/home/dev/.npm-global/lib/node_modules"]);
        let resolved = resolve(&CODEX, "/home/dev/.npm-global/bin/codex", real, &probe);
        assert_eq!(resolved.method, "npm");
        let command = run(&resolved);
        assert_eq!(command.program, "npm");
        assert_eq!(
            command.args,
            [
                "install",
                "-g",
                "--prefix",
                "/home/dev/.npm-global",
                "--allow-scripts=@openai/codex",
                "@openai/codex@latest"
            ]
        );
        assert_eq!(command.lock, "npm-global:/home/dev/.npm-global");
    }

    #[test]
    fn a_root_owned_npm_prefix_is_manual() {
        let real = "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js";
        let resolved = resolve(&CLAUDE, "/usr/bin/claude", real, &Fake::default());
        assert_eq!(resolved.method, "npm");
        assert_eq!(
            manual(&resolved),
            "npm install -g --prefix /usr --allow-scripts=@anthropic-ai/claude-code \
             @anthropic-ai/claude-code@latest"
        );
    }

    #[test]
    fn a_root_owned_native_install_is_manual() {
        let resolved = resolve(
            &CLAUDE,
            "/root/.local/bin/claude",
            "/root/.local/share/claude/versions/2.1.211",
            &Fake::default(),
        );
        assert_eq!(manual(&resolved), "claude update");
    }

    #[test]
    fn homebrew_upgrades_its_own_keg() {
        let real = "/opt/homebrew/Caskroom/claude-code/2.1.211/claude";
        let mut probe = Fake::writable(&["/opt/homebrew"]);
        probe.outputs.insert(
            "/opt/homebrew/bin/brew --prefix".to_owned(),
            "/opt/homebrew\n".to_owned(),
        );
        let resolved = resolve(&CLAUDE, "/opt/homebrew/bin/claude", real, &probe);
        assert_eq!(resolved.method, "brew");
        let command = run(&resolved);
        assert_eq!(command.args, ["upgrade", "--cask", "claude-code"]);
        assert_eq!(command.display(), "brew upgrade --cask claude-code");
    }

    #[test]
    fn nix_mise_and_node_modules_are_manual() {
        let probe = Fake::writable(&[
            "/nix/store/abc-claude/bin",
            "/home/dev/project/node_modules/.bin",
        ]);
        let nix = resolve(
            &CLAUDE,
            "/home/dev/.nix-profile/bin/claude",
            "/nix/store/abc-claude/bin/claude",
            &probe,
        );
        assert_eq!(nix.method, "nix");
        manual(&nix);

        let mise = resolve(
            &CODEX,
            "/home/dev/.local/share/mise/shims/codex",
            "/home/dev/.local/share/mise/installs/npm-openai-codex/0.164.0/bin/codex",
            &probe,
        );
        assert_eq!(mise.method, "mise");

        let project = resolve(
            &CLAUDE,
            "/home/dev/project/node_modules/.bin/claude",
            "/home/dev/project/node_modules/@anthropic-ai/claude-code/cli.js",
            &probe,
        );
        assert_eq!(project.method, "unknown");
        manual(&project);
    }

    #[test]
    fn a_mise_wrapper_that_looks_native_is_manual() {
        let path = "/home/dev/.local/bin/claude";
        let mut probe = Fake::writable(&["/home/dev/.local/bin"]);
        probe.files.insert(
            path.to_owned(),
            "#!/bin/sh\nexec mise x npm:@anthropic-ai/claude-code -- claude \"$@\"\n".to_owned(),
        );
        assert_eq!(resolve(&CLAUDE, path, path, &probe).method, "mise");
    }

    #[test]
    fn a_codex_with_no_known_layout_is_manual() {
        let probe = Fake::writable(&["/home/dev/bin"]);
        let resolved = resolve(&CODEX, "/home/dev/bin/codex", "/home/dev/bin/codex", &probe);
        assert_eq!(resolved.method, "unknown");
        manual(&resolved);
        // Cursor has no package, so MaxHQ never updates it, wherever it lives.
        let probe = Fake::writable(&["/home/dev/.local/bin"]);
        let path = "/home/dev/.local/bin/agent";
        let resolved = resolve(&CURSOR, path, path, &probe);
        assert_eq!(resolved.action, Action::Never);
    }

    #[test]
    fn npm_prefix_skips_mise_and_nested_node_modules() {
        assert_eq!(
            npm_prefix(
                "/usr/local/lib/node_modules/@openai/codex/bin/codex.js",
                "@openai/codex"
            )
            .as_deref(),
            Some("/usr/local")
        );
        assert_eq!(
            npm_prefix(
                "/home/dev/.local/share/mise/installs/node/22.0.0/lib/node_modules/@openai/codex/x",
                "@openai/codex"
            )
            .as_deref(),
            Some("/home/dev/.local/share/mise/installs/node/22.0.0")
        );
        assert_eq!(
            npm_prefix(
                "/home/dev/.local/share/mise/installs/npm-x/1.0.0/lib/node_modules/@openai/codex/x",
                "@openai/codex"
            ),
            None
        );
        assert_eq!(
            npm_prefix(
                "/a/node_modules/b/lib/node_modules/@openai/codex/x",
                "@openai/codex"
            ),
            None
        );
    }
}
