// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Incus client against the fake daemon.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;

use super::fake::{ExecReply, FakeIncus, State};
use super::*;

/// A managed instance's keys.
const MANAGED: &[(&str, &str)] = &[(MANAGED_KEY, "true")];

fn name(name: &str) -> InstanceName {
    InstanceName::new(name).unwrap()
}

#[test]
fn only_mhq_names_are_instance_names() {
    assert_eq!(
        InstanceName::new("mhq-maxhq").unwrap().as_str(),
        "mhq-maxhq"
    );
    assert!(InstanceName::new("mhq-a1-b2").is_ok());
    assert!(InstanceName::new(&format!("mhq-{}", "a".repeat(59))).is_ok());

    for name in [
        "ws-maxhq",
        "mhq-",
        "mhq-a-",
        "mhq--",
        "mhq-Upper",
        "mhq-under_score",
        "mhq-dot.ted",
        "xmhq-maxhq",
        "MHQ-maxhq",
        "",
    ] {
        assert_eq!(
            InstanceName::new(name),
            Err(NameError(name.to_owned())),
            "{name}"
        );
    }
    assert!(InstanceName::new(&format!("mhq-{}", "a".repeat(60))).is_err());
}

#[tokio::test]
async fn reads_the_recorded_server_networks_and_pools() {
    let fake = FakeIncus::start(State::default());
    let incus = Incus::new(&fake.socket);
    incus.ping().await.unwrap();

    let networks = incus.networks().await.unwrap();
    let bridges: Vec<(&str, Option<&str>)> = networks
        .iter()
        .filter(|n| n.is_managed_bridge())
        .map(|n| (n.name.as_str(), n.ipv4()))
        .collect();
    assert_eq!(
        bridges,
        [
            ("agentbr0", Some("10.100.2.1/24")),
            ("incusbr-blue", Some("10.100.3.1/24")),
            ("incusbr-green", Some("10.100.4.1/24")),
            ("incusbr-red", Some("10.100.1.1/24")),
            ("incusbr0", Some("10.100.0.1/24")),
        ]
    );
    assert!(networks.iter().all(|n| n.ipv6().is_none()));

    let pools = incus.pools().await.unwrap();
    let pools: Vec<(&str, &str)> = pools
        .iter()
        .map(|p| (p.name.as_str(), p.driver.as_str()))
        .collect();
    assert_eq!(pools, [("agents", "btrfs"), ("default", "dir")]);
}

#[tokio::test]
async fn a_pool_is_unavailable_when_missing_or_unreadable() {
    // The recorded pools' sources are real folders on the machine they were
    // recorded on, so point the pool at one that exists nowhere.
    let mut state = State::default();
    state.pool_source = Some("/no/such/maxhq/pool".to_owned());
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    assert!(!incus.pool_available("agents").await.unwrap());
    assert!(!incus.pool_available("nope").await.unwrap());

    let pool = Pool {
        config: [("source".to_owned(), "/".to_owned())].into(),
        ..Pool::default()
    };
    assert!(source_present(&pool));
    let missing = Pool {
        config: [("source".to_owned(), "/no/such/maxhq/pool".to_owned())].into(),
        ..Pool::default()
    };
    assert!(!source_present(&missing));
    assert!(source_present(&Pool::default()));
}

#[tokio::test]
async fn the_guard_refuses_unmarked_instances() {
    let mut state = State::default();
    state.add("mhq-plain", "Running", &[]);
    state.add("mhq-other", "Running", &[(MANAGED_KEY, "false")]);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);

    for instance in ["mhq-plain", "mhq-other"] {
        let instance = name(instance);
        let refused = [
            incus
                .set_state(&instance, StateAction::Stop, Duration::from_secs(1))
                .await,
            incus.force_stop(&instance).await,
            incus.delete(&instance).await,
            incus
                .set_config(&instance, [("user.x".into(), "y".into())].into())
                .await,
            incus
                .exec(&instance, &["true"], None, None)
                .await
                .map(|_| ()),
            incus
                .exec(&instance, &["cat"], None, Some(b"x".to_vec()))
                .await
                .map(|_| ()),
            incus
                .push_file(&instance, "/usr/local/lib/maxhq/maxd", vec![1], 0o755)
                .await,
            incus
                .append_file(&instance, "/tmp/app", vec![1], 0o644)
                .await,
            incus.make_folder(&instance, "/tmp/app.d", 0o755).await,
            incus.snapshot(&instance, "clean").await,
            incus.restore(&instance, "clean").await,
            incus.delete_snapshot(&instance, "clean").await,
            incus.copy(&instance, None, &name("mhq-other")).await,
        ];
        for result in refused {
            assert!(
                matches!(result, Err(IncusError::Unmanaged(ref n)) if *n == instance),
                "{result:?}"
            );
        }
    }
    let state = fake.state();
    assert!(state.writes().is_empty(), "{:?}", state.writes());
    assert!(state.execs.is_empty());
    assert!(state.files.is_empty());
}

