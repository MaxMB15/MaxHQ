// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How the manager reaches the vendor drivers: the command lines that start
//! each CLI, the drivers that speak to them, and the readers for sessions
//! started outside MaxHQ.

use std::io;
use std::path::{Path, PathBuf};

use maxhq_protocol::ids::{AccountId, AgentId, DeviceId};
use maxhq_protocol::model::{PermissionMode, Vendor};
use maxhq_protocol::timeline::TimelineItem;
use uuid::Uuid;

use crate::drive::acp::{self, Flavor};
use crate::drive::{claude, codex, cursor, local, Driver, Launch, Login, Place};

/// Reads a session's history from the CLI's own files, a little more each
/// call.
pub trait History: Send {
    /// The items that are new or changed since the last call, whole.
    fn read(&mut self) -> io::Result<Vec<TimelineItem>>;
    /// Takes at most `bytes` of the newest history in a first read, or all
    /// of it when there's no limit. A history that reads it all in one go
    /// anyway takes no notice.
    fn limit(&mut self, bytes: Option<u64>) {
        let _ = bytes;
    }
    /// Names `account`, the one the session runs under, in what a usage
    /// limit or a login that ran out shows, as the live session does. A
    /// history that can't say takes no notice.
    fn account(&mut self, account: AccountId) {
        let _ = account;
    }
}

/// Builds launches, drivers and history readers for each vendor.
pub trait Vendors: Send + Sync + 'static {
    /// The command line for a Claude session. `resume` continues the
    /// session with this UUID instead of starting it.
    fn claude_launch(
        &self,
        login: &Login,
        place: &Place,
        session: Uuid,
        resume: bool,
        model: Option<&str>,
        mode: PermissionMode,
    ) -> Launch;
    /// The driver for one Claude session.
    fn claude_session(
        &self,
        login: &Login,
        place: &Place,
        agent: AgentId,
        mode: PermissionMode,
    ) -> Box<dyn Driver>;
    /// The command line for one agent of an ACP `vendor`: Cursor's
    /// `agent acp`, or a local agent's own program.
    fn acp_launch(
        &self,
        vendor: Vendor,
        login: &Login,
        place: &Place,
        mode: PermissionMode,
    ) -> Launch;
    /// The driver for one agent of an ACP `vendor`, which loads
    /// `kept.session` when there's one.
    fn acp_session(
        &self,
        vendor: Vendor,
        login: &Login,
        place: &Place,
        agent: AgentId,
        kept: acp::Kept,
        mode: PermissionMode,
    ) -> Box<dyn Driver>;
    /// The command line for an account's `codex app-server`.
    fn codex_launch(&self, login: &Login) -> Launch;
    /// The driver for one account's `codex app-server` and its threads.
    fn codex_server(&self, login: &Login, device: DeviceId) -> Box<dyn Driver>;
    /// Whether no CLI is running the session elsewhere, so MaxHQ may take
    /// it over.
    fn can_continue(&self, vendor: Vendor, config_dir: &Path, session: &str) -> bool;
    /// The name of the CLI that runs a vendor's sessions in a terminal,
    /// which MaxHQ checks before it stops one. Empty when MaxHQ can't take
    /// that vendor's sessions over.
    fn terminal_cli(&self, vendor: Vendor) -> &'static str;
    /// A reader for a session's history, or none when its files aren't
    /// there. `subagent` names one of a Claude session's sub-agents, whose
    /// own transcript is read in place of the session's.
    fn history(
        &self,
        vendor: Vendor,
        config_dir: &Path,
        session: &str,
        subagent: Option<&str>,
        agent: AgentId,
    ) -> Option<Box<dyn History>>;
    /// The IDs of a session's sub-agents that keep a transcript of their
    /// own, which only Claude's do.
    fn subagents(&self, vendor: Vendor, config_dir: &Path, session: &str) -> Vec<String>;
}

/// The drivers this build has: Claude Code in stream-json mode, Codex
/// through its app-server, and Cursor over ACP.
pub struct Wired {
    /// The attachments root, where a history read keeps the pictures its
    /// tool results hold. None leaves them out.
    pub pictures: Option<PathBuf>,
}

impl Vendors for Wired {
    fn claude_launch(
        &self,
        login: &Login,
        place: &Place,
        session: Uuid,
        resume: bool,
        model: Option<&str>,
        mode: PermissionMode,
    ) -> Launch {
        claude::launch(login, place, session, resume, model, mode)
    }

