// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What MaxHQ knows about each agent CLI: its binary, its npm package, how
//! its own updater runs and how its version reads. A CLI with neither a
//! package nor an updater is one MaxHQ never updates. The recipe's install
//! steps and the tools updater both read these.
//!
//! [`plan`] says how the installer puts each tool in place: the agent CLIs,
//! the helpers Node, uv and gh, and on a Mac the JDK and the Android SDK's
//! command-line tools.

use maxhq_protocol::model::Tool;
use maxhq_protocol::state::{HelperTool, ToolKind};

use super::install::releases::Feed;
use super::install::{ArchiveFile, InstallDef, Source};
use crate::recipe::steps::{
    Version, CODEX_BUILDS, CODEX_RELEASE, CURSOR_DOWNLOADS, CURSOR_VERSION,
};
use crate::recipe::{self, ExecUser};

/// One agent CLI.
#[derive(Debug)]
pub struct Def {
    /// Which CLI.
    pub tool: Tool,
    /// The command's name on PATH.
    pub binary: &'static str,
    /// Its npm package, which also gives its latest version. None for a
    /// CLI MaxHQ never updates.
    pub package: Option<&'static str>,
    /// Where the recipe puts it in a container workspace.
    pub container_path: &'static str,
    /// How the recipe installs it there: `native`, or `pinned` for one
    /// that follows the recipe's release.
    pub container_method: &'static str,
    /// Who runs it in a container workspace.
    pub container_user: ExecUser,
    /// What prints its version.
    pub version_args: &'static [&'static str],
    /// What its own updater takes, when it has one.
    pub self_update: Option<&'static [&'static str]>,
}

/// Claude Code.
pub const CLAUDE: Def = Def {
    tool: Tool::Claude,
    binary: "claude",
    package: Some("@anthropic-ai/claude-code"),
    container_path: recipe::CLAUDE,
    container_method: "native",
    container_user: ExecUser::Agent,
    version_args: &["--version"],
    self_update: Some(&["update"]),
};

/// The Codex CLI.
pub const CODEX: Def = Def {
    tool: Tool::Codex,
    binary: "codex",
    package: Some("@openai/codex"),
    container_path: recipe::CODEX,
    container_method: "pinned",
    container_user: ExecUser::Root,
    version_args: &["--version"],
    // Codex's standalone install updates itself through `codex update`,
    // but only where its own launcher can find its home.
    self_update: Some(&["update"]),
};

/// The Cursor CLI. MaxHQ never updates it, and
/// containers keep the release `agent acp` was checked against.
pub const CURSOR: Def = Def {
    tool: Tool::Cursor,
    binary: "agent",
    package: None,
    container_path: recipe::CURSOR,
    container_method: "pinned",
    container_user: ExecUser::Agent,
    version_args: &["--version"],
    self_update: None,
};

impl Def {
    /// Whether MaxHQ ever updates it.
    pub fn updates(&self) -> bool {
        self.package.is_some()
    }

    /// Whether MaxHQ updates it in a container workspace. One pinned
    /// there follows the recipe's release, which moves with maxd.
    pub fn updates_in_container(&self) -> bool {
        self.updates() && self.container_method != "pinned"
    }
}

/// The definition for `tool`.
pub fn def(tool: Tool) -> &'static Def {
    match tool {
        Tool::Claude => &CLAUDE,
        Tool::Codex => &CODEX,
        Tool::Cursor => &CURSOR,
    }
}

/// Every CLI MaxHQ knows.
pub const ALL: [&Def; 3] = [&CLAUDE, &CODEX, &CURSOR];

/// The Codex release containers keep, which the recipe checks for.
pub const CODEX_PINNED: &str = "0.164.0";

/// The JDK MaxHQ installs for the Android SDK when the Mac has none:
/// Eclipse Temurin, which publishes a SHA-256 for each build.
pub const JDK_VERSION: &str = "21.0.13+9";
const JDK_RELEASE: &str =
    "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9";
/// Temurin's Mac builds of [`JDK_VERSION`]: target, file, SHA-256, size.
const JDK_BUILDS: [(&str, &str, &str, u64); 2] = [
    (
        "aarch64-apple-darwin",
        "OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.13_9.tar.gz",
        "bf036b35b071bfb2f54f2c0af8ee8d88afa95ee6eb3dce02c0f94cfc2524d04a",
        199_754_598,
    ),
    (
        "x86_64-apple-darwin",
        "OpenJDK21U-jdk_x64_mac_hotspot_21.0.13_9.tar.gz",
        "9aa1d1c17ae5467a022679e7e0af5c00797d94ac462ed1a46f4db07a8a34a6dd",
        193_957_782,
    ),
];

