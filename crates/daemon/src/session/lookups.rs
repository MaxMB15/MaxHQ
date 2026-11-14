// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The composer's lookups: the `@` path search, the `#` GitHub lookup and
//! an agent's excerpt. Each runs off the loop and answers the connection
//! that asked. They skip the action keys, since asking again only looks
//! again, and the app asks on every keystroke. The excerpt and the read
//! tool read a timeline the same way, which is here too.

use std::borrow::Borrow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use maxhq_core::excerpt::{self, Entry, Near, Whose};
use maxhq_protocol::find::GitItemSearch;
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ServerMessage};
use maxhq_protocol::model::{Vendor, WorkspaceLocation};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{ItemId, TimelineItem};
use tracing::warn;

use super::holders::HolderPlaces;
use super::places;
use super::reads::{read_history, Source};
use super::timeline::{Limits, Stored, Timeline};
use super::vendors::Vendors;
use super::{refused, Input, Manager};
use crate::drive::PlaceHost;
use crate::find;
use crate::guestlink::github_repo;
use crate::secrets;

/// Why a lookup names an agent this device can't find.
pub(super) const NO_AGENT: &str = "This device doesn't have that agent.";

/// Why a lookup names an agent whose folder is on another device.
const ELSEWHERE: &str = "That agent's folder is on another device.";

/// Why a `#` lookup has no repo to look in.
const NO_REMOTE: &str = "The agent's folder has no GitHub remote.";

/// Why a read names an agent that runs on another device.
const AWAY: &str = "That agent is on another device, so MaxHQ can't read it from here.";

/// Why a read finds no timeline to read.
const NO_HISTORY: &str = "MaxHQ can't read that agent's timeline.";

/// Why a read names an item the agent's timeline doesn't have.
const NO_ITEM: &str = "That agent's timeline has no item with that id. Give the id exactly \
     as the [Quote N] entry gives it.";

/// Where an agent's files are.
pub(super) struct Folder {
    /// The workspace it runs in.
    pub workspace: WorkspaceId,
    /// The folder its CLI runs in, on this device or in the container.
    pub cwd: PathBuf,
    /// Whose paths `cwd` is.
    pub host: PlaceHost,
}

impl Manager {
    /// Answers a lookup for the connection that asked.
    pub(super) fn found(&mut self, conn: u64, key: IdempotencyKey, outcome: ActionOutcome) {
        self.tell(conn, ServerMessage::ActionResult { key, outcome });
    }