#[tokio::test]
async fn a_missing_instance_is_not_found() {
    let fake = FakeIncus::start(State::default());
    let incus = Incus::new(&fake.socket);
    assert_eq!(incus.instance(&name("mhq-gone")).await.unwrap(), None);
    let error = incus.delete(&name("mhq-gone")).await.unwrap_err();
    assert!(error.is_not_found(), "{error}");
}

#[tokio::test]
async fn creates_an_instance_with_no_profiles_and_the_guest_proxy() {
    let fake = FakeIncus::start(State::default());
    let incus = Incus::new(&fake.socket);
    let folder = std::env::temp_dir();
    let spec = InstanceSpec {
        image: "images:debian/13".into(),
        config: [
            ("security.idmap.isolated".to_owned(), "true".to_owned()),
            ("user.maxhq.project".to_owned(), "p".to_owned()),
        ]
        .into(),
        pool: "agents".into(),
        network: "agentbr0".into(),
        guest_socket: folder.join("mhq-new.sock"),
    };
    incus.create(&name("mhq-new"), spec).await.unwrap();

    let state = fake.state();
    let create = state
        .log
        .iter()
        .find(|l| l.method == "POST" && l.target == "/1.0/instances")
        .unwrap();
    let body = &create.body;
    assert_eq!(body["name"], "mhq-new");
    assert_eq!(body["profiles"], json!([]));
    assert_eq!(body["source"]["alias"], "debian/13");
    assert_eq!(body["source"]["server"], IMAGE_SERVER);
    assert_eq!(body["config"][MANAGED_KEY], "true");
    assert_eq!(body["config"]["security.idmap.isolated"], "true");
    assert_eq!(body["devices"]["root"]["pool"], "agents");
    assert_eq!(body["devices"]["eth0"]["network"], "agentbr0");
    let proxy = &body["devices"]["maxhq-guest"];
    assert_eq!(proxy["type"], "proxy");
    assert_eq!(proxy["bind"], "host");
    assert_eq!(
        proxy["listen"],
        format!("unix:{}", folder.join("mhq-new.sock").display())
    );
    assert_eq!(proxy["connect"], "unix:/run/maxhq/host.sock");
    assert_eq!(proxy["mode"], "0600");
    assert!(state.instances.contains_key("mhq-new"));
}

#[tokio::test]
async fn changes_state_keys_and_deletes() {
    let mut state = State::default();
    state.add("mhq-box", "Stopped", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let instance = name("mhq-box");

    incus
        .set_state(&instance, StateAction::Start, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(incus
        .instance(&instance)
        .await
        .unwrap()
        .unwrap()
        .is_running());

    // A running instance can't be deleted, and the failed operation says so.
    let error = incus.delete(&instance).await.unwrap_err();
    assert!(error.to_string().contains("running"), "{error}");

    incus
        .set_config(
            &instance,
            [("user.maxhq.recipe".to_owned(), "1".to_owned())].into(),
        )
        .await
        .unwrap();
    let read = incus.instance(&instance).await.unwrap().unwrap();
    assert_eq!(read.key("user.maxhq.recipe"), Some("1"));
    assert!(read.is_managed());

    incus.force_stop(&instance).await.unwrap();
    incus.delete(&instance).await.unwrap();
    assert_eq!(incus.instance(&instance).await.unwrap(), None);

    let state = fake.state();
    let stop = state
        .log
        .iter()
        .filter(|l| l.target == "/1.0/instances/mhq-box/state")
        .nth(1)
        .unwrap();
    assert_eq!(stop.body["force"], true);
}

#[tokio::test]
async fn exec_deletes_the_logs_when_one_is_past_the_reply_limit() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    state.exec = Some(Arc::new(|_| ExecReply {
        status: 0,
        stdout: vec![0; http::MAX_REPLY + 1],
        stderr: b"warn\n".to_vec(),
    }));
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);

    let read = incus
        .exec(&name("mhq-box"), &["cat", "big"], Some(AGENT_UID), None)
        .await;
    assert!(read.is_err());
    assert!(fake.state().exec_logs.is_empty());
}

