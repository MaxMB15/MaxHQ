// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What clients ask for: starting agents, acting on them, answering their
//! prompts, reading their timelines, staging attachments and changing the
//! device settings. Each keyed request runs once, and a repeat gets the
//! first outcome.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use maxhq_core::{context, names, pill_text};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::guest::Worktree;
use maxhq_protocol::ids::{
    AccountId, AgentId, AttachmentId, AttentionId, ClientId, DeviceId, IdempotencyKey, WorkspaceId,
};
use maxhq_protocol::leases::MAX_VMS;
use maxhq_protocol::message::{
    ActionOutcome, AgentPlace, AgentSpec, AttachmentSource, ClientMessage, SendMode, ServerMessage,
};
use maxhq_protocol::model::{
    Activity, Agent, DeviceSettingChange, DeviceSettings, LoginKind, PauseReason, PermissionMode,
    Vendor, WorkspaceLocation,
};
use maxhq_protocol::state::{Change, Entity, EntityKey, Seq};
use maxhq_protocol::timeline::{
    AttachmentRef, Delivery, ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineUpdate,
};
use uuid::Uuid;

use super::held::{
    internal_key, is_busy, After, Held, Internal, Pending, Reaching, Server, ServerKey, StartInfo,
    Target,
};
use super::idem::{Outcome, Seen};
use super::loading::Later;
use super::places;
use super::reads::{ReadKind, Reader, Source, Subagent};
use super::revive::{AgentSpawn, Cli};
use super::timeline::MAX_PAGE;
use super::vendors::{full_access_relaunches, speaks_acp, takes_effort};
use super::{is_queued, refused, Input, Manager};
use crate::drive::{acp, Action, Message, Place, PlaceHost, Staged};
use crate::hub::{Command, Hub};
use crate::recipe;
use crate::secrets;
use crate::transfer::disk;

/// What a client is told when it acts on an agent MaxHQ doesn't hold.
const OUTSIDE: &str = "this agent runs outside MaxHQ. Continue it in MaxHQ first";

/// What a client is told when it deletes a sub-agent, which goes only with
/// its lead.
const SUB_AGENT: &str = "this is a sub-agent. Delete the agent that started it";

/// What a client is told when it sends to an agent whose CLI is stopping.
pub(super) const STOPPING: &str = "the agent is still stopping. Send again once it has stopped";

/// What a client is told when a session is still running elsewhere.
pub(super) const ELSEWHERE: &str =
    "the session is still running in another terminal. Close it there first";

/// Why a Cursor session started outside MaxHQ can't move into it.
const OUTSIDE_CURSOR: &str = "MaxHQ can't take over a Cursor session started outside it";

/// Why a local ACP agent's session started outside MaxHQ can't move into
/// it. No observer finds one, so it's never asked.
const OUTSIDE_ACP: &str = "MaxHQ can't take over an ACP agent's session started outside it";

/// Why a Cursor agent can't turn Full access on or off right now.
const SANDBOX_MID_TURN: &str =
    "Cursor sets Full access when it starts, so change it once the turn ends";

/// The longest title taken from a first message.
const TITLE_CHARS: usize = 80;

/// Why an agent can't start in a folder a clone is still filling.
const STILL_CLONING: &str = "this folder is still being cloned";

/// How many sub-agents deep a lead's session is looked for.
const MAX_NESTING: usize = 16;

/// What a client is told when it acts on an agent whose container can't be
/// reached.
pub(super) const UNREACHABLE: &str =
    "the agent's container isn't reachable right now. Try again once it's back";

/// A new agent in a container, waiting for its worktree and for its first
/// message's attachments to reach the guest.
pub(super) struct GuestStart {
    spec: AgentSpec,
    info: StartInfo,
    /// The Claude session it runs, unused for Codex.
    uuid: Uuid,
    /// Where it runs. Its folder is the worktree's once that's made.
    place: Place,
}

/// One request to act on a held agent.
enum Ask {
    /// Send a message.
    Send {
        text: String,
        refs: Vec<InlineRef>,
        attachments: Vec<AttachmentId>,
        mode: SendMode,
    },
    /// Take back a queued message.
    Cancel(ItemId),
    /// Stop the current turn.
    Interrupt,
    /// Change the permission mode.
    Mode(PermissionMode),
    /// Change the model.
    Model(String),
    /// Change the effort level. None is the model's default.
    Effort(Option<String>),
    /// Stop the agent and keep it from coming back.
    Stop,
    /// Stop the agent if it runs, then forget it for good.
    Delete,
}

