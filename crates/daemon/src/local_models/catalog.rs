// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The curated models `maxd` offers. A copy ships inside `maxd`; a newer
//! one comes from the release a build names in `MAXHQ_CATALOG_URL`, and
//! the last one read is kept on disk for when the device is offline. Each
//! file's size and SHA-256 were checked against Hugging Face at a pinned
//! commit, so a download is checked against them.
//!
//! The release signs its catalog with the release key, as it signs fleet
//! updates, in `<catalog>.sig`. A catalog from the release, or the copy
//! kept on disk, counts only while that signature checks out against
//! [`release::release_keys`]; otherwise maxd keeps the one it shipped with
//! and logs why.

use std::path::Path;

use maxhq_protocol::local_models::{CatalogModel, CatalogRef, ModelCatalog};
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use crate::fleet::release::{self, BadSignature};
use crate::tools::install::fetch;

/// The catalog this build shipped with.
const BUILTIN: &str = include_str!("catalog.json");

/// Where a newer catalog comes from, when the build names one.
pub const REFRESH_URL: Option<&str> = option_env!("MAXHQ_CATALOG_URL");

/// The file the last catalog read is kept in, in `maxd`'s data folder.
pub const CACHE_FILE: &str = "model-catalog.json";

/// The file the kept catalog's signature is kept in, beside it.
pub const CACHE_SIGNATURE: &str = "model-catalog.json.sig";

/// The most a catalog from the release may be. The shipped one is about
/// 12 KB.
const MOST: u64 = 1024 * 1024;

/// The most its signature may be. One is about 400 bytes.
const MOST_SIGNATURE: u64 = 4 * 1024;

/// How far ahead of this device's clock a catalog may be dated, in
/// milliseconds. A newer catalog replaces the one kept only when it's
/// dated later, so one dated years ahead would stay for good.
const AHEAD: u64 = 24 * 60 * 60 * 1000;

/// Why a catalog wasn't used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// Its signature didn't check out.
    #[error("{0}")]
    Signature(BadSignature),
    /// It doesn't read, or lists a file that can't be checked.
    #[error("it doesn't read, or lists a file that can't be checked")]
    Unreadable,
    /// It's dated more than a day ahead of this device's clock.
    #[error("it's dated ahead of this device's clock")]
    Ahead,
}

/// The catalog this build shipped with.
pub fn builtin() -> ModelCatalog {
    // A test proves the shipped file reads, so this never fails.
    read(BUILTIN).unwrap_or(ModelCatalog {
        updated_at: maxhq_protocol::time::Timestamp(0),
        models: Vec::new(),
    })
}

/// A catalog from its JSON, when every file in it can be checked.
pub fn read(text: &str) -> Option<ModelCatalog> {
    let catalog: ModelCatalog = serde_json::from_str(text).ok()?;
    let checked = catalog.models.iter().all(|model| {
        !model.id.is_empty()
            && !model.id.contains('@')
            && model.commit.len() == 40
            && model.files.iter().all(|file| {
                file.sha256.len() == 64
                    && file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                    && file.bytes > 0
                    && !file.name.contains('/')
                    && !file.name.starts_with('.')
            })
    });
    checked.then_some(catalog)
}

/// A catalog from its JSON and `signature`, when one of `keys` signed it,
/// every file in it can be checked, and it isn't dated more than a day
/// after `now`.
pub fn verified(
    text: &str,
    signature: &str,
    keys: &[&str],
    now: Timestamp,
) -> Result<ModelCatalog, Refused> {
    release::signed_by(text.as_bytes(), signature, keys).map_err(Refused::Signature)?;
    let catalog = read(text).ok_or(Refused::Unreadable)?;
    if catalog.updated_at.0 > now.0.saturating_add(AHEAD) {
        return Err(Refused::Ahead);
    }
    Ok(catalog)
}

/// The newer of the shipped catalog and the one kept on disk, while the
/// kept one's signature checks out.
pub fn current(data: &Path) -> ModelCatalog {
    current_with(data, release::release_keys(), Timestamp::now())
}

