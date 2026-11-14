// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Binding agents to a run and its nodes, through the descriptor's
//! patterns over facts the hub already keeps. The engine never reads a
//! transcript.

use std::collections::HashMap;
use std::sync::OnceLock;

use maxhq_protocol::ids::AgentId;
use regex::{Captures, Regex};

use crate::descriptor::{Bind, NodePattern, Pattern};
use crate::inputs::{AgentFacts, AgentState, Agents};

/// The line a dispatch prompt carries to bind with no `bind` block.
pub const DEFAULT_LINE: &str = "maxhq-node:";

fn default_patterns() -> &'static [NodePattern] {
    static DEFAULT: OnceLock<Vec<NodePattern>> = OnceLock::new();
    DEFAULT.get_or_init(|| {
        let regex = Regex::new(r"(?m)^\s*maxhq-node:\s*(?<run>\S+)\s+(?<node>\S+)\s*$")
            .expect("a valid pattern");
        vec![NodePattern {
            prompt: Some(Pattern(regex)),
            ..NodePattern::default()
        }]
    })
}

/// Who works on a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Binding {
    /// The agents per node, oldest first.
    pub nodes: HashMap<String, Vec<AgentId>>,
    /// The agents that lead the run: `run.agent`, then the leads of bound
    /// sub-agents, then agents whose commands drive it.
    pub orchestrators: Vec<AgentId>,
}

/// What the run itself says about its agents.
pub(crate) struct Claims<'a> {
    pub run: &'a str,
    /// The run's `agent`.
    pub lead: Option<AgentId>,
    /// Agents the run file names per node, and pids read per node.
    pub node_agents: Vec<(String, AgentId)>,
    pub node_pids: Vec<(String, u32)>,
    /// The node ids in the run.
    pub node_ids: &'a [String],
}

pub(crate) fn bind(
    bind: Option<&Bind>,
    claims: &Claims,
    facts: &[AgentFacts],
    agents: &dyn Agents,
) -> Binding {
    let patterns = bind.map_or(default_patterns(), |b| b.node.as_slice());
    let wants_prompt = patterns.iter().any(|p| p.prompt.is_some());
    let mut binding = Binding::default();
    let add = |node: &str, agent: AgentId, binding: &mut Binding| {
        if claims.node_ids.iter().any(|id| id == node) {
            let list = binding.nodes.entry(node.to_owned()).or_default();
            if !list.contains(&agent) {
                list.push(agent);
            }
        }
    };
    for (node, agent) in &claims.node_agents {
        add(node, *agent, &mut binding);
    }
    for fact in facts {
        if let Some(pid) = fact.pid {
            for (node, _) in claims.node_pids.iter().filter(|(_, p)| *p == pid) {
                add(node, fact.id, &mut binding);
            }
        }
        let prompt = wants_prompt.then(|| agents.prompt_head(fact.id)).flatten();
        for pattern in patterns {
            if let Some(node) = node_match(
                pattern,
                fact.title.as_deref(),
                prompt.as_deref(),
                claims.run,
            ) {
                add(&node, fact.id, &mut binding);
            }
        }
    }
    let started = |id: &AgentId| facts.iter().find(|f| f.id == *id).and_then(|f| f.started);
    for list in binding.nodes.values_mut() {
        list.sort_by_key(started);
    }

    let mut leads = Vec::new();
    let mut lead = |id: AgentId| {
        if !leads.contains(&id) {
            leads.push(id);
        }
    };
    if let Some(id) = claims.lead {
        lead(id);
    }
    let mut bound: Vec<&AgentId> = binding.nodes.values().flatten().collect();
    bound.sort_by_key(|id| started(id));
    for id in bound {
        if let Some(parent) = facts.iter().find(|f| f.id == *id).and_then(|f| f.lead) {
            lead(parent);
        }
    }
    let run_patterns = bind.map_or(&[][..], |b| b.run.as_slice());
    if !run_patterns.is_empty() {
        let prefixes: Vec<String> = run_patterns
            .iter()
            .map(|p| literal_prefix(p.command.0.as_str()))
            .collect();
        for fact in facts {
            let mut drives = false;
            agents.each_command(fact.id, &mut |command| {
                drives = drives
                    || run_patterns.iter().zip(&prefixes).any(|(p, prefix)| {
                        command.contains(prefix.as_str())
                            && p.command
                                .0
                                .captures(command)
                                .is_some_and(|c| group(&c, "run") == Some(claims.run))
                    });
            });
            if drives {
                lead(fact.id);
            }
        }
    }
    binding.orchestrators = leads;
    binding
}