impl Manager {
    /// Handles one message from a client.
    pub(super) fn message(&mut self, conn: u64, message: ClientMessage) {
        let key = match &message {
            ClientMessage::Answer {
                idempotency_key,
                event,
                answer,
                refs,
            } => {
                self.answer(conn, *idempotency_key, *event, answer.clone(), refs);
                return;
            }
            ClientMessage::OpenTimeline {
                agent,
                since,
                limit,
                lead,
            } => {
                self.open_timeline(conn, *agent, *since, *limit, *lead);
                return;
            }
            ClientMessage::LoadOlder {
                agent,
                before,
                limit,
            } => {
                self.load_older(conn, *agent, *before, *limit);
                return;
            }
            ClientMessage::CloseTimeline { agent } => {
                self.close_timeline(conn, *agent);
                return;
            }
            ClientMessage::FindPaths { key, agent, query } => {
                self.find_paths(conn, *key, *agent, query.clone());
                return;
            }
            ClientMessage::ListProcesses { key, agent } => {
                self.list_processes(conn, *key, *agent);
                return;
            }
            ClientMessage::SignalProcess {
                key,
                agent,
                pid,
                start,
                signal,
            } => {
                self.signal_process(conn, *key, *agent, (*pid, *start), *signal);
                return;
            }
            ClientMessage::FindGitItems {
                key,
                agent,
                query,
                repo: None,
            } => {
                self.find_git_items(conn, *key, *agent, query.clone());
                return;
            }
            ClientMessage::FindGitItems {
                key,
                agent,
                query,
                repo: Some(repo),
            } => {
                self.find_git_items_in(conn, *key, *agent, repo.clone(), query.clone());
                return;
            }
            ClientMessage::ExcerptAgent { key, agent, budget } => {
                self.excerpt_agent(conn, *key, *agent, *budget);
                return;
            }
            ClientMessage::FindInTimeline {
                key,
                agent,
                query,
                at,
            } => {
                self.find_in_timeline(conn, *key, *agent, query, at.clone());
                return;
            }
            ClientMessage::AuthKey { .. }
            | ClientMessage::CallTool { .. }
            | ClientMessage::GrantPermission { .. }
            | ClientMessage::RevokePermission { .. }
            | ClientMessage::ListPermissions { .. }
            | ClientMessage::MintKey { .. }
            | ClientMessage::ListKeys { .. } => return self.agent_message(conn, message),
            ClientMessage::SaveTask { .. }
            | ClientMessage::SetTaskEnabled { .. }
            | ClientMessage::DeleteTask { .. }
            | ClientMessage::RunTaskNow { .. }
            | ClientMessage::ListRuns { .. } => return self.task_message(conn, message),
            ClientMessage::AskDispatcher { .. }
            | ClientMessage::ConfirmPlan { .. }
            | ClientMessage::DismissPlan { .. }
            | ClientMessage::UndoDispatcherAction { .. } => {
                return self.dispatcher_message(conn, message)
            }
            ClientMessage::MoveAgent { .. } => return self.move_message(conn, message),
            ClientMessage::StartAgent { key, .. }
            | ClientMessage::SendInput { key, .. }
            | ClientMessage::CancelQueued { key, .. }
            | ClientMessage::Interrupt { key, .. }
            | ClientMessage::SetPermission { key, .. }
            | ClientMessage::SetModel { key, .. }
            | ClientMessage::SetEffort { key, .. }
            | ClientMessage::StopAgent { key, .. }
            | ClientMessage::DeleteAgent { key, .. }
            | ClientMessage::RenameAgent { key, .. }
            | ClientMessage::SetInbox { key, .. }
            | ClientMessage::SetProjectHidden { key, .. }
            | ClientMessage::RemoveCommandRule { key, .. }
            | ClientMessage::ContinueHere { key, .. }
            | ClientMessage::TakeOver { key, .. }
            | ClientMessage::StageAttachment { key, .. }
            | ClientMessage::SetDeviceSettings { key, .. }
            | ClientMessage::UpdateDeviceSettings { key, .. }
            | ClientMessage::WindDown { key, .. }
            | ClientMessage::PullsDue { key, .. }
            | ClientMessage::LinkPull { key, .. }
            | ClientMessage::UnlinkPull { key, .. } => *key,
            // The connection handles the rest itself.
            ClientMessage::Hello(_)
            | ClientMessage::Sync { .. }
            | ClientMessage::MarkSeen { .. }
            | ClientMessage::RefreshUsage { .. }
            | ClientMessage::Ping { .. }
            | ClientMessage::CreateWorkspace { .. }
            | ClientMessage::StartWorkspace { .. }
            | ClientMessage::StopWorkspace { .. }
            | ClientMessage::DeleteWorkspace { .. }
            | ClientMessage::SetProjectSettings { .. }
            | ClientMessage::CloneFolder { .. }
            | ClientMessage::CloneCancel { .. }
            | ClientMessage::CloneRetry { .. }
            | ClientMessage::CloneRemove { .. }
            | ClientMessage::StoreClaudeToken { .. }
            | ClientMessage::StoreCursorKey { .. }
            | ClientMessage::BeginChatGptSignIn { .. }
            | ClientMessage::FinishChatGptSignIn { .. }
            | ClientMessage::SetGithubApp { .. }
            | ClientMessage::CheckGithubApp { .. }
            | ClientMessage::ForgetCredential { .. }
            | ClientMessage::UploadBegin { .. }
            | ClientMessage::UploadCancel { .. }
            | ClientMessage::FetchBegin { .. }
            | ClientMessage::ListIgnoredFiles { .. }
            | ClientMessage::FetchFiles { .. }
            | ClientMessage::FetchSkillTree { .. }
            | ClientMessage::PlaceStaged { .. }
            | ClientMessage::TransferCredit { .. }
            | ClientMessage::OpenForward { .. }
            | ClientMessage::CloseForward { .. }
            | ClientMessage::TunnelOpen { .. }
            | ClientMessage::TunnelCredit { .. }
            | ClientMessage::TunnelClose { .. }
            | ClientMessage::GuestSshOpen { .. }
            | ClientMessage::SetGuestSshKey { .. }
            | ClientMessage::InstallUpdate { .. }
            | ClientMessage::UpdateTools { .. }
            | ClientMessage::ReadLoad { .. }
            | ClientMessage::ReadDiagnostics { .. }
            | ClientMessage::SetHookOn { .. }
            | ClientMessage::RotateHookToken { .. }
            | ClientMessage::SetHookSecret { .. }
            | ClientMessage::SetHookAddress { .. }
            | ClientMessage::ListDeliveries { .. }
            | ClientMessage::SetAcpAgent { .. }
            | ClientMessage::DeleteAcpAgent { .. }
            | ClientMessage::Peer { .. }
            | ClientMessage::PeerRequest { .. }
            | ClientMessage::PeerAnswer { .. }
            | ClientMessage::PeerKey { .. }
            | ClientMessage::AuthorizePeer { .. }
            | ClientMessage::RevokePeer { .. }
            | ClientMessage::SetPeerBook { .. }
            | ClientMessage::BeginGithubSignIn { .. }
            | ClientMessage::CancelGithubSignIn { .. }
            | ClientMessage::CheckGithubSignIn { .. }
            | ClientMessage::BeginCliLogin { .. }
            | ClientMessage::FinishCliLogin { .. }
            | ClientMessage::CheckCliLogin { .. }
            | ClientMessage::CancelCliLogin { .. }
            | ClientMessage::ListPulls { .. }
            | ClientMessage::ListBranches { .. }
            | ClientMessage::GetPull { .. }
            | ClientMessage::PullFiles { .. }
            | ClientMessage::PullComments { .. }
            | ClientMessage::PullChecks { .. }
            | ClientMessage::PullAction { .. }
            | ClientMessage::ReadNote { .. }
            | ClientMessage::SaveNote { .. }
            | ClientMessage::NoteHistory { .. }
            | ClientMessage::NoteHashes { .. }
            | ClientMessage::NoteVersions { .. }
            | ClientMessage::PutNoteVersions { .. }
            | ClientMessage::CheckTestHosts { .. }
            | ClientMessage::BuildTestHost { .. }
            | ClientMessage::ResetTestHost { .. }
            | ClientMessage::EndLease { .. }
            | ClientMessage::ReadLeaseScreen { .. }
            | ClientMessage::InstallTool { .. }
            | ClientMessage::RemoveTool { .. }
            | ClientMessage::CheckHost { .. }
            | ClientMessage::ApplyHost { .. }
            | ClientMessage::ListSnapshots { .. }
            | ClientMessage::RestoreSnapshot { .. }
            | ClientMessage::PlanMove { .. }
            | ClientMessage::RunMoveStep { .. }
            | ClientMessage::MoveBack { .. }
            | ClientMessage::RetireMove { .. }
            | ClientMessage::InstallAcpAgent { .. }
            | ClientMessage::AcceptLicence { .. }
            | ClientMessage::OpenWorkflow { .. }
            | ClientMessage::CloseWorkflow { .. }
            | ClientMessage::NodeDetail { .. }
            | ClientMessage::SkillSourceAdd { .. }
            | ClientMessage::SkillSourceFetch { .. }
            | ClientMessage::SkillSourceRemove { .. }
            | ClientMessage::SkillSourcePush { .. }
            | ClientMessage::SkillsTakeLibrary { .. }
            | ClientMessage::SkillsSetProject { .. }
            | ClientMessage::SkillsApply { .. }
            | ClientMessage::SkillsScan { .. }
            | ClientMessage::SkillsTreesNeeded { .. }
            | ClientMessage::SkillsSetAlways { .. }
            | ClientMessage::SkillProposalAction { .. }
            | ClientMessage::SkillProposalDiff { .. }
            | ClientMessage::SkillsGlobalsPlan { .. }
            | ClientMessage::SkillsGlobalsApply { .. }
            | ClientMessage::SkillsGlobalsUndo { .. }
            | ClientMessage::Phone { .. }
            | ClientMessage::Pair { .. }
            | ClientMessage::BeginPhonePair { .. }
            | ClientMessage::PairPhone { .. }
            | ClientMessage::AddPhone { .. }
            | ClientMessage::RemovePhone { .. }
            | ClientMessage::RevokePhoneSession { .. }
            | ClientMessage::SetPhoneBook { .. }
            | ClientMessage::SetPhoneEverything { .. }
            | ClientMessage::SetPushToken { .. }
            | ClientMessage::Presence { .. }
            | ClientMessage::SetApnsKey { .. }
            | ClientMessage::Prove { .. }
            | ClientMessage::EnrollAppKey { .. }
            | ClientMessage::RemoveAppKey { .. }
            | ClientMessage::AnswerAdmission { .. }
            | ClientMessage::TerminalOpen { .. }
            | ClientMessage::TerminalAttach { .. }
            | ClientMessage::TerminalDetach { .. }
            | ClientMessage::TerminalResize { .. }
            | ClientMessage::TerminalInterrupt { .. }
            | ClientMessage::TerminalClose { .. }
            | ClientMessage::TerminalRename { .. }
            | ClientMessage::TerminalMove { .. }
            | ClientMessage::ListFolder { .. }
            | ClientMessage::MakeFolder { .. }
            | ClientMessage::Notices { .. }
            | ClientMessage::SessionRun { .. }
            | ClientMessage::SessionShare { .. }
            | ClientMessage::SessionCd { .. }
            | ClientMessage::SessionEnd { .. } => return,
        };
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        // A message a person sent seals its agent's session cards once it
        // goes; one from an agent's tools doesn't.
        if matches!(message, ClientMessage::SendInput { .. })
            && self.clients.get(&conn).is_some_and(|viewer| viewer.person)
        {
            self.yours.insert(key);
        }
        if let Some(outcome) = self.action(key, message) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Carries out a keyed request. Returns its outcome, or none when it
    /// comes later.
    pub(super) fn action(
        &mut self,
        key: IdempotencyKey,
        message: ClientMessage,
    ) -> Option<ActionOutcome> {
        let message = self.after_loading(key, message)?;
        match message {
            ClientMessage::StartAgent { spec, .. } => self.start_agent(key, spec),
            ClientMessage::TerminalOpen {
                agent,
                kind,
                cols,
                rows,
                ..
            } => self.open_terminal(key, agent, kind, cols, rows),
            ClientMessage::SessionRun {
                agent,
                command,
                private,
                fresh,
                cols,
                rows,
                ..
            } => self.session_run(key, agent, (command, private, fresh), (cols, rows)),
            ClientMessage::SessionCd { agent, path, .. } => self.session_cd(key, agent, path),
            ClientMessage::SessionShare {
                agent,
                item,
                block,
                private,
                ..
            } => Some(self.share_block(agent, &item, block, private)),
            ClientMessage::SendInput {
                agent, input, mode, ..
            } => {
                if let Err(reason) = context::check(&input.text, &input.refs, &input.attachments) {
                    return Some(refused(reason));
                }
                let ask = Ask::Send {
                    text: input.text,
                    refs: input.refs,
                    attachments: input.attachments,
                    mode,
                };
                self.route(key, agent, ask)
            }
            ClientMessage::CancelQueued { agent, item, .. } => {
                self.route(key, agent, Ask::Cancel(item))
            }
            ClientMessage::Interrupt { agent, .. } => self.route(key, agent, Ask::Interrupt),
            ClientMessage::SetPermission { agent, .. } if self.is_dispatcher(agent) => {
                Some(refused("The dispatcher's mode stays as it is."))
            }
            ClientMessage::SetPermission { agent, mode, .. } => {
                self.route(key, agent, Ask::Mode(mode))
            }
            ClientMessage::SetModel {
                agent,
                model,
                account: Some(account),
                ..
            } if self.moves(agent, account) => Some(self.switch_vendor(agent, account, model)),
            ClientMessage::SetModel { agent, model, .. } => {
                self.route(key, agent, Ask::Model(model))
            }
            ClientMessage::SetEffort { agent, effort, .. } => {
                self.route(key, agent, Ask::Effort(effort))
            }
            ClientMessage::StopAgent { agent, .. } => self.route(key, agent, Ask::Stop),
            // A resend, under any key, finds the agent already deleted.
            ClientMessage::DeleteAgent { agent, .. } if self.hub.is_deleted(agent) => {
                Some(ActionOutcome::Done)
            }
            ClientMessage::DeleteAgent { agent, .. } => self.route(key, agent, Ask::Delete),
            ClientMessage::RenameAgent { agent, title, .. } => Some(self.rename(agent, &title)),
            ClientMessage::SetInbox { agent, change, .. } => Some(self.set_inbox(agent, &change)),
            ClientMessage::SetProjectHidden {
                device,
                project,
                hidden,
                ..
            } => Some(self.set_project_hidden(device, project, hidden)),
            ClientMessage::RemoveCommandRule {
                device,
                project,
                rule,
                ..
            } => Some(self.remove_command_rule(device, project, &rule)),
            ClientMessage::ContinueHere { agent, .. } => self.continue_here(key, agent),
            ClientMessage::TakeOver { agent, mode, .. } => self.take_over(key, agent, mode),
            ClientMessage::StageAttachment { source, .. } => {
                self.stage(key, source);
                None
            }
            ClientMessage::SetDeviceSettings { settings, .. } => Some(self.settings(settings)),
            ClientMessage::UpdateDeviceSettings {
                device, changes, ..
            } => Some(self.update_settings(device, changes)),
            ClientMessage::WindDown { window_secs, .. } => self.wind_down(key, window_secs),
            ClientMessage::PullsDue { repo, .. } => Some(self.pulls_due(&repo)),
            ClientMessage::LinkPull { agent, number, .. } => Some(self.link_by_you(agent, number)),
            ClientMessage::UnlinkPull {
                agent,
                repo,
                number,
                ..
            } => Some(self.unlink_by_you(agent, &repo, number)),
            _ => None,
        }
    }

    /// Answers an attention event. The first answer wins, and every later
    /// one hears who gave it. The agent gets the pills in what you typed as
    /// it would in a message.
    fn answer(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        refs: &[InlineRef],
    ) {
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        let by = self
            .clients
            .get(&conn)
            .map_or_else(|| ClientId::derive("unknown"), |viewer| viewer.by);
        let outcome = if let Some(row) = self.answers.get(&event) {
            if row.key == key {
                AnswerOutcome::Accepted
            } else {
                AnswerOutcome::AlreadyAnswered {
                    answered_at: row.at,
                    answered_by: row.by,
                }
            }
        } else if let Some(open) = self.attention.get(&event) {
            let agent = open.agent;
            if open.expires_at.is_some_and(|at| at <= self.clock.wall()) {
                AnswerOutcome::Expired
            } else if self.is_folder_card(event) {
                self.answer_folder_card(conn, key, event, &answer, by)
            } else if self.is_consent_card(event) {
                self.answer_consent(conn, key, event, &answer, by)
            } else if self.is_env_card(event) {
                self.answer_env_card(conn, key, event, &answer, by)
            } else if self.is_task_card(event) {
                self.answer_task_card(conn, key, event, &answer, by)
            } else if self.is_move_card(event) {
                self.answer_move_card(conn, key, event, &answer, by)
            } else if open.ours {
                // MaxHQ's own events only tell you something, so answering
                // one dismisses it.
                self.record_answer(event, key, by);
                self.close_attention(event);
                AnswerOutcome::Accepted
            } else {
                let mut refs = refs.to_vec();
                self.keep_shared(&mut refs);
                // An answer that took a session pill along seals its card.
                if self.clients.get(&conn).is_some_and(|viewer| viewer.person) {
                    self.seal_cards(self.lead_of(agent), &refs, false);
                }
                // A plan can hand on the pills you answered the dispatcher with.
                if self.is_dispatcher(agent) {
                    self.dispatcher.carried.keep(&refs);
                }
                self.answering.insert(key, (event, by));
                self.work.push_back((
                    Target::Agent(agent),
                    Action::Answer {
                        key,
                        event,
                        answer: pill_text::answer(answer, &refs, agent),
                        by,
                    },
                ));
                return;
            }
        } else {
            AnswerOutcome::UnknownEvent
        };
        self.finish(key, Outcome::Answer(outcome));
    }

    /// Starts a new agent. A Claude agent exists once its holder is up. A
    /// Codex agent exists once the app-server makes its thread. A folder is
    /// looked up off the loop first, since that reads its checkout.
    fn start_agent(&mut self, key: IdempotencyKey, mut spec: AgentSpec) -> Option<ActionOutcome> {
        if let Some(first) = &mut spec.first {
            self.keep_shared(&mut first.refs);
        }
        if let Err(reason) = self.account_dir(&spec) {
            return Some(refused(reason));
        }
        if let Some(first) = &spec.first
            && let Err(reason) = context::check(&first.text, &first.refs, &first.attachments)
        {
            return Some(refused(reason));
        }
        let folder = match &spec.place {
            AgentPlace::Folder { path } => Some(PathBuf::from(path)),
            AgentPlace::Workspace { .. } => None,
        };
        let Some(path) = folder else {
            return self.located(key, spec, None);
        };
        let hub = self.hub.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let folder = usable_folder(&hub, &path).map(|()| {
                let workspace = hub.locate_folder(&path);
                (path, workspace)
            });
            let _ = tx.send(Input::Located {
                key,
                spec: Box::new(spec),
                folder,
            });
        });
        None
    }

    /// Starts a new agent once the folder it named, if any, is looked up.
    pub(super) fn located(
        &mut self,
        key: IdempotencyKey,
        spec: AgentSpec,
        folder: Option<Result<(PathBuf, WorkspaceId), String>>,
    ) -> Option<ActionOutcome> {
        let folder = match folder.transpose() {
            Ok(folder) => folder,
            Err(reason) => return Some(refused(reason)),
        };
        let (info, found) = match self.prepare(&spec, folder) {
            Ok(prepared) => prepared,
            Err(reason) => return Some(refused(reason)),
        };
        if let Some(origin) = &spec.origin
            && let Err(reason) = self.still_allowed(origin, &spec, info.workspace)
        {
            return Some(refused(reason));
        }
        let uuid = Uuid::now_v7();
        // A Codex agent's ID isn't known until its thread exists, so its
        // attachments wait in a folder named for the request.
        let name = match spec.vendor {
            Vendor::Claude => names::agent(Vendor::Claude, &uuid.to_string()).to_string(),
            Vendor::Codex => format!("start-{key}"),
            Vendor::Cursor => names::agent(Vendor::Cursor, &uuid.to_string()).to_string(),
            Vendor::Acp => names::agent(Vendor::Acp, &uuid.to_string()).to_string(),
        };
        let place = self.place_in(info.workspace, info.guest, info.cwd.clone(), &name);
        let start = GuestStart {
            spec,
            info,
            uuid,
            place,
        };
        if let Some(guest) = start.info.guest {
            self.prepare_guest(key, start, guest);
            return None;
        }
        let GuestStart {
            mut spec,
            info,
            uuid,
            place,
        } = start;
        let first = match spec.first.take() {
            Some(first) => match self.move_attachments(&found, &place.attachments) {
                Ok(staged) => Some(Message {
                    text: first.text,
                    refs: first.refs,
                    attachments: staged,
                }),
                Err(reason) => return Some(refused(reason)),
            },
            None => None,
        };
        self.launch(key, spec, info, uuid, place, first)
    }

    /// Starts a new agent's CLI, with its first message ready.
    fn launch(
        &mut self,
        key: IdempotencyKey,
        spec: AgentSpec,
        info: StartInfo,
        uuid: Uuid,
        place: Place,
        first: Option<Message>,
    ) -> Option<ActionOutcome> {
        // An agent starts even when its skills couldn't be put in.
        if let Err(err) = crate::skills::deliver::ensure(&place) {
            tracing::warn!(%err, cwd = %place.cwd.display(), "couldn't put the project's skills in");
        }
        match spec.vendor {
            Vendor::Codex => self.start_codex(key, spec, info, place, first),
            vendor => self.start_cli(key, vendor, info, uuid, place, first),
        }
    }

    /// Has a container's guest make a new agent's worktree, on a branch of
    /// its own, then delivers its first message's attachments there. The
    /// agent starts once both are done.
    fn prepare_guest(&mut self, key: IdempotencyKey, mut start: GuestStart, guest: WorkspaceId) {
        let agent = match start.spec.vendor {
            Vendor::Claude => names::agent(Vendor::Claude, &start.uuid.to_string()),
            Vendor::Codex => AgentId::derive(&format!("start:{key}")),
            Vendor::Cursor => names::agent(Vendor::Cursor, &start.uuid.to_string()),
            Vendor::Acp => names::agent(Vendor::Acp, &start.uuid.to_string()),
        };
        let branch = start
            .spec
            .branch
            .clone()
            .unwrap_or_else(|| default_branch(agent));
        let ids = start
            .spec
            .first
            .as_ref()
            .map(|first| first.attachments.clone())
            .unwrap_or_default();
        let homes = crate::skills::deliver::guest_homes(start.spec.vendor, &start.info.config_dir);
        let places = self.places.clone();
        let files = self.files.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = async {
                let worktree = places
                    .worktree(guest, agent, branch, None)
                    .await
                    .map_err(|err| format!("couldn't make the agent's worktree: {err}"))?;
                start.place.cwd = PathBuf::from(&worktree.path);
                let linked =
                    crate::skills::deliver::ensure_guest(guest, worktree.path.clone(), homes).await;
                if let Err(err) = linked {
                    tracing::warn!(%err, path = worktree.path, "couldn't link the worktree's skills");
                }
                let staged = if ids.is_empty() {
                    Vec::new()
                } else {
                    files
                        .deliver(&start.place, ids)
                        .await
                        .map_err(|err| err.to_string())?
                };
                Ok((worktree, staged))
            }
            .await;
            let _ = tx.send(Input::Prepared {
                key,
                start: Box::new(start),
                result,
            });
        });
    }

    /// Starts a new agent in a container once its worktree is made and its
    /// attachments are in.
    pub(super) fn prepared(
        &mut self,
        key: IdempotencyKey,
        start: GuestStart,
        result: Result<(Worktree, Vec<Staged>), String>,
    ) -> Option<ActionOutcome> {
        let GuestStart {
            mut spec,
            mut info,
            uuid,
            place,
        } = start;
        let (worktree, staged) = match result {
            Ok(done) => done,
            Err(reason) => return Some(refused(reason)),
        };
        for attachment in &staged {
            self.staged.remove(&attachment.attachment.id);
        }
        info.cwd.clone_from(&place.cwd);
        info.branch = Some(worktree.branch);
        let first = spec.first.take().map(|first| Message {
            text: first.text,
            refs: first.refs,
            attachments: staged,
        });
        self.launch(key, spec, info, uuid, place, first)
    }

    /// The config folder of the account a new agent asks for, if it can
    /// run there, and how the account signs in.
    fn account_dir(&self, spec: &AgentSpec) -> Result<(PathBuf, LoginKind), String> {
        let Some(Entity::Account(account)) = self.hub.get(&EntityKey::Account(spec.account)) else {
            return Err("that account isn't on this device".into());
        };
        if account.device != self.device {
            return Err("that account is on another device".into());
        }
        if account.vendor != spec.vendor {
            return Err("that account is for another vendor".into());
        }
        Ok((PathBuf::from(&account.config_dir), account.login))
    }

    /// Refuses a container workspace that can't take a new agent now.
    pub(super) fn check_container(&self, workspace: WorkspaceId) -> Result<(), String> {
        let Some(container) = self.workspaces.container(workspace) else {
            return Err("MaxHQ doesn't manage that workspace's container".into());
        };
        if let Some(reason) = container.blocked {
            return Err(reason);
        }
        if !self.link_up(workspace) {
            return Err(
                "the workspace's container isn't reachable right now. Start it, or \
                        wait for it to come back"
                    .into(),
            );
        }
        Ok(())
    }

    /// Checks a new agent's spec, and works out where and how it runs and
    /// which staged attachments its first message takes. `folder` is where
    /// a folder place was found.
    pub(super) fn prepare(
        &self,
        spec: &AgentSpec,
        folder: Option<(PathBuf, WorkspaceId)>,
    ) -> Result<(StartInfo, Vec<AttachmentRef>), String> {
        let (config_dir, login) = self.account_dir(spec)?;
        let (cwd, workspace, guest) = match (&spec.place, folder) {
            (AgentPlace::Workspace { workspace }, _) => {
                let Some(Entity::Workspace(found)) =
                    self.hub.get(&EntityKey::Workspace(*workspace))
                else {
                    return Err("that workspace doesn't exist".into());
                };
                if found.device != self.device {
                    return Err("that workspace is on another device".into());
                }
                if found.clone.is_some() {
                    return Err(STILL_CLONING.into());
                }
                match found.location {
                    WorkspaceLocation::Folder { path } => (PathBuf::from(path), *workspace, None),
                    WorkspaceLocation::Container { .. } => {
                        self.check_container(*workspace)?;
                        // The agent's worktree takes its place once it's made.
                        (PathBuf::from(recipe::WORK), *workspace, Some(*workspace))
                    }
                }
            }
            (AgentPlace::Folder { .. }, Some((cwd, workspace))) => (cwd, workspace, None),
            (AgentPlace::Folder { .. }, None) => return Err("that folder doesn't exist".into()),
        };
        // In a container, the account has a folder of its own in `agent`'s
        // home.
        let config_dir = match guest {
            Some(_) => guest_folder(spec.vendor, spec.account),
            None => config_dir,
        };
        let checked = self.login_for(
            spec.vendor,
            spec.account,
            config_dir.clone(),
            login,
            guest.is_some(),
        )?;
        if spec.vendor == Vendor::Codex {
            self.check_sign_in(&checked, guest.is_some())?;
        }
        // A container's first message goes to the file delivery, which
        // looks its attachments up itself. A new agent's folder holds none
        // yet.
        let found = match &spec.first {
            Some(first) if guest.is_none() => self.find_attachments(&first.attachments, None)?,
            _ => Vec::new(),
        };
        Ok((
            StartInfo {
                account: spec.account,
                config_dir,
                cwd,
                workspace,
                guest,
                login,
                branch: None,
                mode: spec.permission.unwrap_or_else(|| self.default_mode()),
                model: spec.model.clone(),
                effort: self.start_effort(
                    spec.vendor,
                    spec.account,
                    spec.model.as_deref(),
                    spec.effort.clone(),
                ),
                title: title(spec, |id| {
                    found
                        .iter()
                        .find(|one| one.id == *id)
                        .or_else(|| self.staged.get(id))
                        .map(|one| one.name.clone())
                }),
                asked_title: asked_title(spec),
                origin: spec.origin.as_deref().copied(),
            },
            found,
        ))
    }

    /// Starts a Claude or Cursor agent in a holder of its own, then sends
    /// its first message. The agent shows, and the request is answered,
    /// once the holder is up.
    fn start_cli(
        &mut self,
        key: IdempotencyKey,
        vendor: Vendor,
        info: StartInfo,
        uuid: Uuid,
        place: Place,
        first: Option<Message>,
    ) -> Option<ActionOutcome> {
        let id = names::agent(vendor, &uuid.to_string());
        let guest = info.guest.is_some();
        let login = match self.login_for(
            vendor,
            info.account,
            info.config_dir.clone(),
            info.login,
            guest,
        ) {
            Ok(login) => login,
            Err(reason) => return Some(refused(reason)),
        };
        // A Claude session's UUID is MaxHQ's to pick. Cursor names its
        // session once it's made, and the driver reports it.
        // A container's CLI goes without the agent tools, as
        // `give_agent_key` says.
        let tools_key = (speaks_acp(vendor) && !guest)
            .then(|| self.mint_agent_key(id))
            .flatten();
        let (cli, vendor_session) = match vendor {
            vendor if speaks_acp(vendor) => {
                let kept = acp::Kept {
                    session: None,
                    model: info.model.clone(),
                    title: info.asked_title.clone(),
                    tools: self.agent_mcp(id, tools_key.as_ref()),
                };
                (Cli::Acp { vendor, kept }, String::new())
            }
            _ => {
                let claude = Cli::Claude {
                    uuid,
                    resume: false,
                    seed: false,
                };
                (claude, uuid.to_string())
            }
        };
        let spawn = AgentSpawn {
            lead: id,
            login,
            place,
            cli,
            model: info.model.clone(),
            effort: info.effort.clone(),
            mode: info.mode,
        };
        // The CLI says nothing until its first message.
        let activity = if first.is_some() {
            Activity::Starting
        } else {
            Activity::Idle
        };
        let shown = Some(vendor_session.clone()).filter(|session| !session.is_empty());
        let agent = self.placeholder(id, vendor, &info, shown, activity);
        let session = self.spawn_agent(spawn, Reaching::Start(key));
        // The CLI names a session from its first message, so the title you
        // gave is kept with the held agent instead.
        let title = info.asked_title.clone();
        let mut held = Held::new(agent, info, vendor_session);
        held.session = Some(session);
        held.title = title;
        held.tools_key = tools_key;
        self.held.insert(id, held);
        self.timeline_mut(id);
        self.save(id);
        if let Some(first) = first {
            self.send_own(id, first);
        }
        None
    }

    /// Asks the account's app-server for a new Codex thread. The outcome
    /// comes once the thread exists.
    fn start_codex(
        &mut self,
        key: IdempotencyKey,
        mut spec: AgentSpec,
        info: StartInfo,
        place: Place,
        first: Option<Message>,
    ) -> Option<ActionOutcome> {
        spec.first = None;
        spec.permission = Some(info.mode);
        spec.title.clone_from(&info.asked_title);
        spec.effort.clone_from(&info.effort);
        // Each account has an app-server on this device and one in each
        // container it runs in.
        let server_key = ServerKey {
            account: info.account,
            workspace: info.guest,
        };
        let login = self.codex_login(info.account, info.config_dir.clone(), info.login);
        self.servers
            .entry(server_key)
            .or_insert_with(|| Server::new(login));
        self.pending.insert(key, Pending::Start(Box::new(info)));
        self.work.push_back((
            Target::Codex(server_key),
            Action::Start {
                key,
                place,
                spec: Box::new(spec),
                first,
                agent: None,
            },
        ));
        None
    }

    /// Sends a message on MaxHQ's own behalf.
    pub(super) fn send_own(&mut self, lead: AgentId, message: Message) {
        let key = internal_key();
        self.internal.insert(key, Internal::Plain);
        self.work.push_back((
            Target::Agent(lead),
            Action::Send {
                key,
                agent: lead,
                text: message.text,
                refs: message.refs,
                attachments: message.attachments,
                mode: SendMode::Queue,
            },
        ));
    }

    /// Acts on a held agent. A stopped or paused agent comes back for a
    /// message, and settings changed while it's down apply when it does.
    fn route(&mut self, key: IdempotencyKey, agent: AgentId, ask: Ask) -> Option<ActionOutcome> {
        let lead = self.lead_of(agent);
        let Some(held) = self.held.get(&lead) else {
            return Some(if self.hub.get(&EntityKey::Agent(agent)).is_some() {
                refused(OUTSIDE)
            } else {
                ActionOutcome::UnknownAgent
            });
        };
        let vendor = held.vendor();
        let paused = held.paused.is_some();
        let guest = held.guest;
        let full_access = held.mode == PermissionMode::FullAccess;
        // A Claude or Cursor agent runs while its holder does. A Codex agent
        // can run whenever its app-server does, adopting its thread first.
        let running = self.session_of(held).is_some();
        let parked = paused || !running;
        // A guest's CLI can't hear an interrupt while its link is down, and
        // one that lands later could stop the wrong turn.
        let unreachable = guest.is_some_and(|guest| !self.link_up(guest));
        // A client from before every level was named asks for the model's
        // default, which `maxd` names too.
        let ask = match ask {
            Ask::Effort(None) => {
                Ask::Effort(self.model_effort(vendor, held.account, held.model.as_deref()))
            }
            ask => ask,
        };
        let target = Target::Agent(lead);
        let action = match ask {
            Ask::Stop => {
                // Its terminals keep running: a stopped agent comes back
                // with its next message, and a dev server it started
                // shouldn't go down meanwhile. Deleting it ends them.
                if !self.stop_lead(key, lead) {
                    return Some(ActionOutcome::Done);
                }
                self.after.insert(key, (lead, After::Stopped));
                return None;
            }
            Ask::Delete if lead != agent => return Some(refused(SUB_AGENT)),
            Ask::Delete => {
                if !self.stop_lead(key, lead) {
                    self.delete_lead(lead);
                    return Some(ActionOutcome::Done);
                }
                self.after.insert(key, (lead, After::Deleted));
                return None;
            }
            Ask::Mode(mode) if parked && vendor != Vendor::Codex => {
                if let Some(held) = self.held.get_mut(&lead) {
                    held.mode = mode;
                }
                self.save(lead);
                self.publish(lead);
                return Some(ActionOutcome::Done);
            }
            Ask::Mode(mode)
                if full_access_relaunches(vendor)
                    && full_access != (mode == PermissionMode::FullAccess) =>
            {
                return Some(self.relaunch_acp(lead, mode, unreachable));
            }
            Ask::Mode(mode) => {
                self.after.insert(key, (lead, After::Mode(mode)));
                Action::SetPermission {
                    key,
                    agent: lead,
                    mode,
                }
            }
            Ask::Model(model) if parked && vendor != Vendor::Codex => {
                if let Some(held) = self.held.get_mut(&lead) {
                    held.model = Some(model);
                }
                self.save(lead);
                self.publish(lead);
                return Some(ActionOutcome::Done);
            }
            Ask::Model(model) => {
                self.after.insert(key, (lead, After::Model(model.clone())));
                Action::SetModel {
                    key,
                    agent: lead,
                    model,
                }
            }
            Ask::Effort(_) if !takes_effort(vendor) => {
                return Some(refused(format!(
                    "{} agents have no effort level to set.",
                    vendor.name()
                )));
            }
            Ask::Effort(effort) if parked && vendor != Vendor::Codex => {
                if let Some(held) = self.held.get_mut(&lead) {
                    held.effort = effort;
                }
                self.save(lead);
                self.publish(lead);
                return Some(ActionOutcome::Done);
            }
            Ask::Effort(effort) => {
                self.after
                    .insert(key, (lead, After::Effort(effort.clone())));
                Action::SetEffort {
                    key,
                    agent: lead,
                    effort,
                }
            }
            Ask::Interrupt if parked => return Some(refused("the agent isn't running")),
            Ask::Interrupt | Ask::Cancel(_) if unreachable => return Some(refused(UNREACHABLE)),
            Ask::Interrupt => Action::Interrupt { key, agent: lead },
            Ask::Cancel(item) if parked => return Some(self.cancel_parked(lead, &item)),
            Ask::Cancel(item) => Action::CancelQueued {
                key,
                agent: lead,
                item,
            },
            Ask::Send {
                text,
                refs,
                attachments,
                mode,
            } => {
                if paused && self.is_stopping(lead) {
                    return Some(refused(STOPPING));
                }
                // Only the file delivery knows an upload that went straight
                // into the container, so it looks every ID up itself.
                if guest.is_some() && !attachments.is_empty() {
                    self.deliver(key, lead, (text, refs), attachments, mode);
                    return None;
                }
                let into = self.attachments.agent_dir(&lead.to_string());
                let attachments = match self.take_attachments(&attachments, &into) {
                    Ok(staged) => staged,
                    Err(reason) => return Some(refused(reason)),
                };
                let message = Message {
                    text,
                    refs,
                    attachments,
                };
                self.send_input(key, lead, message, mode);
                return None;
            }
        };
        self.work.push_back((target, action));
        None
    }

    /// Copies a message's attachments into its agent's container off the
    /// loop. The message goes once they're there.
    fn deliver(
        &self,
        key: IdempotencyKey,
        lead: AgentId,
        (text, refs): (String, Vec<InlineRef>),
        attachments: Vec<AttachmentId>,
        mode: SendMode,
    ) {
        let Some(place) = self.held.get(&lead).map(|held| self.place(held)) else {
            return;
        };
        let files = self.files.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = files
                .deliver(&place, attachments)
                .await
                .map_err(|err| err.to_string());
            let _ = tx.send(Input::Delivered {
                key,
                lead,
                text,
                refs,
                mode,
                result,
            });
        });
    }

    /// Sends a message whose attachments reached its agent's container.
    pub(super) fn delivered(
        &mut self,
        key: IdempotencyKey,
        lead: AgentId,
        (text, refs): (String, Vec<InlineRef>),
        mode: SendMode,
        result: Result<Vec<Staged>, String>,
    ) -> Option<ActionOutcome> {
        let staged = match result {
            Ok(staged) => staged,
            Err(reason) => return Some(refused(reason)),
        };
        if !self.held.contains_key(&lead) {
            return Some(ActionOutcome::UnknownAgent);
        }
        for attachment in &staged {
            self.staged.remove(&attachment.attachment.id);
        }
        let message = Message {
            text,
            refs,
            attachments: staged,
        };
        self.send_input(key, lead, message, mode);
        None
    }

    /// Sends your message to a held agent. One that's paused or whose CLI
    /// is down comes back for it, and one waiting for its limit to reset
    /// stops waiting.
    fn send_input(
        &mut self,
        key: IdempotencyKey,
        lead: AgentId,
        mut message: Message,
        mode: SendMode,
    ) {
        // A restart flag left from a wind-down is stale once you write, and
        // so is a carry-on owed for a limit.
        self.drop_flag(lead);
        self.drop_carry_on(lead);
        if !self.held.contains_key(&lead) {
            return;
        }
        // A session pill keeps only what its card shares, and a message
        // you sent seals the cards as it goes, queued or not.
        self.keep_shared(&mut message.refs);
        if self.yours.remove(&key) {
            self.seal_cards(lead, &message.refs, true);
        }
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        let paused = held.paused.is_some();
        let down = held.own_process() && self.session_of(held).is_none();
        if paused || down {
            self.unpause(lead, false);
            self.revive(lead);
        }
        self.after.insert(key, (lead, After::Sent));
        self.work.push_back((
            Target::Agent(lead),
            Action::Send {
                key,
                agent: lead,
                text: message.text,
                refs: message.refs,
                attachments: message.attachments,
                mode,
            },
        ));
    }

    /// Takes back a message queued on an agent that isn't running, which
    /// only MaxHQ still has.
    fn cancel_parked(&mut self, lead: AgentId, item: &ItemId) -> ActionOutcome {
        let queued = self
            .timelines
            .get(&lead)
            .and_then(|timeline| timeline.get(item));
        let Some(mut queued) = queued
            .filter(|queued| is_queued(queued))
            .map(Cow::into_owned)
        else {
            return refused("that message already went to the agent");
        };
        queued.status = ItemStatus::Cancelled;
        if let ItemBody::UserMessage { delivery, .. } = &mut queued.body {
            *delivery = Delivery::Cancelled;
        }
        self.timeline_events(vec![(lead, TimelineEvent::Upsert { item: queued })]);
        ActionOutcome::Done
    }

    /// Turns Full access on or off for a running ACP agent whose CLI takes
    /// it as a launch flag. Cursor's sandbox is fixed for the life of its
    /// process, so the process ends and
    /// starts again with the new flags, loading the same session. Not
    /// during a turn, which the restart would cut off.
    fn relaunch_acp(
        &mut self,
        lead: AgentId,
        mode: PermissionMode,
        unreachable: bool,
    ) -> ActionOutcome {
        if unreachable {
            return refused(UNREACHABLE);
        }
        if self.cut(lead).busy {
            return refused(SANDBOX_MID_TURN);
        }
        let Some(held) = self.held.get_mut(&lead) else {
            return ActionOutcome::UnknownAgent;
        };
        held.mode = mode;
        let session = held.session;
        self.save(lead);
        self.publish(lead);
        if let Some(session) = session {
            self.stop_to_relaunch(session);
        }
        ActionOutcome::Done
    }

    /// Whether a held Claude agent's CLI was asked to stop and hasn't yet.
    pub(super) fn is_stopping(&self, lead: AgentId) -> bool {
        self.held
            .get(&lead)
            .and_then(|held| held.session)
            .and_then(|session| self.holds.get(&session))
            .is_some_and(|hold| hold.stopping)
    }

    /// Lets an agent run again after you paused it or it paused itself.
    /// The continue message it owed gives way to what you send, unless
    /// `keep_owed` says it still goes. It stops waiting for a limit.
    fn unpause(&mut self, lead: AgentId, keep_owed: bool) {
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        held.paused = None;
        held.resumes = Default::default();
        held.fault = None;
        held.resume_after = None;
        if !keep_owed {
            held.owed = None;
        }
        let notice = held.notice.take();
        self.idle_ends.remove(&lead);
        self.limit_waits.remove(&lead);
        if let Some(notice) = notice {
            self.close_attention(notice);
        }
        self.save(lead);
        self.publish(lead);
    }

    /// Brings a held agent back: a Claude or Cursor agent gets a new CLI,
    /// and a Codex agent sends what it has queued.
    pub(super) fn revive(&mut self, lead: AgentId) {
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        if held.own_process() && self.session_of(held).is_none() {
            self.resume_agent(lead);
        } else {
            self.follow_up(lead);
        }
    }

    /// Moves an agent into MaxHQ. A held agent that's down comes back. An
    /// outside session is resumed in a holder, once nothing else runs it.
    pub(super) fn continue_here(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
    ) -> Option<ActionOutcome> {
        let lead = self.lead_of(agent);
        if let Some(held) = self.held.get(&lead) {
            let down =
                held.paused.is_some() || (held.own_process() && self.session_of(held).is_none());
            // An agent kept paused after a restart still owes its continue
            // message.
            let keep_owed = held.paused == Some(PauseReason::ResumeOff);
            if down {
                if self.is_stopping(lead) {
                    return Some(refused(STOPPING));
                }
                self.unpause(lead, keep_owed);
                self.revive(lead);
            }
            return Some(ActionOutcome::Done);
        }
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return Some(ActionOutcome::UnknownAgent);
        };
        match self.outside_held(found) {
            Ok(held) => self.continue_outside(key, held),
            Err(reason) => Some(refused(reason)),
        }
    }

    /// Resumes an outside session MaxHQ now holds, in its vendor's way.
    pub(super) fn continue_outside(
        &mut self,
        key: IdempotencyKey,
        held: Held,
    ) -> Option<ActionOutcome> {
        let agent = held.agent.id;
        // A read still filling its timeline goes on, so clients waiting
        // for it get it.
        self.outside.remove(&agent);
        match held.vendor() {
            Vendor::Claude => self.continue_claude(key, held),
            Vendor::Cursor => Some(refused(OUTSIDE_CURSOR)),
            Vendor::Acp => Some(refused(OUTSIDE_ACP)),
            Vendor::Codex => {
                if !self.timelines.contains_key(&agent) {
                    let source = Source {
                        vendor: held.vendor(),
                        config_dir: held.config_dir.clone(),
                        session: held.vendor_session.clone(),
                        subagent: None,
                        workspace: None,
                        account: Some(held.account),
                    };
                    self.timeline_mut(agent);
                    self.read(agent, ReadKind::Seed, Reader::Open(source));
                }
                self.continue_codex(key, held);
                None
            }
        }
    }

    /// What MaxHQ holds for an outside session it takes over, if it can.
    fn outside_held(&self, found: Agent) -> Result<Held, String> {
        let held = self.outside_start(found)?;
        if is_busy(&held.agent.activity)
            || !self
                .vendors
                .can_continue(held.vendor(), &held.config_dir, &held.vendor_session)
        {
            return Err(ELSEWHERE.into());
        }
        Ok(held)
    }

    /// What MaxHQ would hold for an outside session, whether or not a
    /// terminal still runs it.
    pub(super) fn outside_start(&self, found: Agent) -> Result<Held, String> {
        if found.parent.is_some() {
            return Err("a sub-agent follows its lead, so continue the lead instead".into());
        }
        let (Some(account), Some(session)) = (found.account, found.vendor_session_id.clone())
        else {
            return Err("MaxHQ doesn't know this agent's account or session".into());
        };
        let Some(Entity::Account(login)) = self.hub.get(&EntityKey::Account(account)) else {
            return Err("that account isn't on this device".into());
        };
        let config_dir = PathBuf::from(&login.config_dir);
        let cwd = match self.hub.get(&EntityKey::Workspace(found.workspace)) {
            Some(Entity::Workspace(workspace)) => match workspace.location {
                WorkspaceLocation::Folder { path } => PathBuf::from(path),
                WorkspaceLocation::Container { .. } => {
                    return Err("MaxHQ can't take over an agent that runs in a container".into());
                }
            },
            _ => return Err("MaxHQ doesn't know which folder this agent runs in".into()),
        };
        let info = StartInfo {
            account,
            config_dir,
            cwd,
            workspace: found.workspace,
            guest: None,
            login: login.login,
            branch: None,
            mode: found
                .permission
                .as_ref()
                .map_or_else(|| self.default_mode(), |permission| permission.requested),
            model: found.model.clone(),
            effort: self.start_effort(found.vendor, account, found.model.as_deref(), None),
            title: found.title.clone(),
            asked_title: None,
            origin: None,
        };
        Ok(Held::new(found, info, session))
    }

    /// Resumes an outside Claude session in a holder. It's done once the
    /// holder is up.
    pub(super) fn continue_claude(
        &mut self,
        key: IdempotencyKey,
        mut held: Held,
    ) -> Option<ActionOutcome> {
        let lead = held.agent.id;
        let Ok(uuid) = Uuid::parse_str(&held.vendor_session) else {
            return Some(refused("that isn't a Claude session MaxHQ can resume"));
        };
        let login = match self.login_of(&held) {
            Ok(login) => login,
            Err(reason) => return Some(refused(reason)),
        };
        let spawn = AgentSpawn {
            lead,
            login,
            place: self.place(&held),
            cli: Cli::Claude {
                uuid,
                resume: true,
                seed: !self.timelines.contains_key(&lead),
            },
            model: held.model.clone(),
            effort: held.effort.clone(),
            mode: held.mode,
        };
        self.timeline_mut(lead);
        let session = self.spawn_agent(spawn, Reaching::Continue(key));
        held.session = Some(session);
        held.resuming = true;
        self.held.insert(lead, held);
        self.save(lead);
        None
    }

    /// Adopts an outside Codex thread on the account's app-server. It's
    /// held once the app-server loads it.
    pub(super) fn continue_codex(&mut self, key: IdempotencyKey, mut held: Held) {
        let server_key = held.server_key();
        let login = self.codex_login(held.account, held.config_dir.clone(), held.login);
        self.servers
            .entry(server_key)
            .or_insert_with(|| Server::new(login));
        held.adopt = true;
        let action = Action::Adopt {
            key,
            place: self.place(&held),
            agent: held.agent.id,
            thread: held.vendor_session.clone(),
        };
        let settings = self.set_mode_and_model(
            held.agent.id,
            held.mode,
            held.model.clone(),
            held.effort.clone(),
        );
        self.followups.insert(key, settings);
        self.pending.insert(key, Pending::Adopt(Box::new(held)));
        self.work.push_back((Target::Codex(server_key), action));
    }

    /// Opens a timeline for a client: the events after its seq, or a reset.
    /// An outside session's timeline comes from its CLI's files, read again
    /// while the client keeps it open. A sub-agent of `lead` that isn't
    /// listed, such as one that finished, is read once from its transcript.
    fn open_timeline(
        &mut self,
        conn: u64,
        agent: AgentId,
        since: Option<Seq>,
        limit: u32,
        lead: Option<AgentId>,
    ) {
        let limit = page_size(limit);
        let Some(viewer) = self.clients.get_mut(&conn) else {
            return;
        };
        viewer.open.insert(agent, limit);
        if !self.timelines.contains_key(&agent) && !self.held.contains_key(&self.lead_of(agent)) {
            match lead {
                Some(lead) if self.hub.get(&EntityKey::Agent(agent)).is_none() => {
                    self.open_finished(agent, lead);
                }
                _ => self.open_outside(agent),
            }
        }
        self.hurry(self.lead_of(agent));
        if self.filling(agent) {
            // The read sends every client a reset once it's done.
            return;
        }
        let update = self.timeline_mut(agent).open(since, limit);
        self.tell(conn, ServerMessage::Timeline { agent, update });
    }

    /// Starts reading an outside session's history, off the loop.
    pub(super) fn open_outside(&mut self, agent: AgentId) {
        let Some(source) = self.outside_source(agent) else {
            return;
        };
        self.timeline_mut(agent);
        self.outside.insert(agent, None);
        self.read(agent, ReadKind::Outside, Reader::Open(source));
    }

    /// Starts reading the transcript of a Claude sub-agent of `lead` that
    /// isn't listed, off the loop. It's read once and never polled, since a
    /// sub-agent that finished writes nothing more. Nothing is read once
    /// the lead isn't listed either.
    fn open_finished(&mut self, agent: AgentId, lead: AgentId) {
        let Some(top) = self.top_lead(lead) else {
            return;
        };
        let source = match self.held.get(&top.id) {
            Some(held) => Some(places::source(held)),
            None => self.outside_source(top.id),
        };
        let Some(source) = source.filter(|source| source.vendor == Vendor::Claude) else {
            return;
        };
        self.timeline_mut(agent);
        let source = Source {
            subagent: Some(Subagent::Of(top.id)),
            ..source
        };
        self.read(agent, ReadKind::Outside, Reader::Open(source));
    }

    /// Where a session this device doesn't hold keeps its history, when
    /// MaxHQ knows its account and session. A Claude sub-agent's is its own
    /// transcript, kept with its lead's session.
    pub(super) fn outside_source(&self, agent: AgentId) -> Option<Source> {
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return None;
        };
        let (Some(account), Some(session)) = (found.account, found.vendor_session_id) else {
            return None;
        };
        let Some(Entity::Account(login)) = self.hub.get(&EntityKey::Account(account)) else {
            return None;
        };
        let (session, subagent) = match (found.vendor, found.parent) {
            (Vendor::Claude, Some(parent)) => (
                self.top_lead(parent)?.vendor_session_id?,
                Some(Subagent::Id(session)),
            ),
            _ => (session, None),
        };
        Some(Source {
            vendor: found.vendor,
            config_dir: PathBuf::from(&login.config_dir),
            session,
            subagent,
            workspace: None,
            account: Some(account),
        })
    }

    /// The lead at the top of `agent`'s parents, or `agent` when it's the
    /// lead, while the hub lists them all.
    fn top_lead(&self, mut agent: AgentId) -> Option<Agent> {
        for _ in 0..MAX_NESTING {
            let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
                return None;
            };
            match found.parent {
                Some(parent) => agent = parent,
                None => return Some(found),
            }
        }
        None
    }

    /// Reads what outside sessions wrote since the last poll, off the loop.
    /// One whose last read isn't back yet waits for the next poll.
    pub(super) fn poll_outside(&mut self) {
        let ready: Vec<AgentId> = self
            .outside
            .iter()
            .filter(|(_, history)| history.is_some())
            .map(|(agent, _)| *agent)
            .collect();
        for agent in ready {
            if let Some(history) = self.outside.get_mut(&agent).and_then(Option::take) {
                self.read(agent, ReadKind::Poll, Reader::Kept(history));
            }
        }
    }

    /// Sends a client the items before an ordinal. A held agent's
    /// timeline still read after a restart is read next, and answers then.
    pub(super) fn load_older(&mut self, conn: u64, agent: AgentId, before: u64, limit: u32) {
        if self.loading.waits(agent) {
            self.hurry(agent);
            self.wait_for(
                agent,
                Later::Page {
                    conn,
                    before,
                    limit,
                },
            );
            return;
        }
        let (items, has_older) = self.timelines.get(&agent).map_or_else(
            || (Vec::new(), false),
            |timeline| timeline.older(before, page_size(limit)),
        );
        self.tell(
            conn,
            ServerMessage::Timeline {
                agent,
                update: TimelineUpdate::Older { items, has_older },
            },
        );
    }

    /// Closes a timeline for a client.
    fn close_timeline(&mut self, conn: u64, agent: AgentId) {
        if let Some(viewer) = self.clients.get_mut(&conn) {
            viewer.open.remove(&agent);
        }
        self.drop_unwatched([agent]);
    }

    /// A client's connection ended.
    pub(super) fn gone_client(&mut self, conn: u64) {
        if let Some(viewer) = self.clients.remove(&conn) {
            self.drop_unwatched(viewer.open.into_keys());
        }
    }

    /// Drops the timelines of agents MaxHQ doesn't hold once no client has
    /// them open.
    pub(super) fn drop_unwatched(&mut self, agents: impl IntoIterator<Item = AgentId>) {
        for agent in agents {
            let watched = self
                .clients
                .values()
                .any(|viewer| viewer.open.contains_key(&agent));
            if watched || self.held.contains_key(&self.lead_of(agent)) {
                continue;
            }
            self.outside.remove(&agent);
            self.reads.remove(&agent);
            if let Some(timeline) = self.timelines.remove(&agent) {
                self.retired.insert(agent, timeline.seq().0);
            }
        }
        if self.outside.is_empty() {
            self.next_poll = None;
        }
    }

    /// Stages an attachment off the loop, since it may copy a large file.
    fn stage(&self, key: IdempotencyKey, source: AttachmentSource) {
        if let AttachmentSource::Found { agent, path } = source {
            return self.stage_found(key, agent, path);
        }
        let attachments = self.attachments.clone();
        let tx = self.tx.clone();
        let id = AttachmentId::generate();
        tokio::task::spawn_blocking(move || {
            let result = attachments
                .stage(id, &source)
                .map_err(|err| err.to_string());
            let _ = tx.send(Input::Staged { key, result });
        });
    }

    /// Stages what the `@` menu found, from the folder its search looked
    /// in, or a file the agent was sent, from its attachments folder, such
    /// as one a pasted message names. The path can't leave those folders or
    /// follow a link out of them. A container agent's file comes out of its
    /// container, as one its send tool sends does.
    fn stage_found(&self, key: IdempotencyKey, agent: AgentId, path: String) {
        let tx = self.tx.clone();
        let place = match self.found_place(agent) {
            Ok(place) => place,
            Err(reason) => {
                let _ = tx.send(Input::Staged {
                    key,
                    result: Err(reason),
                });
                return;
            }
        };
        let attachments = self.attachments.clone();
        let files = self.files.clone();
        let id = AttachmentId::generate();
        tokio::spawn(async move {
            let result = match place.host {
                PlaceHost::Local => tokio::task::spawn_blocking(move || {
                    let found = disk::resolve(&[place.cwd, place.attachments], &path)
                        .map_err(|failure| format!("{path}: {}", failure.message))?;
                    let path = found.to_string_lossy().into_owned();
                    attachments
                        .stage(id, &AttachmentSource::Path { path })
                        .map_err(|err| err.to_string())
                })
                .await
                .unwrap_or_else(|err| Err(format!("the copy stopped: {err}"))),
                PlaceHost::Guest { .. } => files.stage_found(&place, id, &path).await,
            };
            let _ = tx.send(Input::Staged { key, result });
        });
    }

    /// Answers `FindGitItems` for a repo the client names, such as one a
    /// pasted link names, rather than the agent's own. It reads with this
    /// device's GitHub App where the App reaches that repo, and with `gh`
    /// where it doesn't, as the agent's own repo is read, so it sees only
    /// what those already can.
    fn find_git_items_in(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        repo: String,
        query: String,
    ) {
        if let Err(reason) = self.folder_of(agent) {
            return self.found(conn, key, refused(reason));
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let search = secrets::find_git_items(&repo, &query).await;
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(ActionOutcome::GitItems { search }),
            });
        });
    }

    /// Where an agent's files are, as a held agent's place is.
    pub(super) fn found_place(&self, agent: AgentId) -> Result<Place, String> {
        let lead = self.lead_of(agent);
        if let Some(held) = self.held.get(&lead) {
            return Ok(self.place(held));
        }
        let folder = self.folder_of(agent)?;
        let guest = match folder.host {
            PlaceHost::Guest { workspace } => Some(workspace),
            PlaceHost::Local => None,
        };
        Ok(self.place_in(folder.workspace, guest, folder.cwd, &lead.to_string()))
    }

    /// Changes some of this device's settings, keeping the rest as it has
    /// them.
    fn update_settings(
        &mut self,
        device: DeviceId,
        changes: Vec<DeviceSettingChange>,
    ) -> ActionOutcome {
        if device != self.device {
            return refused("those settings are for another device");
        }
        let mut settings = self.device_settings();
        changes.into_iter().for_each(|one| settings.change(one));
        self.settings(settings)
    }

    /// Stores and publishes this device's settings. A memory limit out of
    /// range is kept to the nearest it can be.
    fn settings(&mut self, settings: DeviceSettings) -> ActionOutcome {
        if settings.device != self.device {
            return refused("those settings are for another device");
        }
        let settings = settings.clamped();
        let before = self.device_settings();
        if let Err(reason) = check_settings(&before, &settings) {
            return refused(reason);
        }
        if let Err(err) = self.db.set_device_settings(&settings) {
            return refused(format!("couldn't store the settings: {err}"));
        }
        if settings.network.on != before.network.on {
            tracing::info!(
                on = settings.network.on,
                port = settings.network.port,
                "network access for phones changed"
            );
        }
        if settings.default_permission != before.default_permission {
            tracing::info!(
                mode = ?settings.default_permission,
                "the default permission for new agents changed"
            );
        }
        if settings.claude_setup_token != before.claude_setup_token {
            tracing::info!(
                on = settings.claude_setup_token,
                "Claude setup-tokens turned on or off"
            );
        }
        self.power.set_mode(settings.keep_awake);
        self.keep_to(&settings);
        // Turning Cursor usage on or off shows now, not at the next check.
        let recheck = settings.cursor_usage != before.cursor_usage;
        self.hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(settings),
        }]);
        if recheck {
            self.hub.send(Command::RefreshUsage(None));
        }
        ActionOutcome::Done
    }
}

