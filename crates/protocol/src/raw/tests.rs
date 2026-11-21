// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use serde_json::{json, Value};

use super::{FromRaw, ITEM_FIELDS, MAX_SMALL_LEN, MAX_VALUES};
use crate::agents::PeerRequest;
use crate::message::{ActionOutcome, ClientMessage, ServerMessage};
use crate::timeline::{ItemBody, TimelineItem};

const ONE: &str = "0199a7c2-0000-7000-8000-000000000001";
const TWO: &str = "0199a7c2-0000-7000-8000-000000000002";
const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";

/// What reading a piece at a time gives, which must be what the derived
/// reader gives.
fn read(message: &Value) -> Option<ServerMessage> {
    let json = message.to_string();
    let read = ServerMessage::read(json.as_bytes());
    assert_eq!(
        read,
        serde_json::from_str::<ServerMessage>(&json).ok(),
        "{message}"
    );
    read
}

/// A timeline item's own fields, then its body's.
fn item(id: &str, body: Value) -> Value {
    let mut item = json!({
        "id": id,
        "agent": ONE,
        "turn": null,
        "parent": null,
        "ordinal": 3,
        "status": "completed",
        "started_at": null,
        "completed_at": null,
    });
    let (Value::Object(item_fields), Value::Object(body)) = (&mut item, body) else {
        panic!("objects only");
    };
    item_fields.extend(body);
    item
}

fn hidden(device: &str) -> Value {
    json!({ "entity": "hidden_projects", "device": device, "projects": [] })
}

#[test]
fn timeline_items_read_as_the_derived_reader_reads_them() {
    let bodies = [
        serde_json::to_value(ItemBody::Interrupted).unwrap(),
        serde_json::to_value(ItemBody::AgentOpenedFile {
            path: "/tmp/example/src/main.rs".into(),
            line: Some(12),
            outside: false,
        })
        .unwrap(),
        json!({ "kind": "assistant_message", "text": "Done", "streaming": false }),
        json!({ "kind": "hologram", "beam": 3 }),
        json!({ "kind": "hologram", "type": "beam" }),
        json!({ "kind": "x".repeat(300) }),
        json!({ "beam": 3 }),
        json!({ "kind": 4 }),
    ];
    for body in bodies {
        let item = item("claude:msg_01", body).to_string();
        let raw = TimelineItem::from_raw(&item).unwrap();
        assert_eq!(raw, serde_json::from_str::<TimelineItem>(&item).unwrap());
    }
    // The item's own fields still have to read.
    let broken = json!({ "id": "claude:msg_01", "kind": "interrupted" }).to_string();
    assert!(TimelineItem::from_raw(&broken).is_err());
    assert!(serde_json::from_str::<TimelineItem>(&broken).is_err());
}

#[test]
fn no_item_body_field_shares_a_name_with_the_item() {
    // A body is read from the whole item, so none of its fields may share
    // a name with the item's own, and none may refuse fields it doesn't
    // know.
    let schema = serde_json::to_value(schemars::schema_for!(ItemBody)).unwrap();
    let variants = schema["oneOf"].as_array().expect("one schema per kind");
    assert!(variants.len() > 20);
    for variant in variants {
        assert_ne!(variant["additionalProperties"], json!(false), "{variant}");
        let fields = variant["properties"].as_object().expect("fields");
        for field in ITEM_FIELDS {
            assert!(!fields.contains_key(field), "{field} in {variant}");
        }
    }
}

#[test]
fn messages_read_as_the_derived_reader_reads_them() {
    let messages = [
        json!({
            "type": "snapshot",
            "seq": 4,
            "entities": [{ "entity": "hologram", "id": "h1" }, hidden(ONE), 7, null],
        }),
        json!({
            "type": "delta",
            "seq": 5,
            "changes": [
                { "op": "upsert", "value": { "entity": "hologram" } },
                { "value": hidden(TWO), "op": "upsert" },
                { "op": "remove", "key": { "entity": "hidden_projects", "id": ONE } },
                { "op": "upsert" },
                { "op": "rename", "value": hidden(ONE) },
            ],
        }),
        json!({
            "type": "timeline",
            "agent": ONE,
            "update": {
                "type": "events",
                "seq": 9,
                "events": [
                    { "type": "upsert", "item": item("a", json!({ "kind": "hologram" })) },
                    { "item": item("b", json!({ "kind": "interrupted" })), "type": "upsert" },
                    { "type": "upsert", "item": { "id": "c" } },
                    { "type": "teleport", "id": "c" },
                    { "type": "append", "id": "a", "field": "text", "text": "more" },
                    { "type": "remove", "id": "b" },
                ],
            },
        }),
        json!({
            "type": "timeline",
            "agent": ONE,
            "update": {
                "has_older": true,
                "items": [item("a", json!({ "kind": "hologram" }))],
                "seq": 2,
                "type": "reset",
            },
        }),
        json!({
            "type": "timeline",
            "agent": ONE,
            "update": {
                "type": "reset",
                "seq": 2,
                "items": [item("a", json!({ "kind": "interrupted" })), { "id": "b" }],
                "has_older": false,
            },
        }),
        json!({
            "type": "timeline",
            "agent": ONE,
            "update": { "type": "older", "items": [], "has_older": false },
        }),
        json!({
            "type": "timeline",
            "agent": ONE,
            "update": { "type": "rewind", "to": 3 },
        }),
        json!({
            "type": "workflow",
            "update": {
                "run": {
                    "key": { "device": ONE, "project": TWO, "workflow": "w", "run": "r" },
                    "rev": 2,
                    "nodes": [],
                    "loops": null,
                    "header": null,
                },
                "kind": "patch",
            },
            "run": { "device": ONE, "project": TWO, "workflow": "w", "run": "r" },
        }),
        json!({
            "type": "workflow",
            "run": { "device": ONE, "project": TWO, "workflow": "w", "run": "r" },
            "update": { "kind": "rewind", "run": {} },
        }),
        json!({
            "type": "fetch_manifest",
            "transfer": TWO,
            "entries": [{
                "path": "src/main.rs",
                "kind": "file",
                "executable": false,
                "size": 12,
                "sha256": null,
            }],
        }),
        json!({ "type": "action_result", "key": KEY, "outcome": { "status": "done" } }),
        json!({ "type": "action_result", "key": KEY, "outcome": { "status": "teleported" } }),
        json!({ "outcome": { "status": "done" }, "key": KEY, "type": "action_result" }),
        json!({ "type": "peer_request", "key": KEY, "request": { "kind": "dance" } }),
        json!({ "type": "pong", "nonce": 7 }),
        json!({ "type": "error", "code": "overheated", "message": "Too hot" }),
        json!({ "type": "teleport", "to": "mars" }),
        json!({ "type": 7 }),
        json!({ "seq": 4 }),
        json!([1, 2]),
    ];
    for message in &messages {
        read(message);
    }
    assert!(matches!(
        read(&messages[0]),
        Some(ServerMessage::Snapshot(snapshot)) if snapshot.entities.len() == 1
    ));
}

