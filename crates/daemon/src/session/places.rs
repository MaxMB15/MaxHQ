// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where agents run, and what they sign in with. An agent runs on this
//! device, or in a container workspace whose guest holds its CLI and runs
//! it as `agent`. A guest is reached over its link, which goes down and
//! comes back up while its agents keep running.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use maxhq_protocol::acp::AcpAgent;
use maxhq_protocol::guest::{WatchedHolder, ATTACHMENTS_DIR};
use maxhq_protocol::ids::{AccountId, AgentId, BootId, IdempotencyKey, SessionId, WorkspaceId};
use maxhq_protocol::model::{LoginKind, Vendor, WorkspaceState};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::{info, warn};

use super::held::{Held, Reaching, ServerKey, Target, Waiting, Written};
use super::holders::{AgentSummary, Holders, PlaceChange, SessionOwner, SessionOwners};
use super::reads::Source;
use super::revive::MAX_RETRIES;
use super::vendors::one_process_per_agent;
use super::{refused, Manager};
use crate::acp_agents;
use crate::drive::{codex, local, Action, Launch, Login, LoginAuth, Place, PlaceHost, RunAs};
use crate::ports::{Holding, Holdings};
use crate::recipe;

/// How long a guest's holder that couldn't be reached waits before the
/// next try, while its link is up.
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// The variable Claude reads a setup-token from.
const TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Why an agent can't start on a setup-token that's gone.
const NO_TOKEN: &str = "The setup-token for this account is gone. Add it again to carry on";

/// Why an agent can't start on a setup-token while the device's setting is
/// off.
const TOKENS_OFF: &str =
    "Setup-tokens are off on this device. Turn them on in its settings, or sign in with claude there";

/// Why a Codex agent can't start without its ChatGPT sign-in.
const NO_SIGN_IN: &str = "This account isn't signed in to Codex. Sign in again to carry on";

/// Why a Cursor agent can't start on an API key that's gone.
const NO_KEY: &str = "The Cursor API key for this account is gone. Add it again to carry on";

/// Why a Cursor agent on the Mac's CLI login can't run in a container.
const NO_GUEST_LOGIN: &str =
    "Cursor agents in containers sign in with this device's Cursor API key. Add one in Settings";

/// Why a Codex agent on a device's own sign-in can't run outside a
/// container.
const HOST_SIGN_IN: &str = "This account's Codex sign-in runs agents in container workspaces only";

/// Why an agent can't start on a local ACP agent that was deleted.
const NO_ACP_AGENT: &str = "This ACP agent isn't set up on this device any more";

/// Why a local agent can't start on a model server that's gone.
const NO_MODEL_SERVER: &str =
    "This model server isn't set up on this device any more, or has no model ready";

/// Why a local agent can't run in a container.
const NO_GUEST_MODEL: &str =
    "Local agents run on the device itself for now, not in a container workspace";

/// Why a guest's link is down once you stop its container.
const STOPPED: &str = "the container stopped";

/// Why what waited for an agent in a deleted workspace is refused.
const DELETED: &str = "the agent's workspace was deleted";

/// A container workspace's guest link, as last heard.
#[derive(Default)]
pub struct Link {
    /// The container's boot ID when the link last came up.
    pub boot: Option<BootId>,
    /// Why the link is down and since when, while it's down.
    pub down: Option<(String, Timestamp)>,
    /// Whether it went down because you stopped the container.
    pub stopped: bool,
}

/// Who owns each session held in a guest, for the guest link. The manager
/// keeps it current, and the link reads it from its own tasks.
#[derive(Clone, Default)]
pub struct Owners(Arc<Mutex<HashMap<SessionId, SessionOwner>>>);

impl Owners {
    /// Replaces what's known, if it changed.
    pub(crate) fn set(&self, owners: HashMap<SessionId, SessionOwner>) {
        let mut known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if *known != owners {
            *known = owners;
        }
    }
}

impl SessionOwners for Owners {
    fn owner(&self, session: SessionId) -> Option<SessionOwner> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .copied()
    }
}

