// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Running the calls agents make to MaxHQ's own tools. A driver names a
//! call in its effects, the manager runs it off the loop, and its outcome
//! goes back to the same driver, which answers the CLI.

use maxhq_core::attached::clean_name;
use maxhq_core::excerpt;
use maxhq_core::quote_span::Quote;
use maxhq_core::tools::{
    stretches, visual_name, Call, Open, Opened, Read, Send, Show, Target, Visual,
};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{AgentId, SessionId};
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemId};

use super::agent_tools::{Later, Reply};
use super::lookups::{Take, Taken};
use super::{Input, Manager};
use crate::drive::{Effects, ToolCall, ToolDone, ToolResult};

/// What the agent reads when `maxd` no longer holds it.
const UNKNOWN_AGENT: &str = "MaxHQ doesn't hold this agent any more.";

/// Why the read tool refuses an agent the user didn't attach or quote.
const NOT_ATTACHED: &str = "You can read only the agents the user attached to one of your \
     messages and the items the user quoted in them. None of those agents has that id or \
     title. Ask the user to attach the agent.";

/// Why a read under a permission can't find the agent it names.
const NOT_AN_ID: &str = "Name the agent by the id list_agents gives.";

/// Why the read tool can't tell which attached or quoted agent a title
/// means.
const SAME_TITLE: &str =
    "More than one attached or quoted agent has that title. Name it by its id.";

/// Why the read tool refuses a page of an agent the user only quoted.
const QUOTED_PAGE: &str =
    "The user quoted this agent but didn't attach it, so you can't read its timeline by page.";

/// Why the show tool refuses a `replaces` that names no visual it showed.
const NOT_A_VISUAL: &str = "`replaces` names no visual you showed in this conversation. Give \
     the id an earlier show_visual call returned, or leave `replaces` out to show a new one.";

/// Why the read tool refuses an item the user didn't quote, of an agent
/// the user only quoted.
const NOT_QUOTED: &str =
    "The user quoted other items of this agent, not that one, and didn't attach it.";

/// What the read tool lets the caller read of an agent the user only
/// quoted. The quoted items follow it.
const QUOTED_ITEMS: &str =
    "You can read each item the user quoted with `item`, and the messages around it with `around`:";

/// How the caller gets to read more of an agent the user only quoted.
const ATTACH_FOR_MORE: &str = "To read more of the agent, ask the user to attach it.";

impl Manager {
    /// Starts a tool call from `session`'s driver. The dispatcher's calls
    /// run in [`super::dispatcher`], a read of what the user attached and a
    /// send of files run off the loop here, and the agent tools run in
    /// [`super::agent_tools`].
    pub(super) fn run_tool(&mut self, session: SessionId, call: ToolCall) {
        if self.is_dispatcher(call.agent) {
            return self.dispatcher_tool(session, call);
        }
        match call.call {
            Call::SendAttachments(send) => {
                self.send_attachments(session, call.key, call.call_id, call.agent, send);
            }
            Call::OpenFile(open) => self.open_file(session, call.key, call.agent, open),
            Call::ShowVisual(show) => {
                self.show_visual(session, call.key, call.call_id, call.agent, show);
            }
            Call::ReadAgent(read) if !self.reads_widely(call.agent, &read) => {
                let reply = Reply::Driver {
                    session,
                    key: call.key,
                };
                self.read_agent(reply, Some(call.agent), read, false);
            }
            _ => self.agent_tool(session, call),
        }
    }

    /// Copies the files a send names and shows them on the timeline.
    fn send_attachments(
        &self,
        session: SessionId,
        key: String,
        call_id: String,
        agent: AgentId,
        send: Send,
    ) {
        let tx = self.tx.clone();
        // A sub-agent works in its lead's folders.
        let lead = self.lead_of(agent);
        let Some(place) = self.held.get(&lead).map(|held| self.place(held)) else {
            let done = ToolDone {
                key,
                result: Err(UNKNOWN_AGENT.to_owned()),
            };
            let _ = tx.send(Input::ToolDone { session, done });
            return;
        };
        let files = self.files.clone();
        tokio::spawn(async move {
            let result = match send.to {
                // Sending to another agent's timeline goes here, once
                // there's a target for it.
                Target::Own => files
                    .collect(&place, &call_id, send.paths)
                    .await
                    .map(ToolResult::Sent),
            };
            let _ = tx.send(Input::ToolDone {
                session,
                done: ToolDone { key, result },
            });
        });
    }

