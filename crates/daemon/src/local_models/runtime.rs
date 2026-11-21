// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The llama-server a managed server runs: llama.cpp's own macOS arm64
//! build at a pinned release, checked against its SHA-256 and installed
//! with the tools installer under `~/.local/lib/maxhq/tools/llama-server`.
//! It has no link in `~/.local/bin`, since only `maxd` runs it.

use std::path::{Path, PathBuf};

use maxhq_protocol::state::{ToolKind, ToolState};

use crate::tools::install::releases::Publishers;
use crate::tools::install::{flow, ArchiveFile, InstallDef, Layout, Report, Source, Spot};
use crate::tools::HostExec;

/// The folder and program name.
pub const NAME: &str = "llama-server";

/// The llama.cpp release this build runs.
pub const VERSION: &str = "b12359";

/// The release's macOS arm64 archive.
const URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b12359/llama-b12359-bin-macos-arm64.tar.gz";

/// The archive's SHA-256, as checked when the release was pinned.
const SHA256: &str = "82b00abc740160e1b9980e6cc79c117345fa34af058c80e6c0a69aa8ddccde75";

/// The archive's size in bytes.
const SIZE: u64 = 16_907_811;

/// The only target the archive is for.
const TARGET: &str = "aarch64-apple-darwin";

/// What the installer installs.
pub fn def() -> InstallDef {
    InstallDef {
        // The kind only names a release feed when no files are pinned.
        kind: ToolKind::Acp {
            id: NAME.to_owned(),
        },
        binary: None,
        source: Source::Archive {
            files: vec![ArchiveFile {
                target: TARGET.to_owned(),
                url: URL.to_owned(),
                sha256: Some(SHA256.to_owned()),
                size: Some(SIZE),
            }],
            confirmed_unchecked: false,
        },
    }
}

/// The current version's folder under `home`.
fn current(home: &Path) -> PathBuf {
    home.join(".local/lib/maxhq/tools")
        .join(NAME)
        .join("current")
}

/// The program under `home`, whether it's there or not.
pub fn program(home: &Path) -> PathBuf {
    current(home).join(NAME)
}

/// The version installed under `home`, when one is whole.
pub fn installed(home: &Path) -> Option<String> {
    let record = std::fs::read_to_string(current(home).join(".install-complete")).ok()?;
    let version = record
        .lines()
        .find_map(|line| line.strip_prefix("version "))?
        .trim()
        .to_owned();
    program(home).is_file().then_some(version)
}

/// Installs the pinned release under `home`, reporting as tools do, and
/// gives the program.
pub async fn install(home: &Path, report: Report<'_>) -> Result<PathBuf, String> {
    let home_text = home.to_string_lossy();
    let spot = Spot {
        exec: &HostExec,
        layout: Layout::host(&home_text),
    };
    match flow::install_as(
        &spot,
        &def(),
        NAME,
        Some(VERSION),
        &Publishers::default(),
        report,
    )
    .await
    {
        Ok(_) => {
            report(ToolState::Idle);
            Ok(program(home))
        }
        Err(err) => {
            let reason = format!("Couldn't install llama-server: {err}");
            report(ToolState::Failed {
                reason: reason.clone(),
            });
            Err(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::tests::Scratch;

    #[test]
    fn the_pinned_release_is_checked_and_for_apple_silicon_only() {
        let Source::Archive {
            files,
            confirmed_unchecked,
        } = def().source
        else {
            panic!("an archive");
        };
        assert!(!confirmed_unchecked);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].target, TARGET);
        assert_eq!(files[0].sha256.as_deref().map(str::len), Some(64));
        assert!(files[0].url.contains(VERSION));
        assert_eq!(
            crate::tools::install::target_for("aarch64-apple-darwin"),
            TARGET
        );
    }

    #[test]
    fn only_a_whole_install_counts() {
        let home = Scratch::new();
        assert_eq!(installed(home.path()), None);
        let dir = current(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".install-complete"),
            "version b12359\nentry llama-server\n",
        )
        .unwrap();
        assert_eq!(installed(home.path()), None, "no program yet");
        std::fs::write(program(home.path()), "#!/bin/sh\n").unwrap();
        assert_eq!(installed(home.path()).as_deref(), Some(VERSION));
    }
}