/// The login kind behind a login's auth.
pub(super) fn kind_of(auth: &LoginAuth) -> LoginKind {
    match auth {
        LoginAuth::ConfigDir => LoginKind::ConfigDir,
        LoginAuth::SetupToken(_) => LoginKind::SetupToken,
        LoginAuth::ChatGptBroker => LoginKind::ChatGptSignIn,
        LoginAuth::CursorApiKey(_) => LoginKind::CursorApiKey,
        LoginAuth::AcpCommand(_) => LoginKind::AcpCommand,
        LoginAuth::ModelServer(_) => LoginKind::ModelServer,
    }
}

/// Where a held agent's history is read from.
pub(super) fn source(held: &Held) -> Source {
    Source {
        vendor: held.vendor(),
        config_dir: held.config_dir.clone(),
        session: held.vendor_session.clone(),
        subagent: None,
        workspace: held.guest,
        account: Some(held.account),
    }
}

/// The guest's `PATH` for an agent's CLI.
fn guest_path() -> String {
    format!(
        "{}/.local/bin:/usr/local/bin:/usr/bin:/bin",
        recipe::AGENT_HOME
    )
}

/// The folder in a guest that an agent's attachments go to.
pub fn guest_attachments(name: &str) -> PathBuf {
    Path::new(ATTACHMENTS_DIR).join(name)
}

impl Manager {
    /// The holders for a workspace's guest, or this device's for none.
    /// There are none while the guest's link is down.
    pub(super) fn holders_at(&self, workspace: Option<WorkspaceId>) -> Option<Arc<dyn Holders>> {
        let Some(workspace) = workspace else {
            return Some(self.holders.clone());
        };
        if !self.link_up(workspace) {
            return None;
        }
        self.places.guest(workspace)
    }

    /// Whether a guest's link is up.
    pub(super) fn link_up(&self, workspace: WorkspaceId) -> bool {
        self.links
            .get(&workspace)
            .is_some_and(|link| link.down.is_none() && link.boot.is_some())
    }

    /// The login a Claude agent runs under. A setup-token comes from the
    /// secret store each launch, so a token you replace is the one used.
    pub(super) fn claude_login(
        &self,
        account: AccountId,
        config_dir: PathBuf,
        kind: LoginKind,
    ) -> Result<Login, String> {
        let auth = match kind {
            LoginKind::SetupToken if !self.device_settings().claude_setup_token => {
                return Err(TOKENS_OFF.to_owned());
            }
            LoginKind::SetupToken => match self.credentials.claude_token(account) {
                Some(token) if !token.is_empty() => LoginAuth::SetupToken(token),
                _ => return Err(NO_TOKEN.to_owned()),
            },
            LoginKind::ModelServer => match self.credentials.model_server(account) {
                Some(server) => LoginAuth::ModelServer(Arc::new(server)),
                None => return Err(NO_MODEL_SERVER.to_owned()),
            },
            _ => LoginAuth::ConfigDir,
        };
        Ok(Login {
            account,
            config_dir,
            auth,
        })
    }

    /// The login a Codex app-server runs under.
    pub(super) fn codex_login(
        &self,
        account: AccountId,
        config_dir: PathBuf,
        kind: LoginKind,
    ) -> Login {
        let auth = match kind {
            LoginKind::ChatGptSignIn => LoginAuth::ChatGptBroker,
            _ => LoginAuth::ConfigDir,
        };
        Login {
            account,
            config_dir,
            auth,
        }
    }

    /// The login a Cursor agent runs under. An API key comes from the
    /// secret store each launch, so a key you replace is the one used. The
    /// CLI's own login is on this device only.
    pub(super) fn cursor_login(
        &self,
        account: AccountId,
        config_dir: PathBuf,
        kind: LoginKind,
        guest: bool,
    ) -> Result<Login, String> {
        let auth = match kind {
            LoginKind::CursorApiKey => match self.credentials.cursor_key(account) {
                Some(key) if !key.is_empty() => LoginAuth::CursorApiKey(key),
                _ => return Err(NO_KEY.to_owned()),
            },
            _ if guest => return Err(NO_GUEST_LOGIN.to_owned()),
            _ => LoginAuth::ConfigDir,
        };
        Ok(Login {
            account,
            config_dir,
            auth,
        })
    }

