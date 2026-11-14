// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The licences you accepted, in a file of maxd's: each one's ID, the
//! SHA-256 of the text you read and the day you accepted it.

use std::path::PathBuf;

use maxhq_core::fs::{write_atomic, Atomic};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// One licence you accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    /// The licence's ID, such as `android-sdk-license`.
    pub id: String,
    /// The SHA-256 of the text you accepted, in lowercase hex.
    pub text_sha256: String,
    /// The day you accepted it, as `YYYY-MM-DD`.
    pub accepted_on: String,
}

/// Whether `accepted` holds the licence `id` with the text `text_sha256`.
pub fn holds(accepted: &[Accepted], id: &str, text_sha256: &str) -> bool {
    accepted
        .iter()
        .any(|licence| licence.id == id && licence.text_sha256 == text_sha256)
}

/// The record file.
#[derive(Clone, Debug)]
pub struct Licences {
    path: PathBuf,
}

impl Licences {
    /// The record at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Every licence you accepted. A missing file holds none, and so does
    /// one that doesn't read, so nothing is accepted that you didn't.
    pub fn read(&self) -> Vec<Accepted> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(err) => {
                warn!(%err, "couldn't read the accepted licences");
                return Vec::new();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            warn!(%err, "the accepted licences don't read");
            Vec::new()
        })
    }

    /// Records that you accepted the licence `id` with the text
    /// `text_sha256` today.
    pub fn accept(&self, id: &str, text_sha256: &str) -> std::io::Result<()> {
        let mut accepted = self.read();
        if holds(&accepted, id, text_sha256) {
            return Ok(());
        }
        accepted.push(Accepted {
            id: id.to_owned(),
            text_sha256: text_sha256.to_owned(),
            accepted_on: jiff::Zoned::now().date().to_string(),
        });
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(&accepted).map_err(std::io::Error::other)?;
        write_atomic(&self.path, &bytes, Atomic::PRIVATE)
    }
}