    fn claude_session(
        &self,
        login: &Login,
        place: &Place,
        agent: AgentId,
        mode: PermissionMode,
    ) -> Box<dyn Driver> {
        Box::new(claude::ClaudeSession::new(
            login.clone(),
            place.clone(),
            agent,
            mode,
        ))
    }

    fn acp_launch(
        &self,
        vendor: Vendor,
        login: &Login,
        place: &Place,
        mode: PermissionMode,
    ) -> Launch {
        match vendor {
            Vendor::Acp => local::launch(login, place),
            _ => cursor::launch(login, place, mode),
        }
    }

    fn acp_session(
        &self,
        vendor: Vendor,
        login: &Login,
        place: &Place,
        agent: AgentId,
        kept: acp::Kept,
        mode: PermissionMode,
    ) -> Box<dyn Driver> {
        match vendor {
            Vendor::Acp => local::session(login, place, agent, kept, mode),
            _ => cursor::session(login, place, agent, kept, mode),
        }
    }

    fn codex_launch(&self, login: &Login) -> Launch {
        codex::launch(login)
    }

    fn codex_server(&self, login: &Login, device: DeviceId) -> Box<dyn Driver> {
        Box::new(codex::CodexServer::new(login.clone(), device))
    }

    fn can_continue(&self, vendor: Vendor, config_dir: &Path, session: &str) -> bool {
        match vendor {
            Vendor::Claude => claude::can_continue(config_dir, session),
            // The app-server refuses to adopt a thread another process
            // writes to, so a Codex thread may always be tried.
            Vendor::Codex => true,
            // Cursor and other ACP sessions started outside MaxHQ can't
            // be found, so none is taken over.
            Vendor::Cursor | Vendor::Acp => false,
        }
    }

    fn terminal_cli(&self, vendor: Vendor) -> &'static str {
        match vendor {
            Vendor::Claude => "claude",
            Vendor::Codex => "codex",
            Vendor::Cursor | Vendor::Acp => "",
        }
    }

    fn history(
        &self,
        vendor: Vendor,
        config_dir: &Path,
        session: &str,
        subagent: Option<&str>,
        agent: AgentId,
    ) -> Option<Box<dyn History>> {
        match vendor {
            Vendor::Codex => codex::History::open(config_dir, session, agent).map(|history| {
                let history = match &self.pictures {
                    Some(root) => history.keeping_pictures_in(root.clone()),
                    None => history,
                };
                Box::new(Rollout(history)) as Box<dyn History>
            }),
            Vendor::Claude => match subagent {
                Some(id) => claude::History::find_subagent(config_dir, session, id, agent),
                None => claude::History::find(config_dir, session, agent),
            }
            .map(|history| {
                let history = match &self.pictures {
                    Some(root) => history.keeping_pictures_in(root.clone()),
                    None => history,
                };
                Box::new(Transcript(history)) as Box<dyn History>
            }),
            // Cursor keeps no history MaxHQ reads from disk. A held Cursor
            // agent's history comes from `session/load`, and so does any
            // other ACP agent's.
            Vendor::Cursor | Vendor::Acp => None,
        }
    }

    fn subagents(&self, vendor: Vendor, config_dir: &Path, session: &str) -> Vec<String> {
        match vendor {
            Vendor::Claude => claude::subagent_ids(config_dir, session),
            // A Codex sub-agent is a thread of its own.
            Vendor::Codex | Vendor::Cursor | Vendor::Acp => Vec::new(),
        }
    }
}

/// Whether each of `vendor`'s agents runs in a CLI of its own, as Claude's
/// and every ACP CLI's do. Codex agents share their account's app-server.
pub fn one_process_per_agent(vendor: Vendor) -> bool {
    match vendor {
        Vendor::Claude | Vendor::Cursor | Vendor::Acp => true,
        Vendor::Codex => false,
    }
}

/// Whether `vendor`'s agents take an effort level.
pub fn takes_effort(vendor: Vendor) -> bool {
    matches!(vendor, Vendor::Claude | Vendor::Codex)
}

/// Whether `vendor`'s CLI speaks the Agent Client Protocol.
pub fn speaks_acp(vendor: Vendor) -> bool {
    match vendor {
        Vendor::Cursor | Vendor::Acp => true,
        Vendor::Claude | Vendor::Codex => false,
    }
}

