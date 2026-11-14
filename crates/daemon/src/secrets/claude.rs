// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Claude setup-tokens, per W-5. Each one stands for an account on the
//! Mac, and gives this device a token account that the holder launches
//! with `CLAUDE_CODE_OAUTH_TOKEN`.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, CredentialId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Credential, CredentialKind, LoginKind, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::{claude_token_file, login_account, named, pasted, refused, Secrets, TOO_LONG};

/// The name a token gets when the user gives none.
const DEFAULT_LABEL: &str = "Setup-token";
/// The longest token taken. Real ones are about 110 characters.
const MAX_TOKEN: usize = 4096;
/// Where a token account keeps Claude's config inside containers, before
/// the account's ID.
const CONTAINER_CONFIG: &str = "/home/agent/.claude-";
/// Why a token was refused while the device's setting is off.
const TOKENS_OFF: &str =
    "This device doesn't take Claude setup-tokens. Turn them on in its settings first.";

/// The tokens the store holds, by the token account each one gives.
#[derive(Default)]
pub struct Tokens(Mutex<HashMap<AccountId, Token>>);

impl Tokens {
    fn lock(&self) -> MutexGuard<'_, HashMap<AccountId, Token>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One token, as its file holds it.
#[derive(Serialize, Deserialize)]
struct Token {
    origin: AccountId,
    label: String,
    stored_at: Timestamp,
    token: Secret,
}

/// The token account the token from `origin` gives `device`.
pub(super) fn token_account(device: DeviceId, origin: AccountId) -> AccountId {
    names::account(device, Vendor::Claude, &format!("token:{origin}"))
}

/// The credential for the token from `origin` on `device`.
fn credential_id(device: DeviceId, origin: AccountId) -> CredentialId {
    CredentialId::derive(&format!("claude-token:{device}:{origin}"))
}

impl Secrets {
    /// Answers `StoreClaudeToken`. A second token from the same Mac account
    /// replaces the first.
    pub(super) fn store_claude_token(
        &self,
        origin: AccountId,
        label: String,
        token: Secret,
    ) -> ActionOutcome {
        if !self.takes_setup_tokens() {
            return refused(TOKENS_OFF);
        }
        let Some(token) = pasted(token, MAX_TOKEN) else {
            return refused(
                "That isn't a setup-token. Paste the line `claude setup-token` printed, and nothing else.",
            );
        };
        let Some(label) = named(&label, DEFAULT_LABEL) else {
            return refused(TOO_LONG);
        };
        let token = Token {
            origin,
            label,
            stored_at: self.now(),
            token,
        };
        let Ok(text) = serde_json::to_vec(&token).map(Zeroizing::new) else {
            return refused("This device couldn't store the token.");
        };

        let mut tokens = self.inner.claude.lock();
        if let Err(err) = self.inner.folder.write(&claude_token_file(origin), &text) {
            warn!(%origin, "couldn't store a Claude setup-token: {err}");
            return refused(format!("This device couldn't store the token: {err}"));
        }
        let account = self.publish_token(&token);
        tokens.insert(account, token);
        info!(%origin, %account, "stored a Claude setup-token");
        ActionOutcome::Done
    }

    /// The setup-token behind a token account, or none while the device's
    /// setting is off.
    pub(super) fn setup_token(&self, account: AccountId) -> Option<Secret> {
        if !self.takes_setup_tokens() {
            return None;
        }
        let tokens = self.inner.claude.lock();
        tokens.get(&account).map(|token| token.token.clone())
    }

    /// Whether the store holds a setup-token, whatever the setting.
    pub(super) fn holds_setup_token(&self) -> bool {
        !self.inner.claude.lock().is_empty()
    }

    /// Whether you turned setup-tokens on for this device.
    fn takes_setup_tokens(&self) -> bool {
        let hub = &self.inner.hub;
        matches!(
            hub.get(&EntityKey::DeviceSettings(hub.device().id)),
            Some(Entity::DeviceSettings(settings)) if settings.claude_setup_token
        )
    }

    /// Reads every token the folder holds, and publishes each one's account.
    pub(super) fn load_claude(&self) {
        let names = match self.inner.folder.names() {
            Ok(names) => names,
            Err(err) => {
                warn!("couldn't list the stored setup-tokens: {err}");
                return;
            }
        };
        let mut tokens = self.inner.claude.lock();
        for name in names {
            if !(name.starts_with("claude-") && name.ends_with(".token")) {
                continue;
            }
            match self.read_token(&name) {
                Ok(token) => {
                    let account = self.publish_token(&token);
                    tokens.insert(account, token);
                }
                Err(why) => warn!("skipped {name}: {why}"),
            }
        }
    }