#[tokio::test]
async fn exec_records_output_and_deletes_the_logs() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    state.exec = Some(Arc::new(|call| {
        if call.command == ["whoami"] {
            ExecReply {
                status: 3,
                stdout: b"agent\n".to_vec(),
                stderr: b"warn\n".to_vec(),
            }
        } else {
            ExecReply::default()
        }
    }));
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);

    let output = incus
        .exec(&name("mhq-box"), &["whoami"], Some(AGENT_UID), None)
        .await
        .unwrap();
    assert_eq!(output.status, 3);
    assert_eq!(output.stdout, b"agent\n");
    assert_eq!(output.stderr, b"warn\n");

    let state = fake.state();
    let call = &state.execs[0];
    assert_eq!(call.user, AGENT_UID);
    assert_eq!(call.cwd, AGENT_HOME);
    assert!(!call.streamed);
    assert_eq!(
        call.environment,
        BTreeMap::from([
            ("HOME".to_owned(), AGENT_HOME.to_owned()),
            ("LOGNAME".to_owned(), AGENT.to_owned()),
            ("USER".to_owned(), AGENT.to_owned()),
        ])
    );
    assert!(state.exec_logs.is_empty(), "the logs were deleted");
}

#[tokio::test]
async fn exec_with_stdin_streams_it_over_websockets() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    state.exec = Some(Arc::new(|call| ExecReply {
        status: 0,
        stdout: call.stdin.iter().rev().copied().collect(),
        stderr: b"done".to_vec(),
    }));
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);

    let output = incus
        .exec(
            &name("mhq-box"),
            &["sh", "-c", "read x"],
            None,
            Some(b"test-not-a-real-token".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, b"nekot-laer-a-ton-tset");
    assert_eq!(output.stderr, b"done");

    let state = fake.state();
    let call = &state.execs[0];
    assert!(call.streamed);
    assert_eq!(call.stdin, b"test-not-a-real-token");
    assert_eq!(call.user, 0);
    // The secret never shows up in the request.
    for logged in &state.log {
        assert!(!logged.body.to_string().contains("test-not-a-real-token"));
    }
}

#[tokio::test]
async fn exec_counts_126_and_127_as_exits() {
    // Incus marks these as failed operations, but the command ran, as env
    // does when the program it runs isn't there.
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    state.exec = Some(Arc::new(|call| ExecReply {
        status: call.command[1].parse().unwrap(),
        stdout: Vec::new(),
        stderr: b"env: '/opt/tool': No such file or directory\n".to_vec(),
    }));
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);

    for code in [126, 127] {
        let arg = code.to_string();
        let cmd = ["env", arg.as_str()];
        let recorded = incus
            .exec(&name("mhq-box"), &cmd, None, None)
            .await
            .unwrap();
        assert_eq!(recorded.status, code);
        assert_eq!(
            recorded.stderr,
            b"env: '/opt/tool': No such file or directory\n"
        );
        let streamed = incus
            .exec(&name("mhq-box"), &cmd, None, Some(b"x".to_vec()))
            .await
            .unwrap();
        assert_eq!(streamed.status, code);
        assert_eq!(
            streamed.stderr,
            b"env: '/opt/tool': No such file or directory\n"
        );
    }
    assert!(fake.state().exec_logs.is_empty(), "the logs were deleted");
}

#[tokio::test]
async fn pushes_files_with_their_mode_as_root() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    incus
        .push_file(
            &name("mhq-box"),
            "/usr/local/lib/maxhq/maxd.new",
            b"\x7fELF".to_vec(),
            0o755,
        )
        .await
        .unwrap();
    let state = fake.state();
    let pushed = &state.files[&(
        "mhq-box".to_owned(),
        "/usr/local/lib/maxhq/maxd.new".to_owned(),
    )];
    assert_eq!(pushed.mode, "0755");
    assert_eq!(pushed.owner, ("0".to_owned(), "0".to_owned()));
    assert_eq!(pushed.bytes, b"\x7fELF");
}

#[tokio::test]
async fn appends_to_a_file_and_makes_folders() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let instance = name("mhq-box");
    incus.make_folder(&instance, "/up", 0o700).await.unwrap();
    incus
        .push_file(&instance, "/up/app", b"ab".to_vec(), 0o755)
        .await
        .unwrap();
    incus
        .append_file(&instance, "/up/app", b"cd".to_vec(), 0o755)
        .await
        .unwrap();
    let state = fake.state();
    let file = |path: &str| &state.files[&("mhq-box".to_owned(), path.to_owned())];
    assert_eq!(file("/up").kind, "directory");
    assert_eq!(file("/up").mode, "0700");
    assert_eq!(file("/up/app").kind, "file");
    assert_eq!(file("/up/app").bytes, b"abcd");
}

