// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A model file's download. It comes from Hugging Face at the commit the
//! catalog pins, into a `.part` file beside where it goes, carries on from
//! what's there with an HTTP Range request, and is checked against the
//! catalog's SHA-256 before it's renamed into place. A file that doesn't
//! match is deleted, so the next try starts clean.

use std::path::{Path, PathBuf};

use maxhq_protocol::local_models::{CatalogFile, CatalogModel};

use crate::tools::install::fetch::{self, Kind, Progress};

/// Where model files come from.
pub const HUGGING_FACE: &str = "https://huggingface.co";

/// What a device keeps free beyond a download, so the disk never fills.
pub const HEADROOM: u64 = 2 << 30;

/// One file to fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// Where it comes from.
    pub url: String,
    /// Where it goes once checked.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256, in hex.
    pub sha256: String,
}

impl Wanted {
    /// `file` of `model`, from `base` into `dir`.
    pub fn of(base: &str, dir: &Path, model: &CatalogModel, file: &CatalogFile) -> Self {
        Self {
            url: format!(
                "{}/{}/resolve/{}/{}",
                base.trim_end_matches('/'),
                model.repo,
                model.commit,
                file.name
            ),
            path: path(dir, model, file),
            size: file.bytes,
            sha256: file.sha256.to_ascii_lowercase(),
        }
    }

    /// The file it's written to until it's checked.
    pub fn part(&self) -> PathBuf {
        part(&self.path)
    }
}

/// Where `file` of `model` goes: `<dir>/<org>/<repo>/<file>`.
pub fn path(dir: &Path, model: &CatalogModel, file: &CatalogFile) -> PathBuf {
    let mut path = dir.to_owned();
    for part in model.repo.split('/').filter(|part| safe(part)) {
        path.push(part);
    }
    path.join(&file.name)
}

/// The `.part` file beside `path`.
pub fn part(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    path.with_file_name(name)
}

/// Whether a path piece stays where it's put.
fn safe(piece: &str) -> bool {
    !piece.is_empty() && piece != "." && piece != ".." && !piece.contains('\\')
}

/// The bytes already in the `.part` file, or 0.
pub fn have(wanted: &Wanted) -> u64 {
    std::fs::metadata(wanted.part()).map_or(0, |meta| meta.len())
}

