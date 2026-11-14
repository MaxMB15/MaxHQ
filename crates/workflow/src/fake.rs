// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Fakes of [`Files`] and [`Agents`], for this crate's tests and for any
//! crate that tests against the engine.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::time::Timestamp;

use crate::inputs::{AgentFacts, AgentState, Agents, Entry, Files, Stat};

/// Files held in memory. Folders exist wherever a file is under them.
#[derive(Debug, Default)]
pub struct MemFiles {
    files: BTreeMap<PathBuf, (Vec<u8>, Timestamp)>,
    alive: BTreeSet<u32>,
    reads: Cell<usize>,
    clock: u64,
}

impl MemFiles {
    /// Loads every file under a real folder, at the same paths.
    pub fn from_dir(dir: &Path) -> io::Result<Self> {
        let mut files = Self::default();
        let mut todo = vec![dir.to_owned()];
        while let Some(dir) = todo.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    todo.push(path);
                } else {
                    let bytes = std::fs::read(&path)?;
                    files.put(path, bytes);
                }
            }
        }
        Ok(files)
    }

    /// Writes a whole file, as a writer replacing it would.
    pub fn put(&mut self, path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) {
        self.clock += 1;
        self.files
            .insert(path.into(), (contents.into(), Timestamp(self.clock)));
    }

    /// Adds to the end of a file, making it if needed.
    pub fn append(&mut self, path: impl Into<PathBuf>, contents: impl AsRef<[u8]>) {
        self.clock += 1;
        let entry = self
            .files
            .entry(path.into())
            .or_insert_with(|| (Vec::new(), Timestamp(0)));
        entry.0.extend_from_slice(contents.as_ref());
        entry.1 = Timestamp(self.clock);
    }

    /// Removes a file.
    pub fn remove(&mut self, path: impl AsRef<Path>) {
        self.files.remove(path.as_ref());
    }

    /// Says whether a pid runs.
    pub fn set_alive(&mut self, pid: u32, alive: bool) {
        if alive {
            self.alive.insert(pid);
        } else {
            self.alive.remove(&pid);
        }
    }

    /// How many reads have been made, whole or from an offset.
    pub fn reads(&self) -> usize {
        self.reads.get()
    }

    fn get(&self, path: &Path) -> io::Result<&(Vec<u8>, Timestamp)> {
        self.files
            .get(path)
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}

impl Files for MemFiles {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        self.get(path).map(|(bytes, _)| bytes.clone())
    }

    fn read_from(&self, path: &Path, offset: u64, max: usize) -> io::Result<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        let (bytes, _) = self.get(path)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        Ok(bytes[start..bytes.len().min(start.saturating_add(max))].to_vec())
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let (bytes, modified) = self.get(path)?;
        Ok(Stat {
            size: bytes.len() as u64,
            modified: *modified,
        })
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let mut entries: BTreeMap<String, bool> = BTreeMap::new();
        for path in self.files.keys() {
            let Ok(rest) = path.strip_prefix(dir) else {
                continue;
            };
            let mut parts = rest.components();
            if let Some(first) = parts.next() {
                let dir = parts.next().is_some();
                let name = first.as_os_str().to_string_lossy().into_owned();
                *entries.entry(name).or_default() |= dir;
            }
        }
        if entries.is_empty() {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(entries
            .into_iter()
            .map(|(name, dir)| Entry { name, dir })
            .collect())
    }

    fn alive(&self, pid: u32) -> bool {
        self.alive.contains(&pid)
    }
}

/// A tree of agents, built up by hand.
#[derive(Debug)]
pub struct FakeAgents {
    /// The time [`Agents::now`] gives.
    pub now: Timestamp,
    agents: Vec<AgentFacts>,
    prompts: BTreeMap<AgentId, String>,
    commands: BTreeMap<AgentId, Vec<String>>,
}

impl Default for FakeAgents {
    fn default() -> Self {
        Self {
            now: Timestamp(0),
            agents: Vec::new(),
            prompts: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }
}

impl FakeAgents {
    /// Adds a busy agent with a title, under a lead or not.
    pub fn add(&mut self, title: &str, lead: Option<AgentId>) -> AgentId {
        let id = AgentId::derive(&format!("fake/{}", self.agents.len()));
        self.agents.push(AgentFacts {
            id,
            lead,
            title: Some(title.to_owned()),
            pid: None,
            state: AgentState::Busy,
            started: Some(Timestamp(self.agents.len() as u64)),
            last_write: None,
            tool_open: false,
        });
        id
    }

    fn facts(&mut self, id: AgentId) -> &mut AgentFacts {
        self.agents
            .iter_mut()
            .find(|f| f.id == id)
            .expect("an agent added before")
    }

    /// Takes an agent off the list, as when it leaves.
    pub fn remove(&mut self, id: AgentId) {
        self.agents.retain(|f| f.id != id);
    }

    /// Sets how an agent stands.
    pub fn state(&mut self, id: AgentId, state: AgentState) {
        self.facts(id).state = state;
    }

    /// Sets an agent's pid.
    pub fn pid(&mut self, id: AgentId, pid: u32) {
        self.facts(id).pid = Some(pid);
    }

    /// Sets when an agent started.
    pub fn started(&mut self, id: AgentId, ms: u64) {
        self.facts(id).started = Some(Timestamp(ms));
    }

    /// Sets when an agent last wrote, and whether a tool call is open.
    pub fn wrote(&mut self, id: AgentId, ms: u64, tool_open: bool) {
        let facts = self.facts(id);
        facts.last_write = Some(Timestamp(ms));
        facts.tool_open = tool_open;
    }

    /// Sets an agent's first prompt.
    pub fn prompt(&mut self, id: AgentId, text: &str) {
        self.prompts.insert(id, text.to_owned());
    }

    /// Adds a finished command to an agent.
    pub fn command(&mut self, id: AgentId, text: &str) {
        self.commands.entry(id).or_default().push(text.to_owned());
    }
}

impl Agents for FakeAgents {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn list(&self) -> Vec<AgentFacts> {
        self.agents.clone()
    }

    fn prompt_head(&self, agent: AgentId) -> Option<String> {
        self.prompts.get(&agent).cloned()
    }

    fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str)) {
        for command in self.commands.get(&agent).into_iter().flatten() {
            visit(command);
        }
    }
}
