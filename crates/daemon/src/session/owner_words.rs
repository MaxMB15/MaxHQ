// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What you said to an agent in its turn now, so a tool can tell a step
//! you asked for from text that only claims you did. A message counts when
//! a person sent it in MaxHQ: to an agent, or to the dispatcher. One an
//! agent, a task or another device sends ends the turn's words, and what
//! a tool reads never reaches here. Words from a phone whose switch is off
//! never ask for Auto or Full access.

use std::collections::{HashMap, HashSet};

use maxhq_core::agents::needs_switch;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::model::PermissionMode;

use super::held::Viewer;
use super::Manager;

/// The messages you sent and what each agent heard from you last.
#[derive(Default)]
pub(super) struct OwnerWords {
    /// Each message you sent, by its request's key, until it reaches its
    /// agent.
    sent: HashMap<IdempotencyKey, Sent>,
    /// What you last said to each lead agent, until anything else reaches
    /// it.
    said: HashMap<AgentId, String>,
    /// The lead agents you last sent to from a phone whose switch is off,
    /// until you send to them from elsewhere.
    phoned: HashSet<AgentId>,
}

/// A message you sent.
struct Sent {
    text: String,
    /// Whether it came from a phone whose switch is off.
    held_back: bool,
}

impl OwnerWords {
    /// Notes a message you sent, under its request's key, and whether it
    /// came from a phone whose switch is off.
    pub(super) fn sent(&mut self, key: IdempotencyKey, text: &str, held_back: bool) {
        let text = text.to_owned();
        self.sent.insert(key, Sent { text, held_back });
    }

    /// Hears that the message under `key` reached `agent`: your words for
    /// its turn now, or from anyone else, the end of them.
    pub(super) fn reached(&mut self, key: IdempotencyKey, agent: AgentId) {
        match self.sent.remove(&key) {
            Some(sent) => {
                self.said.insert(agent, sent.text);
                if sent.held_back {
                    self.phoned.insert(agent);
                } else {
                    self.phoned.remove(&agent);
                }
            }
            None => {
                self.said.remove(&agent);
            }
        }
    }

    /// Forgets the message under `key` once its request is done.
    pub(super) fn done(&mut self, key: IdempotencyKey) {
        self.sent.remove(&key);
    }

    /// Forgets an agent that's gone.
    pub(super) fn forget(&mut self, agent: AgentId) {
        self.said.remove(&agent);
        self.phoned.remove(&agent);
    }

    /// Whether you last sent to `agent` from a phone whose switch is off.
    pub(super) fn phoned(&self, agent: AgentId) -> bool {
        self.phoned.contains(&agent)
    }

    /// Whether what you said to `agent` holds `quote`, and `quote` names
    /// `mode`.
    fn asked_for(&self, agent: AgentId, quote: &str, mode: PermissionMode) -> bool {
        if needs_switch(mode) && self.phoned(agent) {
            return false;
        }
        let quote = plain(quote);
        if quote.is_empty() || !names_of(mode).iter().any(|name| names(&quote, name)) {
            return false;
        }
        self.said
            .get(&agent)
            .is_some_and(|said| plain(said).contains(&quote))
    }
}

impl Manager {
    /// Whether you asked `agent` for `mode` in its turn now: `quote` is in
    /// a message you sent it, and names the mode by its id or by its label
    /// in any of the app's languages. A tool that goes past a default
    /// because you asked for it checks this, with the words the agent
    /// quotes.
    pub(super) fn owner_asked_for(
        &self,
        agent: AgentId,
        quote: &str,
        mode: PermissionMode,
    ) -> bool {
        self.owner_words.asked_for(self.lead_of(agent), quote, mode)
    }

    /// Whether you last sent to `agent` from a phone whose switch is off,
    /// so a step to Auto or Full access waits for the Mac's confirm.
    pub(super) fn phone_asked(&self, agent: AgentId) -> bool {
        self.owner_words.phoned(self.lead_of(agent))
    }

    /// Whether the connection `conn` is a phone whose switch is off.
    pub(super) fn held_back(&self, conn: u64) -> bool {
        self.clients.get(&conn).is_some_and(Viewer::held_back)
    }
}

/// Text in lower case, with each run of whitespace one space and curly
/// apostrophes straight, so a quote matches however it was typed.
fn plain(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.join(" ").to_lowercase().replace('\u{2019}', "'")
}