    /// Copies the page a show names into the agent's attachments, so its
    /// card can go on the timeline. A `replaces` must name a visual the
    /// agent, or its lead, showed before.
    fn show_visual(
        &self,
        session: SessionId,
        key: String,
        call_id: String,
        agent: AgentId,
        show: Show,
    ) {
        let tx = self.tx.clone();
        // A sub-agent works in its lead's folders.
        let lead = self.lead_of(agent);
        let place = self.held.get(&lead).map(|held| self.place(held));
        let checked = match (place, &show.replaces) {
            (None, _) => Err(UNKNOWN_AGENT.to_owned()),
            (Some(_), Some(id)) if !self.showed(agent, lead, id) => Err(NOT_A_VISUAL.to_owned()),
            (Some(place), _) => Ok(place),
        };
        let place = match checked {
            Ok(place) => place,
            Err(message) => {
                let done = ToolDone {
                    key,
                    result: Err(message),
                };
                let _ = tx.send(Input::ToolDone { session, done });
                return;
            }
        };
        let files = self.files.clone();
        tokio::spawn(async move {
            let from_file = matches!(show.page, maxhq_core::tools::Page::Path(_));
            let name = visual_name(&show.title);
            let result =
                files
                    .show(&place, &call_id, show.page, name)
                    .await
                    .map(|(attachment, text)| {
                        ToolResult::Visual(Visual {
                            title: show.title,
                            attachment,
                            replaces: show.replaces.map(ItemId),
                            from_file,
                            stretches: stretches(&text),
                        })
                    });
            let _ = tx.send(Input::ToolDone {
                session,
                done: ToolDone { key, result },
            });
        });
    }

    /// Whether `id` names a visual on the timeline of `agent` or its lead.
    fn showed(&self, agent: AgentId, lead: AgentId, id: &str) -> bool {
        let id = ItemId(id.to_owned());
        [agent, lead]
            .iter()
            .filter_map(|whose| self.timelines.get(whose))
            .filter_map(|timeline| timeline.get(&id))
            .any(|item| matches!(item.body, ItemBody::AgentVisual { .. }))
    }

    /// Checks the file an open names, so its card can go on the timeline.
    /// The file itself is never read.
    fn open_file(&self, session: SessionId, key: String, agent: AgentId, open: Open) {
        let tx = self.tx.clone();
        // A sub-agent works in its lead's folders.
        let lead = self.lead_of(agent);
        let Some(place) = self.held.get(&lead).map(|held| self.place(held)) else {
            let done = ToolDone {
                key,
                result: Err(UNKNOWN_AGENT.to_owned()),
            };
            let _ = tx.send(Input::ToolDone { session, done });
            return;
        };
        let files = self.files.clone();
        tokio::spawn(async move {
            let result = files.look(&place, open.path).await.map(|looked| {
                ToolResult::Opened(Opened {
                    path: looked.path,
                    line: open.line,
                    outside: looked.outside,
                })
            });
            let _ = tx.send(Input::ToolDone {
                session,
                done: ToolDone { key, result },
            });
        });
    }