#[test]
fn a_large_message_of_a_kind_that_is_always_small_is_left_out() {
    let pad = "x".repeat(MAX_SMALL_LEN);
    let pong = json!({ "type": "pong", "nonce": 7, "pad": pad }).to_string();
    assert!(serde_json::from_str::<ServerMessage>(&pong).is_ok());
    assert_eq!(ServerMessage::read(pong.as_bytes()), None);
    assert_eq!(
        ServerMessage::read(br#"{"type":"pong","nonce":7}"#),
        Some(ServerMessage::Pong { nonce: 7 })
    );

    // The kinds read a piece at a time take as much as a frame does.
    let big = json!({
        "type": "snapshot",
        "seq": 1,
        "entities": [{ "entity": "hologram", "beam": pad }, hidden(ONE)],
    })
    .to_string();
    let Some(ServerMessage::Snapshot(snapshot)) = ServerMessage::read(big.as_bytes()) else {
        panic!("expected a snapshot");
    };
    assert_eq!(snapshot.entities.len(), 1);
}

#[test]
fn peer_requests_and_answers_read_a_piece_at_a_time() {
    let peer = |message: Value| {
        let json = message.to_string();
        let read = ClientMessage::read_peer(json.as_bytes()).ok();
        assert_eq!(
            read,
            serde_json::from_str::<ClientMessage>(&json).ok(),
            "{message}"
        );
        read
    };
    assert_eq!(
        peer(json!({ "type": "peer_request", "key": KEY, "request": { "kind": "dance" } })),
        Some(ClientMessage::PeerRequest {
            key: KEY.parse().unwrap(),
            request: PeerRequest::Unknown,
        })
    );
    assert_eq!(
        peer(json!({ "outcome": { "status": "done" }, "type": "peer_answer", "key": KEY })),
        Some(ClientMessage::PeerAnswer {
            key: KEY.parse().unwrap(),
            outcome: ActionOutcome::Done,
        })
    );
    assert_eq!(peer(json!({ "type": "peer_answer", "key": KEY })), None);
    assert_eq!(peer(json!({ "type": "teleport" })), None);

    // A request may be as large as a frame; anything else is refused past
    // 1 MiB.
    let pad = "x".repeat(MAX_SMALL_LEN);
    let request = json!({
        "type": "peer_request",
        "key": KEY,
        "request": { "kind": "dance", "steps": pad },
    });
    assert!(peer(request).is_some());
    let other = json!({ "type": "ping", "nonce": 7, "pad": pad }).to_string();
    assert!(ClientMessage::read_peer(other.as_bytes()).is_err());
}

#[test]
fn a_piece_with_too_many_values_is_left_out_or_read_as_unknown() {
    // Each zero is one value; the rest of a piece holds a handful.
    let zeros = |n: usize| format!("[{}0]", "0,".repeat(n - 1));
    let with = |piece: Value, n: usize| {
        let piece = piece.to_string();
        format!(r#"{},"j":{}}}"#, &piece[..piece.len() - 1], zeros(n))
    };

    // Counted, being past 64 KiB, and still read.
    let snapshot = |n| {
        format!(
            r#"{{"type":"snapshot","seq":1,"entities":[{},{}]}}"#,
            with(hidden(ONE), n),
            hidden(TWO)
        )
    };
    let entities = |n| match ServerMessage::read(snapshot(n).as_bytes()) {
        Some(ServerMessage::Snapshot(snapshot)) => snapshot.entities.len(),
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert_eq!(entities(100_000), 2);
    assert_eq!(entities(MAX_VALUES), 1);

    let body = json!({ "kind": "assistant_message", "text": "", "streaming": false });
    let read = TimelineItem::from_raw(&with(item("a", body.clone()), 100_000)).unwrap();
    assert!(matches!(read.body, ItemBody::AssistantMessage { .. }));
    let read = TimelineItem::from_raw(&with(item("a", body), MAX_VALUES)).unwrap();
    assert_eq!(
        read.body,
        ItemBody::Unknown {
            native_kind: "assistant_message".into()
        }
    );

    let result = format!(
        r#"{{"type":"action_result","key":"{KEY}","outcome":{}}}"#,
        with(json!({ "status": "done" }), MAX_VALUES)
    );
    assert_eq!(
        ServerMessage::read(result.as_bytes()),
        Some(ServerMessage::ActionResult {
            key: KEY.parse().unwrap(),
            outcome: ActionOutcome::Unknown,
        })
    );
}
