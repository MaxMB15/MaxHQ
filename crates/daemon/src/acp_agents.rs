// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The local ACP agents defined on this device. Each one is a program, its
//! arguments and the names of its variables, kept in the database, with
//! the variables' values in the secret store. Each shows as its own
//! account, which agents start on like any other.
//!
//! Saving a definition runs its program once with `initialize`, as a
//! probe, and keeps what the agent said it can do on the account. A probe
//! that fails still saves, with why on the definition.
//!
//! Requests arrive through the free functions here, which reach the store
//! [`install`] put in place. Only the app sends them: the server takes them
//! from no agent tools connection and no peer link.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use maxhq_core::names;
use maxhq_protocol::acp::{AcpAgent, AcpEnvValue, AcpFeatures};
use maxhq_protocol::ids::{AccountId, AcpAgentId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Activity, LoginKind, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, warn};

use crate::db::Db;
use crate::drive::acp::rpc::{self, Incoming, Wire};
use crate::drive::acp::{features, launch};
use crate::drive::local::AcpCommand;
use crate::drive::{Launch, Place, PlaceHost};
use crate::hub::Hub;
use crate::secrets::Secrets;
use crate::server::not_yet;
use crate::session::holders::Credentials;

/// The longest name, program or argument a definition takes.
const MAX_FIELD: usize = 4096;
/// The longest name an agent takes.
const MAX_NAME: usize = 200;
/// How long the probe waits for the `initialize` reply. An agent that
/// fetches itself on first run, as `npx` does, can take a while.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// The probe's request ID.
const PROBE_ID: &str = "probe";

/// The store the free functions reach, once `maxd` has opened it.
static STORE: OnceLock<AcpAgents> = OnceLock::new();

/// The account a definition shows as.
pub fn account_of(agent: &AcpAgent) -> AccountId {
    names::account(agent.device, Vendor::Acp, &agent.id.to_string())
}

/// A definition as the database keeps it, with what its last probe found.
#[derive(Serialize, Deserialize)]
struct Record {
    agent: AcpAgent,
    features: Option<AcpFeatures>,
    /// The sign-in methods the agent runs in a terminal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    terminal: Vec<String>,
}

/// The device's local ACP agents. Clones share it.
#[derive(Clone)]
pub struct AcpAgents {
    inner: Arc<Inner>,
}

struct Inner {
    hub: Hub,
    db: Db,
    secrets: Secrets,
    timeout: Duration,
    /// Each definition's terminal sign-in methods, which the account
    /// doesn't show.
    terminal: Mutex<HashMap<AcpAgentId, Vec<String>>>,
}

impl AcpAgents {
    /// The store, showing the definitions the database holds.
    pub fn open(hub: Hub, db: Db, secrets: Secrets) -> Self {
        Self::with_timeout(hub, db, secrets, PROBE_TIMEOUT)
    }

