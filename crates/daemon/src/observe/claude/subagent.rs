// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The sub-agents a session started. Each one is an `agent-<id>.meta.json`
//! with its transcript, `agent-<id>.jsonl`, beside it in the `subagents`
//! folder next to the lead's transcript.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use maxhq_protocol::time::Timestamp;
use serde_json::Value;

/// The first version that marks a sub-agent `finished` when it ends. Before
/// it, an ended sub-agent looks the same as a running one.
const FINISHED_SINCE: (u32, u32, u32) = (2, 1, 301);

/// What a sub-agent's meta file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// The kind of agent, such as `general-purpose`.
    pub agent_type: Option<String>,
    /// The short task the lead gave it.
    pub description: Option<String>,
    /// The sub-agent that started this one, for one started by another
    /// sub-agent.
    pub parent_agent_id: Option<String>,
    /// The worktree it runs in, when it has its own.
    pub worktree_path: Option<String>,
    /// Set once it ends.
    pub finished: bool,
    /// Set when you stopped it.
    pub stopped_by_user: bool,
}

impl Meta {
    /// Reads a meta file's JSON leniently, so a field this doesn't know or
    /// can't read doesn't hide the sub-agent.
    pub fn parse(json: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(json).ok()?;
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
        };
        // Any value but `false` or `null` counts, in case a later version
        // writes a time instead of `true`.
        let set = |key: &str| {
            value
                .get(key)
                .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
        };
        Some(Self {
            agent_type: text("agentType"),
            description: text("description"),
            parent_agent_id: text("parentAgentId"),
            worktree_path: text("worktreePath"),
            finished: set("finished"),
            stopped_by_user: set("stoppedByUser"),
        })
    }

    /// Whether the meta says it has ended.
    pub fn ended(&self) -> bool {
        self.finished || self.stopped_by_user
    }
}

/// One sub-agent on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subagent {
    /// The ID Claude Code gives it within the lead's session, such as
    /// `a1b2c3d4e5f60718`.
    pub id: String,
    /// What its meta file says.
    pub meta: Meta,
    /// Its transcript, which may not exist yet.
    pub transcript: PathBuf,
    /// When its meta file was created.
    pub created_at: Option<Timestamp>,
    /// When it last wrote to its transcript, or its meta file if there's no
    /// transcript yet.
    pub touched_at: Option<Timestamp>,
}

impl Subagent {
    /// Whether it runs, given that its lead runs and started at
    /// `lead_started`. One the meta doesn't mark as ended still has to have
    /// written something since the lead started, since a sub-agent that was
    /// running when an earlier process for the same session died never gets
    /// marked.
    pub fn runs(&self, lead_started: Timestamp) -> bool {
        !self.meta.ended() && self.touched_at.is_some_and(|at| at >= lead_started)
    }
}

/// Whether a Claude Code version marks ended sub-agents. A version that
/// can't be read is taken to be a current one.
pub fn marks_finished(version: Option<&str>) -> bool {
    let Some(version) = version else {
        return true;
    };
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u32>().ok());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some(major)), Some(Some(minor)), Some(Some(patch))) => {
            (major, minor, patch) >= FINISHED_SINCE
        }
        _ => true,
    }
}

/// Reads `subagents` folders, keeping each meta file until it changes, and
/// never reading one again once it says the sub-agent ended.
#[derive(Debug, Default)]
pub struct Metas {
    /// Each meta file read, with its modification time then.
    cache: HashMap<PathBuf, (Option<SystemTime>, Meta)>,
}

