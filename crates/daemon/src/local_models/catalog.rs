// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The curated models `maxd` offers. A copy ships inside `maxd`; a newer
//! one comes from the release a build names in `MAXHQ_CATALOG_URL`, and
//! the last one read is kept on disk for when the device is offline. Each
//! file's size and SHA-256 were checked against Hugging Face at a pinned
//! commit, so a download is checked against them.

use std::path::Path;

use maxhq_protocol::local_models::{CatalogModel, CatalogRef, ModelCatalog};

/// The catalog this build shipped with.
const BUILTIN: &str = include_str!("catalog.json");

/// Where a newer catalog comes from, when the build names one.
pub const REFRESH_URL: Option<&str> = option_env!("MAXHQ_CATALOG_URL");

/// The file the last catalog read is kept in, in `maxd`'s data folder.
pub const CACHE_FILE: &str = "model-catalog.json";

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

/// The newer of the shipped catalog and the one kept on disk.
pub fn current(data: &Path) -> ModelCatalog {
    let shipped = builtin();
    match std::fs::read_to_string(data.join(CACHE_FILE))
        .ok()
        .and_then(|text| read(&text))
    {
        Some(kept) if kept.updated_at > shipped.updated_at => kept,
        _ => shipped,
    }
}

/// Reads the catalog from the build's release and keeps it, when it's
/// newer than `have`. None when the build names no URL, the device is
/// offline, or the catalog there is no newer.
pub async fn refresh(data: &Path, have: &ModelCatalog) -> Option<ModelCatalog> {
    let url = REFRESH_URL?;
    let text = crate::tools::install::fetch::text(url).await.ok()?;
    let fresh = read(&text)?;
    if fresh.updated_at <= have.updated_at {
        return None;
    }
    let _ = std::fs::write(data.join(CACHE_FILE), text);
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
    use super::*;

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
    fn a_kept_catalog_counts_only_when_its_newer() {
        let dir = super::super::tests::Scratch::new();
        let mut older = builtin();
        older.updated_at = maxhq_protocol::time::Timestamp(1);
        older.models.truncate(1);
        std::fs::write(
            dir.path().join(CACHE_FILE),
            serde_json::to_string(&older).unwrap(),
        )
        .unwrap();
        assert_eq!(current(dir.path()).models.len(), 6);
        let mut newer = older;
        newer.updated_at = maxhq_protocol::time::Timestamp(builtin().updated_at.0 + 1);
        std::fs::write(
            dir.path().join(CACHE_FILE),
            serde_json::to_string(&newer).unwrap(),
        )
        .unwrap();
        assert_eq!(current(dir.path()).models.len(), 1);
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
