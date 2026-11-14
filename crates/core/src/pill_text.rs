// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pills outside a message: in an answer to an agent, and in the first
//! messages of the agents a dispatcher's plan starts. An answer's text
//! gets the markers and block a message gets. The dispatcher sees its
//! request's markers, and a marker it copies into a first message hands
//! that agent the pill.

use maxhq_protocol::attention::AttentionAnswer;
use maxhq_protocol::context::{InlineRef, REF_MARK};
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::message::UserInput;

use crate::context;

/// The text `to` gets for a typed answer with pills: a marker for each
/// U+FFFC and the block, as [`context::encode`] writes for a message.
/// Without refs it's the text as typed. When the refs don't fit the text,
/// or name a file, which an answer can't carry, each pill goes as the
/// words it shows.
pub fn encode(text: &str, refs: &[InlineRef], to: AgentId) -> String {
    if refs.is_empty() {
        return text.to_owned();
    }
    if context::check(text, refs, &[]).is_ok() {
        return context::encode(text, refs, &[], to).text;
    }
    let shown = context::plain(text, refs, |_| None);
    let git: Vec<InlineRef> = refs
        .iter()
        .filter(|one| !one.is_placed())
        .cloned()
        .collect();
    context::encode(&shown, &git, &[], to).text
}

/// An answer as `to` gets it, with [`encode`] for what you typed. In
/// answers to questions, each value takes the placed refs for its U+FFFCs
/// in turn, and the pull requests and issues it names. A picked option
/// has no text.
pub fn answer(answer: AttentionAnswer, refs: &[InlineRef], to: AgentId) -> AttentionAnswer {
    if refs.is_empty() {
        return answer;
    }
    match answer {
        AttentionAnswer::Text { text } => AttentionAnswer::Text {
            text: encode(&text, refs, to),
        },
        AttentionAnswer::Questions { mut answers } => {
            let mut placed = refs.iter().filter(|one| one.is_placed());
            for value in answers.iter_mut().flat_map(|one| one.values.iter_mut()) {
                let marks = value.matches(REF_MARK).count();
                let mut own: Vec<InlineRef> = placed.by_ref().take(marks).cloned().collect();
                own.extend(
                    refs.iter()
                        .filter(|one| context::named_at(value, one).is_some())
                        .cloned(),
                );
                *value = encode(value, &own, to);
            }
            AttentionAnswer::Questions { answers }
        }
        other @ (AttentionAnswer::Option { .. } | AttentionAnswer::Dismiss) => other,
    }
}

/// The pills in the requests a dispatcher got, so the first messages its
/// plan writes carry the ones it copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Carried {
    /// Each placed pill with the marker the dispatcher saw, newest first.
    placed: Vec<(String, InlineRef)>,
    /// The pull requests and issues, newest first.
    git: Vec<InlineRef>,
}

impl Carried {
    /// Keeps a request's pills. A pill with the marker an older one has
    /// takes its place, as does a newer look at the same pull request.
    pub fn keep(&mut self, refs: &[InlineRef]) {
        let mut placed: Vec<(String, InlineRef)> = Vec::new();
        let marked = context::markers(refs)
            .into_iter()
            .zip(refs.iter().filter(|one| one.is_placed()).cloned());
        for (marker, one) in marked {
            if !marker.is_empty() && !placed.iter().any(|(seen, _)| *seen == marker) {
                placed.push((marker, one));
            }
        }
        self.placed
            .retain(|(old, _)| !placed.iter().any(|(new, _)| new == old));
        placed.append(&mut self.placed);
        self.placed = placed;

        let mut git: Vec<InlineRef> = refs
            .iter()
            .filter(|one| !one.is_placed())
            .cloned()
            .collect();
        self.git
            .retain(|old| !git.iter().any(|new| item_key(new) == item_key(old)));
        git.append(&mut self.git);
        self.git = git;
    }

