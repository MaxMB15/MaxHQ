// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The fixtures in `test-only/` were signed with a throwaway key made for
//! these tests alone, whose private half was thrown away. They're no
//! release's, and `TEST_ONLY_KEY` is not the release key.

use std::fs;
use std::path::PathBuf;

use super::*;
use crate::testing::TempDir;

/// The throwaway key's public half. Test only.
const TEST_ONLY_KEY: &str = include_str!("test-only/key.pub");
/// A manifest for version 0.2.0 that the throwaway key signed. Test only.
const TEST_ONLY_MANIFEST: &str = include_str!("test-only/maxd-release.json");
/// Its signature. Test only.
const TEST_ONLY_SIGNATURE: &str = include_str!("test-only/maxd-release.json.sig");

const LINUX: &str = "x86_64-unknown-linux-musl";

fn trust(target: &'static str, version: &'static str) -> Trust<'static> {
    Trust {
        key: TEST_ONLY_KEY,
        target,
        version,
    }
}

fn signed() -> SignedRelease {
    SignedRelease {
        manifest: TEST_ONLY_MANIFEST.to_owned(),
        signature: TEST_ONLY_SIGNATURE.to_owned(),
    }
}

/// Stages the bytes the test manifest lists for `target`, and gives the
/// path and its sha256.
fn stage(dir: &TempDir, target: &str) -> (PathBuf, String) {
    stage_text(dir, &format!("test-only maxd {target}"))
}

fn stage_text(dir: &TempDir, text: &str) -> (PathBuf, String) {
    let path = dir.path().join("staged");
    fs::write(&path, text).unwrap();
    let sha = crate::identity::sha256_of(&path).unwrap();
    (path, sha)
}

#[test]
fn a_signed_release_that_lists_the_upload_is_accepted() {
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, LINUX);
    let release = signed();
    for version in ["0.1.0", "0.2.0", "0.2.0-rc.1"] {
        assert_eq!(
            vouch(
                &staged,
                &sha,
                UpdateKind::Maxd,
                Some(&release),
                false,
                trust(LINUX, version)
            ),
            Ok(()),
            "{version}"
        );
    }
    // The mark changes nothing when a release came too.
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&release),
            true,
            trust(LINUX, "0.2.0")
        ),
        Ok(())
    );
}

#[test]
fn a_guest_checks_the_musl_entry_for_its_architecture() {
    assert_eq!(
        entry_for(UpdateKind::Maxd, LINUX),
        "maxd-x86_64-unknown-linux-musl"
    );
    assert_eq!(
        entry_for(UpdateKind::Guest, "x86_64-unknown-linux-gnu"),
        "maxd-x86_64-unknown-linux-musl"
    );
    assert_eq!(
        entry_for(UpdateKind::Guest, "aarch64-unknown-linux-musl"),
        "maxd-aarch64-unknown-linux-musl"
    );
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, "aarch64-unknown-linux-musl");
    let release = signed();
    let on = trust("aarch64-unknown-linux-gnu", "0.2.0");
    assert_eq!(
        vouch(&staged, &sha, UpdateKind::Guest, Some(&release), false, on),
        Ok(())
    );
    // A gnu maxd has no entry of its own, so it takes no release as maxd.
    assert!(matches!(
        vouch(&staged, &sha, UpdateKind::Maxd, Some(&release), false, on),
        Err(Refusal::OtherTarget { .. })
    ));
}

#[test]
fn a_manifest_changed_after_signing_is_refused() {
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, LINUX);
    let changed = SignedRelease {
        manifest: TEST_ONLY_MANIFEST.replace("0.2.0", "9.9.9"),
        signature: TEST_ONLY_SIGNATURE.to_owned(),
    };
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&changed),
            false,
            trust(LINUX, "0.1.0")
        ),
        Err(Refusal::Signature)
    );
}