/// Whether the disk has room for the rest of a download and the headroom.
pub fn room(free: u64, wanted: &Wanted, have: u64) -> Result<(), String> {
    let needs = wanted.size.saturating_sub(have) + HEADROOM;
    if free >= needs {
        return Ok(());
    }
    Err(format!(
        "There isn't room for it: it needs {} free and the disk has {}.",
        gigabytes(needs),
        gigabytes(free)
    ))
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Downloads `wanted`, carrying on from its `.part` file, checks it and
/// renames it into place. A file that doesn't match its checksum is
/// deleted. Dropping the future pauses it.
pub async fn fetch(wanted: &Wanted, progress: Progress<'_>) -> Result<(), String> {
    if let Some(dir) = wanted.path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|err| format!("couldn't make {}: {err}", dir.display()))?;
    }
    let part = wanted.part();
    let got = fetch::resume(&wanted.url, &part, Some(wanted.size), Kind::Model, progress).await?;
    if got.size != wanted.size || got.sha256 != wanted.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(if got.size == wanted.size {
            "The download didn't match its checksum, so it was deleted. Try again.".to_owned()
        } else {
            format!(
                "The download was {} bytes, not {}, so it was deleted. Try again.",
                got.size, wanted.size
            )
        });
    }
    tokio::fs::rename(&part, &wanted.path)
        .await
        .map_err(|err| format!("couldn't put the model in place: {err}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::catalog;
    use super::super::tests::{Fake, Route, Scratch};
    use super::*;

    fn sha(bytes: &[u8]) -> String {
        fetch::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
    }

    /// The first catalog model, with one small file of `body`.
    fn small(body: &[u8]) -> (CatalogModel, CatalogFile) {
        let mut model = catalog::builtin().models.remove(0);
        let mut file = model.files.remove(0);
        file.bytes = body.len() as u64;
        file.sha256 = sha(body);
        model.files = vec![file.clone()];
        (model, file)
    }

    fn body() -> Vec<u8> {
        (0..200_000u32).map(|n| (n % 251) as u8).collect()
    }

    #[test]
    fn a_file_goes_under_its_repo_and_comes_from_the_pinned_commit() {
        let (model, file) = small(b"x");
        let wanted = Wanted::of(HUGGING_FACE, Path::new("/models"), &model, &file);
        assert_eq!(
            wanted.url,
            format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                model.repo, model.commit, file.name
            )
        );
        assert_eq!(
            wanted.path,
            Path::new("/models").join(&model.repo).join(&file.name)
        );
        assert_eq!(
            wanted.part().file_name().unwrap().to_str().unwrap(),
            format!("{}.part", file.name)
        );
        let mut odd = model.clone();
        odd.repo = "../../etc".to_owned();
        assert_eq!(
            path(Path::new("/models"), &odd, &file),
            Path::new("/models/etc").join(&file.name)
        );
    }

    #[test]
    fn a_download_needs_room_for_the_rest_and_the_headroom() {
        let (_, file) = small(b"x");
        let wanted = Wanted {
            url: String::new(),
            path: PathBuf::from("/m/f.gguf"),
            size: 20 << 30,
            sha256: file.sha256,
        };
        assert!(room(30 << 30, &wanted, 0).is_ok());
        let refused = room(10 << 30, &wanted, 0).unwrap_err();
        assert!(refused.contains("isn't room"), "{refused}");
        // What's already there counts.
        assert!(room(10 << 30, &wanted, 15 << 30).is_ok());
    }

    #[tokio::test]
    async fn a_cut_download_resumes_from_its_part_file() {
        let fake = Fake::start().await;
        let dir = Scratch::new();
        let body = body();
        let (model, file) = small(&body);
        let wanted = Wanted::of(&fake.base, dir.path(), &model, &file);
        let route = format!("/{}/resolve/{}/{}", model.repo, model.commit, file.name);
        fake.route(
            &route,
            Route::File {
                body: body.clone(),
                cut: Some(70_000),
            },
        );
        let quiet = |_: u64, _: Option<u64>| {};
        let cut = fetch(&wanted, &quiet).await;
        assert!(cut.is_err(), "the cut stops the first try");
        assert_eq!(have(&wanted), 70_000);
        let last = AtomicU64::new(0);
        let seen = |done: u64, _: Option<u64>| last.store(done, Ordering::Relaxed);
        fetch(&wanted, &seen).await.unwrap();
        assert_eq!(std::fs::read(&wanted.path).unwrap(), body);
        assert!(!wanted.part().exists());
        assert_eq!(last.load(Ordering::Relaxed), body.len() as u64);
        let asked = fake.seen();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].header("range"), None);
        assert_eq!(asked[1].header("range"), Some("bytes=70000-"));
    }

    #[tokio::test]
    async fn a_download_that_doesnt_match_its_checksum_is_deleted() {
        let fake = Fake::start().await;
        let dir = Scratch::new();
        let body = body();
        let (model, mut file) = small(&body);
        file.sha256 = sha(b"something else");
        let wanted = Wanted::of(&fake.base, dir.path(), &model, &file);
        let route = format!("/{}/resolve/{}/{}", model.repo, model.commit, file.name);
        fake.route(&route, Route::File { body, cut: None });
        let quiet = |_: u64, _: Option<u64>| {};
        let failed = fetch(&wanted, &quiet).await.unwrap_err();
        assert!(failed.contains("checksum"), "{failed}");
        assert!(!wanted.part().exists());
        assert!(!wanted.path.exists());
    }

    #[tokio::test]
    async fn a_server_that_sends_the_whole_file_again_starts_it_over() {
        let fake = Fake::start().await;
        let dir = Scratch::new();
        let body = body();
        let (model, file) = small(&body);
        let wanted = Wanted::of(&fake.base, dir.path(), &model, &file);
        std::fs::create_dir_all(wanted.path.parent().unwrap()).unwrap();
        // Bytes that aren't the file's start, longer than the file: they
        // can't be resumed from.
        std::fs::write(wanted.part(), vec![7u8; body.len() + 10]).unwrap();
        let route = format!("/{}/resolve/{}/{}", model.repo, model.commit, file.name);
        fake.route(
            &route,
            Route::File {
                body: body.clone(),
                cut: None,
            },
        );
        let quiet = |_: u64, _: Option<u64>| {};
        fetch(&wanted, &quiet).await.unwrap();
        assert_eq!(std::fs::read(&wanted.path).unwrap(), body);
    }
}