fn current_with(data: &Path, keys: &[&str], now: Timestamp) -> ModelCatalog {
    let shipped = builtin();
    let Ok(text) = std::fs::read_to_string(data.join(CACHE_FILE)) else {
        return shipped;
    };
    let signature = std::fs::read_to_string(data.join(CACHE_SIGNATURE)).unwrap_or_default();
    match verified(&text, &signature, keys, now) {
        Ok(kept) if kept.updated_at > shipped.updated_at => kept,
        Ok(_) => shipped,
        Err(why) => {
            warn!(%why, "not using the model catalog kept on disk; using the one maxd shipped with");
            shipped
        }
    }
}

/// Reads the catalog from the build's release and keeps it, when its
/// signature checks out and it's newer than `have`. None when the build
/// names no URL, the device is offline, or the catalog there is unsigned
/// or no newer.
pub async fn refresh(data: &Path, have: &ModelCatalog) -> Option<ModelCatalog> {
    refresh_from(
        REFRESH_URL?,
        data,
        have,
        release::release_keys(),
        Timestamp::now(),
    )
    .await
}

async fn refresh_from(
    url: &str,
    data: &Path,
    have: &ModelCatalog,
    keys: &[&str],
    now: Timestamp,
) -> Option<ModelCatalog> {
    let text = match fetch::text_up_to(url, MOST).await {
        Ok(text) => text,
        Err(err) => {
            tracing::debug!(%err, "couldn't read the release's model catalog");
            return None;
        }
    };
    let signature = match fetch::text_up_to(&format!("{url}.sig"), MOST_SIGNATURE).await {
        Ok(signature) => signature,
        Err(err) => {
            warn!(%err, "the release's model catalog has no signature to read, so maxd keeps its own");
            return None;
        }
    };
    let fresh = match verified(&text, &signature, keys, now) {
        Ok(fresh) => fresh,
        Err(why) => {
            warn!(%why, "not using the release's model catalog; maxd keeps its own");
            return None;
        }
    };
    if fresh.updated_at <= have.updated_at {
        return None;
    }
    // A kept catalog counts only beside its own signature, so a write that
    // fails halfway leaves maxd on the one it shipped with.
    let kept = std::fs::write(data.join(CACHE_SIGNATURE), &signature)
        .and_then(|()| std::fs::write(data.join(CACHE_FILE), &text));
    if let Err(err) = kept {
        warn!(%err, "couldn't keep the model catalog on disk");
    }
    Some(fresh)
}

/// The catalog model and file a managed server's model ID names,
/// `<catalog id>@<quant>`.
pub fn managed(catalog: &ModelCatalog, id: &str) -> Option<CatalogRef> {
    let (model, quant) = id.split_once('@')?;
    let entry = catalog.models.iter().find(|m| m.id == model)?;
    let file = entry
        .files
        .iter()
        .find(|f| f.quant.eq_ignore_ascii_case(quant))?;
    Some(CatalogRef {
        model: entry.id.clone(),
        file: file.name.clone(),
    })
}

/// The catalog model a server's own model ID looks like, such as
/// `qwen3.6:35b-a3b` on Ollama or `unsloth/Qwen3.6-35B-A3B-GGUF` on
/// LM Studio, with its recommended file. The longest catalog ID that
/// matches wins.
pub fn recognize(catalog: &ModelCatalog, id: &str) -> Option<CatalogRef> {
    if let Some(found) = managed(catalog, id) {
        return Some(found);
    }
    let seen = squash(id);
    let entry: &CatalogModel = catalog
        .models
        .iter()
        .filter(|m| seen.contains(&squash(&m.id)))
        .max_by_key(|m| squash(&m.id).len())?;
    let file = entry
        .files
        .iter()
        .find(|f| f.recommended)
        .or_else(|| entry.files.first())?;
    Some(CatalogRef {
        model: entry.id.clone(),
        file: file.name.clone(),
    })
}

