// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving an agent to another device: the Mac runs the move, the agent
//! stops where it was and a new one starts on the other device from a
//! summary, under the same name, and the tasks follow it.

use maxhq_protocol::agents::{AgentAt, MoveTo, Moved};
use maxhq_protocol::tasks::TaskTarget;

use super::tasks::{hourly, task};
use super::*;
use crate::session::move_agent::NOT_HOME;

/// The device agents move to.
fn devbox() -> Device {
    Device {
        id: DeviceId::derive("devbox"),
        name: "devbox".into(),
        os: "linux".into(),
        ..device()
    }
}

/// The Mac, which runs each move, and devbox, which answers its line.
fn pair() -> (Harness, Harness) {
    let mac = Harness::new();
    let devbox = Harness::on(devbox());
    mac.fakes.peers.answer_with(&devbox.sessions, mac.host.id);
    (mac, devbox)
}

impl Harness {
    /// A move to this device's Claude account, in its project folder.
    fn move_here(&self) -> MoveTo {
        MoveTo {
            device: self.host.id,
            account: self.claude,
            model: None,
            place: AgentPlace::Folder {
                path: self.folder.display().to_string(),
            },
        }
    }

    /// Moves `agent` from `from` as `to` says.
    async fn move_agent(
        &mut self,
        agent: AgentId,
        from: DeviceId,
        to: MoveTo,
        stop: bool,
    ) -> ActionOutcome {
        self.act(|key| ClientMessage::MoveAgent {
            key,
            agent,
            from,
            to,
            stop,
        })
        .await
    }
}

#[tokio::test]
async fn an_agent_moves_with_a_summary_its_name_and_its_tasks() {
    let (mut mac, mut devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    let target = TaskTarget::Agent {
        device: here,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), target)).await;

    let to = devbox.move_here();
    let new = started(mac.move_agent(old, here, to, false).await);

    // The old agent stopped, settled and points at the new one.
    eventually("the old CLI stopping", || {
        !mac.holders.stops(session).is_empty()
    })
    .await;
    let there = AgentAt {
        device: devbox.host.id,
        agent: new,
    };
    let moved = Some(Box::new(Moved {
        to: Some(there),
        from: None,
    }));
    assert_eq!(mac.agent(old).moved, moved);
    assert_eq!(mac.stored(old).moved, moved);
    assert!(mac.hub.inbox(old).settled.is_some());

    // The new one has its name and points back.
    let shown = devbox.agent(new);
    assert_eq!(shown.title, mac.agent(old).title);
    let back = AgentAt {
        device: here,
        agent: old,
    };
    assert_eq!(
        shown.moved,
        Some(Box::new(Moved {
            to: None,
            from: Some(back),
        }))
    );

    // Its first message carries the conversation in front of it.
    assert_eq!(devbox.send(new, "Ship it").await, ActionOutcome::Done);
    let server = devbox.holders.last();
    eventually("the first message", || !devbox.sent(server).is_empty()).await;
    let sent = devbox.sent(server);
    assert!(sent[0].starts_with("<maxhq_handoff>\n"), "{}", sent[0]);
    assert!(sent[0].contains("Built it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nShip it"), "{}", sent[0]);

    // The task follows it, and devbox keeps a copy of its permission.
    let follows = mac.published_task(nightly.id).expect("the task stays");
    assert_eq!(
        follows.target,
        TaskTarget::Agent {
            device: devbox.host.id,
            agent: new,
        }
    );
    let kept = devbox.db.permissions().expect("permissions can be read");
    assert!(kept
        .iter()
        .any(|(permission, _)| permission.id == nightly.permission));
}

#[tokio::test]
async fn an_agent_on_another_device_moves_here_and_its_device_gives_back_the_tasks_copy() {
    let (mut mac, mut devbox) = pair();
    let (old, _) = devbox.claude_after_a_turn().await;
    let away = TaskTarget::Agent {
        device: devbox.host.id,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), away)).await;
    let copy = maxhq_core::agents::task_permission(&nightly, mac.host.id);
    devbox.give(copy).await;

    let to = mac.move_here();
    let new = started(mac.move_agent(old, devbox.host.id, to, false).await);

    let here = AgentAt {
        device: mac.host.id,
        agent: new,
    };
    assert_eq!(
        devbox.agent(old).moved.and_then(|moved| moved.to),
        Some(here)
    );
    assert_eq!(
        mac.published_task(nightly.id).map(|task| task.target),
        Some(TaskTarget::Agent {
            device: mac.host.id,
            agent: new,
        })
    );
    let kept = devbox.db.permissions().expect("permissions can be read");
    assert!(!kept
        .iter()
        .any(|(permission, _)| permission.id == nightly.permission));
}

#[tokio::test]
async fn a_busy_agent_moves_only_when_told_to_stop() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.start_claude(Some("Build it")).await;
    let say = Say {
        agents: vec![mac.reported(old, Activity::Busy)],
        timeline: vec![upsert(
            old,
            reply(old, "a1", "Building it", ItemStatus::Running),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    let here = mac.host.id;

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(
        reason(&outcome),
        "The agent is working. Wait for its turn to end, or stop it and move it."
    );
    assert!(mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);

    let new = started(mac.move_agent(old, here, devbox.move_here(), true).await);
    assert!(!mac.holders.stops(session).is_empty());
    assert_eq!(
        mac.agent(old).moved.and_then(|moved| moved.to),
        Some(AgentAt {
            device: devbox.host.id,
            agent: new,
        })
    );
}

#[tokio::test]
async fn a_move_checks_the_device_it_goes_to_before_it_stops_the_agent() {
    let (mut mac, mut devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    devbox.claude_model(&[], "");

    let nobody = MoveTo {
        account: AccountId::derive("nobody"),
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, nobody, false).await;
    assert_eq!(reason(&outcome), "That account isn't on that device.");

    let model = MoveTo {
        model: Some("haiku".into()),
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, model, false).await;
    assert_eq!(
        reason(&outcome),
        "Claude doesn't offer haiku under that account."
    );

    let gone = MoveTo {
        place: AgentPlace::Folder {
            path: devbox.folder.join("gone").display().to_string(),
        },
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, gone, false).await;
    assert_eq!(reason(&outcome), "that folder doesn't exist");

    mac.fakes.peers.set_down(true);
    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(reason(&outcome), "no line to that device");

    // The agent never stopped, and nothing started on devbox.
    assert!(mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);
    assert!(devbox.holders.sessions().is_empty());
}

#[tokio::test]
async fn a_start_that_fails_leaves_the_old_agent_stopped_and_unmarked() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    let target = TaskTarget::Agent {
        device: here,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), target.clone())).await;
    devbox.holders.fail_spawns(1);

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    let said = reason(&outcome);
    assert!(
        said.starts_with("The agent stopped, but the new one didn't start there: "),
        "{said}"
    );
    assert!(
        said.ends_with("It stays where it was, and a message resumes it."),
        "{said}"
    );
    assert!(!mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);
    assert!(mac.hub.inbox(old).settled.is_none());
    assert_eq!(
        mac.published_task(nightly.id).map(|task| task.target),
        Some(target)
    );

    // A message resumes it where it was.
    assert_eq!(mac.send(old, "Go on").await, ActionOutcome::Done);
}

#[tokio::test]
async fn only_the_device_that_runs_the_app_moves_agents() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    mac.fakes.peers.set_home(devbox.host.id);
    let here = mac.host.id;

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(reason(&outcome), NOT_HOME);
    assert!(mac.holders.stops(session).is_empty());
}