    /// Reads a page of another agent's timeline, if the user attached
    /// that agent to one of the caller's messages, or of the caller's
    /// own. Reads one item whole, with the messages around it when the
    /// call asks, if the user attached the agent or quoted that item. A
    /// `wide` read, under a permission to act on this device, may also
    /// read any agent here by its id.
    pub(super) fn read_agent(&self, reply: Reply, caller: Option<AgentId>, read: Read, wide: bool) {
        let what = match (&read.item, read.around) {
            (None, _) => Take::All,
            (Some(item), None) => Take::Item(ItemId(item.clone())),
            (Some(item), Some(_)) => Take::Around(ItemId(item.clone())),
        };
        let attached = match caller {
            Some(agent) if self.held.contains_key(&self.lead_of(agent)) => {
                self.attached(agent, &read.agent, what.item())
            }
            _ => Err(UNKNOWN_AGENT.to_owned()),
        };
        let target = match attached {
            Err(_) if wide => unmarked(&read.agent)
                .parse::<AgentId>()
                .map(|id| (id, true, Vec::new()))
                .map_err(|_| NOT_AN_ID.to_owned()),
            attached => attached,
        };
        let reading = target.and_then(|(target, pageable, quotes)| {
            let quote = match (what.item(), read.position) {
                (Some(item), Some(position)) => Some(starting(&quotes, item, position)?),
                _ => None,
            };
            Ok((self.reading(target, &what)?, pageable, quotes, quote))
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match reading {
                Ok(((named, from), pageable, quotes, quote)) => {
                    from.taken(named.agent).await.map(|taken| {
                        let whose = named.whose();
                        let id = read.item.as_deref().unwrap_or_default();
                        ToolResult::Text(match taken {
                            Taken::All(entries) => {
                                excerpt::page(whose, &entries, read.before, read.limit)
                            }
                            Taken::Item(entry) => match &quote {
                                Some(quote) => excerpt::window(
                                    whose,
                                    id,
                                    entry.as_ref(),
                                    &quotes,
                                    quote,
                                    read.context,
                                ),
                                None => excerpt::item(whose, id, entry.as_ref(), &quotes),
                            },
                            Taken::Around(near) => {
                                let count = read.around.unwrap_or_default();
                                excerpt::around(whose, id, &near, count, pageable, &quotes)
                            }
                        })
                    })
                }
                Err(reason) => Err(reason),
            };
            let later = Later::Answer {
                reply,
                result,
                release: None,
            };
            let _ = tx.send(Input::Agents(Box::new(later)));
        });
    }

    /// The agent a wide read reaches by its id, or none for one the user
    /// attached or quoted, the caller's own, or a name that isn't an id.
    pub(super) fn read_by_id(&self, caller: Option<AgentId>, read: &Read) -> Option<AgentId> {
        let item = read.item.clone().map(ItemId);
        let attached = caller.is_some_and(|agent| {
            self.held.contains_key(&self.lead_of(agent))
                && self.attached(agent, &read.agent, item.as_ref()).is_ok()
        });
        if attached {
            return None;
        }
        unmarked(&read.agent).parse().ok()
    }

    /// The agent `name` means among those the user attached to a message
    /// `caller` has, or quoted in one: by id, or else by the title the
    /// attachment or quote gave it. With it comes whether the user
    /// attached it, so the caller may page through it, and what they
    /// quoted of `item`, to mark when it's read. Of an agent the
    /// user only quoted, the caller reads only a quoted `item`, and the
    /// messages around it. A sub-agent has its lead's messages. A message
    /// the agent hasn't had, queued or taken back, doesn't count.
    fn attached(
        &self,
        caller: AgentId,
        name: &str,
        item: Option<&ItemId>,
    ) -> Result<(AgentId, bool, Vec<Quote>), String> {
        let lead = self.lead_of(caller);
        // An agent reads its own conversation, as a handoff tells it to
        // for what didn't fit.
        if unmarked(name).parse::<AgentId>().is_ok_and(|id| id == lead) {
            return Ok((lead, true, Vec::new()));
        }
        // Each agent the messages name, once for each title they give it.
        let mut seen: Vec<(AgentId, &str)> = Vec::new();
        let mut whole: Vec<AgentId> = Vec::new();
        let mut quoted: Vec<(AgentId, &ItemId)> = Vec::new();
        let mut quotes: Vec<(AgentId, Quote)> = Vec::new();
        let whose: &[AgentId] = if lead == caller {
            &[caller]
        } else {
            &[caller, lead]
        };
        for timeline in whose.iter().filter_map(|agent| self.timelines.get(agent)) {
            // Messages with pills stay in memory.
            for message in timeline.kept() {
                let ItemBody::UserMessage { refs, delivery, .. } = &message.body else {
                    continue;
                };
                if !matches!(delivery, Delivery::Sent | Delivery::Steer) {
                    continue;
                }
                for inline in refs {
                    let (agent, title) = match inline {
                        InlineRef::Agent { agent, title, .. } => {
                            whole.push(*agent);
                            (*agent, title)
                        }
                        InlineRef::Quote {
                            agent,
                            agent_title,
                            item: quoted_item,
                            text,
                            position,
                            ..
                        } => {
                            if !quoted.contains(&(*agent, quoted_item)) {
                                quoted.push((*agent, quoted_item));
                            }
                            if item == Some(quoted_item) {
                                let quote = Quote {
                                    text: text.clone(),
                                    position: *position,
                                };
                                if !quotes.contains(&(*agent, quote.clone())) {
                                    quotes.push((*agent, quote));
                                }
                            }
                            (*agent, agent_title)
                        }
                        _ => continue,
                    };
                    if !seen.contains(&(agent, title.as_str())) {
                        seen.push((agent, title));
                    }
                }
            }
        }
        let target = named(&seen, unmarked(name))?;
        let quotes: Vec<Quote> = quotes
            .into_iter()
            .filter(|(agent, _)| *agent == target)
            .map(|(_, quote)| quote)
            .collect();
        if whole.contains(&target) {
            return Ok((target, true, quotes));
        }
        let items: Vec<&ItemId> = quoted
            .iter()
            .filter(|(agent, _)| *agent == target)
            .map(|(_, item)| *item)
            .collect();
        match item {
            Some(item) if items.contains(&item) => Ok((target, false, quotes)),
            Some(_) => Err(quoted_only(NOT_QUOTED, &items)),
            None => Err(quoted_only(QUOTED_PAGE, &items)),
        }
    }