/// The Android SDK command-line tools MaxHQ installs, as Google numbers
/// the package.
pub const ANDROID_TOOLS_VERSION: &str = "23.0";
const ANDROID_TOOLS_RELEASE: &str = "https://dl.google.com/android/repository";
/// Google's Mac builds of [`ANDROID_TOOLS_VERSION`]: target, file, SHA-256,
/// size. Google lists a SHA-1 in its repository and the SHA-256 on its
/// download page.
const ANDROID_TOOLS_BUILDS: [(&str, &str, &str, u64); 2] = [
    (
        "aarch64-apple-darwin",
        "commandlinetools-mac_arm64-16111833_latest.zip",
        "58da8f215781cc0208743e88b65ae259780c55e7eb40e50373d377cf369232d6",
        155_384_151,
    ),
    (
        "x86_64-apple-darwin",
        "commandlinetools-mac_x86_64-16111833_latest.zip",
        "7e601c04e7173754b3d051a87bb4c3a8243c65a9d7bfa9356ed6499fb8953fa3",
        155_582_504,
    ),
];

/// Pinned archives at `release`, one per target.
fn pinned(release: &str, builds: &[(&str, &str, &str, u64)]) -> Source {
    Source::Archive {
        files: builds
            .iter()
            .map(|(target, file, sha256, size)| ArchiveFile {
                target: (*target).to_owned(),
                url: format!("{release}/{file}"),
                sha256: Some((*sha256).to_owned()),
                size: Some(*size),
            })
            .collect(),
        confirmed_unchecked: false,
    }
}

/// Where a tool's releases are published, for one installed from its
/// latest release.
pub fn feed(kind: &ToolKind) -> Option<Feed> {
    Some(match kind {
        ToolKind::Agent { tool: Tool::Codex } => Feed::GitHub {
            repo: "openai/codex",
            prefix: "rust-v",
            assets: &[
                "codex-{target}.tar.gz",
                "codex-code-mode-host-{target}.tar.gz",
            ],
        },
        ToolKind::Helper {
            tool: HelperTool::Node,
        } => Feed::Node,
        ToolKind::Helper {
            tool: HelperTool::Uv,
        } => Feed::GitHub {
            repo: "astral-sh/uv",
            prefix: "",
            assets: &["uv-{target}.tar.gz"],
        },
        ToolKind::Helper {
            tool: HelperTool::Gh,
        } => Feed::GitHub {
            repo: "cli/cli",
            prefix: "v",
            assets: &["gh_{version}_{gh}"],
        },
        _ => return None,
    })
}

/// How the installer puts `kind` in place, on the device or in a container
/// workspace, or `None` for a tool it can't install yet.
pub fn plan(kind: &ToolKind, workspace: bool) -> Option<InstallDef> {
    let latest = || Source::Archive {
        files: Vec::new(),
        confirmed_unchecked: false,
    };
    let (binary, source) = match kind {
        ToolKind::Agent { tool: Tool::Claude } => (
            Some("claude"),
            Source::Script {
                url: "https://claude.ai/install.sh".to_owned(),
            },
        ),
        ToolKind::Agent { tool: Tool::Codex } if workspace => (
            Some("codex"),
            Source::Archive {
                files: CODEX_BUILDS
                    .iter()
                    .flat_map(|build| {
                        [
                            ("codex", build.sha256),
                            ("codex-code-mode-host", build.host_sha256),
                        ]
                        .map(|(name, sha256)| ArchiveFile {
                            target: build.target.to_owned(),
                            url: format!("{CODEX_RELEASE}/{name}-{}.tar.gz", build.target),
                            sha256: Some(sha256.to_owned()),
                            size: None,
                        })
                    })
                    .collect(),
                confirmed_unchecked: false,
            },
        ),
        ToolKind::Agent { tool: Tool::Codex } => (Some("codex"), latest()),
        // Cursor publishes no checksums for its CLI's packages. The
        // workspace one is the pinned release `agent acp` was checked
        // against, from Cursor's own downloads.
        ToolKind::Agent { tool: Tool::Cursor } if workspace => (
            Some("cursor-agent"),
            Source::Archive {
                files: [
                    ("x86_64-unknown-linux-musl", "x64"),
                    ("aarch64-unknown-linux-musl", "arm64"),
                ]
                .map(|(target, arch)| ArchiveFile {
                    target: target.to_owned(),
                    url: format!(
                        "{CURSOR_DOWNLOADS}/{CURSOR_VERSION}/linux/{arch}/agent-cli-package.tar.gz"
                    ),
                    sha256: None,
                    size: None,
                })
                .to_vec(),
                confirmed_unchecked: true,
            },
        ),
        ToolKind::Agent { tool: Tool::Cursor } => (
            Some("agent"),
            Source::Script {
                url: "https://cursor.com/install".to_owned(),
            },
        ),
        ToolKind::Helper {
            tool: HelperTool::Node,
        } => (None, latest()),
        ToolKind::Helper {
            tool: HelperTool::Uv,
        } => (Some("uv"), latest()),
        ToolKind::Helper {
            tool: HelperTool::Gh,
        } => (Some("gh"), latest()),
        // Neither gets a link on PATH: the Android SDK's own steps run
        // them from their folders.
        ToolKind::Helper {
            tool: HelperTool::Jdk,
        } if !workspace => (None, pinned(JDK_RELEASE, &JDK_BUILDS)),
        ToolKind::Helper {
            tool: HelperTool::AndroidSdk,
        } if !workspace => (None, pinned(ANDROID_TOOLS_RELEASE, &ANDROID_TOOLS_BUILDS)),
        _ => return None,
    };
    Some(InstallDef {
        kind: kind.clone(),
        binary: binary.map(str::to_owned),
        source,
    })
}