    /// The first message for `text`, which the dispatcher wrote: each
    /// marker it copied from a request is that pill again, and each pull
    /// request or issue it names comes along. A U+FFFC it wrote itself
    /// goes, so the marks stay one for one with the refs.
    pub fn input(&self, text: &str) -> UserInput {
        let typed = text.replace(REF_MARK, "");
        let mut rest = typed.as_str();
        let mut out = String::with_capacity(typed.len());
        let mut refs = Vec::new();
        // The marker that comes first, and the newest on a tie.
        while let Some((at, marker, one)) = self
            .placed
            .iter()
            .filter_map(|(marker, one)| rest.find(marker.as_str()).map(|at| (at, marker, one)))
            .min_by_key(|(at, _, _)| *at)
        {
            out.push_str(&rest[..at]);
            out.push(REF_MARK);
            refs.push(one.clone());
            rest = &rest[at + marker.len()..];
        }
        out.push_str(rest);
        let mut git: Vec<(usize, &InlineRef)> = self
            .git
            .iter()
            .filter_map(|one| context::named_at(&out, one).map(|at| (at, one)))
            .collect();
        git.sort_by_key(|(at, _)| *at);
        refs.extend(git.into_iter().map(|(_, one)| one.clone()));
        UserInput {
            text: out,
            attachments: Vec::new(),
            refs,
        }
    }
}

