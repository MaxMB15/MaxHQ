// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the look reports: the plan's words for each step, which files show
//! that the original changed, when the old scripts' reboot fires, and the
//! checklist for after the last move.

use std::path::Path;

use maxhq_protocol::hosts::{MoveLook, MovePlan, MoveStep, PlannedStep};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::time::Timestamp;

use super::record::Found;
use crate::incus::moves::MOVE_SNAPSHOT;
use crate::observe::incus::look::SESSION_PATH;
use crate::posix_path;
use crate::recipe::WORK;
use crate::workspace::Repo;

/// The agent's home in the old containers.
const HOME: &str = "/home/agent";

/// The work folder's name in that home.
const WORK_NAME: &str = "work";

/// How long the old reboot waits after a boot: it fires at 06:00 once the
/// host has been up this long.
const REBOOT_UPTIME_HOURS: i64 = 60;

/// The hour the old reboot fires at.
const REBOOT_HOUR: i8 = 6;

/// What's left to do by hand once no original is left. `maxd` never does
/// any of it.
pub const CHECKLIST: [&str; 9] = [
    "Turn off the agent-workspaces-quiesce, agent-workspaces-resume and agent-workspaces-cgroup units.",
    "Set manage_parent = true in the host setup.",
    "Take quiesce-workspaces out of the scheduled reboot.",
    "Remove the agent-ws profile and the agent-creds and agent-shared volumes.",
    "Remove the agent-creds-vault volume once you're sure nothing needs it.",
    "Revoke the shared GitHub token on GitHub.",
    "Remove the ws-* include from the Mac's SSH config.",
    "Add any old shared skills and prompts you still want to the skills library. A moved agent no longer sees /opt/agent-shared.",
    "Check each moved copy for old sign-ins the move doesn't clean: .git-credentials, tokens in .git/config URLs, ~/.ssh, shell history and a Cursor sign-in.",
];

/// Where the original keeps the repo, which is where the old scripts kept
/// it. A session in the home folder itself says nothing about the repo.
pub fn repo_path(found: &Found) -> Option<String> {
    if let Some(cwd) = found
        .cwd
        .as_deref()
        .filter(|cwd| plain_path(cwd) && in_home(cwd).is_some_and(|inside| !inside.is_empty()))
    {
        return Some(cwd.to_owned());
    }
    repo_name(found).map(|name| format!("{HOME}/{name}"))
}

