// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading the credits of the servers you added that meter them, which is
//! OpenRouter for now, when [`credits::Meter`] says a read is due, with the
//! account's balance when you added an admin key. The figures stay in
//! memory, never in the database, and show on the server's entity.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use maxhq_protocol::ids::{AccountId, ModelServerId};
use maxhq_protocol::local_models::ModelServer;
use maxhq_protocol::model::Activity;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::Entity;

use super::super::cloud;
use super::super::credits::{self, Failure};
use super::{admin_name, token_name, Service, State, NO_SERVER};

/// Why an admin key was refused for a server that isn't OpenRouter.
const NOT_OPENROUTER: &str = "Only an OpenRouter server takes an admin key.";

/// Why an admin key was refused when OpenRouter asked to wait.
const WAIT_TO_CHECK: &str =
    "OpenRouter asked MaxHQ to wait before checking the admin key. Try again in a minute.";

/// Where a server's credits can be read: an OpenRouter server with a token.
fn metered(server: &ModelServer) -> Option<&str> {
    server
        .base_url
        .as_deref()
        .filter(|base| server.token && cloud::is_openrouter(base))
}

impl Service {
    /// `server` as its entity shows it, with the credits read last.
    pub(super) fn with_credits(state: &State, server: &ModelServer) -> ModelServer {
        ModelServer {
            credits: state
                .credits
                .get(&server.id)
                .and_then(|meter| meter.shown.clone()),
            ..server.clone()
        }
    }

    /// Notes that a turn of a local agent on `account` ended, so its
    /// server's credits are read again soon.
    pub fn turn_ended(&self, account: AccountId) {
        let mut state = self.lock();
        let State {
            servers, credits, ..
        } = &mut *state;
        let Some(server) = servers
            .values()
            .find(|server| metered(server).is_some() && self.account_of(server.id) == account)
        else {
            return;
        };
        credits
            .entry(server.id)
            .or_default()
            .turn_ended(Instant::now());
    }

    /// Reads `server`'s credits now, as on Test or right after it's added,
    /// unless a read is on its way or OpenRouter asked to wait.
    pub(super) async fn read_credits_now(&self, server: ModelServerId) {
        let base = {
            let mut state = self.lock();
            let State {
                servers, credits, ..
            } = &mut *state;
            let Some(base) = servers.get(&server).and_then(metered) else {
                return;
            };
            let meter = credits.entry(server).or_default();
            meter.want();
            let now = Instant::now();
            if !meter.due(now, || false) {
                return;
            }
            meter.start(now);
            base.to_owned()
        };
        self.read_credits(server, &base).await;
    }

    /// Starts each read that's due, in the background, and forgets the
    /// figures of servers that went.
    pub(super) fn read_due_credits(self: &Arc<Self>) {
        let watched: Vec<ModelServerId> = {
            let mut state = self.lock();
            let State {
                servers, credits, ..
            } = &mut *state;
            credits.retain(|id, _| servers.get(id).and_then(metered).is_some());
            servers
                .values()
                .filter(|server| metered(server).is_some())
                .map(|server| server.id)
                .collect()
        };
        if watched.is_empty() {
            return;
        }
        let working = self.working_accounts();
        let now = Instant::now();
        let due: Vec<(ModelServerId, String)> = {
            let mut state = self.lock();
            let State {
                servers, credits, ..
            } = &mut *state;
            watched
                .into_iter()
                .filter_map(|id| {
                    let base = servers.get(&id).and_then(metered)?.to_owned();
                    let meter = credits.entry(id).or_default();
                    if !meter.due(now, || working.contains(&self.account_of(id))) {
                        return None;
                    }
                    meter.start(now);
                    Some((id, base))
                })
                .collect()
        };
        for (id, base) in due {
            let service = self.clone();
            tokio::spawn(async move { service.read_credits(id, &base).await });
        }
    }