fn group<'h>(caps: &Captures<'h>, name: &str) -> Option<&'h str> {
    caps.name(name).map(|m| m.as_str())
}

/// The node a pattern binds an agent to in this run, if any. A pattern with
/// a `run` group binds only when it names this run.
fn node_match(
    p: &NodePattern,
    title: Option<&str>,
    prompt: Option<&str>,
    run: &str,
) -> Option<String> {
    let (first, text) = match (&p.title, &p.prompt) {
        (Some(title_pattern), _) => (title_pattern, title?),
        (None, Some(prompt_pattern)) => (prompt_pattern, prompt?),
        (None, None) => return None,
    };
    let caps = first.0.captures(text)?;
    let more = match &p.and {
        Some(and) => Some(and.0.captures(text)?),
        None => None,
    };
    let find = |name| group(&caps, name).or_else(|| more.as_ref().and_then(|c| group(c, name)));
    if find("run").is_some_and(|r| r != run) {
        return None;
    }
    find("node").map(str::to_owned)
}

/// The text every match of a pattern starts with, so most commands are
/// passed over with a substring check.
pub(crate) fn literal_prefix(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.trim_start_matches('^').chars().peekable();
    while let Some(c) = chars.next() {
        let literal = match c {
            '\\' => match chars.next() {
                Some(e) if e.is_ascii_punctuation() => e,
                _ => break,
            },
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => break,
            c => c,
        };
        if matches!(chars.peek(), Some('*' | '?' | '{')) {
            break;
        }
        out.push(literal);
    }
    out
}