fn repo_name(found: &Found) -> Option<&str> {
    let repo = found.repo.as_deref()?.trim_end_matches('/');
    let name = repo.rsplit(['/', ':']).next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

/// Where the repo is in the copy, which is where its agent runs. MaxHQ's
/// guest works only in the work folder, so the converge moves the repos
/// there and leaves a link at each old path.
pub fn copy_path(found: &Found) -> Option<String> {
    let path = repo_path(found)?;
    let inside = in_home(&path)?;
    if inside.first() == Some(&WORK_NAME) {
        return Some(path);
    }
    Some(format!("{WORK}/{}", inside.join("/")))
}

/// The folder in the agent's home that holds the repo.
pub fn top_folder(found: &Found) -> Option<String> {
    let path = repo_path(found)?;
    in_home(&path)?.first().map(|first| (*first).to_owned())
}

/// The parts of a container path below the agent's home, or none when it
/// isn't there.
fn in_home(path: &str) -> Option<Vec<&str>> {
    posix_path::below(path, HOME)
}

/// Whether a folder the original names is absolute with no `..` in it. It
/// names a folder in a container, so Linux's rules judge it.
pub fn plain_path(path: &str) -> bool {
    posix_path::is_absolute(path) && !posix_path::climbs(path)
}

/// Whether a session ID the original names can go in a path.
pub fn plain_session(session: &str) -> bool {
    !session.is_empty()
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The files whose versions show the original changed since the snapshot:
/// its git HEAD, index and reflog, the session file and the Claude
/// transcript. Changes git doesn't see yet, such as an edit not staged,
/// can't show without a command in the original, which a move never runs.
pub fn marks(found: &Found) -> Vec<String> {
    let mut paths = vec![SESSION_PATH.to_owned()];
    for dir in git_folders(found) {
        for file in ["HEAD", "index", "logs/HEAD"] {
            paths.push(format!("{dir}/{file}"));
        }
    }
    if found.vendor == Some(Vendor::Claude)
        && let Some(session) = &found.session
    {
        // A session whose hook last wrote from the home folder keeps its
        // transcript under the repo's folder.
        let mut folders: Vec<String> = [found.cwd.clone(), repo_path(found)]
            .into_iter()
            .flatten()
            .map(|path| claude_folder(&path))
            .collect();
        folders.sort();
        folders.dedup();
        for folder in folders {
            paths.push(format!("{HOME}/.claude/projects/{folder}/{session}.jsonl"));
        }
    }
    paths
}

/// Where git can keep the state of the checkout a session runs in. It may be
/// below the repo's top, or a linked checkout, whose state is in the main
/// repo's `.git/worktrees`. A folder that isn't there has no version to change.
fn git_folders(found: &Found) -> Vec<String> {
    let top = top_folder(found);
    let mut folders: Vec<String> = repo_path(found)
        .into_iter()
        .chain(top.iter().map(|top| format!("{HOME}/{top}")))
        .chain(repo_name(found).map(|name| format!("{HOME}/{name}")))
        .map(|dir| format!("{dir}/.git"))
        .collect();
    if let (Some(name), Some(top)) = (repo_name(found), &top) {
        folders.push(format!("{HOME}/{name}/.git/worktrees/{top}"));
    }
    folders.sort();
    folders.dedup();
    folders
}

/// The folder Claude keeps a project's transcripts in, from its path.
pub fn claude_folder(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// When the old reboot can next fire, from the host's uptime.
pub fn next_reboot(uptime: &Path, now: Timestamp) -> Option<Timestamp> {
    let text = std::fs::read_to_string(uptime).ok()?;
    let seconds: f64 = text.split_whitespace().next()?.parse().ok()?;
    let now = jiff::Timestamp::from_millisecond(i64::try_from(now.0).ok()?).ok()?;
    let booted = now
        .checked_sub(jiff::SignedDuration::from_secs_f64(seconds))
        .ok()?;
    let earliest = booted
        .checked_add(jiff::SignedDuration::from_hours(REBOOT_UPTIME_HOURS))
        .ok()?
        .max(now);
    let zone = jiff::tz::TimeZone::system();
    let local = earliest.to_zoned(zone);
    let mut at = local
        .with()
        .hour(REBOOT_HOUR)
        .minute(0)
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .ok()?;
    if at < local {
        at = at.tomorrow().ok()?;
    }
    u64::try_from(at.timestamp().as_millisecond())
        .ok()
        .map(Timestamp)
}

/// The plan the look shows: what it found, and what each later step
/// changes.
pub struct Look<'a> {
    pub source: &'a str,
    pub target: &'a str,
    pub found: &'a Found,
    pub agent_state: Option<String>,
    pub profiles: Vec<String>,
    pub devices: Vec<String>,
    pub disk_used: Option<u64>,
    pub resume_flag: bool,
    pub next_reboot: Option<Timestamp>,
    /// The mode and account a resumed agent runs with, in words.
    pub runs_with: Option<String>,
}

impl Look<'_> {
    pub fn plan(self) -> MovePlan {
        let runs_with = self.runs_with.clone();
        let Look {
            source,
            target,
            found,
            ..
        } = self;
        let pool = found.pool.as_deref().unwrap_or("its pool");
        let repo = repo_path(found);
        let copy_repo = copy_path(found);
        let dropped: Vec<&str> = self
            .devices
            .iter()
            .map(String::as_str)
            .filter(|device| *device != "root" && *device != "eth0")
            .collect();
        let mut converge = Vec::new();
        if !self.profiles.is_empty() {
            converge.push(format!(
                "Takes {} off {target}.",
                listed("profile", &self.profiles.join(", "))
            ));
        }
        if !dropped.is_empty() {
            converge.push(format!(
                "Leaves out {}.",
                listed("device", &dropped.join(", "))
            ));
        }
        converge.extend([
            format!("Gives {target} its own disk on {pool}, a network card and the MaxHQ guest."),
            "Sets MaxHQ's keys, isolated ID maps, its caps and snapshots, and no start at boot."
                .to_owned(),
            "Starts it and removes the old token file, the old state hook, gh's stored token \
             and git helper, the links into /opt/agent-creds and /opt/agent-shared, and the \
             copied Claude and Codex sign-ins. Old shared skills and prompts don't come along."
                .to_owned(),
            "Copies git's name, email and default branch from /etc/gitconfig to the home \
             folder's .gitconfig, because MaxHQ's setup replaces that file."
                .to_owned(),
            format!(
                "Moves each repo in the home folder to {WORK}, where MaxHQ works, and leaves a \
                 link at its old path."
            ),
            "Runs MaxHQ's setup in it. No agent starts.".to_owned(),
        ]);
        // Anything running in the original can write its git config, so this
        // says what the copy gets, in the same parsed form the copy keeps.
        converge.push(match found.repo.as_deref().and_then(Repo::parse) {
            Some(repo) => format!(
                "Gives {target} GitHub write access to {}, the origin remote in {source}'s \
                 .git/config or the repo the old scripts named on it, and puts it in that \
                 repo's project. Anything running in {source} can change that file, so check \
                 it's the repo you expect.",
                repo.clone_url()
            ),
            None => format!(
                "Finds no GitHub repo in {source}'s origin remote, so {target} gets no GitHub \
                 access."
            ),
        });
        let resume = match (found.vendor, &found.session) {
            (Some(vendor), Some(session)) => format!(
                "Resumes the {} session {session} in {target}{}{}.",
                vendor_name(vendor),
                copy_repo
                    .as_deref()
                    .map(|dir| format!(", in {dir}"))
                    .unwrap_or_default(),
                runs_with
                    .as_deref()
                    .map(|words| format!(", {words}"))
                    .unwrap_or_default()
            ),
            _ => format!("Finds no session to resume, so no agent starts in {target}."),
        };
        let steps = vec![
            PlannedStep {
                step: MoveStep::Copy,
                changes: vec![
                    format!("Takes the snapshot {MOVE_SNAPSHOT} of {source}, without a restart."),
                    format!("Copies it to {target} on {pool}, stopped."),
                ],
            },
            PlannedStep {
                step: MoveStep::Converge,
                changes: converge,
            },
            PlannedStep {
                step: MoveStep::Switch,
                changes: vec![
                    format!("Waits until the agent in {source} is idle."),
                    format!("Stops {source} and keeps it from starting at boot."),
                    "Copies the session's history, your CLAUDE.md, AGENTS.md and settings, and \
                     each project's Claude memory to the account's folder."
                        .to_owned(),
                    resume,
                ],
            },
            PlannedStep {
                step: MoveStep::Keep,
                changes: vec![format!(
                    "Keeps {source} stopped for 7 days, so you can move back."
                )],
            },
            PlannedStep {
                step: MoveStep::Retire,
                changes: vec![format!(
                    "Shows any work in {source} that isn't saved, then deletes it and its \
                     {MOVE_SNAPSHOT} snapshot."
                )],
            },
        ];
        MovePlan {
            look: MoveLook {
                vendor: found.vendor,
                session: found.session.clone(),
                agent_state: self.agent_state,
                repo_path: repo,
                profiles: self.profiles,
                devices: self.devices,
                disk_used: self.disk_used,
                resume_flag: self.resume_flag,
                next_reboot: self.next_reboot,
            },
            steps,
        }
    }
}

/// "the profile a" or "the profiles a, b".
fn listed(what: &str, names: &str) -> String {
    if names.contains(", ") {
        format!("the {what}s {names}")
    } else {
        format!("the {what} {names}")
    }
}

/// A vendor's name as the card says it.
pub fn vendor_name(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "Claude",
        Vendor::Codex => "Codex",
        Vendor::Cursor => "Cursor",
        Vendor::Acp => "ACP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repo_goes_in_the_work_folder_in_the_copy() {
        let in_cwd = |cwd: &str| Found {
            cwd: Some(cwd.into()),
            ..Found::default()
        };
        let found = in_cwd("/home/agent/widgets/packages/app");
        assert_eq!(
            copy_path(&found).as_deref(),
            Some("/home/agent/work/widgets/packages/app")
        );
        assert_eq!(top_folder(&found).as_deref(), Some("widgets"));
        let found = in_cwd("/home/agent/work/widgets");
        assert_eq!(
            copy_path(&found).as_deref(),
            Some("/home/agent/work/widgets")
        );
        assert_eq!(top_folder(&found).as_deref(), Some("work"));
    }

    #[test]
    fn a_container_path_is_read_as_linux_reads_it() {
        let in_cwd = |cwd: &str| Found {
            cwd: Some(cwd.into()),
            ..Found::default()
        };
        // A backslash is part of a Linux name, not a separator.
        assert_eq!(
            copy_path(&in_cwd(r"/home/agent/odd\name")).as_deref(),
            Some(r"/home/agent/work/odd\name")
        );
        assert_eq!(top_folder(&in_cwd("/home/agentx/widgets")), None);
        assert!(!plain_path("/home/agent/../root"));
        assert!(!plain_path("home/agent/widgets"));
        assert!(plain_path("/home/agent/widgets"));
    }

    #[test]
    fn a_session_in_the_home_folder_finds_the_repo_from_its_remote() {
        let found = Found {
            cwd: Some("/home/agent".into()),
            repo: Some("example-user/widgets".into()),
            ..Found::default()
        };
        assert_eq!(repo_path(&found).as_deref(), Some("/home/agent/widgets"));
        assert_eq!(
            copy_path(&found).as_deref(),
            Some("/home/agent/work/widgets")
        );
    }

    #[test]
    fn the_repo_stays_where_the_old_scripts_kept_it() {
        let found = Found {
            cwd: Some("/home/agent/widgets".into()),
            ..Found::default()
        };
        assert_eq!(repo_path(&found).as_deref(), Some("/home/agent/widgets"));
        let found = Found {
            repo: Some("example-user/widgets".into()),
            ..Found::default()
        };
        assert_eq!(repo_path(&found).as_deref(), Some("/home/agent/widgets"));
    }

    #[test]
    fn a_claude_transcript_is_one_of_the_marks() {
        let found = Found {
            vendor: Some(Vendor::Claude),
            session: Some("5d0c".into()),
            cwd: Some("/home/agent/widgets".into()),
            ..Found::default()
        };
        let marks = marks(&found);
        assert!(marks.contains(&"/home/agent/widgets/.git/HEAD".to_owned()));
        assert!(marks
            .contains(&"/home/agent/.claude/projects/-home-agent-widgets/5d0c.jsonl".to_owned()));
    }

    #[test]
    fn the_marks_find_the_repo_from_wherever_the_session_runs() {
        let session_in = |cwd: &str| Found {
            vendor: Some(Vendor::Claude),
            session: Some("5d0c".into()),
            cwd: Some(cwd.into()),
            repo: Some("example-user/widgets".into()),
            ..Found::default()
        };
        let main = "/home/agent/widgets/.git/HEAD".to_owned();

        let below = marks(&session_in("/home/agent/widgets/app"));
        assert!(below.contains(&main), "{below:?}");

        let linked = marks(&session_in("/home/agent/widgets-two"));
        assert!(linked.contains(&main), "{linked:?}");
        assert!(
            linked.contains(&"/home/agent/widgets/.git/worktrees/widgets-two/index".to_owned()),
            "{linked:?}"
        );

        let home = marks(&session_in("/home/agent"));
        assert!(home.contains(&main), "{home:?}");
        assert!(
            home.contains(
                &"/home/agent/.claude/projects/-home-agent-widgets/5d0c.jsonl".to_owned()
            ),
            "{home:?}"
        );
    }

    #[test]
    fn the_reboot_waits_for_sixty_hours_of_uptime() {
        let dir = std::env::temp_dir().join(format!("maxd-uptime-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let uptime = dir.join("uptime");
        let now = Timestamp(1_800_000_000_000);
        std::fs::write(&uptime, "3600.00 7000.00\n").unwrap();
        let next = next_reboot(&uptime, now).unwrap();
        let hours = (next.0 - now.0) / 3_600_000;
        assert!((59..=83).contains(&hours), "{hours}");
        std::fs::write(&uptime, "720000.00 7000.00\n").unwrap();
        let next = next_reboot(&uptime, now).unwrap();
        assert!(next.0 - now.0 <= 24 * 3_600_000);
        assert!(next_reboot(&dir.join("none"), now).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