    /// The login a held agent runs under.
    pub(super) fn login_of(&self, held: &Held) -> Result<Login, String> {
        let config_dir = held.config_dir.clone();
        let guest = held.guest.is_some();
        self.login_for(held.vendor(), held.account, config_dir, held.login, guest)
    }

    /// The login an agent of `vendor` runs under, in a container when
    /// `guest` is set.
    pub(super) fn login_for(
        &self,
        vendor: Vendor,
        account: AccountId,
        config_dir: PathBuf,
        kind: LoginKind,
        guest: bool,
    ) -> Result<Login, String> {
        match vendor {
            // A managed server listens on this device's loopback only.
            Vendor::Claude if guest && kind == LoginKind::ModelServer => {
                Err(NO_GUEST_MODEL.to_owned())
            }
            Vendor::Claude => self.claude_login(account, config_dir, kind),
            Vendor::Codex => Ok(self.codex_login(account, config_dir, kind)),
            Vendor::Cursor => self.cursor_login(account, config_dir, kind, guest),
            Vendor::Acp => self.acp_login(account, config_dir, guest),
        }
    }

    /// The local ACP agent `account` runs, as the device defines it now.
    pub(super) fn acp_agent(&self, account: AccountId) -> Option<AcpAgent> {
        self.hub
            .pick(|entity| match entity {
                Entity::AcpAgent(agent) if acp_agents::account_of(agent) == account => {
                    Some(agent.clone())
                }
                _ => None,
            })
            .pop()
    }

    /// The login a local ACP agent runs under: the account's definition,
    /// as the device keeps it now, and its variables' values from the
    /// secret store. A container runs it only when the definition says so.
    fn acp_login(
        &self,
        account: AccountId,
        config_dir: PathBuf,
        guest: bool,
    ) -> Result<Login, String> {
        let agent = self.acp_agent(account).ok_or(NO_ACP_AGENT)?;
        if guest && !agent.containers {
            return Err(format!(
                "{} doesn't run in containers. Turn that on in its settings to carry on",
                agent.name
            ));
        }
        let env = self
            .credentials
            .acp_env(agent.id)
            .into_iter()
            .filter(|value| agent.env.contains(&value.name))
            .collect();
        let command = local::AcpCommand { agent, env };
        Ok(Login {
            account,
            config_dir,
            auth: LoginAuth::AcpCommand(Arc::new(command)),
        })
    }

    /// Refuses a Codex app-server whose ChatGPT sign-in is gone, or that
    /// would run on it outside a container, where no guest answers its
    /// auth command.
    pub(super) fn check_sign_in(&self, login: &Login, guest: bool) -> Result<(), String> {
        if login.auth != LoginAuth::ChatGptBroker {
            return Ok(());
        }
        if !guest {
            return Err(HOST_SIGN_IN.to_owned());
        }
        if !self.credentials.has_chatgpt(login.account) {
            return Err(NO_SIGN_IN.to_owned());
        }
        Ok(())
    }

    /// Where a held agent runs.
    pub(super) fn place(&self, held: &Held) -> Place {
        self.place_in(
            held.agent.workspace,
            held.guest,
            held.cwd.clone(),
            &held.agent.id.to_string(),
        )
    }

    /// Where an agent runs. A guest's agent gets its attachments in the
    /// guest's own folder, which file delivery copies them into. One in
    /// the dispatcher's folder runs as the dispatcher.
    pub(super) fn place_in(
        &self,
        workspace: WorkspaceId,
        guest: Option<WorkspaceId>,
        cwd: PathBuf,
        name: &str,
    ) -> Place {
        let (attachments, host) = match guest {
            Some(guest) => (
                guest_attachments(name),
                PlaceHost::Guest { workspace: guest },
            ),
            None => (self.attachments.agent_dir(name), PlaceHost::Local),
        };
        Place {
            device: self.device,
            workspace,
            cwd,
            attachments,
            host,
            dispatcher: guest.is_none() && self.dispatcher.home() == Some(workspace),
            trust: None,
        }
    }

