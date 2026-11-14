// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The push token each phone sent, kept in a file only `maxd` reads, since
//! a token is what reaches the phone.

use std::collections::BTreeMap;
use std::io;

use maxhq_protocol::ids::PhoneId;
use maxhq_protocol::phones::PushToken;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::secrets::files::Folder;

/// The tokens' file in the push folder.
const FILE: &str = "tokens.json";

/// The tokens, by phone.
pub struct Tokens {
    folder: Folder,
    tokens: BTreeMap<PhoneId, PushToken>,
}

/// One phone's line in the file.
#[derive(Serialize, Deserialize)]
struct Entry {
    phone: PhoneId,
    push: PushToken,
}

impl Tokens {
    /// Reads the tokens the folder holds. A file that doesn't read is
    /// left for the next token to replace, and each phone sends its token
    /// again when it connects.
    pub fn open(folder: Folder) -> Self {
        folder.clear_leftovers();
        let tokens = match folder.read(FILE) {
            Ok(Some(text)) => match serde_json::from_str::<Vec<Entry>>(text.expose()) {
                Ok(entries) => entries.into_iter().map(|e| (e.phone, e.push)).collect(),
                Err(_) => {
                    warn!("skipped the push tokens: {FILE} doesn't describe any");
                    BTreeMap::new()
                }
            },
            Ok(None) => BTreeMap::new(),
            Err(err) => {
                warn!("couldn't read the push tokens: {err}");
                BTreeMap::new()
            }
        };
        Self { folder, tokens }
    }

    /// The phones and their tokens.
    pub fn iter(&self) -> impl Iterator<Item = (&PhoneId, &PushToken)> {
        self.tokens.iter()
    }

    /// `phone`'s token, if it sent one.
    #[cfg(test)]
    pub fn get(&self, phone: PhoneId) -> Option<&PushToken> {
        self.tokens.get(&phone)
    }

    /// Keeps `push` as `phone`'s token, replacing the one before.
    pub fn set(&mut self, phone: PhoneId, push: PushToken) -> io::Result<()> {
        let before = self.tokens.insert(phone, push);
        self.save().inspect_err(|_| {
            match before {
                Some(before) => self.tokens.insert(phone, before),
                None => self.tokens.remove(&phone),
            };
        })
    }

    /// Drops `phone`'s token, if it's still `token`, or any token when
    /// `token` is none. Says whether one went.
    pub fn remove(&mut self, phone: PhoneId, token: Option<&PushToken>) -> bool {
        match (self.tokens.get(&phone), token) {
            (Some(held), Some(token)) if held.token != token.token => return false,
            (None, _) => return false,
            _ => {}
        }
        self.tokens.remove(&phone);
        if let Err(err) = self.save() {
            warn!(%phone, "couldn't save the push tokens: {err}");
        }
        true
    }

    fn save(&self) -> io::Result<()> {
        if self.tokens.is_empty() {
            return self.folder.remove(FILE);
        }
        let entries: Vec<Entry> = self
            .tokens
            .iter()
            .map(|(phone, push)| Entry {
                phone: *phone,
                push: push.clone(),
            })
            .collect();
        let json = serde_json::to_vec(&entries).map_err(io::Error::other)?;
        self.folder.write(FILE, &json)
    }
}
