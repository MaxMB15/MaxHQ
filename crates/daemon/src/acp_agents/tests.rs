// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use maxhq_protocol::ids::{AgentId, DeviceId, WorkspaceId};
use maxhq_protocol::model::{Agent, AgentLink, Control, PauseReason};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;

use super::*;
use crate::secrets::testing::hub;
use crate::secrets::DIR;
use crate::testing::TempDir;

/// A value's shape, with nothing real in it.
const VALUE: &str = "test-value-not-a-real-key";

/// What the fake agent answers `initialize` with.
const REPLY: &str = r#"{"jsonrpc":"2.0","id":"probe","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true}},"authMethods":[{"id":"browser","name":"Sign in","description":"Opens a browser"}]}}"#;

struct Fixture {
    temp: TempDir,
    hub: Hub,
    db: Db,
    store: AcpAgents,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new();
        let hub = hub();
        let db = Db::open_in_memory().unwrap();
        let store = open(&temp, &hub, &db);
        Self {
            temp,
            hub,
            db,
            store,
        }
    }

    /// A fake agent: a script that reads the request, then runs `body`.
    fn script(&self, name: &str, body: &str) -> String {
        let path = self.temp.path().join(name);
        fs::write(&path, format!("#!/bin/sh\nread line\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    fn shown(&self, id: AcpAgentId) -> Option<AcpAgent> {
        self.store.find(id)
    }

    fn account(&self, account: AccountId) -> Option<Account> {
        match self.hub.get(&EntityKey::Account(account)) {
            Some(Entity::Account(account)) => Some(account),
            _ => None,
        }
    }
}

fn open(temp: &TempDir, hub: &Hub, db: &Db) -> AcpAgents {
    let secrets = Secrets::open(temp.path().join(DIR), hub.clone()).unwrap();
    AcpAgents::with_timeout(hub.clone(), db.clone(), secrets, Duration::from_secs(5))
}

fn definition(program: &str, env: &[&str]) -> AcpAgent {
    AcpAgent {
        id: AcpAgentId::derive("gemini"),
        device: DeviceId::derive("elsewhere"),
        name: " Gemini ".to_owned(),
        program: program.to_owned(),
        args: vec!["--acp".to_owned()],
        env: env.iter().map(|name| (*name).to_owned()).collect(),
        containers: false,
        problem: Some("from the app".to_owned()),
        skills: Default::default(),
    }
}

fn value(name: &str, value: &str) -> AcpEnvValue {
    AcpEnvValue {
        name: name.to_owned(),
        value: Secret::new(value.to_owned()),
    }
}

fn reason(outcome: ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("not refused: {other:?}"),
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[tokio::test]
async fn saving_probes_the_agent_and_shows_its_account() {
    let f = Fixture::new();
    // The agent answers only when its value reached it.
    let body = format!("[ \"$API_KEY\" = \"{VALUE}\" ] || exit 1\necho '{REPLY}'");
    let program = f.script("agent", &body);
    let outcome = f
        .store
        .set(
            definition(&program, &["API_KEY"]),
            vec![value("API_KEY", VALUE)],
        )
        .await;
    assert_eq!(outcome, ActionOutcome::Done);

    let agent = f.shown(AcpAgentId::derive("gemini")).unwrap();
    assert_eq!(agent.name, "Gemini");
    assert_eq!(agent.device, f.hub.device().id);
    assert_eq!(agent.problem, None);
    let account = f.account(account_of(&agent)).unwrap();
    assert_eq!(account.vendor, Vendor::Acp);
    assert_eq!(account.label, "Gemini");
    assert_eq!(account.login, LoginKind::AcpCommand);
    let features = account.acp.unwrap();
    assert!(features.images && features.load && !features.plan);
    assert_eq!(features.auth[0].id, "browser");
    assert_eq!(
        features.auth[0].description.as_deref(),
        Some("Opens a browser")
    );

    // The value is in the secret store alone.
    let snapshot = serde_json::to_string(&f.hub.snapshot()).unwrap();
    assert!(!snapshot.contains(VALUE));
    let records = f.db.acp_agents().unwrap().join("\n");
    assert!(records.contains("API_KEY") && !records.contains(VALUE));

    // A restart shows it again, with its value.
    let hub = hub();
    let reopened = open(&f.temp, &hub, &f.db);
    assert_eq!(reopened.find(agent.id).unwrap().name, "Gemini");
    let secrets = Secrets::open(f.temp.path().join(DIR), hub.clone()).unwrap();
    assert_eq!(secrets.acp_env(agent.id)[0].value.expose(), VALUE);
}

#[tokio::test]
async fn a_value_left_out_is_kept_and_a_name_left_out_loses_it() {
    let f = Fixture::new();
    let program = f.script("agent", &format!("echo '{REPLY}'"));
    let both = definition(&program, &["A", "B"]);
    let values = vec![value("A", "1"), value("B", "2")];
    assert_eq!(f.store.set(both.clone(), values).await, ActionOutcome::Done);
    assert_eq!(f.store.set(both, Vec::new()).await, ActionOutcome::Done);
    let secrets = &f.store.inner.secrets;
    assert_eq!(secrets.acp_env(AcpAgentId::derive("gemini")).len(), 2);

    let one = definition(&program, &["B"]);
    assert_eq!(f.store.set(one, Vec::new()).await, ActionOutcome::Done);
    let kept = secrets.acp_env(AcpAgentId::derive("gemini"));
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "B");
}

#[tokio::test]
async fn a_probe_reads_a_v2_agent() {
    let f = Fixture::new();
    // The agent answers only a request that asks in both v1 and v2.
    let reply = r#"{"jsonrpc":"2.0","id":"probe","result":{"protocolVersion":2,"info":{"name":"gemini"},"capabilities":{"session":{"prompt":{"image":{}}}},"authMethods":[{"methodId":"browser","name":"Sign in","type":"agent"}]}}"#;
    let body = format!(
        "case \"$line\" in *'\"info\"'*'\"clientCapabilities\"'*|*'\"clientCapabilities\"'*'\"info\"'*) ;; *) exit 1 ;; esac\necho '{reply}'"
    );
    let program = f.script("agent", &body);
    let outcome = f.store.set(definition(&program, &[]), Vec::new()).await;
    assert_eq!(outcome, ActionOutcome::Done);
    let agent = f.shown(AcpAgentId::derive("gemini")).unwrap();
    assert_eq!(agent.problem, None);
    let features = f.account(account_of(&agent)).unwrap().acp.unwrap();
    assert!(features.images && features.load);
    assert_eq!(features.auth[0].id, "browser");
}

#[tokio::test]
async fn a_failed_probe_still_saves_with_why() {
    let f = Fixture::new();
    let missing = f.temp.path().join("not-here").display().to_string();
    let quiet = f.script("quiet", "exit 0");
    let newer = f.script(
        "newer",
        r#"echo '{"jsonrpc":"2.0","id":"probe","result":{"protocolVersion":3}}'"#,
    );
    let refusing = f.script(
        "refusing",
        r#"echo '{"jsonrpc":"2.0","id":"probe","error":{"code":-32603,"message":"broken"}}'"#,
    );
    let cases = [
        (
            missing.clone(),
            format!("{missing} isn't installed on this device"),
        ),
        (
            quiet.clone(),
            format!("{quiet} exited without answering initialize"),
        ),
        (
            newer.clone(),
            format!("{newer} speaks ACP version 3, which MaxHQ doesn't"),
        ),
        (
            refusing.clone(),
            format!("{refusing} refused initialize: broken"),
        ),
    ];
    for (program, problem) in cases {
        let outcome = f.store.set(definition(&program, &[]), Vec::new()).await;
        assert_eq!(outcome, ActionOutcome::Done);
        let agent = f.shown(AcpAgentId::derive("gemini")).unwrap();
        assert_eq!(agent.problem.as_deref(), Some(problem.as_str()));
        assert_eq!(f.account(account_of(&agent)).unwrap().acp, None);
    }
}

#[tokio::test]
async fn a_probe_that_hangs_gives_up() {
    let temp = TempDir::new();
    let hub = hub();
    let db = Db::open_in_memory().unwrap();
    let secrets = Secrets::open(temp.path().join(DIR), hub.clone()).unwrap();
    let store = AcpAgents::with_timeout(hub, db, secrets, Duration::from_millis(200));
    let path = temp.path().join("slow");
    fs::write(&path, "#!/bin/sh\nexec sleep 30\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let program = path.display().to_string();
    assert_eq!(
        store.set(definition(&program, &[]), Vec::new()).await,
        ActionOutcome::Done
    );
    let agent = store.find(AcpAgentId::derive("gemini")).unwrap();
    assert_eq!(
        agent.problem,
        Some(format!("{program} didn't answer initialize"))
    );
}

#[tokio::test]
async fn a_definition_that_cant_run_is_refused() {
    let f = Fixture::new();
    let cases = [
        (
            definition("  ", &[]),
            Vec::new(),
            "Give the program to run.",
        ),
        (
            AcpAgent {
                name: " ".to_owned(),
                ..definition("agent", &[])
            },
            Vec::new(),
            "Give the agent a name.",
        ),
        (
            definition("agent", &["1X"]),
            Vec::new(),
            "\"1X\" isn't a variable name.",
        ),
        (
            definition("agent", &["A", "A"]),
            Vec::new(),
            "A is listed twice.",
        ),
        (
            definition("agent", &["A"]),
            vec![value("B", "1")],
            "B isn't one of the agent's variables.",
        ),
        (
            AcpAgent {
                args: vec!["a\0b".to_owned()],
                ..definition("agent", &[])
            },
            Vec::new(),
            "The program and its arguments can't hold that.",
        ),
    ];
    for (agent, env, why) in cases {
        assert_eq!(reason(f.store.set(agent, env).await), why);
    }
    assert!(f.shown(AcpAgentId::derive("gemini")).is_none());
    assert!(files(&f.temp.path().join(DIR)).is_empty());
}

fn agent_on(account: AccountId, activity: Activity) -> Entity {
    Entity::Agent(Agent {
        id: AgentId::derive("a"),
        workspace: WorkspaceId::derive("w"),
        parent: None,
        vendor: Vendor::Acp,
        account: Some(account),
        model: None,
        permission: None,
        session: None,
        vendor_session_id: None,
        control: Control::Observed,
        pid: None,
        link: AgentLink::Live,
        title: "maxhq".into(),
        activity,
        context: None,
        started_at: Timestamp(1),
        last_activity_at: Timestamp(1),
        branch: None,
        inbox: Default::default(),
        origin: None,
        pulls: Vec::new(),
        taking_over: false,
        asleep: false,
        effort: None,
        running: None,
        moved: None,
    })
}

#[tokio::test]
async fn delete_waits_for_its_agents_to_stop() {
    let f = Fixture::new();
    let program = f.script("agent", &format!("echo '{REPLY}'"));
    let saved = f
        .store
        .set(definition(&program, &["A"]), vec![value("A", VALUE)])
        .await;
    assert_eq!(saved, ActionOutcome::Done);
    let id = AcpAgentId::derive("gemini");
    let account = account_of(&f.shown(id).unwrap());

    f.hub.apply([Change::Upsert {
        value: agent_on(account, Activity::Idle),
    }]);
    assert_eq!(
        reason(f.store.delete(id)),
        "Agents of Gemini are still running. Stop them first."
    );

    let paused = Activity::Paused {
        reason: PauseReason::ByYou,
    };
    f.hub.apply([Change::Upsert {
        value: agent_on(account, paused),
    }]);
    assert_eq!(f.store.delete(id), ActionOutcome::Done);
    assert!(f.shown(id).is_none());
    assert!(f.account(account).is_none());
    assert!(f.db.acp_agents().unwrap().is_empty());
    assert!(f.store.inner.secrets.acp_env(id).is_empty());
    assert!(files(&f.temp.path().join(DIR)).is_empty());

    assert_eq!(
        reason(f.store.delete(id)),
        "This device doesn't have that ACP agent."
    );
}

#[tokio::test]
async fn what_a_session_finds_replaces_the_probes_problem() {
    let f = Fixture::new();
    let quiet = f.script("quiet", "exit 0");
    let saved = f.store.set(definition(&quiet, &[]), Vec::new()).await;
    assert_eq!(saved, ActionOutcome::Done);
    let id = AcpAgentId::derive("gemini");
    let account = account_of(&f.shown(id).unwrap());
    assert!(f.shown(id).unwrap().problem.is_some());

    let features = AcpFeatures {
        images: false,
        load: true,
        plan: true,
        auth: Vec::new(),
    };
    f.store.found(account, features.clone());
    assert_eq!(f.shown(id).unwrap().problem, None);
    assert_eq!(f.account(account).unwrap().acp, Some(features.clone()));

    // A restart shows it again.
    let reopened = open(&f.temp, &hub(), &f.db);
    let account = reopened.inner.hub.get(&EntityKey::Account(account));
    assert!(matches!(account, Some(Entity::Account(shown)) if shown.acp == Some(features)));
}