    fn with_timeout(hub: Hub, db: Db, secrets: Secrets, timeout: Duration) -> Self {
        let store = Self {
            inner: Arc::new(Inner {
                hub,
                db,
                secrets,
                timeout,
                terminal: Mutex::default(),
            }),
        };
        let records = store.inner.db.acp_agents().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the ACP agents");
            Vec::new()
        });
        for record in records {
            match serde_json::from_str::<Record>(&record) {
                Ok(record) => store.publish(&record),
                Err(err) => warn!(%err, "skipping an ACP agent that doesn't parse"),
            }
        }
        store
    }

    /// Answers `SetAcpAgent`.
    pub(crate) async fn set(&self, agent: AcpAgent, env: Vec<AcpEnvValue>) -> ActionOutcome {
        self.store(agent, env, true).await
    }

    /// Stores a definition, probing it on this device only when `probe`
    /// says it runs here. One that runs only in containers keeps what its
    /// sessions last found, and its first session reports a failure.
    pub(crate) async fn store(
        &self,
        mut agent: AcpAgent,
        env: Vec<AcpEnvValue>,
        probe: bool,
    ) -> ActionOutcome {
        if let Err(reason) = check(&mut agent, &env) {
            return refused(reason);
        }
        agent.device = self.inner.hub.device().id;
        let keep = agent.env.clone();
        let secrets = self.inner.secrets.clone();
        let id = agent.id;
        let stored = tokio::task::spawn_blocking(move || secrets.set_acp_env(id, &keep, env)).await;
        if let Err(err) = stored.unwrap_or_else(|err| Err(io::Error::other(err))) {
            warn!(agent = %id, "couldn't store an ACP agent's values: {err}");
            return refused(format!("This device couldn't store the values: {err}"));
        }

        let values = self.inner.secrets.acp_env(id);
        let command = AcpCommand {
            agent: agent.clone(),
            env: values,
        };
        let (features, terminal, problem) = if !probe {
            let features = match self.inner.hub.get(&EntityKey::Account(account_of(&agent))) {
                Some(Entity::Account(account)) => account.acp,
                _ => None,
            };
            (features, self.terminal(id), None)
        } else {
            match self::probe(command, self.inner.timeout).await {
                Ok((features, terminal)) => (Some(features), terminal, None),
                Err(problem) => (None, Vec::new(), Some(problem)),
            }
        };
        agent.problem = problem;
        let record = Record {
            agent,
            features,
            terminal,
        };
        let stored = serde_json::to_string(&record)
            .map_err(|err| err.to_string())
            .and_then(|text| {
                self.inner
                    .db
                    .put_acp_agent(id, &text)
                    .map_err(|err| err.to_string())
            });
        if let Err(err) = stored {
            warn!(agent = %id, "couldn't store an ACP agent: {err}");
            return refused(format!("This device couldn't store the agent: {err}"));
        }
        self.publish(&record);
        info!(agent = %id, "stored an ACP agent");
        ActionOutcome::Done
    }

    /// Answers `DeleteAcpAgent`.
    pub(crate) fn delete(&self, id: AcpAgentId) -> ActionOutcome {
        let Some(agent) = self.find(id) else {
            return refused("This device doesn't have that ACP agent.");
        };
        let account = account_of(&agent);
        if self.runs_on(account) {
            return refused(format!(
                "Agents of {} are still running. Stop them first.",
                agent.name
            ));
        }
        if let Err(err) = self.inner.db.remove_acp_agent(id) {
            warn!(agent = %id, %err, "couldn't delete an ACP agent");
            return refused(format!("This device couldn't delete the agent: {err}"));
        }
        if let Err(err) = self.inner.secrets.set_acp_env(id, &[], Vec::new()) {
            warn!(agent = %id, "couldn't forget an ACP agent's values: {err}");
        }
        self.terminals().remove(&id);
        self.inner.hub.forget_models(account);
        self.inner.hub.apply([
            Change::Remove {
                key: EntityKey::AcpAgent(id),
            },
            Change::Remove {
                key: EntityKey::Account(account),
            },
        ]);
        info!(agent = %id, "deleted an ACP agent");
        ActionOutcome::Done
    }

    /// Keeps what a session's `initialize` said the agent on `account`
    /// can do, when it differs from what the account shows. An agent that
    /// answered has no problem left to show.
    fn found(&self, account: AccountId, features: AcpFeatures) {
        let shown = self.inner.hub.pick(|entity| match entity {
            Entity::AcpAgent(agent) if account_of(agent) == account => Some(agent.clone()),
            _ => None,
        });
        let Some(mut agent) = shown.into_iter().next() else {
            return;
        };
        let same = matches!(
            self.inner.hub.get(&EntityKey::Account(account)),
            Some(Entity::Account(shown)) if shown.acp.as_ref() == Some(&features)
        );
        if same && agent.problem.is_none() {
            return;
        }
        agent.problem = None;
        let id = agent.id;
        let terminal = self.terminal(id);
        let record = Record {
            agent,
            features: Some(features),
            terminal,
        };
        match serde_json::to_string(&record) {
            Ok(text) => {
                if let Err(err) = self.inner.db.put_acp_agent(id, &text) {
                    warn!(agent = %id, %err, "couldn't store what an ACP agent can do");
                }
            }
            Err(err) => warn!(agent = %id, %err, "couldn't store what an ACP agent can do"),
        }
        self.publish(&record);
    }

    /// The sign-in methods the agent defined as `id` runs in a terminal.
    pub(crate) fn terminal(&self, id: AcpAgentId) -> Vec<String> {
        self.terminals().get(&id).cloned().unwrap_or_default()
    }

    fn terminals(&self) -> std::sync::MutexGuard<'_, HashMap<AcpAgentId, Vec<String>>> {
        self.inner
            .terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The definition shown as `id`.
    fn find(&self, id: AcpAgentId) -> Option<AcpAgent> {
        match self.inner.hub.get(&EntityKey::AcpAgent(id)) {
            Some(Entity::AcpAgent(agent)) => Some(agent),
            _ => None,
        }
    }

    /// Whether an agent on `account` is running, or may be: one in a
    /// container whose link is down counts.
    fn runs_on(&self, account: AccountId) -> bool {
        let running = self.inner.hub.pick(|entity| match entity {
            Entity::Agent(agent)
                if agent.account == Some(account)
                    && !matches!(
                        agent.activity,
                        Activity::Paused { .. } | Activity::Error { .. }
                    ) =>
            {
                Some(())
            }
            _ => None,
        });
        !running.is_empty()
    }

    /// Shows a definition and the account its agents run on.
    fn publish(&self, record: &Record) {
        let agent = &record.agent;
        self.terminals().insert(agent.id, record.terminal.clone());
        let account = Account {
            id: account_of(agent),
            device: agent.device,
            vendor: Vendor::Acp,
            label: agent.name.clone(),
            login: LoginKind::AcpCommand,
            origin: None,
            config_dir: String::new(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: record.features.clone(),
        };
        self.inner.hub.apply([
            Change::Upsert {
                value: Entity::AcpAgent(agent.clone()),
            },
            Change::Upsert {
                value: Entity::Account(account),
            },
        ]);
    }
}