/// An ID with only its letters and digits, in lowercase.
fn squash(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    //! The fixtures in `test-only/` were signed with a throwaway key made
    //! for these tests alone, through `scripts/release/sign.mjs` as the
    //! release signs, and its private half was thrown away.

    use super::*;
    use crate::tools::install::tests::{Answer, Fixture};

    /// The throwaway key's public half. Test only.
    const TEST_ONLY_KEY: &str = include_str!("test-only/key.pub");
    /// The shipped catalog's first model, dated 2100-01-01. Test only.
    const TEST_ONLY_CATALOG: &str = include_str!("test-only/catalog.json");
    /// Its signature. Test only.
    const TEST_ONLY_SIGNATURE: &str = include_str!("test-only/catalog.json.sig");

    /// The fixture's date, as a clock that reads it.
    fn then() -> Timestamp {
        read(TEST_ONLY_CATALOG).unwrap().updated_at
    }

    #[test]
    fn a_signed_catalog_is_verified_against_the_release_keys() {
        let keys = [TEST_ONLY_KEY];
        let catalog = verified(TEST_ONLY_CATALOG, TEST_ONLY_SIGNATURE, &keys, then()).unwrap();
        assert_eq!(catalog.models.len(), 1);
        // Either key of a change of the update key will do.
        assert!(verified(
            TEST_ONLY_CATALOG,
            TEST_ONLY_SIGNATURE,
            &[release::RELEASE_PUBLIC_KEY, TEST_ONLY_KEY],
            then()
        )
        .is_ok());
        assert_eq!(
            verified(
                TEST_ONLY_CATALOG,
                TEST_ONLY_SIGNATURE,
                &[release::RELEASE_PUBLIC_KEY],
                then()
            ),
            Err(Refused::Signature(BadSignature::Wrong))
        );
        let changed = TEST_ONLY_CATALOG.replace("\"bytes\": 2", "\"bytes\": 3");
        assert_ne!(changed, TEST_ONLY_CATALOG);
        assert_eq!(
            verified(&changed, TEST_ONLY_SIGNATURE, &keys, then()),
            Err(Refused::Signature(BadSignature::Wrong))
        );
        assert_eq!(
            verified(TEST_ONLY_CATALOG, "", &keys, then()),
            Err(Refused::Signature(BadSignature::Unreadable))
        );
    }

    #[test]
    fn a_catalog_dated_more_than_a_day_ahead_is_refused() {
        let keys = [TEST_ONLY_KEY];
        let day_before = Timestamp(then().0 - AHEAD);
        assert!(verified(TEST_ONLY_CATALOG, TEST_ONLY_SIGNATURE, &keys, day_before).is_ok());
        assert_eq!(
            verified(
                TEST_ONLY_CATALOG,
                TEST_ONLY_SIGNATURE,
                &keys,
                Timestamp(day_before.0 - 1)
            ),
            Err(Refused::Ahead)
        );
    }

    #[test]
    fn a_kept_catalog_counts_only_with_its_signature_and_when_its_newer() {
        let dir = super::super::tests::Scratch::new();
        let keys = [TEST_ONLY_KEY];
        let kept = |dir: &Path| current_with(dir, &keys, then()).models.len();
        // Nothing kept.
        assert_eq!(kept(dir.path()), 6);
        std::fs::write(dir.path().join(CACHE_FILE), TEST_ONLY_CATALOG).unwrap();
        // Kept without a signature, as a maxd before signed catalogs did.
        assert_eq!(kept(dir.path()), 6);
        std::fs::write(dir.path().join(CACHE_SIGNATURE), TEST_ONLY_SIGNATURE).unwrap();
        assert_eq!(kept(dir.path()), 1);
        // Signed by a key this maxd doesn't trust.
        assert_eq!(
            current_with(dir.path(), &[release::RELEASE_PUBLIC_KEY], then())
                .models
                .len(),
            6
        );
        // Changed on disk after it was kept.
        std::fs::write(
            dir.path().join(CACHE_FILE),
            TEST_ONLY_CATALOG.replace("4102444800000", "4102444800001"),
        )
        .unwrap();
        assert_eq!(kept(dir.path()), 6);
    }

    #[tokio::test]
    async fn a_refreshed_catalog_is_kept_only_when_signed_and_newer() {
        let server = Fixture::start().await;
        let dir = super::super::tests::Scratch::new();
        let keys = [TEST_ONLY_KEY];
        let shipped = builtin();
        let url = server.serve(
            "/model-catalog.json",
            Answer::Body(TEST_ONLY_CATALOG.into()),
        );

        // No signature beside it.
        assert!(refresh_from(&url, dir.path(), &shipped, &keys, then())
            .await
            .is_none());
        assert!(!dir.path().join(CACHE_FILE).exists());

        // Signed with a key this maxd doesn't trust.
        server.serve(
            "/model-catalog.json.sig",
            Answer::Body(TEST_ONLY_SIGNATURE.into()),
        );
        assert!(refresh_from(
            &url,
            dir.path(),
            &shipped,
            &[release::RELEASE_PUBLIC_KEY],
            then()
        )
        .await
        .is_none());
        assert!(!dir.path().join(CACHE_FILE).exists());

        // Signed, and newer than what maxd has.
        let fresh = refresh_from(&url, dir.path(), &shipped, &keys, then())
            .await
            .expect("a signed, newer catalog is taken");
        assert_eq!(fresh.models.len(), 1);
        assert_eq!(current_with(dir.path(), &keys, then()), fresh);

        // No newer than what maxd has.
        assert!(refresh_from(&url, dir.path(), &fresh, &keys, then())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn a_catalog_or_signature_past_its_size_is_refused() {
        let server = Fixture::start().await;
        let dir = super::super::tests::Scratch::new();
        let url = server.serve("/big.json", Answer::Body(vec![b' '; MOST as usize + 1]));
        server.serve("/big.json.sig", Answer::Body(TEST_ONLY_SIGNATURE.into()));
        assert!(
            refresh_from(&url, dir.path(), &builtin(), &[TEST_ONLY_KEY], then())
                .await
                .is_none()
        );
        let url = server.serve(
            "/model-catalog.json",
            Answer::Body(TEST_ONLY_CATALOG.into()),
        );
        server.serve(
            "/model-catalog.json.sig",
            Answer::Body(vec![b'A'; MOST_SIGNATURE as usize + 1]),
        );
        assert!(
            refresh_from(&url, dir.path(), &builtin(), &[TEST_ONLY_KEY], then())
                .await
                .is_none()
        );
        assert!(!dir.path().join(CACHE_FILE).exists());
        assert!(fetch::text_up_to(&url, 10)
            .await
            .unwrap_err()
            .contains("bigger"));
    }

    #[test]
    fn the_shipped_catalog_reads_and_names_six_checked_models() {
        let catalog = read(BUILTIN).expect("the shipped catalog reads");
        assert_eq!(catalog.models.len(), 6);
        assert_eq!(catalog.models[0].id, "qwen3.6-35b-a3b");
        for model in &catalog.models {
            assert_eq!(
                model.files.iter().filter(|f| f.recommended).count(),
                1,
                "{} has one recommended file",
                model.id
            );
            assert!(!model.scores.is_empty(), "{} has scores", model.id);
            assert!(model.notes.len() < 400, "{}'s notes are short", model.id);
        }
    }

    #[test]
    fn a_catalog_with_a_file_it_cant_check_is_refused() {
        let mut catalog = builtin();
        catalog.models[0].files[0].sha256 = "abc".to_owned();
        let text = serde_json::to_string(&catalog).unwrap();
        assert!(read(&text).is_none());
        catalog = builtin();
        catalog.models[0].files[0].name = "../escape.gguf".to_owned();
        assert!(read(&serde_json::to_string(&catalog).unwrap()).is_none());
    }

    #[test]
    fn server_model_ids_are_matched_to_the_catalog() {
        let catalog = builtin();
        let model = |id: &str| recognize(&catalog, id).map(|r| r.model);
        assert_eq!(model("qwen3.6:35b-a3b").as_deref(), Some("qwen3.6-35b-a3b"));
        assert_eq!(
            model("unsloth/Qwen3.6-35B-A3B-GGUF").as_deref(),
            Some("qwen3.6-35b-a3b")
        );
        assert_eq!(model("gpt-oss:20b").as_deref(), Some("gpt-oss-20b"));
        assert_eq!(model("ornith-1.5-9b").as_deref(), Some("ornith-1.5-9b"));
        assert_eq!(model("llama3.2:3b"), None);
        let managed = recognize(&catalog, "qwen3.6-35b-a3b@ud-iq4_xs").unwrap();
        assert_eq!(managed.file, "Qwen3.6-35B-A3B-UD-IQ4_XS.gguf");
    }
}