    /// The boot ID of the machine an agent runs on: this device's, or its
    /// container's while the guest's link is up. None says it isn't known.
    pub(super) fn boot_at(&self, workspace: Option<WorkspaceId>) -> Option<BootId> {
        match workspace {
            None => Some(self.hub.device().boot_id.clone()),
            Some(workspace) => self
                .links
                .get(&workspace)
                .filter(|link| link.down.is_none())
                .and_then(|link| link.boot.clone()),
        }
    }

    /// Tries a guest's holder again later. Its holder runs on, so a holder
    /// that can't be reached a few times while its link is up counts as
    /// gone. While the link is down, it waits for the link instead.
    pub(super) fn retry_later(
        &mut self,
        session: SessionId,
        reaching: Reaching,
        parked: Vec<(Target, Action)>,
    ) {
        let Some(workspace) = self
            .holds
            .get(&session)
            .and_then(|hold| hold.record.workspace)
        else {
            return;
        };
        let up = self.link_up(workspace);
        let now = self.clock.now();
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        if up {
            hold.retries += 1;
        }
        if hold.retries > MAX_RETRIES {
            warn!(%session, "the holder in the guest can't be reached, so maxd counts it as gone");
            self.refuse_parked(parked, "couldn't reach the agent's process");
            self.lost(session);
            return;
        }
        let mut waiting = parked;
        waiting.append(&mut hold.parked);
        hold.parked = waiting;
        hold.reaching = Some(reaching);
        hold.waiting = Some(Waiting {
            launch: None,
            after: hold.last_seq,
            seed: None,
            end: None,
        });
        hold.retry_at = up.then(|| now + RETRY_AFTER);
        for lead in self.leads_of(session) {
            self.publish(lead);
        }
    }

    /// Reaches the holders whose retry is due.
    pub(super) fn retry_due(&mut self) {
        let now = self.clock.now();
        let due: Vec<SessionId> = self
            .holds
            .iter()
            .filter(|(_, hold)| hold.retry_at.is_some_and(|at| at <= now))
            .map(|(session, _)| *session)
            .collect();
        for session in due {
            self.wake_waiting(session);
        }
    }