#[tokio::test]
async fn a_restore_takes_the_disk_back_to_its_snapshot() {
    let mut state = State::default();
    state.add("mhq-box", "Stopped", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let instance = name("mhq-box");
    incus
        .push_file(&instance, "/kept", b"1".to_vec(), 0o644)
        .await
        .unwrap();
    incus.snapshot(&instance, "clean").await.unwrap();
    incus
        .push_file(&instance, "/kept", b"2".to_vec(), 0o644)
        .await
        .unwrap();
    incus
        .push_file(&instance, "/later", b"3".to_vec(), 0o644)
        .await
        .unwrap();
    incus.restore(&instance, "clean").await.unwrap();
    {
        let state = fake.state();
        let paths: Vec<&str> = state.files.keys().map(|(_, path)| path.as_str()).collect();
        assert_eq!(paths, ["/kept"]);
        assert_eq!(
            state.files[&("mhq-box".to_owned(), "/kept".to_owned())].bytes,
            b"1"
        );
        let writes: Vec<(&str, &str)> = state
            .writes()
            .into_iter()
            .map(|l| (l.method.as_str(), l.target.as_str()))
            .filter(|(_, target)| !target.contains("/files"))
            .collect();
        assert_eq!(
            writes,
            [
                ("POST", "/1.0/instances/mhq-box/snapshots"),
                ("PUT", "/1.0/instances/mhq-box"),
            ]
        );
        let snapshot = state
            .log
            .iter()
            .find(|l| l.target.ends_with("/snapshots"))
            .unwrap();
        assert_eq!(
            snapshot.body,
            json!({ "name": "clean", "stateful": false, "expires_at": NEVER })
        );
    }

    // A snapshot that isn't there fails the restore.
    let error = incus.restore(&instance, "other").await.unwrap_err();
    assert!(error.to_string().contains("Snapshot not found"), "{error}");
}

#[tokio::test]
async fn a_restore_takes_the_keys_back_and_a_named_snapshot_never_expires() {
    let mut state = State::default();
    state.add(
        "mhq-box",
        "Stopped",
        &[
            (MANAGED_KEY, "true"),
            ("user.maxhq.agents", "a"),
            ("snapshots.expiry", "7d"),
        ],
    );
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let instance = name("mhq-box");
    incus.snapshot(&instance, "mhq-before").await.unwrap();
    let mut keys = BTreeMap::new();
    keys.insert("user.maxhq.agents".to_owned(), "b".to_owned());
    incus.set_config(&instance, keys).await.unwrap();
    incus.restore(&instance, "mhq-before").await.unwrap();
    let back = incus.instance(&instance).await.unwrap().unwrap();
    assert_eq!(back.key("user.maxhq.agents"), Some("a"));
    let listed = incus.snapshots(&instance).await.unwrap();
    assert_eq!(listed[0].expires_at, NEVER);
}

#[tokio::test]
async fn a_copy_gets_its_own_guest_socket() {
    let mut state = State::default();
    state.add("mhq-box", "Stopped", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let copied = name("mhq-box-2");
    incus.copy(&name("mhq-box"), None, &copied).await.unwrap();
    let socket = std::env::temp_dir().join("mhq-box-2.sock");
    incus.set_guest_socket(&copied, &socket).await.unwrap();
    let state = fake.state();
    let proxy = &state.instances["mhq-box-2"]["devices"]["maxhq-guest"];
    assert_eq!(proxy["listen"], format!("unix:{}", socket.display()));
    assert_eq!(proxy["connect"], "unix:/run/maxhq/host.sock");
    assert_eq!(proxy["mode"], "0600");
}

#[tokio::test]
async fn snapshots_list_copy_and_delete() {
    let mut state = State::default();
    state.add("mhq-box", "Running", MANAGED);
    let fake = FakeIncus::start(state);
    let incus = Incus::new(&fake.socket);
    let instance = name("mhq-box");
    incus
        .push_file(&instance, "/kept", b"1".to_vec(), 0o644)
        .await
        .unwrap();
    incus.snapshot(&instance, "auto-0001").await.unwrap();
    incus
        .push_file(&instance, "/kept", b"2".to_vec(), 0o644)
        .await
        .unwrap();
    let listed = incus.snapshots(&instance).await.unwrap();
    assert_eq!(
        listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["auto-0001"]
    );
    assert_eq!(listed[0].created_at, "2026-11-02T00:00:00Z");

    // A copy from the snapshot has the files then, and starts stopped.
    let copied = name("mhq-box-2");
    incus
        .copy(&instance, Some("auto-0001"), &copied)
        .await
        .unwrap();
    let made = incus.instance(&copied).await.unwrap().unwrap();
    assert!(made.is_managed() && !made.is_running());
    // A copy of the instance itself has its files now.
    let live = name("mhq-box-3");
    incus.copy(&instance, None, &live).await.unwrap();
    {
        let state = fake.state();
        let bytes = |instance: &str| &state.files[&(instance.to_owned(), "/kept".to_owned())].bytes;
        assert_eq!(bytes("mhq-box-2"), b"1");
        assert_eq!(bytes("mhq-box-3"), b"2");
        let body = &state
            .log
            .iter()
            .rfind(|l| l.method == "POST" && l.target == "/1.0/instances")
            .unwrap()
            .body;
        assert_eq!(body["source"]["source"], json!("mhq-box"));
    }
    let error = incus
        .copy(&instance, Some("gone"), &name("mhq-box-4"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Snapshot not found"), "{error}");

    incus.delete_snapshot(&instance, "auto-0001").await.unwrap();
    assert!(incus.snapshots(&instance).await.unwrap().is_empty());
    let error = incus
        .delete_snapshot(&instance, "auto-0001")
        .await
        .unwrap_err();
    assert!(error.is_not_found(), "{error}");
}

#[tokio::test]
async fn lifecycle_events_arrive_as_they_happen() {
    let fake = FakeIncus::start(State::default());
    let incus = Incus::new(&fake.socket);
    let mut events = incus.lifecycle();
    let first = tokio::spawn(async move {
        let event = events.next().await;
        (event, events)
    });
    for _ in 0..200 {
        if fake.listeners() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fake.send_event("mhq-box", "instance-started");
    let (event, _events) = first.await.unwrap();
    assert_eq!(
        event,
        Some(Lifecycle {
            instance: "mhq-box".into(),
            action: "instance-started".into(),
        })
    );
}

#[test]
fn reads_lifecycle_events_by_name_or_source() {
    let named = br#"{"type":"lifecycle","metadata":{"action":"instance-stopped","source":"/1.0/instances/mhq-a","name":"mhq-a"}}"#;
    assert_eq!(
        parse_event(named),
        Some(Lifecycle {
            instance: "mhq-a".into(),
            action: "instance-stopped".into()
        })
    );
    let sourced = br#"{"type":"lifecycle","metadata":{"action":"instance-started","source":"/1.0/instances/mhq-b?project=default"}}"#;
    assert_eq!(parse_event(sourced).unwrap().instance, "mhq-b");
    for other in [
        &br#"{"type":"lifecycle","metadata":{"action":"network-updated","source":"/1.0/networks/x","name":"x"}}"#[..],
        br#"{"type":"logging","metadata":{"message":"x"}}"#,
        b"not json",
    ] {
        assert_eq!(parse_event(other), None);
    }
}

#[tokio::test]
async fn a_huge_reply_is_refused_before_it_fills_memory() {
    let mut state = State::default();
    state.exec_logs.insert(
        "/1.0/instances/mhq-a/logs/big".to_owned(),
        vec![b'x'; http::MAX_REPLY + 1],
    );
    state
        .exec_logs
        .insert("/1.0/instances/mhq-a/logs/fine".to_owned(), b"ok".to_vec());
    let fake = FakeIncus::start(state);
    let read = |path: &'static str| {
        let socket = fake.socket.clone();
        async move {
            http::send(
                &socket,
                hyper::Method::GET,
                path,
                &[],
                hyper::body::Bytes::new(),
                Duration::from_secs(30),
            )
            .await
        }
    };
    let fine = read("/1.0/instances/mhq-a/logs/fine").await.unwrap();
    assert_eq!(&fine.body[..], b"ok");
    assert!(read("/1.0/instances/mhq-a/logs/big").await.is_err());
}

#[test]
fn an_exec_stream_keeps_no_more_than_its_limit() {
    let mut out = Vec::new();
    keep_output(&mut out, b"abcd", 6);
    assert_eq!(out, b"abcd");
    keep_output(&mut out, b"efgh", 6);
    assert_eq!(out, b"abcdef");
    keep_output(&mut out, b"ijkl", 6);
    assert_eq!(out, b"abcdef");
}
