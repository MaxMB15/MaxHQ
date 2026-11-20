// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_protocol::attention::AttentionKind;
use maxhq_protocol::ids::{ProjectId, WorkspaceId};
use maxhq_protocol::model::{Activity, Project, Workspace, WorkspaceLocation};

use super::*;
use crate::push::tests::{agent_id, agent_titled, prompt};

const TITLE: &str = "Fix the login";

fn device() -> DeviceId {
    DeviceId::derive("devbox")
}

fn workspace(device: DeviceId) -> Change {
    Change::Upsert {
        value: Entity::Workspace(Workspace {
            id: WorkspaceId::derive("w"),
            project: Some(ProjectId::derive("p")),
            device,
            location: WorkspaceLocation::Folder {
                path: "~/code/example-app".into(),
            },
            managed: None,
            clone: None,
        }),
    }
}

fn project() -> Entity {
    Entity::Project(Project {
        id: ProjectId::derive("p"),
        name: "example-app".into(),
        repo: None,
        default_vendor: None,
        default_account: None,
        default_workspace: None,
    })
}

fn agent(activity: Activity) -> Change {
    agent_titled(activity, TITLE)
}

fn raised(agent: AgentId, at: u64, kind: NoticeKind) -> Raised {
    Raised {
        agent,
        at: Timestamp(at),
        record: Record {
            kind,
            title: TITLE.into(),
            project: None,
            summary: None,
        },
    }
}

/// A log started on a device whose agent was already waiting.
fn started() -> Log {
    let mut log = Log::new(device());
    let mut entities = vec![project()];
    for change in [workspace(device())].into_iter().chain(prompt(
        AttentionKind::Question,
        "Which branch?",
        Vec::new(),
    )) {
        let Change::Upsert { value } = change else {
            unreachable!()
        };
        entities.push(value);
    }
    assert!(
        log.reset(entities, true).is_empty(),
        "already waiting at start"
    );
    log
}

#[test]
fn the_log_keeps_each_alert_with_its_kind_and_wording() {
    let mut log = started();
    let kinds = |raised: Vec<Raised>| -> Vec<(NoticeKind, Option<String>)> {
        raised
            .into_iter()
            .map(|raised| (raised.record.kind, raised.record.summary))
            .collect()
    };

    let finished = log.observe(vec![agent(Activity::DoneUnread)]);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent, agent_id());
    assert_eq!(finished[0].record.title, TITLE);
    assert_eq!(finished[0].record.project.as_deref(), Some("example-app"));
    assert_eq!(kinds(finished), vec![(NoticeKind::Finished, None)]);

    log.observe(vec![agent(Activity::Busy)]);
    let asked = log.observe(prompt(
        AttentionKind::Permission,
        "  Run cargo test?  ",
        Vec::new(),
    ));
    assert_eq!(
        kinds(asked),
        vec![(NoticeKind::Permission, Some("Run cargo test?".into()))]
    );

    log.observe(vec![agent(Activity::Busy)]);
    let failed = log.observe(prompt(AttentionKind::Error, "", Vec::new()));
    assert_eq!(kinds(failed), vec![(NoticeKind::Error, None)]);
}

#[test]
fn the_log_leaves_a_peers_agents_to_the_peer() {
    let mut log = started();
    log.observe(vec![workspace(DeviceId::derive("elsewhere"))]);
    log.observe(vec![agent(Activity::Busy)]);
    assert!(log.observe(vec![agent(Activity::DoneUnread)]).is_empty());
}

#[test]
fn a_page_is_the_newest_first_and_the_log_keeps_the_newest_500() {
    let db = Db::open_in_memory().unwrap();
    let conn = db.lock();
    for at in 0..u64::from(NOTICES_KEPT) + 5 {
        record(&conn, &raised(agent_id(), at, NoticeKind::Finished)).unwrap();
    }
    let first = page(&conn, device(), 20).unwrap();
    assert_eq!(first.total, NOTICES_KEPT);
    assert_eq!(first.unread, NOTICES_KEPT);
    assert_eq!(first.notices.len(), 20);
    assert_eq!(first.notices[0].at, Timestamp(504));
    assert_eq!(first.notices[19].at, Timestamp(485));
    assert!(first.notices.iter().all(|notice| notice.device == device()));

    let all = page(&conn, device(), 10_000).unwrap();
    assert_eq!(all.notices.len(), NOTICES_KEPT as usize);
    assert_eq!(
        all.notices.last().unwrap().at,
        Timestamp(5),
        "oldest dropped"
    );
}

#[test]
fn reading_marks_one_agents_notices_or_all_and_clear_empties_the_log() {
    let db = Db::open_in_memory().unwrap();
    let other = AgentId::derive("other");
    {
        let conn = db.lock();
        record(&conn, &raised(agent_id(), 1, NoticeKind::Question)).unwrap();
        record(&conn, &raised(other, 2, NoticeKind::Finished)).unwrap();
        record(&conn, &raised(agent_id(), 3, NoticeKind::Finished)).unwrap();
        read(&conn, Some(agent_id()), Timestamp(10)).unwrap();
        let after = page(&conn, device(), 20).unwrap();
        assert_eq!(after.unread, 1);
        let unread: Vec<AgentId> = after
            .notices
            .iter()
            .filter(|notice| notice.read_at.is_none())
            .map(|notice| notice.agent)
            .collect();
        assert_eq!(unread, vec![other]);
        read(&conn, None, Timestamp(11)).unwrap();
        let all = page(&conn, device(), 20).unwrap();
        assert_eq!(all.unread, 0);
        assert_eq!(all.notices[2].read_at, Some(Timestamp(10)), "kept its time");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let cleared = answer(db.clone(), device(), device(), NoticeCall::Clear).await;
        assert_eq!(cleared, ActionOutcome::Done);
        let listed = answer(
            db.clone(),
            device(),
            device(),
            NoticeCall::List { limit: 20 },
        )
        .await;
        assert_eq!(
            listed,
            ActionOutcome::Notices {
                page: NoticePage::default()
            }
        );
        let elsewhere = DeviceId::derive("elsewhere");
        let refused = answer(db, device(), elsewhere, NoticeCall::Clear).await;
        assert!(matches!(refused, ActionOutcome::Refused { .. }));
    });
}