    /// Carries on with what waited for a holder's guest: a reach, or the
    /// end of a holder the manager lost.
    fn wake_waiting(&mut self, session: SessionId) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.retry_at = None;
        let Some(waiting) = hold.waiting.take() else {
            return;
        };
        let reaching = hold.reaching.take().unwrap_or(Reaching::Again);
        match waiting.end {
            Some(exit) => {
                let parked = std::mem::take(&mut hold.parked);
                self.refuse_parked(parked, "the agent's process ended");
                self.end_lost(session, exit);
            }
            None => self.reach(
                session,
                reaching,
                waiting.launch,
                waiting.after,
                waiting.seed,
            ),
        }
    }

    /// A guest's link went down or came up, or its workspace was deleted.
    pub(super) fn place_change(&mut self, change: PlaceChange) {
        match change {
            PlaceChange::Down { workspace, reason } => self.link_down(workspace, reason),
            PlaceChange::Up {
                workspace, boot_id, ..
            } => self.link_came_up(workspace, boot_id),
            PlaceChange::Gone { workspace } => self.forget_workspace(workspace),
        }
    }

    /// The guests a holder runs in.
    fn holds_in(&self, workspace: WorkspaceId) -> Vec<SessionId> {
        self.holds
            .iter()
            .filter(|(_, hold)| hold.record.workspace == Some(workspace))
            .map(|(session, _)| *session)
            .collect()
    }

    /// Publishes every lead held in a guest.
    fn publish_in(&self, workspace: WorkspaceId) {
        for held in self.held.values() {
            if held.guest == Some(workspace) {
                self.publish(held.agent.id);
            }
        }
    }

    /// Whether the workspace manager shows a container stopping or stopped,
    /// as it does once you stop it.
    fn stopping(&self, workspace: WorkspaceId) -> bool {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(shown)) => matches!(
                shown.managed.map(|managed| managed.state),
                Some(WorkspaceState::Stopping | WorkspaceState::Stopped)
            ),
            _ => false,
        }
    }

    /// The link to a guest is down. Its agents run on, so each holder there
    /// waits for the link to come back, and its agents show as down with
    /// the link's reason. When you stop the container, the agents at work
    /// there are flagged first, and they show that it stopped and that
    /// nothing reaches them until it starts again.
    fn link_down(&mut self, workspace: WorkspaceId, reason: String) {
        let stopping = self.stopping(workspace);
        if stopping {
            let boot = self
                .links
                .get(&workspace)
                .and_then(|link| link.boot.clone());
            self.flag_stopped(workspace, boot);
        }
        let reason = if stopping { STOPPED.to_owned() } else { reason };
        let now = self.clock.wall();
        let link = self.links.entry(workspace).or_default();
        let since = link.down.take().map_or(now, |(_, since)| since);
        link.down = Some((reason, since));
        link.stopped = stopping;
        for session in self.holds_in(workspace) {
            let Some(hold) = self.holds.get_mut(&session) else {
                continue;
            };
            // The connection went with the link, so anything it still
            // reads is dropped.
            hold.generation += 1;
            hold.writer = None;
            hold.retries = 0;
            hold.retry_at = None;
            hold.handshake_by = None;
            if hold.reaching.is_none() && !hold.ending && hold.waiting.is_none() {
                hold.reaching = Some(Reaching::Again);
                hold.waiting = Some(Waiting {
                    launch: None,
                    after: hold.last_seq,
                    seed: None,
                    end: None,
                });
            }
        }
        self.publish_in(workspace);
    }

    /// The link to a guest is up. With the boot ID it had, each holder
    /// there reconnects from the last line read. With a new one, the
    /// container restarted, so its holders died, and they replay what they
    /// kept before their agents come back as A-11 says.
    fn link_came_up(&mut self, workspace: WorkspaceId, boot: BootId) {
        let link = self.links.entry(workspace).or_default();
        let first = link.boot.is_none();
        link.boot = Some(boot.clone());
        link.down = None;
        link.stopped = false;
        for session in self.holds_in(workspace) {
            let Some(hold) = self.holds.get_mut(&session) else {
                continue;
            };
            match hold.record.boot.clone() {
                None => {
                    hold.record.boot = Some(boot.clone());
                    let record = hold.record.clone();
                    self.store_holder(session, &record);
                }
                Some(old) if old != boot => {
                    hold.restarted = true;
                    if hold.writer.is_some() {
                        // A link that came back without going down first
                        // still means the old connection is gone.
                        hold.generation += 1;
                        hold.writer = None;
                        hold.reaching = Some(Reaching::Again);
                        hold.waiting = Some(Waiting {
                            launch: None,
                            after: hold.last_seq,
                            seed: None,
                            end: None,
                        });
                    }
                }
                Some(_) => {
                    for lead in self.leads_of(session) {
                        self.drop_flag(lead);
                    }
                }
            }
            self.wake_waiting(session);
        }
        if first {
            // The first word from a guest after `maxd` started, so the
            // timelines read back on restore may be behind.
            let leads: Vec<AgentId> = self
                .held
                .values()
                .filter(|held| held.guest == Some(workspace))
                .map(|held| held.agent.id)
                .collect();
            for lead in leads {
                self.reseed(lead);
            }
        }
        self.publish_in(workspace);
    }

    /// A workspace was deleted, and every CLI in it went with its
    /// container. Its agents are forgotten: their holders, their stored
    /// rows and flags, and their timelines once no client has them open.
    /// Clients see them removed, and what waited for them is refused.
    fn forget_workspace(&mut self, workspace: WorkspaceId) {
        let leads: Vec<AgentId> = self
            .held
            .values()
            .filter(|held| held.guest == Some(workspace))
            .map(|held| held.agent.id)
            .collect();

        let mut sessions: HashSet<SessionId> = self.holds_in(workspace).into_iter().collect();
        sessions.extend(
            self.stranded
                .iter()
                .filter(|(_, record)| record.workspace == Some(workspace))
                .map(|(session, _)| *session),
        );
        let mut keys: Vec<IdempotencyKey> = self
            .routed
            .iter()
            .filter(|(_, session)| sessions.contains(session))
            .map(|(key, _)| *key)
            .collect();
        let mut parked = Vec::new();
        for session in &sessions {
            self.stranded.remove(session);
            self.dirty_seq.remove(session);
            if let Some(hold) = self.holds.remove(session) {
                if let Some(Reaching::Start(key) | Reaching::Continue(key)) = hold.reaching {
                    keys.push(key);
                }
                parked.extend(hold.parked);
            }
            if let Err(err) = self.db.remove_holder(*session) {
                warn!(%err, %session, "couldn't forget the holder");
            }
        }
        let servers: Vec<ServerKey> = self
            .servers
            .keys()
            .filter(|key| key.workspace == Some(workspace))
            .copied()
            .collect();
        for key in servers {
            if let Some(server) = self.servers.remove(&key) {
                parked.extend(server.waiting);
            }
        }

        let agents = self.forget_leads(&leads);
        self.drop_unwatched(agents);
        self.strays.retain(|_, agent| agent.workspace != workspace);
        self.links.remove(&workspace);
        // Its record went with it, so there's nowhere to write an empty
        // list.
        self.summaries.remove(&workspace);

        for key in keys {
            self.result(key, refused(DELETED), Written::default());
        }
        self.refuse_parked(parked, DELETED);
        if !leads.is_empty() {
            info!(%workspace, agents = leads.len(), "forgot the agents of a deleted workspace");
        }
    }

    /// Brings what others watch up to date with the held agents: the Mac's
    /// power assertion, who owns each guest session, and the A-10 summary
    /// each container workspace keeps of its agents. It runs after every
    /// input, and only what changed goes out.
    pub(super) fn sync(&mut self) {
        let busy = self.held.values().any(|held| {
            held.guest.is_none()
                && held.paused.is_none()
                && self.session_of(held).is_some()
                && self.working(held.agent.id)
        });
        self.power.set_busy(busy);

        let owners: HashMap<SessionId, SessionOwner> = self
            .holds
            .iter()
            .filter_map(|(session, hold)| {
                let workspace = hold.record.workspace?;
                Some((
                    *session,
                    SessionOwner {
                        workspace,
                        account: hold.record.account,
                        vendor: hold.record.vendor,
                    },
                ))
            })
            .collect();
        self.owners.set(owners);
        self.sync_ports();

        let mut summaries: HashMap<WorkspaceId, Vec<AgentSummary>> = HashMap::new();
        for held in self.held.values() {
            let Some(workspace) = held.guest else {
                continue;
            };
            summaries.entry(workspace).or_default().push(AgentSummary {
                agent: held.agent.id,
                vendor: held.vendor(),
                account: held.account,
                session: held.vendor_session.clone(),
                branch: held.branch.clone().unwrap_or_default(),
                worktree: held.cwd.display().to_string(),
            });
        }
        for list in summaries.values_mut() {
            list.sort_by_key(|summary| summary.agent.to_string());
        }
        let emptied: Vec<WorkspaceId> = self
            .summaries
            .keys()
            .filter(|workspace| !summaries.contains_key(workspace))
            .copied()
            .collect();
        for workspace in emptied {
            summaries.insert(workspace, Vec::new());
        }
        for (workspace, list) in summaries {
            if self.summaries.get(&workspace) == Some(&list) {
                continue;
            }
            if list.is_empty() {
                self.summaries.remove(&workspace);
            } else {
                self.summaries.insert(workspace, list.clone());
            }
            let workspaces = self.workspaces.clone();
            tokio::spawn(async move {
                if let Err(err) = workspaces.record_agents(workspace, list).await {
                    warn!(%err, %workspace, "couldn't record the workspace's agents");
                }
            });
        }
    }

    /// Tells the port watch which holders run, and where, and whether a
    /// client is connected. A holder counts in the workspace of its lead
    /// with the lowest ID, which matters only for a Codex app-server on
    /// this device with agents in several folders.
    fn sync_ports(&self) {
        let holders = self
            .holds
            .keys()
            .filter_map(|session| self.holding(*session))
            .collect();
        self.ports.set(Holdings {
            client: !self.clients.is_empty(),
            holders,
        });
    }

    /// The holder at `session` as the port watch sees it, once it runs.
    pub(super) fn holding(&self, session: SessionId) -> Option<Holding> {
        let hold = self.holds.get(&session)?;
        let pid = hold.holder_pid?;
        let leads = self.leads_of(session);
        let lead = leads.iter().min().and_then(|lead| self.held.get(lead))?;
        Some(Holding {
            workspace: hold.record.workspace.unwrap_or(lead.agent.workspace),
            guest: hold.record.workspace.is_some(),
            holder: WatchedHolder {
                session,
                agent: match hold.record.vendor {
                    vendor if one_process_per_agent(vendor) => hold.record.agent,
                    _ => None,
                },
                pid,
            },
        })
    }
}