    /// The accounts with an agent at work now.
    fn working_accounts(&self) -> HashSet<AccountId> {
        self.hub
            .pick(|entity| match entity {
                Entity::Agent(agent)
                    if matches!(
                        agent.activity,
                        Activity::Starting | Activity::Busy | Activity::BusyInSubagents
                    ) =>
                {
                    agent.account
                }
                _ => None,
            })
            .into_iter()
            .collect()
    }

    /// Keeps `key` as the admin key of `server`, an OpenRouter server,
    /// after one read of the account's balance with it, or removes the one
    /// kept when `key` is None. A key OpenRouter turns down is refused,
    /// and nothing of it is kept.
    pub(super) async fn set_admin_key(
        &self,
        server: ModelServerId,
        key: Option<Secret>,
    ) -> Result<(), String> {
        let base = {
            let state = self.lock();
            let found = state.servers.get(&server).ok_or(NO_SERVER)?;
            found.base_url.clone()
        };
        let name = admin_name(server);
        let balance = match &key {
            Some(key) => {
                let base = base
                    .filter(|base| cloud::is_openrouter(base))
                    .ok_or(NOT_OPENROUTER)?;
                let balance =
                    credits::balance(&base, key)
                        .await
                        .map_err(|failure| match failure {
                            Failure::Busy(_) => WAIT_TO_CHECK.to_owned(),
                            Failure::Other(why) => why,
                        })?;
                self.keys
                    .write(&name, key.expose().as_bytes())
                    .map_err(|err| format!("Couldn't keep the admin key: {err}"))?;
                Some(balance)
            }
            None => {
                self.keys
                    .remove(&name)
                    .map_err(|err| format!("Couldn't delete the admin key: {err}"))?;
                None
            }
        };
        let Some(saved) = self.show_admin_key(server, key.is_some(), balance) else {
            let _ = self.keys.remove(&name);
            return Err(NO_SERVER.to_owned());
        };
        if let Err(reason) = self.save(&saved) {
            if key.is_some() {
                let _ = self.keys.remove(&name);
                self.show_admin_key(server, false, None);
            }
            self.publish();
            return Err(reason);
        }
        self.publish();
        Ok(())
    }

    /// Marks whether `server` has an admin key, with the account's
    /// `balance` it read, and answers the server as kept. None when the
    /// server went.
    fn show_admin_key(
        &self,
        server: ModelServerId,
        held: bool,
        balance: Option<credits::Balance>,
    ) -> Option<ModelServer> {
        let mut state = self.lock();
        let State {
            servers, credits, ..
        } = &mut *state;
        let found = servers.get_mut(&server)?;
        found.admin_key = held;
        if let Some(meter) = credits.get_mut(&server) {
            meter.set_balance(balance);
        }
        Some(found.clone())
    }

    /// Reads the credits of `server` at `base`, whose meter has started
    /// the read, and shows what changed. With an admin key it reads the
    /// account's balance too.
    async fn read_credits(&self, server: ModelServerId, base: &str) {
        let admin_held = self
            .lock()
            .servers
            .get(&server)
            .is_some_and(|found| found.admin_key);
        let admin = if admin_held {
            match self.keys.read(&admin_name(server)) {
                Ok(Some(key)) => Ok(Some(key)),
                Ok(None) | Err(_) => Err(Failure::Other(
                    "Couldn't read the admin key on this device.".to_owned(),
                )),
            }
        } else {
            Ok(None)
        };
        let result = match (self.keys.read(&token_name(server)), admin) {
            (Ok(Some(token)), Ok(admin)) => credits::read(base, &token, admin.as_ref()).await,
            (Ok(Some(_)), Err(failure)) => Err(failure),
            (Ok(None) | Err(_), _) => Err(Failure::Other(
                "Couldn't read the server's token on this device.".to_owned(),
            )),
        };
        let changed = self
            .lock()
            .credits
            .get_mut(&server)
            .is_some_and(|meter| meter.finish(result, Instant::now()));
        if changed {
            self.publish();
        }
    }
}
