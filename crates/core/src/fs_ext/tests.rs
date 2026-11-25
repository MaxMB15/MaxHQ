// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::io::Read;

use super::*;
use crate::test_dir::TempDir;

#[test]
fn a_regular_file_opens_and_a_folder_does_not() {
    let dir = TempDir::new("fs-ext-open");
    let file = dir.0.join("notes.txt");
    fs::write(&file, "kept").unwrap();
    for follow in [true, false] {
        let mut text = String::new();
        open_regular(&file, follow)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "kept");
        assert!(open_regular(&dir.0, follow).is_err());
    }
    let gone = open_regular(&dir.0.join("gone"), false).unwrap_err();
    assert!(absent(&gone), "{gone}");
    let under_a_file = open_regular(&file.join("below"), false).unwrap_err();
    assert!(absent(&under_a_file), "{under_a_file}");
}

#[test]
fn a_link_opens_only_when_followed() {
    let dir = TempDir::new("fs-ext-link");
    let file = dir.0.join("real.txt");
    fs::write(&file, "real").unwrap();
    let link = dir.0.join("link.txt");
    if let Err(err) = symlink(Path::new("real.txt"), &link) {
        // A Windows host without Developer Mode makes no links.
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        return;
    }
    assert_eq!(fs::read_to_string(&link).unwrap(), "real");
    assert!(open_regular(&link, true).is_ok());
    let refused = open_regular(&link, false).unwrap_err();
    assert!(!absent(&refused), "{refused}");
    assert_eq!(read_link(&link).unwrap(), Path::new("real.txt"));

    let renamed = dir.0.join("renamed.txt");
    symlink(Path::new("real.txt"), &renamed).unwrap();
    rename_link(&renamed, &link).unwrap();
    assert!(renamed.symlink_metadata().is_err());
    remove_link(&link).unwrap();
    assert!(link.symlink_metadata().is_err());
    assert!(file.is_file(), "only the link goes");
}

#[test]
fn a_link_to_a_folder_goes_through_it() {
    let dir = TempDir::new("fs-ext-folder-link");
    fs::create_dir_all(dir.0.join("skills/demo")).unwrap();
    fs::write(dir.0.join("skills/demo/SKILL.md"), "demo").unwrap();
    let link = dir.0.join("linked");
    if symlink(Path::new("skills/demo"), &link).is_err() {
        return;
    }
    assert_eq!(
        fs::read_to_string(link.join("SKILL.md")).unwrap(),
        "demo",
        "a `/` target reads on every OS"
    );
    remove_link(&link).unwrap();
    assert!(dir.0.join("skills/demo/SKILL.md").is_file());
}

#[test]
fn a_new_file_refuses_a_link_in_its_place() {
    let dir = TempDir::new("fs-ext-no-follow");
    let path = dir.0.join("new.json");
    no_follow(OpenOptions::new().write(true).create_new(true))
        .open(&path)
        .unwrap();
    assert!(path.is_file());
    let target = dir.0.join("target");
    fs::write(&target, "").unwrap();
    let link = dir.0.join("link.json");
    if symlink(&target, &link).is_ok() {
        assert!(no_follow(OpenOptions::new().write(true).create_new(true))
            .open(&link)
            .is_err());
    }
}

#[test]
fn a_canonical_path_is_one_the_os_opens() {
    let dir = TempDir::new("fs-ext-canonical");
    let file = dir.0.join("here.txt");
    fs::write(&file, "here").unwrap();
    let real = canonicalize(&file).unwrap();
    assert!(real.is_absolute());
    assert!(!real.to_string_lossy().starts_with(r"\\?\"), "{real:?}");
    assert_eq!(fs::read_to_string(&real).unwrap(), "here");
}

#[cfg(unix)]
#[test]
fn unix_owners_are_the_files_own() {
    let dir = TempDir::new("fs-ext-owner");
    let path = dir.0.join("mine");
    let file = File::create(&path).unwrap();
    let meta = file.metadata().unwrap();
    let uid = maxhq_os::fsmode::owner_of(&meta).unwrap();
    let gid = std::os::unix::fs::MetadataExt::gid(&meta);
    chown_file(&file, uid, gid).unwrap();
    chown_link(&path, uid, gid).unwrap();
}