/// How the run's leads stand together: busy if any is, then waiting, then
/// idle, else gone.
pub fn lead_state(orchestrators: &[AgentId], facts: &[AgentFacts]) -> AgentState {
    let states: Vec<AgentState> = orchestrators
        .iter()
        .filter_map(|id| facts.iter().find(|f| f.id == *id).map(|f| f.state))
        .collect();
    [AgentState::Busy, AgentState::Waiting, AgentState::Idle]
        .into_iter()
        .find(|s| states.contains(s))
        .unwrap_or(AgentState::Gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeAgents;

    fn bind_yaml(yaml: &str) -> Bind {
        serde_norway::from_str(yaml).unwrap()
    }

    const DEVFLOW_LIKE: &str = r#"
node:
  - { title: '^(?<node>(?:df|cr)-[\w-]+) (?<run>t-\S+)$' }
  - prompt: '(?m)^\s*block:\s*(?<node>(?:df|cr)-[\w-]+)\s*$'
    and: '(?m)^\s*task:\s*(?<run>t-\S+)\s*$'
run:
  - { command: 'df-state\.sh (?:init|next|set-status) (?<run>t-\S+)' }
"#;

    fn ids() -> Vec<String> {
        ["df-plan", "df-testing", "df-security"]
            .map(String::from)
            .to_vec()
    }

    fn claims(ids: &[String]) -> Claims<'_> {
        Claims {
            run: "t-1",
            lead: None,
            node_agents: Vec::new(),
            node_pids: Vec::new(),
            node_ids: ids,
        }
    }

    #[test]
    fn binds_by_title_and_takes_the_lead() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        let a = agents.add("df-testing t-1", Some(lead));
        let other_run = agents.add("df-security t-2", Some(lead));
        let ids = ids();
        let b = bind(
            Some(&bind_yaml(DEVFLOW_LIKE)),
            &claims(&ids),
            &agents.list(),
            &agents,
        );
        assert_eq!(b.nodes.get("df-testing"), Some(&vec![a]));
        assert!(!b.nodes.values().flatten().any(|id| *id == other_run));
        assert_eq!(b.orchestrators, vec![lead]);
    }

    #[test]
    fn binds_by_prompt_with_and() {
        let mut agents = FakeAgents::default();
        let a = agents.add("worker", None);
        agents.prompt(a, "You are a gate.\nblock: df-security\ntask: t-1\n");
        let b2 = agents.add("worker", None);
        agents.prompt(b2, "block: df-security\n");
        let ids = ids();
        let b = bind(
            Some(&bind_yaml(DEVFLOW_LIKE)),
            &claims(&ids),
            &agents.list(),
            &agents,
        );
        assert_eq!(b.nodes.get("df-security"), Some(&vec![a]));
    }

    #[test]
    fn binds_the_lead_by_a_command_with_its_prefix() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        agents.command(lead, "ls");
        agents.command(lead, ".devflow-vendor/scripts/df-state.sh next t-1 --json");
        let other = agents.add("other", None);
        agents.command(other, "df-state.sh next t-9");
        let ids = ids();
        let b = bind(
            Some(&bind_yaml(DEVFLOW_LIKE)),
            &claims(&ids),
            &agents.list(),
            &agents,
        );
        assert_eq!(b.orchestrators, vec![lead]);
        assert!(b.nodes.is_empty());
    }

    #[test]
    fn the_default_pattern_reads_a_maxhq_node_line() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        let a = agents.add("anything", Some(lead));
        agents.prompt(a, "Do the work.\nmaxhq-node: t-1 df-plan\n");
        let ids = ids();
        let b = bind(None, &claims(&ids), &agents.list(), &agents);
        assert_eq!(b.nodes.get("df-plan"), Some(&vec![a]));
        assert_eq!(b.orchestrators, vec![lead]);
    }

    #[test]
    fn run_agent_pids_and_node_agents_bind_too() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        let bridged = agents.add("codex child", None);
        agents.pid(bridged, 777);
        let named = agents.add("named", None);
        let ids = ids();
        let mut c = claims(&ids);
        c.lead = Some(lead);
        c.node_pids = vec![("df-testing".into(), 777)];
        c.node_agents = vec![("df-plan".into(), named)];
        let b = bind(Some(&Bind::default()), &c, &agents.list(), &agents);
        assert_eq!(b.nodes.get("df-testing"), Some(&vec![bridged]));
        assert_eq!(b.nodes.get("df-plan"), Some(&vec![named]));
        assert_eq!(b.orchestrators, vec![lead]);
    }

    #[test]
    fn many_agents_bind_to_one_node_oldest_first() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        let second = agents.add("df-testing t-1", Some(lead));
        let first = agents.add("df-testing t-1", Some(lead));
        agents.started(second, 2_000);
        agents.started(first, 1_000);
        let resumed = agents.add("lead again", None);
        let third = agents.add("df-plan t-1", Some(resumed));
        agents.started(third, 3_000);
        let ids = ids();
        let b = bind(
            Some(&bind_yaml(DEVFLOW_LIKE)),
            &claims(&ids),
            &agents.list(),
            &agents,
        );
        assert_eq!(b.nodes.get("df-testing"), Some(&vec![first, second]));
        assert_eq!(b.orchestrators, vec![lead, resumed]);
    }

    #[test]
    fn literal_prefixes_stop_at_the_first_pattern_character() {
        assert_eq!(
            literal_prefix(r"df-state\.sh (?:init|next)"),
            "df-state.sh "
        );
        assert_eq!(literal_prefix(r"^abc*d"), "ab");
        assert_eq!(literal_prefix(r"(?m)x"), "");
        assert_eq!(literal_prefix(r"a\d"), "a");
    }

    #[test]
    fn the_lead_state_prefers_the_liveliest() {
        let mut agents = FakeAgents::default();
        let a = agents.add("a", None);
        let b = agents.add("b", None);
        agents.state(a, AgentState::Idle);
        agents.state(b, AgentState::Waiting);
        assert_eq!(lead_state(&[a, b], &agents.list()), AgentState::Waiting);
        assert_eq!(lead_state(&[], &agents.list()), AgentState::Gone);
    }
}