#[test]
fn a_manifest_signed_with_another_key_is_refused() {
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, LINUX);
    // The real release key never signed the test manifest.
    let release_key = Trust {
        key: RELEASE_PUBLIC_KEY,
        target: LINUX,
        version: "0.1.0",
    };
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&signed()),
            false,
            release_key
        ),
        Err(Refusal::Signature)
    );
    for signature in ["", "not base64!", "bm90IGEgc2lnbmF0dXJl"] {
        let garbled = SignedRelease {
            manifest: TEST_ONLY_MANIFEST.to_owned(),
            signature: signature.to_owned(),
        };
        assert_eq!(
            vouch(
                &staged,
                &sha,
                UpdateKind::Maxd,
                Some(&garbled),
                false,
                trust(LINUX, "0.1.0")
            ),
            Err(Refusal::Signature),
            "{signature}"
        );
    }
}

#[test]
fn an_upload_the_release_doesnt_list_is_refused() {
    let dir = TempDir::new();
    let (staged, sha) = stage_text(&dir, "a maxd no release carries");
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&signed()),
            false,
            trust(LINUX, "0.2.0")
        ),
        Err(Refusal::Unlisted {
            entry: "maxd-x86_64-unknown-linux-musl".to_owned()
        })
    );
}

#[test]
fn an_upload_listed_for_another_target_is_refused() {
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, "aarch64-unknown-linux-musl");
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&signed()),
            false,
            trust(LINUX, "0.2.0")
        ),
        Err(Refusal::OtherTarget {
            listed: "maxd-aarch64-unknown-linux-musl".to_owned(),
            wanted: "maxd-x86_64-unknown-linux-musl".to_owned(),
        })
    );
}

#[test]
fn a_release_older_than_the_running_maxd_is_refused() {
    let dir = TempDir::new();
    let (staged, sha) = stage(&dir, LINUX);
    assert_eq!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&signed()),
            false,
            trust(LINUX, "0.2.1")
        ),
        Err(Refusal::Older {
            release: "0.2.0".to_owned(),
            running: "0.2.1".to_owned(),
        })
    );
    assert!(matches!(
        vouch(
            &staged,
            &sha,
            UpdateKind::Maxd,
            Some(&signed()),
            false,
            trust(LINUX, "1.0.0")
        ),
        Err(Refusal::Older { .. })
    ));
}

#[test]
fn the_checksum_the_app_sent_is_checked_first() {
    let dir = TempDir::new();
    let (staged, _) = stage(&dir, LINUX);
    let other = "0".repeat(64);
    assert_eq!(
        vouch(
            &staged,
            &other,
            UpdateKind::Maxd,
            Some(&signed()),
            true,
            trust(LINUX, "0.2.0")
        ),
        Err(Refusal::Checksum)
    );
}

#[test]
fn an_unsigned_upload_needs_its_mark() {
    let dir = TempDir::new();
    let (staged, sha) = stage_text(&dir, "a maxd built from source");
    let on = trust(LINUX, "0.2.0");
    assert_eq!(
        vouch(&staged, &sha, UpdateKind::Maxd, None, false, on),
        Err(Refusal::Unsigned)
    );
    assert_eq!(
        vouch(&staged, &sha, UpdateKind::Guest, None, false, on),
        Err(Refusal::Unsigned)
    );
    assert_eq!(
        vouch(&staged, &sha, UpdateKind::Maxd, None, true, on),
        Ok(())
    );
}

#[test]
fn this_maxd_trusts_the_release_key_and_reads_its_own_version() {
    let own = Trust::own();
    assert_eq!(own.key, RELEASE_PUBLIC_KEY);
    assert!(decode_text(RELEASE_PUBLIC_KEY)
        .and_then(|text| PublicKey::decode(&text).ok())
        .is_some());
    assert!(semver::Version::parse(own.version).is_ok());
    assert_ne!(TEST_ONLY_KEY.trim(), RELEASE_PUBLIC_KEY);
}

#[test]
fn the_release_key_is_the_one_the_app_updates_with() {
    // One key signs the app's own updates and the fleet manifest, so the
    // app's updater and this maxd must trust the same one.
    let config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../apps/desktop/src-tauri/tauri.conf.json"
    ))
    .unwrap();
    assert_eq!(
        config["plugins"]["updater"]["pubkey"].as_str(),
        Some(RELEASE_PUBLIC_KEY)
    );
}