/// Whether `text` holds `name` as words of their own.
fn names(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// A mode's names in lower case: its ids, and its label in each of the
/// app's languages, as the shared locale files have them.
fn names_of(mode: PermissionMode) -> &'static [&'static str] {
    match mode {
        PermissionMode::Plan => &[
            "plan",
            "plan only",
            "alleen plannen",
            "nur planen",
            "planifier uniquement",
            "solo planificar",
        ],
        PermissionMode::Ask => &[
            "ask",
            "ask first",
            "eerst vragen",
            "zuerst fragen",
            "demander d'abord",
            "preguntar primero",
        ],
        PermissionMode::AcceptEdits => &[
            "accept_edits",
            "accept edits",
            "bewerkingen accepteren",
            "änderungen erlauben",
            "accepter les modifications",
            "aceptar cambios",
        ],
        PermissionMode::Auto => &["auto", "automatisch", "automatique", "automático"],
        PermissionMode::FullAccess => &[
            "full_access",
            "full access",
            "volledige toegang",
            "voller zugriff",
            "accès complet",
            "acceso completo",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [PermissionMode; 5] = [
        PermissionMode::Plan,
        PermissionMode::Ask,
        PermissionMode::AcceptEdits,
        PermissionMode::Auto,
        PermissionMode::FullAccess,
    ];

    /// The English label of each mode, the key the locale files use.
    fn label(mode: PermissionMode) -> &'static str {
        match mode {
            PermissionMode::Plan => "Plan only",
            PermissionMode::Ask => "Ask first",
            PermissionMode::AcceptEdits => "Accept edits",
            PermissionMode::Auto => "Auto",
            PermissionMode::FullAccess => "Full access",
        }
    }

    #[test]
    fn each_mode_answers_to_its_id_and_its_label_in_every_language() {
        let locales = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/shared/src/i18n/locales"
        );
        for language in ["en", "nl", "de", "fr", "es"] {
            let path = format!("{locales}/{language}/shared.json");
            let text = std::fs::read_to_string(&path).unwrap();
            let strings: HashMap<String, String> = serde_json::from_str(&text).unwrap();
            for mode in MODES {
                let shown = strings[label(mode)].to_lowercase();
                assert!(
                    names_of(mode).contains(&shown.as_str()),
                    "{language}: {mode:?} is {shown}"
                );
            }
        }
        for mode in MODES {
            let id = serde_json::to_value(mode).unwrap();
            assert!(names_of(mode).contains(&id.as_str().unwrap()), "{mode:?}");
        }
    }

    fn heard(text: &str) -> (OwnerWords, AgentId) {
        let mut words = OwnerWords::default();
        let (key, agent) = (IdempotencyKey::generate(), AgentId::generate());
        words.sent(key, text, false);
        words.reached(key, agent);
        (words, agent)
    }

    #[test]
    fn your_words_naming_the_mode_count() {
        let (words, agent) = heard("Start a reviewer in example-app with full  access, please.");
        let mode = PermissionMode::FullAccess;
        assert!(words.asked_for(agent, "with Full access", mode));
        assert!(words.asked_for(agent, "reviewer in example-app with full access", mode));
        let (words, agent) = heard("Start er een met volledige toegang");
        assert!(words.asked_for(agent, "met Volledige toegang", mode));
        let (words, agent) = heard("Run it as full_access");
        assert!(words.asked_for(agent, "full_access", mode));
    }

    #[test]
    fn words_that_dont_name_the_mode_or_arent_yours_dont() {
        let (words, agent) = heard("Start a reviewer with full access");
        let mode = PermissionMode::FullAccess;
        // It doesn't name the mode.
        assert!(!words.asked_for(agent, "Start a reviewer", mode));
        assert!(!words.asked_for(agent, "", mode));
        // It names another mode.
        assert!(!words.asked_for(agent, "with full access", PermissionMode::Auto));
        // You didn't say it.
        assert!(!words.asked_for(agent, "use full access everywhere", mode));
        // Part of a word isn't the word.
        let (words, agent) = heard("Run it in autopilot");
        assert!(!words.asked_for(agent, "autopilot", PermissionMode::Auto));
        // Nobody said anything to another agent.
        assert!(!words.asked_for(AgentId::generate(), "with full access", mode));
    }

    #[test]
    fn a_phone_whose_switch_is_off_asks_for_no_auto_or_full_access() {
        let mut words = OwnerWords::default();
        let (key, agent) = (IdempotencyKey::generate(), AgentId::generate());
        words.sent(key, "Set it to full access and auto, then ask first", true);
        words.reached(key, agent);
        assert!(words.phoned(agent));
        assert!(!words.asked_for(agent, "full access", PermissionMode::FullAccess));
        assert!(!words.asked_for(agent, "auto", PermissionMode::Auto));
        assert!(words.asked_for(agent, "ask first", PermissionMode::Ask));
        // A message from an agent or a task leaves the phone's mark.
        words.reached(IdempotencyKey::generate(), agent);
        assert!(words.phoned(agent));
        // One you send from elsewhere clears it.
        let key = IdempotencyKey::generate();
        words.sent(key, "Now full access", false);
        words.reached(key, agent);
        assert!(!words.phoned(agent));
        assert!(words.asked_for(agent, "full access", PermissionMode::FullAccess));
    }

    #[test]
    fn a_message_from_anyone_else_ends_your_words() {
        let (mut words, agent) = heard("Start one with full access");
        words.reached(IdempotencyKey::generate(), agent);
        assert!(!words.asked_for(agent, "with full access", PermissionMode::FullAccess));
    }
}
