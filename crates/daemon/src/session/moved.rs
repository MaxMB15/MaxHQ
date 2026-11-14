// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The session side of a workspace move: the switch resumes the original's
//! agent under a holder in the copy, in the repo's main checkout, and a
//! move back stops the agents there.

use std::path::PathBuf;

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, IdempotencyKey, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ClientMessage};
use maxhq_protocol::model::{Activity, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use tokio::sync::oneshot;

use super::held::{internal_key, Held, StartInfo};
use super::idem::Outcome;
use super::{Input, Manager, Sessions};

/// What a switch resumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeMoved {
    /// The copy's workspace.
    pub workspace: WorkspaceId,
    /// The account it runs under on this device.
    pub account: AccountId,
    /// Claude or Codex.
    pub vendor: Vendor,
    /// The CLI's session or thread.
    pub session: String,
    /// The repo's main checkout in the copy.
    pub cwd: String,
}

/// What the manager answers a move with.
pub(super) type Done = oneshot::Sender<Result<(), String>>;

const STOPPING: &str = "MaxHQ is stopping.";

impl Sessions {
    /// Resumes a moved agent in its copy, and waits until its holder is up.
    pub async fn resume_moved(&self, resume: ResumeMoved) -> Result<(), String> {
        let (done, answer) = oneshot::channel();
        if self.tx.send(Input::ResumeMoved { resume, done }).is_err() {
            return Err(STOPPING.to_owned());
        }
        answer.await.unwrap_or_else(|_| Err(STOPPING.to_owned()))
    }

    /// Stops every agent held in a workspace.
    pub async fn stop_in(&self, workspace: WorkspaceId) -> Result<(), String> {
        let (done, answer) = oneshot::channel();
        if self.tx.send(Input::StopIn { workspace, done }).is_err() {
            return Err(STOPPING.to_owned());
        }
        answer.await.unwrap_or_else(|_| Err(STOPPING.to_owned()))
    }
}

impl Manager {
    /// Starts the resume, and answers once the holder is up or the start
    /// failed.
    pub(super) fn resume_moved(&mut self, resume: ResumeMoved, done: Done) {
        let key = internal_key();
        match self.resume_held(&resume) {
            Ok(held) => {
                self.moved.insert(key, done);
                let answered = match resume.vendor {
                    Vendor::Codex => {
                        self.continue_codex(key, held);
                        None
                    }
                    _ => self.continue_claude(key, held),
                };
                if let Some(outcome) = answered {
                    self.finish(key, Outcome::Action(outcome));
                }
            }
            Err(reason) => {
                let _ = done.send(Err(reason));
            }
        }
    }

    /// Answers the move waiting on `key`, if one is.
    pub(super) fn moved_done(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        let Some(done) = self.moved.remove(&key) else {
            return;
        };
        let result = match outcome {
            Outcome::Action(ActionOutcome::Refused { reason }) => Err(reason.clone()),
            Outcome::Action(ActionOutcome::UnknownAgent) => {
                Err("MaxHQ doesn't know that agent".to_owned())
            }
            _ => Ok(()),
        };
        let _ = done.send(result);
    }

    /// The held agent a moved session resumes as.
    fn resume_held(&self, resume: &ResumeMoved) -> Result<Held, String> {
        if !matches!(resume.vendor, Vendor::Claude | Vendor::Codex) {
            return Err("only a Claude or Codex session moves".into());
        }
        let id = names::agent(resume.vendor, &resume.session);
        if self.held.contains_key(&id) {
            return Err("MaxHQ holds that session already".into());
        }
        let Some(Entity::Account(account)) = self.hub.get(&EntityKey::Account(resume.account))
        else {
            return Err("that account isn't on this device".into());
        };
        if account.device != self.device {
            return Err("that account is on another device".into());
        }
        if account.vendor != resume.vendor {
            return Err("that account is for another vendor".into());
        }
        self.check_container(resume.workspace)?;
        let config_dir = super::requests::guest_folder(resume.vendor, resume.account);
        let title = PathBuf::from(&resume.cwd)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = StartInfo {
            account: resume.account,
            config_dir,
            cwd: PathBuf::from(&resume.cwd),
            workspace: resume.workspace,
            guest: Some(resume.workspace),
            login: account.login,
            branch: None,
            mode: self.default_mode(),
            model: None,
            effort: self.start_effort(resume.vendor, resume.account, None, None),
            title,
            asked_title: None,
            origin: None,
        };
        let agent = self.placeholder(
            id,
            resume.vendor,
            &info,
            Some(resume.session.clone()),
            Activity::Idle,
        );
        let held = Held::new(agent, info, resume.session.clone());
        let login = self.login_of(&held)?;
        self.check_sign_in(&login, true)?;
        Ok(held)
    }

    /// Asks each lead agent held in `workspace` to stop.
    pub(super) fn stop_in(&mut self, workspace: WorkspaceId, done: Done) {
        let leads: Vec<_> = self
            .held
            .values()
            .filter(|held| held.agent.workspace == workspace)
            .map(|held| held.agent.id)
            .collect();
        for agent in leads {
            let key = internal_key();
            self.message(u64::MAX, ClientMessage::StopAgent { key, agent });
        }
        let _ = done.send(Ok(()));
    }
}