/// Whether turning full access on or off for one of `vendor`'s agents
/// takes a new process, as its flavor says.
pub fn full_access_relaunches(vendor: Vendor) -> bool {
    match vendor {
        Vendor::Cursor => cursor::Cursor.modes_are_launch_flags(),
        Vendor::Claude | Vendor::Codex | Vendor::Acp => false,
    }
}

/// A Claude session's history, read from its transcript.
struct Transcript(claude::History);

impl History for Transcript {
    fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        self.0.read()
    }

    fn limit(&mut self, bytes: Option<u64>) {
        self.0.limit(bytes);
    }

    fn account(&mut self, account: AccountId) {
        self.0.set_account(account);
    }
}

/// A Codex thread's history, read from its rollout.
struct Rollout(codex::History);

impl History for Rollout {
    fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        self.0.read()
    }

    fn account(&mut self, account: AccountId) {
        self.0.set_account(account);
    }
}

#[cfg(test)]
pub use unwired::Unwired;

#[cfg(test)]
mod unwired {
    use std::path::Path;
    use std::time::Instant;

    use maxhq_protocol::ids::{AgentId, DeviceId};
    use maxhq_protocol::message::ActionOutcome;
    use maxhq_protocol::model::{PermissionMode, Vendor};
    use uuid::Uuid;

    use super::{History, Vendors};
    use crate::drive::{
        Action, ActionKey, ChildExit, Driver, Effects, Framing, HeldLine, Launch, Login, Place,
    };

    /// Stands in for the drivers in the tests of the modules around the
    /// manager. Its drivers refuse every action, and it finds no history.
    pub struct Unwired;

    const NOT_WIRED: &str = "this build of maxd has no drivers wired in yet";

    impl Vendors for Unwired {
        fn claude_launch(
            &self,
            _login: &Login,
            place: &Place,
            _session: Uuid,
            _resume: bool,
            _model: Option<&str>,
            _mode: PermissionMode,
        ) -> Launch {
            unwired_launch(place.cwd.clone(), Framing::ClaudeStreamJson)
        }

        fn claude_session(
            &self,
            _login: &Login,
            _place: &Place,
            _agent: AgentId,
            _mode: PermissionMode,
        ) -> Box<dyn Driver> {
            Box::new(Refusing)
        }

        fn acp_launch(
            &self,
            _vendor: Vendor,
            _login: &Login,
            place: &Place,
            _mode: PermissionMode,
        ) -> Launch {
            unwired_launch(place.cwd.clone(), Framing::JsonRpc)
        }

        fn acp_session(
            &self,
            _vendor: Vendor,
            _login: &Login,
            _place: &Place,
            _agent: AgentId,
            _kept: crate::drive::acp::Kept,
            _mode: PermissionMode,
        ) -> Box<dyn Driver> {
            Box::new(Refusing)
        }

        fn codex_launch(&self, login: &Login) -> Launch {
            unwired_launch(login.config_dir.clone(), Framing::JsonRpc)
        }

        fn codex_server(&self, _login: &Login, _device: DeviceId) -> Box<dyn Driver> {
            Box::new(Refusing)
        }

        fn can_continue(&self, _vendor: Vendor, _config_dir: &Path, _session: &str) -> bool {
            false
        }

        fn terminal_cli(&self, _vendor: Vendor) -> &'static str {
            ""
        }

        fn history(
            &self,
            _vendor: Vendor,
            _config_dir: &Path,
            _session: &str,
            _subagent: Option<&str>,
            _agent: AgentId,
        ) -> Option<Box<dyn History>> {
            None
        }

        fn subagents(&self, _vendor: Vendor, _config_dir: &Path, _session: &str) -> Vec<String> {
            Vec::new()
        }
    }

    fn unwired_launch(cwd: std::path::PathBuf, framing: Framing) -> Launch {
        Launch {
            program: "false".into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd,
            framing,
            secret_env: Vec::new(),
            run_as: None,
        }
    }

    /// A driver that refuses everything.
    struct Refusing;

    impl Driver for Refusing {
        fn connected(&mut self, _fresh: bool, _pending: &[HeldLine], _fx: &mut Effects) {}

        fn line(&mut self, _line: &HeldLine, _fx: &mut Effects) {}

        fn act(&mut self, action: Action, fx: &mut Effects) {
            let reason = NOT_WIRED.to_owned();
            fx.results
                .push((action.key(), ActionOutcome::Refused { reason }));
        }

        fn exited(&mut self, _exit: &ChildExit, _fx: &mut Effects) {}

        fn tick(&mut self, _now: Instant, _fx: &mut Effects) {}
    }
}