/// Why this device won't keep `new`. Only what changes is checked, so a
/// value kept from before these checks doesn't block other changes.
fn check_settings(before: &DeviceSettings, new: &DeviceSettings) -> Result<(), &'static str> {
    if new.network != before.network && new.network.on && new.network.port == 0 {
        return Err("Pick a port from 1 to 65535.");
    }
    let hosts = &new.test_hosts;
    if *hosts != before.test_hosts {
        if hosts.max_vms == 0 || hosts.max_vms > MAX_VMS {
            return Err("A device runs 1 or 2 macOS VMs at once.");
        }
        if hosts.longest_minutes == 0 || hosts.default_minutes == 0 || hosts.idle_minutes == 0 {
            return Err("A lease needs at least a minute.");
        }
    }
    Ok(())
}

/// The config folder an account has in a container, in `agent`'s home.
/// Cursor's key accounts share the CLI's own folder, since a device has one
/// key.
pub(super) fn guest_folder(vendor: Vendor, account: AccountId) -> PathBuf {
    let account = account.to_string();
    match vendor {
        Vendor::Claude => recipe::accounts::claude_folder(&account).into(),
        Vendor::Codex => recipe::accounts::codex_folder(&account).into(),
        Vendor::Cursor => recipe::CURSOR_CONFIG.into(),
        Vendor::Acp => PathBuf::new(),
    }
}