    /// Hands a finished tool call back to the driver that asked for it.
    pub(super) async fn tool_done(&mut self, session: SessionId, done: ToolDone) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        let mut fx = Effects::default();
        hold.driver.tool_done(done, &mut fx);
        self.apply_fx(session, fx).await;
    }
}

/// The agent `name` means among those `seen` holds: by id, or else by a
/// title they give it.
fn named(seen: &[(AgentId, &str)], name: &str) -> Result<AgentId, String> {
    if let Ok(id) = name.parse::<AgentId>()
        && seen.iter().any(|(agent, _)| *agent == id)
    {
        return Ok(id);
    }
    let mut titled: Vec<AgentId> = Vec::new();
    for (agent, title) in seen {
        if title.trim().eq_ignore_ascii_case(name) && !titled.contains(agent) {
            titled.push(*agent);
        }
    }
    match titled.as_slice() {
        [agent] => Ok(*agent),
        [] => Err(NOT_ATTACHED.to_owned()),
        _ => Err(SAME_TITLE.to_owned()),
    }
}

/// The quote of `item` among `quotes` that starts at `position`, or why
/// the read tool refuses it, with where the item's quotes do start.
fn starting(quotes: &[Quote], item: &ItemId, position: u32) -> Result<Quote, String> {
    if let Some(quote) = quotes.iter().find(|quote| quote.position == position) {
        return Ok(quote.clone());
    }
    let mut starts: Vec<u32> = quotes.iter().map(|quote| quote.position).collect();
    starts.sort_unstable();
    starts.dedup();
    let starts: Vec<String> = starts.iter().map(u32::to_string).collect();
    let only = match starts.split_last() {
        None => String::new(),
        Some((last, [])) => format!(", only at {last}"),
        Some((last, rest)) => format!(", only at {} and {last}", rest.join(", ")),
    };
    Err(format!(
        "No quote of item {} starts at position {position}{only}.",
        clean_name(&item.0)
    ))
}

/// Why the read tool refuses more of an agent than the user quoted, then
/// what the caller can read of it instead.
fn quoted_only(why: &str, items: &[&ItemId]) -> String {
    let items: Vec<_> = items.iter().map(|item| clean_name(&item.0)).collect();
    format!(
        "{why} {QUOTED_ITEMS} {}. {ATTACH_FOR_MORE}",
        items.join(", ")
    )
}

/// A name as the model may copy it from the context entry, with the
/// marker's `[Agent: ` and `]` around it, or a quote entry's double quotes.
fn unmarked(name: &str) -> &str {
    let name = name.trim();
    let name = name
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(name);
    let name = name.strip_prefix("Agent:").unwrap_or(name).trim();
    name.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(name)
        .trim()
}