/// Fits a launch to its login and to where it runs. A setup-token goes in
/// as a secret variable only, never in the plain ones a record names. A
/// guest's CLI runs as `agent` from the recipe's path, with its own
/// folders.
pub(super) fn fit_launch(mut launch: Launch, vendor: Vendor, login: &Login, guest: bool) -> Launch {
    if let LoginAuth::SetupToken(token) = &login.auth {
        // The C-5 scrub leaves the token out for this launch only.
        launch.env_remove.retain(|name| name != TOKEN_VAR);
        launch.env.retain(|(name, _)| name != TOKEN_VAR);
        launch.secret_env.retain(|(name, _)| name != TOKEN_VAR);
        launch
            .secret_env
            .push((OsString::from(TOKEN_VAR), token.clone()));
    }
    if !guest {
        return launch;
    }
    // Cursor has no variable for its folder, and keeps it in `agent`'s
    // home. A local ACP command keeps its own program and has no folder.
    let (program, folder, extra) = match vendor {
        Vendor::Claude => (
            Some(recipe::CLAUDE),
            Some("CLAUDE_CONFIG_DIR"),
            // The recipe installs the CLI, and an update comes with the
            // recipe, not mid-session.
            Some(("DISABLE_AUTOUPDATER", "1")),
        ),
        Vendor::Codex => (Some(recipe::CODEX), Some("CODEX_HOME"), None),
        Vendor::Cursor => (Some(recipe::CURSOR), None, None),
        Vendor::Acp => (None, None, None),
    };
    match (vendor, program) {
        // The container finds the program on its own `PATH`.
        (Vendor::Acp, _) => {
            if let LoginAuth::AcpCommand(command) = &login.auth {
                launch.program = PathBuf::from(&command.agent.program);
            }
        }
        (_, None) => {}
        (Vendor::Codex, Some(program)) => codex::set_program(&mut launch, PathBuf::from(program)),
        (_, Some(program)) => launch.program = PathBuf::from(program),
    }
    launch.env.retain(|(name, _)| {
        name != "PATH" && name != "HOME" && folder.is_none_or(|folder| name != folder)
    });
    launch.env.push(("PATH".into(), guest_path().into()));
    if let Some(folder) = folder {
        launch
            .env
            .push((folder.into(), login.config_dir.clone().into_os_string()));
    }
    if let Some((name, value)) = extra {
        launch.env.push((name.into(), value.into()));
    }
    if vendor == Vendor::Codex {
        launch.cwd = login.config_dir.clone();
    }
    launch.run_as = Some(RunAs {
        user: recipe::AGENT.to_owned(),
        uid: recipe::AGENT_UID,
        gid: recipe::AGENT_UID,
        home: PathBuf::from(recipe::AGENT_HOME),
    });
    launch
}