    /// The token in `name`. The reason it gives for a bad file never quotes
    /// the file.
    fn read_token(&self, name: &str) -> Result<Token, String> {
        let text = self
            .inner
            .folder
            .read(name)
            .map_err(|err| err.to_string())?
            .ok_or("it went away")?;
        let token: Token = serde_json::from_str(text.expose())
            .map_err(|_| "it doesn't hold a stored setup-token".to_owned())?;
        if claude_token_file(token.origin) != name {
            return Err("its name doesn't match the account in it".to_owned());
        }
        Ok(token)
    }

    /// Publishes the token's account and credential, and returns the
    /// account.
    fn publish_token(&self, token: &Token) -> AccountId {
        let device = self.device();
        let account = token_account(device, token.origin);
        let credential = Credential {
            id: credential_id(device, token.origin),
            device,
            kind: CredentialKind::ClaudeToken {
                origin: token.origin,
                account,
                label: token.label.clone(),
            },
            stored_at: token.stored_at,
            problem: None,
        };
        let shown = login_account(
            account,
            device,
            Vendor::Claude,
            token.label.clone(),
            LoginKind::SetupToken,
            format!("{CONTAINER_CONFIG}{account}"),
        );
        let shown = Account {
            origin: Some(token.origin),
            ..shown
        };
        self.publish(credential, shown);
        account
    }

    /// Forgets the token behind `credential`, with its account. Returns
    /// none when `credential` isn't a token's.
    pub(super) fn forget_claude(&self, credential: CredentialId) -> Option<ActionOutcome> {
        let device = self.device();
        let mut tokens = self.inner.claude.lock();
        let (account, origin) = tokens
            .iter()
            .find(|(_, token)| credential_id(device, token.origin) == credential)
            .map(|(account, token)| (*account, token.origin))?;
        if let Err(err) = self.inner.folder.remove(&claude_token_file(origin)) {
            warn!(%origin, "couldn't delete a Claude setup-token: {err}");
            return Some(refused(format!(
                "This device couldn't delete the token: {err}"
            )));
        }
        tokens.remove(&account);
        self.unpublish(credential, account);
        info!(%origin, %account, "forgot a Claude setup-token");
        Some(ActionOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::secrets::testing::{account, credentials, hub, token_hub, CLAUDE_TOKEN};
    use crate::secrets::DIR;
    use crate::session::holders::Credentials;
    use crate::testing::TempDir;

    fn open(temp: &TempDir) -> Secrets {
        Secrets::open(temp.path().join(DIR), token_hub()).unwrap()
    }

    fn origin() -> AccountId {
        AccountId::derive("mac-personal")
    }

    #[test]
    fn stores_a_token_and_publishes_its_account() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let outcome = secrets.store_claude_token(
            origin(),
            " Personal ".into(),
            Secret::new(format!("{CLAUDE_TOKEN}\n")),
        );
        assert_eq!(outcome, ActionOutcome::Done);

        let id = token_account(secrets.device(), origin());
        let account = account(&secrets, id).unwrap();
        assert_eq!(account.login, LoginKind::SetupToken);
        assert_eq!(account.origin, Some(origin()));
        assert_eq!(account.label, "Personal");
        assert_eq!(account.config_dir, format!("/home/agent/.claude-{id}"));
        assert_eq!(account.vendor, Vendor::Claude);

        let [credential] = &credentials(&secrets)[..] else {
            panic!("one credential");
        };
        assert_eq!(
            credential.kind,
            CredentialKind::ClaudeToken {
                origin: origin(),
                account: id,
                label: "Personal".into(),
            }
        );
        assert_eq!(credential.id, credential_id(secrets.device(), origin()));
        assert_eq!(
            secrets.claude_token(id).unwrap().expose(),
            CLAUDE_TOKEN,
            "trimmed"
        );
        assert!(secrets.claude_token(origin()).is_none());

        let file = temp.path().join(DIR).join(claude_token_file(origin()));
        let mode =
            |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&temp.path().join(DIR)), 0o700);
    }

    #[test]
    fn refuses_and_withholds_tokens_while_the_setting_is_off() {
        let temp = TempDir::new();
        let off = Secrets::open(temp.path().join(DIR), hub()).unwrap();
        let outcome =
            off.store_claude_token(origin(), String::new(), Secret::new(CLAUDE_TOKEN.into()));
        assert_eq!(outcome, refused(TOKENS_OFF));
        assert!(credentials(&off).is_empty());
        assert!(off.inner.folder.names().unwrap().is_empty());
        assert!(!off.holds_claude_token());

        // A token stored while it was on stays listed, so it can be
        // forgotten, but no agent gets it.
        let on = open(&temp);
        on.store_claude_token(origin(), String::new(), Secret::new(CLAUDE_TOKEN.into()));
        drop(on);
        let off = Secrets::open(temp.path().join(DIR), hub()).unwrap();
        let id = token_account(off.device(), origin());
        assert!(off.holds_claude_token());
        assert!(off.claude_token(id).is_none());
        assert_eq!(credentials(&off).len(), 1);
        let credential = credential_id(off.device(), origin());
        assert_eq!(off.forget_file(credential), ActionOutcome::Done);
        assert!(!off.holds_claude_token());
    }