    /// Where an agent's files are: a held agent's own folder, or its
    /// workspace's for one this device only sees. A sub-agent works in its
    /// lead's.
    pub(super) fn folder_of(&self, agent: AgentId) -> Result<Folder, String> {
        if let Some(held) = self.held.get(&self.lead_of(agent)) {
            let place = self.place(held);
            return Ok(Folder {
                workspace: place.workspace,
                cwd: place.cwd,
                host: place.host,
            });
        }
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return Err(NO_AGENT.to_owned());
        };
        let Some(Entity::Workspace(workspace)) =
            self.hub.get(&EntityKey::Workspace(found.workspace))
        else {
            return Err(NO_AGENT.to_owned());
        };
        if workspace.device != self.device {
            return Err(ELSEWHERE.to_owned());
        }
        Ok(match workspace.location {
            WorkspaceLocation::Folder { path } => Folder {
                workspace: workspace.id,
                cwd: PathBuf::from(path),
                host: PlaceHost::Local,
            },
            WorkspaceLocation::Container { path, .. } => Folder {
                workspace: workspace.id,
                cwd: PathBuf::from(path),
                host: PlaceHost::Guest {
                    workspace: workspace.id,
                },
            },
        })
    }

    /// Answers `FindPaths`: walks the agent's folder where it is, on this
    /// device or through its container's guest.
    pub(super) fn find_paths(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        query: String,
    ) {
        let folder = match self.folder_of(agent) {
            Ok(folder) => folder,
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let places = self.places.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match folder.host {
                PlaceHost::Local => {
                    let cwd = folder.cwd;
                    tokio::task::spawn_blocking(move || find::find_paths(&cwd, &query))
                        .await
                        .unwrap_or_else(|err| Err(format!("the search stopped: {err}")))
                }
                PlaceHost::Guest { workspace } => {
                    let cwd = folder.cwd.to_string_lossy().into_owned();
                    places.find_paths(workspace, cwd, query).await
                }
            };
            let outcome = match result {
                Ok(matches) => ActionOutcome::Paths { matches },
                Err(reason) => refused(format!("Couldn't search the agent's folder: {reason}")),
            };
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(outcome),
            });
        });
    }

    /// The GitHub repo of a workspace's project, as its settings name it.
    pub(super) fn project_repo(&self, workspace: WorkspaceId) -> Option<String> {
        let Some(Entity::Workspace(workspace)) = self.hub.get(&EntityKey::Workspace(workspace))
        else {
            return None;
        };
        let Some(Entity::Project(project)) = self.hub.get(&EntityKey::Project(workspace.project?))
        else {
            return None;
        };
        project.repo
    }

    /// Answers `FindGitItems`: looks in the GitHub repo of the agent's
    /// folder, which its `origin` remote names on this device, and its
    /// project's settings name for a container.
    pub(super) fn find_git_items(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        query: String,
    ) {
        let folder = match self.folder_of(agent) {
            Ok(folder) => folder,
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let project = self.project_repo(folder.workspace);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let remote = match folder.host {
                PlaceHost::Local => {
                    let cwd = folder.cwd;
                    tokio::task::spawn_blocking(move || {
                        maxhq_core::git::find_checkout(&cwd).and_then(|checkout| checkout.remote)
                    })
                    .await
                    .ok()
                    .flatten()
                }
                // The guest's checkout is its project's.
                PlaceHost::Guest { .. } => None,
            };
            let repo = [remote.as_deref(), project.as_deref()]
                .into_iter()
                .flatten()
                .find_map(github_repo);
            let search = match repo {
                Some(repo) => secrets::find_git_items(&repo, &query).await,
                None => GitItemSearch {
                    repo: None,
                    items: Vec::new(),
                    unavailable: Some(
                        if remote.is_some() || project.is_some() {
                            secrets::NOT_GITHUB
                        } else {
                            NO_REMOTE
                        }
                        .to_owned(),
                    ),
                },
            };
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(ActionOutcome::GitItems { search }),
            });
        });
    }

    /// Answers `ExcerptAgent`: the agent's timeline as Markdown, cut to
    /// the budget.
    pub(super) fn excerpt_agent(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        budget: u32,
    ) {
        let (named, from) = match self.reading(agent, &Take::All) {
            Ok(reading) => reading,
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = match from.taken(agent).await {
                Ok(taken) => ActionOutcome::Excerpt {
                    excerpt: excerpt::excerpt(named.whose(), &taken.entries(), budget as usize),
                },
                Err(reason) => refused(reason),
            };
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(outcome),
            });
        });
    }

    /// What names an agent, and where its timeline is read: the one this
    /// device keeps, or else its session's history. `what` says how much of
    /// it the read takes.
    pub(super) fn reading(&self, agent: AgentId, what: &Take) -> Result<(Named, ReadFrom), String> {
        let (named, located) = self.timeline_at(agent)?;
        let from = match located {
            Located::Kept(timeline) => ReadFrom::Kept {
                items: timeline.stored().map(Stored::into_owned).collect(),
                what: what.clone(),
            },
            Located::History(source) => ReadFrom::History {
                source,
                what: what.clone(),
                vendors: self.vendors.clone(),
                places: self.places.clone(),
            },
        };
        Ok((named, from))
    }

    /// What names an agent, and where its timeline is: the one this device
    /// keeps, or else its session's history. A timeline that's still
    /// filling, or empty while there's history, is read from the history.
    /// A sub-agent has only the timeline its lead's driver keeps.
    pub(super) fn timeline_at(&self, agent: AgentId) -> Result<(Named, Located<'_>), String> {
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return Err(NO_AGENT.to_owned());
        };
        let named = Named {
            agent,
            title: found.title.clone(),
            vendor: found.vendor,
        };
        let mut why = NO_HISTORY;
        let source = if found.parent.is_some() {
            None
        } else if let Some(held) = self.held.get(&agent) {
            Some(places::source(held))
        } else if self.device_of(found.workspace) != Some(self.device) {
            why = AWAY;
            None
        } else {
            self.outside_source(agent)
        };
        if let Some(timeline) = self.timelines.get(&agent)
            && !self.filling(agent)
            && (source.is_none() || !timeline.is_empty())
        {
            return Ok((named, Located::Kept(timeline)));
        }
        match source {
            Some(source) => Ok((named, Located::History(source))),
            None => Err(why.to_owned()),
        }
    }

    /// The device a workspace is on.
    fn device_of(&self, workspace: WorkspaceId) -> Option<DeviceId> {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(workspace)) => Some(workspace.device),
            _ => None,
        }
    }
}

/// The agent a read is for, as its heading names it.
pub(super) struct Named {
    /// The agent.
    pub agent: AgentId,
    /// Its title now.
    pub title: String,
    /// Its CLI.
    pub vendor: Vendor,
}

impl Named {
    /// The heading's names, for the renderer.
    pub fn whose(&self) -> Whose<'_> {
        Whose {
            agent: self.agent,
            title: &self.title,
            vendor: self.vendor,
        }
    }
}