/// The branch a new agent in a container works on, unless it asks for
/// one: `maxhq/` and the start of its ID.
fn default_branch(agent: AgentId) -> String {
    let id = agent.to_string();
    format!("maxhq/{}", id.get(..8).unwrap_or(&id))
}

/// A page size a client asked for, kept within bounds.
fn page_size(limit: u32) -> usize {
    usize::try_from(limit)
        .unwrap_or(MAX_PAGE)
        .clamp(1, MAX_PAGE)
}

/// A new agent's title: the one asked for, else the first line of its
/// first message, each pill shown as its text. `name` gives a staged
/// file's name.
fn title(spec: &AgentSpec, name: impl Fn(&AttachmentId) -> Option<String>) -> String {
    let first = || {
        spec.first.as_ref().and_then(|first| {
            context::plain(&first.text, &first.refs, name)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(clip_title)
        })
    };
    asked_title(spec)
        .or_else(first)
        .unwrap_or_else(|| "New agent".to_owned())
}

/// Whether a new agent can run in the folder at `path`, read off the loop.
pub(super) fn usable_folder(hub: &Hub, path: &Path) -> Result<(), String> {
    if hub.in_clone(path) {
        Err(STILL_CLONING.to_owned())
    } else if path.is_absolute() && path.is_dir() {
        Ok(())
    } else {
        Err("that folder doesn't exist".to_owned())
    }
}

/// The title a new agent's spec asks for, if it names one that isn't
/// blank.
fn asked_title(spec: &AgentSpec) -> Option<String> {
    spec.title.as_deref().and_then(given_title)
}

/// A title you gave, trimmed and cut to [`TITLE_CHARS`], unless it's
/// blank.
pub(super) fn given_title(title: &str) -> Option<String> {
    Some(title.trim())
        .filter(|title| !title.is_empty())
        .map(clip_title)
}

/// A title cut to [`TITLE_CHARS`].
fn clip_title(title: &str) -> String {
    title.chars().take(TITLE_CHARS).collect()
}