/// The repo, in lowercase, and number a pull request or an issue has.
fn item_key(one: &InlineRef) -> Option<(String, u32)> {
    match one {
        InlineRef::GitItem { repo, number, .. } => Some((repo.to_ascii_lowercase(), *number)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::context::{GitItemKind, GitItemState, QuoteSource};
    use maxhq_protocol::ids::{DeviceId, NoteId, ProjectId};
    use maxhq_protocol::model::Vendor;
    use maxhq_protocol::time::Timestamp;
    use maxhq_protocol::timeline::{ItemId, QuestionAnswer};

    use super::*;

    fn to() -> AgentId {
        AgentId::derive("answered")
    }

    fn agent(title: &str) -> InlineRef {
        InlineRef::Agent {
            agent: AgentId::derive(title),
            title: title.to_owned(),
            vendor: Vendor::Claude,
            device: DeviceId::derive("mac"),
            excerpt: None,
        }
    }

    fn quote(text: &str) -> InlineRef {
        InlineRef::Quote {
            agent: AgentId::derive("quoted"),
            agent_title: "api refactor".to_owned(),
            item: ItemId("claude:msg_01".to_owned()),
            text: text.to_owned(),
            position: 0,
            source: QuoteSource::Reply,
            from: None,
        }
    }

    fn note(title: &str) -> InlineRef {
        InlineRef::Note {
            project: ProjectId::derive("project"),
            note: NoteId::derive(title),
            title: title.to_owned(),
            copy: None,
        }
    }

    fn pr(number: u32, title: &str) -> InlineRef {
        InlineRef::GitItem {
            repo: "owner/repo".to_owned(),
            number,
            kind: GitItemKind::PullRequest,
            title: title.to_owned(),
            state: GitItemState::Open,
            url: format!("https://github.com/owner/repo/pull/{number}"),
            head: None,
            base: None,
            body: String::new(),
            checked_at: Timestamp(1_793_368_931_000),
        }
    }

    #[test]
    fn a_typed_answer_gets_the_markers_and_block_a_message_gets() {
        let refs = vec![agent("cache fix"), pr(12, "Fix cache")];
        let text = "Do what \u{FFFC} did in #12";
        let sent = encode(text, &refs, to());
        assert_eq!(sent, context::encode(text, &refs, &[], to()).text);
        assert!(sent.starts_with("Do what [Agent: cache fix] did in #12\n\n<maxhq_context>\n"));
        assert!(sent.contains("[PR #12] owner/repo, open"));
        assert!(sent.ends_with("</maxhq_context>"));
    }

    #[test]
    fn an_answer_without_pills_goes_as_typed() {
        let text = "Use <maxhq_context> as is";
        assert_eq!(encode(text, &[], to()), text);
        let typed = AttentionAnswer::Text {
            text: text.to_owned(),
        };
        assert_eq!(answer(typed.clone(), &[], to()), typed);
    }

    #[test]
    fn pills_that_dont_fit_go_as_the_words_they_show() {
        let shown = encode("Like \u{FFFC} and \u{FFFC}", &[agent("cache fix")], to());
        assert_eq!(shown, "Like cache fix and ");
        let file = InlineRef::Attachment {
            id: maxhq_protocol::ids::AttachmentId::derive("/path/to/shot.png"),
        };
        assert_eq!(encode("See \u{FFFC}", &[file], to()), "See ");
    }

    #[test]
    fn each_typed_answer_to_a_question_takes_its_own_pills() {
        let refs = vec![
            agent("first"),
            quote("the cache never expires"),
            pr(7, "Seven"),
        ];
        let questions = AttentionAnswer::Questions {
            answers: vec![
                QuestionAnswer {
                    id: "how".to_owned(),
                    values: vec!["Fast".to_owned(), "Like \u{FFFC}".to_owned()],
                },
                QuestionAnswer {
                    id: "why".to_owned(),
                    values: vec!["Because \u{FFFC}, see #7".to_owned()],
                },
            ],
        };
        let AttentionAnswer::Questions { answers } = answer(questions, &refs, to()) else {
            panic!("still answers to questions");
        };
        assert_eq!(answers[0].values[0], "Fast");
        assert_eq!(
            answers[0].values[1],
            context::encode("Like \u{FFFC}", &refs[..1], &[], to()).text
        );
        let why = &answers[1].values[0];
        assert!(why.starts_with(
            "Because [Quote 1: \"the cache never expires\"], see #7\n\n<maxhq_context>"
        ));
        assert!(why.contains("[PR #7] owner/repo"));
        assert!(!why.contains("[Agent: first]"));
    }

    #[test]
    fn a_picked_option_stays_as_it_is() {
        let picked = AttentionAnswer::Option {
            id: "allow".to_owned(),
        };
        assert_eq!(answer(picked.clone(), &[agent("a")], to()), picked);
    }

    #[test]
    fn a_marker_the_dispatcher_copies_is_the_pill_again() {
        let mut carried = Carried::default();
        carried.keep(&[agent("cache fix"), note("Plan"), pr(12, "Fix cache")]);
        let first = carried.input("Read [Note 1: Plan], then finish [Agent: cache fix] and #12.");
        assert_eq!(first.text, "Read \u{FFFC}, then finish \u{FFFC} and #12.");
        assert_eq!(
            first.refs,
            vec![note("Plan"), agent("cache fix"), pr(12, "Fix cache")]
        );
        assert!(first.attachments.is_empty());
        assert!(context::check(&first.text, &first.refs, &first.attachments).is_ok());
    }

    #[test]
    fn text_without_markers_starts_as_plain_text() {
        let mut carried = Carried::default();
        carried.keep(&[agent("cache fix"), pr(12, "Fix cache")]);
        let first = carried.input("Tidy up \u{FFFC}the docs");
        assert_eq!(first.text, "Tidy up the docs");
        assert!(first.refs.is_empty());
    }

    #[test]
    fn a_newer_request_takes_over_its_markers() {
        let older = agent("cache fix");
        let newer = InlineRef::Agent {
            agent: AgentId::derive("another"),
            title: "cache fix".to_owned(),
            vendor: Vendor::Codex,
            device: DeviceId::derive("mac"),
            excerpt: None,
        };
        let mut carried = Carried::default();
        carried.keep(&[older, quote("old words"), pr(3, "Old title")]);
        let latest = [newer.clone(), quote("new words"), pr(3, "New title")];
        carried.keep(&latest);
        let first =
            carried.input("[Agent: cache fix] and [Quote 1: \"old words\"], #3 [Quote 1: \"new");
        assert_eq!(first.text, "\u{FFFC} and \u{FFFC}, #3 [Quote 1: \"new");
        assert_eq!(
            first.refs,
            vec![newer, quote("old words"), pr(3, "New title")]
        );
        let mut again = carried.clone();
        again.keep(&latest);
        assert_eq!(again, carried);
    }
}