impl Metas {
    /// The sub-agents in `dir` whose meta files don't say they ended.
    pub fn open(&mut self, dir: &Path) -> Vec<Subagent> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_prefix("agent-"))
                .and_then(|name| name.strip_suffix(".meta.json"))
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let path = entry.path();
            if self.cache.get(&path).is_some_and(|(_, meta)| meta.ended()) {
                continue;
            }
            let Ok(info) = entry.metadata() else {
                continue;
            };
            let modified = info.modified().ok();
            let meta = match self.cache.get(&path) {
                Some((at, meta)) if *at == modified => meta.clone(),
                _ => {
                    let Some(meta) = fs::read(&path).ok().and_then(|json| Meta::parse(&json))
                    else {
                        continue;
                    };
                    self.cache.insert(path.clone(), (modified, meta.clone()));
                    meta
                }
            };
            if meta.ended() {
                continue;
            }
            let transcript = dir.join(format!("agent-{id}.jsonl"));
            let touched = fs::metadata(&transcript)
                .and_then(|info| info.modified())
                .ok()
                .or(modified);
            found.push(Subagent {
                id: id.to_owned(),
                meta,
                transcript,
                created_at: info.created().ok().or(modified).map(Timestamp::from),
                touched_at: touched.map(Timestamp::from),
            });
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Forgets meta files outside the folders `keep` accepts.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        self.cache
            .retain(|path, _| path.parent().is_some_and(&keep));
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;

    use super::*;
    use crate::observe::claude::tests::{fixture, scratch};

    const DIR: &str = "config/projects/-work-maxhq/6f1d2c3b-0a4e-4b8f-9c11-2d3e4f5a6b7c/subagents";

    #[test]
    fn reads_a_meta_file() {
        let path = fixture(DIR).join("agent-a1f00d0000000001.meta.json");
        let meta = Meta::parse(&fs::read(path).unwrap()).unwrap();
        assert_eq!(meta.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(meta.description.as_deref(), Some("Map the session files"));
        assert!(!meta.finished && !meta.stopped_by_user);

        let nested = fixture(DIR).join("agent-a1f00d0000000004.meta.json");
        let nested = Meta::parse(&fs::read(nested).unwrap()).unwrap();
        assert_eq!(nested.parent_agent_id.as_deref(), Some("a1f00d0000000001"));
        assert_eq!(nested.worktree_path.as_deref(), Some("/work/maxhq-wt"));
    }

    #[test]
    fn finished_and_stopped_sub_agents_have_ended() {
        let mut metas = Metas::default();
        let open = metas.open(&fixture(DIR));
        let ids: Vec<&str> = open.iter().map(|s| s.id.as_str()).collect();
        // 2 finished and 3 was stopped by you.
        assert_eq!(ids, ["a1f00d0000000001", "a1f00d0000000004"]);
    }

    #[test]
    fn a_sub_agent_runs_only_if_it_wrote_since_the_lead_started() {
        let dir = scratch();
        for file in [
            "agent-a1f00d0000000001.meta.json",
            "agent-a1f00d0000000001.jsonl",
        ] {
            fs::copy(fixture(DIR).join(file), dir.join(file)).unwrap();
        }
        let started = Timestamp::now();
        let old = SystemTime::now() - Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(dir.join("agent-a1f00d0000000001.jsonl"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut metas = Metas::default();
        let found = metas.open(&dir);
        assert_eq!(found.len(), 1);
        assert!(!found[0].runs(started), "left over from an earlier process");
        assert!(found[0].runs(Timestamp::from(old - Duration::from_secs(1))));
    }

    #[test]
    fn a_meta_marked_finished_later_is_read_again() {
        let dir = scratch();
        let meta = dir.join("agent-a9.meta.json");
        fs::write(&meta, r#"{"agentType":"Explore","description":"look"}"#).unwrap();
        let mut metas = Metas::default();
        assert_eq!(metas.open(&dir).len(), 1);

        fs::write(
            &meta,
            r#"{"agentType":"Explore","description":"look","finished":true}"#,
        )
        .unwrap();
        // Make sure the change shows in the modification time.
        File::options()
            .write(true)
            .open(&meta)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        assert!(metas.open(&dir).is_empty());
    }

    #[test]
    fn only_newer_versions_mark_finished() {
        assert!(marks_finished(Some("2.1.301")));
        assert!(marks_finished(Some("2.1.310")));
        assert!(marks_finished(Some("3.0.0")));
        assert!(!marks_finished(Some("2.1.300")));
        assert!(!marks_finished(Some("2.0.999")));
        assert!(marks_finished(Some("next")));
        assert!(marks_finished(None));
    }
}