/// The version a tool is pinned to, where [`plan`] pins one.
pub fn pin(kind: &ToolKind, workspace: bool) -> Option<&'static str> {
    match kind {
        ToolKind::Agent { tool: Tool::Codex } if workspace => Some(CODEX_PINNED),
        ToolKind::Agent { tool: Tool::Cursor } if workspace => Some(CURSOR_VERSION),
        ToolKind::Helper {
            tool: HelperTool::Jdk,
        } => Some(JDK_VERSION),
        ToolKind::Helper {
            tool: HelperTool::AndroidSdk,
        } => Some(ANDROID_TOOLS_VERSION),
        _ => None,
    }
}

/// The version a `--version` line gives, as in `2.1.211 (Claude Code)` or
/// `codex-cli 0.164.0`: the first word that reads as a version.
pub fn version_of(stdout: &str) -> Option<Version> {
    stdout
        .lines()
        .next()?
        .split_whitespace()
        .find_map(Version::parse)
}

/// [`version_of`] as text, without any leading `v`.
pub fn version_text(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?;
    line.split_whitespace()
        .find(|word| Version::parse(word).is_some())
        .map(|word| word.trim_start_matches('v').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_cli_s_version_line() {
        assert_eq!(
            version_text("2.1.211 (Claude Code)\n").as_deref(),
            Some("2.1.211")
        );
        assert_eq!(
            version_text("codex-cli 0.164.0\n").as_deref(),
            Some("0.164.0")
        );
        assert_eq!(version_of("codex-cli 0.164.0").map(|v| v.minor), Some(164));
        let cursor = version_of("2026.10.22-27dcf5f\n").unwrap();
        assert_eq!((cursor.major, cursor.minor, cursor.patch), (2026, 10, 22));
        assert_eq!(version_text("command not found"), None);
    }

    #[test]
    fn plans_cover_the_clis_and_the_helpers() {
        for def in ALL {
            let kind = ToolKind::Agent { tool: def.tool };
            for workspace in [false, true] {
                let plan = plan(&kind, workspace).unwrap();
                assert!(plan.binary.is_some());
            }
        }
        for tool in [HelperTool::Node, HelperTool::Uv, HelperTool::Gh] {
            let kind = ToolKind::Helper { tool };
            assert!(plan(&kind, false).is_some());
            assert!(feed(&kind).is_some());
        }
        assert!(plan(
            &ToolKind::Helper {
                tool: HelperTool::AgentDevice
            },
            false
        )
        .is_none());
        for tool in [HelperTool::AndroidSdk, HelperTool::Jdk] {
            let kind = ToolKind::Helper { tool };
            assert!(plan(&kind, true).is_none());
            let Some(Source::Archive { files, .. }) = plan(&kind, false).map(|plan| plan.source)
            else {
                panic!("{kind:?} is an archive");
            };
            let targets: Vec<&str> = files.iter().map(|file| file.target.as_str()).collect();
            assert_eq!(targets, ["aarch64-apple-darwin", "x86_64-apple-darwin"]);
            assert!(files
                .iter()
                .all(|file| file.sha256.as_ref().is_some_and(|sum| sum.len() == 64)));
            assert!(pin(&kind, false).is_some());
        }
        let codex = ToolKind::Agent { tool: Tool::Codex };
        let Some(Source::Archive { files, .. }) = plan(&codex, true).map(|plan| plan.source) else {
            panic!("Codex's workspace plan is an archive");
        };
        assert!(files.iter().all(|file| file.sha256.is_some()));
        assert_eq!(pin(&codex, true), Some(CODEX_PINNED));
        assert_eq!(pin(&codex, false), None);
        assert_eq!(
            CODEX_PINNED,
            format!(
                "{}.{}.{}",
                crate::recipe::steps::CODEX_MIN.major,
                crate::recipe::steps::CODEX_MIN.minor,
                crate::recipe::steps::CODEX_MIN.patch
            )
        );
        assert_eq!(def(Tool::Codex).package, Some("@openai/codex"));
        assert!(!def(Tool::Cursor).updates());
        assert!(def(Tool::Cursor).self_update.is_none());
        assert!(def(Tool::Claude).updates_in_container());
        assert!(!def(Tool::Codex).updates_in_container());
        assert!(!def(Tool::Cursor).updates_in_container());
    }
}