/// How much of an agent's timeline a read takes.
#[derive(Clone)]
pub(super) enum Take {
    /// All of it.
    All,
    /// One item, alone and whole.
    Item(ItemId),
    /// One item whole, and the entries on each side of it.
    Around(ItemId),
}

impl Take {
    /// The one item the read is for, if it's for one.
    pub fn item(&self) -> Option<&ItemId> {
        match self {
            Take::All => None,
            Take::Item(item) | Take::Around(item) => Some(item),
        }
    }
}

/// What a read took of an agent's timeline.
pub(super) enum Taken {
    /// Every entry, oldest first.
    All(Vec<Entry>),
    /// The one item's entry, or none for an item that shows nothing.
    Item(Option<Entry>),
    /// The one item and the entries on each side of it.
    Around(Near),
}

impl Taken {
    /// Every entry it holds, oldest first.
    pub fn entries(self) -> Vec<Entry> {
        match self {
            Taken::All(entries) => entries,
            Taken::Item(entry) => entry.into_iter().collect(),
            Taken::Around(near) => near
                .before
                .into_iter()
                .chain(near.item)
                .chain(near.after)
                .collect(),
        }
    }
}

/// Where an agent's timeline is.
pub(super) enum Located<'a> {
    /// The timeline this device keeps.
    Kept(&'a Timeline),
    /// The session's history, which is read off the loop.
    History(Source),
}

/// Where a read gets an agent's timeline.
pub(super) enum ReadFrom {
    /// The timeline this device keeps, whose older items are on disk, so
    /// it's read off the loop too.
    Kept {
        items: Vec<Stored<'static>>,
        /// How much of it the read takes.
        what: Take,
    },
    /// The session's history, which is read off the loop.
    History {
        source: Source,
        /// How much of it the read takes.
        what: Take,
        vendors: Arc<dyn Vendors>,
        places: Arc<dyn HolderPlaces>,
    },
}

impl ReadFrom {
    /// What the read takes, read off the loop.
    pub async fn taken(self, agent: AgentId) -> Result<Taken, String> {
        match self {
            ReadFrom::Kept { items, what } => {
                let read = tokio::task::spawn_blocking(move || {
                    let items = items.into_iter().filter_map(|stored| {
                        stored
                            .load()
                            .inspect_err(
                                |err| warn!(%err, %agent, "couldn't read a timeline item back"),
                            )
                            .ok()
                    });
                    take(items, &what)
                });
                read.await.unwrap_or_else(|_| Err(NO_HISTORY.to_owned()))
            }
            ReadFrom::History {
                source,
                what,
                vendors,
                places,
            } => take(&history(vendors, places, source, agent).await?, &what),
        }
    }
}

/// The items of a session's history, read off the loop, once each and
/// numbered in order.
pub(super) async fn history(
    vendors: Arc<dyn Vendors>,
    places: Arc<dyn HolderPlaces>,
    source: Source,
    agent: AgentId,
) -> Result<Vec<TimelineItem>, String> {
    // A lookup holds what it reads at once, so it reads no more than an
    // agent keeps by default.
    let bytes = Limits::default().disk;
    let read = tokio::task::spawn_blocking(move || {
        read_history(&*vendors, &*places, &source, agent, bytes)
    })
    .await;
    match read {
        Ok(Some(Ok(items))) => Ok(numbered(items)),
        Ok(Some(Err(err))) => {
            warn!(%agent, "couldn't read the history to show: {err}");
            Err(NO_HISTORY.to_owned())
        }
        Ok(None) | Err(_) => Err(NO_HISTORY.to_owned()),
    }
}

/// What a read takes of `items`: all their entries, or one item's alone
/// and whole, which is none for an item that shows nothing, or that item
/// with the entries on each side of it.
fn take(
    items: impl IntoIterator<Item = impl Borrow<TimelineItem>>,
    what: &Take,
) -> Result<Taken, String> {
    let Some(item) = what.item() else {
        return Ok(Taken::All(excerpt::entries(items)));
    };
    let items: Vec<_> = items.into_iter().collect();
    let items: Vec<&TimelineItem> = items.iter().map(Borrow::borrow).collect();
    let Some(at) = items.iter().position(|one| one.id == *item) else {
        return Err(NO_ITEM.to_owned());
    };
    Ok(match what {
        Take::Around(_) => Taken::Around(excerpt::near(&items, at)),
        Take::All | Take::Item(_) => Taken::Item(excerpt::lone(items[at])),
    })
}

/// History items once each, numbered in order the way a timeline numbers
/// them, so a page names the same item each time it's read.
fn numbered(items: Vec<TimelineItem>) -> Vec<TimelineItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .zip(0..)
        .map(|(mut item, ordinal)| {
            item.ordinal = ordinal;
            item
        })
        .collect()
}