    #[test]
    fn joins_a_token_the_terminal_wrapped() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (head, tail) = CLAUDE_TOKEN.split_at(24);
        let wrapped = Secret::new(format!("  {head}\n   {tail} \r\n"));
        let outcome = secrets.store_claude_token(origin(), String::new(), wrapped);
        assert_eq!(outcome, ActionOutcome::Done);

        let id = token_account(secrets.device(), origin());
        assert_eq!(secrets.claude_token(id).unwrap().expose(), CLAUDE_TOKEN);
        drop(secrets);
        assert_eq!(open(&temp).claude_token(id).unwrap().expose(), CLAUDE_TOKEN);
    }

    #[test]
    fn refuses_what_isnt_a_token() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        for bad in [
            "",
            "  \n",
            "test-not-a-real\0-token",
            "test-not-a-réal-token",
        ] {
            let outcome =
                secrets.store_claude_token(origin(), String::new(), Secret::new(bad.to_owned()));
            assert!(matches!(outcome, ActionOutcome::Refused { .. }), "{bad:?}");
        }
        let long = "a".repeat(crate::secrets::MAX_LABEL + 1);
        let outcome =
            secrets.store_claude_token(origin(), long, Secret::new(CLAUDE_TOKEN.to_owned()));
        assert!(matches!(outcome, ActionOutcome::Refused { .. }));
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.inner.folder.names().unwrap().is_empty());
    }

    #[test]
    fn a_second_token_replaces_the_first() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let first =
            secrets.store_claude_token(origin(), String::new(), Secret::new("test-first".into()));
        assert_eq!(first, ActionOutcome::Done);
        let second =
            secrets.store_claude_token(origin(), "Work".into(), Secret::new("test-second".into()));
        assert_eq!(second, ActionOutcome::Done);

        let id = token_account(secrets.device(), origin());
        assert_eq!(secrets.claude_token(id).unwrap().expose(), "test-second");
        assert_eq!(account(&secrets, id).unwrap().label, "Work");
        assert_eq!(credentials(&secrets).len(), 1);
        assert_eq!(secrets.inner.folder.names().unwrap().len(), 1);
    }

    #[test]
    fn names_a_token_with_no_name() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        secrets.store_claude_token(origin(), "  ".into(), Secret::new(CLAUDE_TOKEN.into()));
        let id = token_account(secrets.device(), origin());
        assert_eq!(account(&secrets, id).unwrap().label, DEFAULT_LABEL);
    }

    #[test]
    fn opening_again_brings_tokens_back() {
        let temp = TempDir::new();
        let id = {
            let secrets = open(&temp);
            secrets.store_claude_token(
                origin(),
                "Personal".into(),
                Secret::new(CLAUDE_TOKEN.into()),
            );
            token_account(secrets.device(), origin())
        };
        // A file someone dropped in, and one renamed by hand.
        let dir = temp.path().join(DIR);
        fs::write(dir.join("claude-junk.token"), "not json").unwrap();
        fs::copy(
            dir.join(claude_token_file(origin())),
            dir.join(claude_token_file(AccountId::derive("other"))),
        )
        .unwrap();

        let secrets = open(&temp);
        assert_eq!(secrets.claude_token(id).unwrap().expose(), CLAUDE_TOKEN);
        assert_eq!(account(&secrets, id).unwrap().label, "Personal");
        assert_eq!(credentials(&secrets).len(), 1);
    }

    #[test]
    fn forgetting_a_token_drops_its_account() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        secrets.store_claude_token(origin(), String::new(), Secret::new(CLAUDE_TOKEN.into()));
        let id = token_account(secrets.device(), origin());
        let credential = credential_id(secrets.device(), origin());

        assert_eq!(secrets.forget_file(credential), ActionOutcome::Done);
        assert!(secrets.claude_token(id).is_none());
        assert!(account(&secrets, id).is_none());
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.inner.folder.names().unwrap().is_empty());
        assert!(matches!(
            secrets.forget_file(credential),
            ActionOutcome::Refused { .. }
        ));
        assert!(open(&temp).claude_token(id).is_none());

        // Signing in again doesn't bring back the models the old login
        // listed.
        let listed = maxhq_protocol::model::ModelChoice {
            id: "opus".into(),
            label: "Opus".into(),
            default: true,
            efforts: Vec::new(),
            default_effort: None,
        };
        secrets.store_claude_token(origin(), String::new(), Secret::new(CLAUDE_TOKEN.into()));
        secrets.inner.hub.set_models(id, vec![listed]);
        assert_eq!(account(&secrets, id).unwrap().models.len(), 1);
        assert_eq!(secrets.forget_file(credential), ActionOutcome::Done);
        secrets.store_claude_token(origin(), String::new(), Secret::new(CLAUDE_TOKEN.into()));
        assert!(account(&secrets, id).unwrap().models.is_empty());
    }
}