/// Checks a definition the app sent, trimming its name and program, and
/// that each value is for one of its variables.
fn check(agent: &mut AcpAgent, env: &[AcpEnvValue]) -> Result<(), String> {
    agent.name = agent.name.trim().to_owned();
    agent.program = agent.program.trim().to_owned();
    if agent.name.is_empty() {
        return Err("Give the agent a name.".to_owned());
    }
    if agent.name.chars().count() > MAX_NAME {
        return Err("That name is too long.".to_owned());
    }
    if agent.program.is_empty() {
        return Err("Give the program to run.".to_owned());
    }
    let fields = std::iter::once(&agent.program).chain(&agent.args);
    for field in fields {
        if field.len() > MAX_FIELD || field.contains('\0') {
            return Err("The program and its arguments can't hold that.".to_owned());
        }
    }
    for (at, name) in agent.env.iter().enumerate() {
        if !variable_name(name) {
            return Err(format!("{name:?} isn't a variable name."));
        }
        if agent.env[..at].contains(name) {
            return Err(format!("{name} is listed twice."));
        }
    }
    if let Some(value) = env.iter().find(|value| !agent.env.contains(&value.name)) {
        return Err(format!(
            "{} isn't one of the agent's variables.",
            value.name
        ));
    }
    Ok(())
}

/// Whether `name` is a shell variable's name: a letter or underscore, then
/// letters, digits and underscores.
pub(crate) fn variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= MAX_NAME
}

/// Runs the agent's program once and asks it to `initialize`, the way its
/// sessions will run it, with what it said it can do or why that failed.
async fn probe(
    command: AcpCommand,
    timeout: Duration,
) -> Result<(AcpFeatures, Vec<String>), String> {
    let name = command.agent.program.clone();
    if launch::program(&name, std::env::var_os("PATH").as_deref()).is_none() {
        return Err(format!("{name} isn't installed on this device"));
    }
    let cwd = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let place = Place {
        device: command.agent.device,
        workspace: WorkspaceId::derive("acp-probe"),
        attachments: cwd.clone(),
        cwd,
        host: PlaceHost::Local,
        dispatcher: false,
        trust: None,
    };
    let launch = launch::launch(&command, &place);
    let mut child = spawn(&launch).map_err(|err| format!("couldn't start {name}: {err}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(format!("couldn't talk to {name}"));
    };
    let mut wire = Wire::default();
    let asked = async {
        let mut line = wire.request(PROBE_ID, "initialize", rpc::initialize(json!({})));
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(Incoming::Response { id, outcome }) = wire.read(&line)
                && id == PROBE_ID
            {
                return Ok(Some(outcome));
            }
        }
        Ok::<_, io::Error>(None)
    };
    let reply = tokio::time::timeout(timeout, asked).await;
    let _ = child.start_kill();
    let result = match reply {
        Err(_) => return Err(format!("{name} didn't answer initialize")),
        Ok(Err(err)) => return Err(format!("couldn't talk to {name}: {err}")),
        Ok(Ok(None)) => return Err(format!("{name} exited without answering initialize")),
        Ok(Ok(Some(Err(err)))) => {
            return Err(format!("{name} refused initialize: {}", err.message))
        }
        Ok(Ok(Some(Ok(result)))) => result,
    };
    if wire.generation().is_none() {
        return Err(rpc::unspoken(&name, &result));
    }
    Ok((
        features::features(&result),
        features::terminal_methods(&result),
    ))
}

/// Starts `launch` as the probe's child, which dies with the probe.
fn spawn(launch: &Launch) -> io::Result<tokio::process::Child> {
    launch
        .command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// Makes `store` the one the free functions reach. Only the first call
/// counts, since `maxd` opens one store.
pub fn install(store: AcpAgents) {
    let _ = STORE.set(store);
}

/// Answers `SetAcpAgent`.
pub async fn set(agent: AcpAgent, env: Vec<AcpEnvValue>) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.set(agent, env).await,
        None => not_yet(),
    }
}

/// Answers `DeleteAcpAgent`.
pub fn delete(id: AcpAgentId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.delete(id),
        None => not_yet(),
    }
}

/// Keeps what a session found the agent on `account` can do.
pub fn found(account: AccountId, features: AcpFeatures) {
    if let Some(store) = STORE.get() {
        store.found(account, features);
    }
}

/// A refusal for a person to read.
fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
