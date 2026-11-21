// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How much memory reading one message from another device or a guest
//! takes. Each message here is filled with small junk objects, which cost
//! the most to buffer. The allocator counts the whole process, so this
//! binary holds one test.

// Counting the heap takes an allocator of our own, which is unsafe to write.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use maxhq_protocol::guest::GuestToHost;
use maxhq_protocol::{ClientMessage, ServerMessage};

/// The system allocator, counting what it holds and the most it held.
struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn took(size: usize) {
    let held = HELD.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(held, Relaxed);
}

// SAFETY: each call goes to the system allocator unchanged; the counters
// only watch.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract passes on unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            took(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as for `alloc`.
        unsafe { System.dealloc(ptr, layout) };
        HELD.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let moved = unsafe { System.realloc(ptr, layout, size) };
        if !moved.is_null() {
            // Both are held while it moves, at worst.
            took(size);
            HELD.fetch_sub(layout.size(), Relaxed);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MIB: usize = 1024 * 1024;
const AGENT: &str = "0199a7c2-0000-7000-8000-0000000000e2";
const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";

/// A snapshot whose one entity has a list of junk beside its fields.
const ENTITY: &str = r#"{"type":"snapshot","seq":1,"entities":[{"entity":"hidden_projects","device":"0199a7c2-0000-7000-8000-000000000001","projects":[],"j":["#;

/// `head`, then junk objects until the message takes about `size` bytes,
/// then `tail`.
fn junk(head: &str, tail: &str, size: usize) -> Vec<u8> {
    let mut json = String::with_capacity(size + 64);
    json.push_str(head);
    json.push_str(r#"{"":0}"#);
    while json.len() + 7 + tail.len() < size {
        json.push_str(r#",{"":0}"#);
    }
    json.push_str(tail);
    json.into_bytes()
}

/// A timeline item with junk of about `size` bytes beside its body.
fn item(size: usize) -> String {
    let head = format!(
        r#"{{"id":"i","agent":"{AGENT}","turn":null,"parent":null,"ordinal":0,"status":"completed","started_at":null,"completed_at":null,"kind":"assistant_message","text":"","streaming":false,"j":["#
    );
    String::from_utf8(junk(&head, "]}", size)).unwrap()
}

/// Items of about 4 KiB each until the message takes about `size` bytes.
fn items(head: &str, tail: &str, size: usize) -> Vec<u8> {
    let one = item(4096);
    let mut json = String::from(head);
    json.push_str(&one);
    while json.len() + one.len() + 1 + tail.len() < size {
        json.push(',');
        json.push_str(&one);
    }
    json.push_str(tail);
    json.into_bytes()
}

/// The most the heap grew by while `read` ran.
fn peak(read: impl FnOnce()) -> usize {
    let before = HELD.load(Relaxed);
    PEAK.store(before, Relaxed);
    read();
    PEAK.load(Relaxed).saturating_sub(before)
}

#[test]
fn reading_a_message_takes_a_few_times_its_size_at_most() {
    let size = 4 * MIB;
    let server = [
        (
            "snapshot",
            junk(r#"{"type":"snapshot","seq":1,"entities":["#, "]}", size),
        ),
        (
            "delta",
            junk(r#"{"type":"delta","seq":2,"changes":["#, "]}", size),
        ),
        (
            "timeline events",
            junk(
                &format!(r#"{{"type":"timeline","agent":"{AGENT}","update":{{"type":"events","seq":1,"events":["#),
                "]}}",
                size,
            ),
        ),
        (
            "timeline reset",
            items(
                &format!(r#"{{"type":"timeline","agent":"{AGENT}","update":{{"type":"reset","seq":1,"has_older":false,"items":["#),
                "]}}",
                size,
            ),
        ),
        (
            "action result",
            junk(
                &format!(r#"{{"type":"action_result","key":"{KEY}","outcome":["#),
                "]}",
                size,
            ),
        ),
        ("one large entity", junk(ENTITY, "]}]}", size)),
        (
            "one large item",
            format!(
                r#"{{"type":"timeline","agent":"{AGENT}","update":{{"type":"reset","seq":1,"has_older":false,"items":[{}]}}}}"#,
                item(size)
            )
            .into_bytes(),
        ),
        (
            "one large outcome",
            junk(
                &format!(r#"{{"type":"action_result","key":"{KEY}","outcome":{{"j":["#),
                "]}}",
                size,
            ),
        ),
        (
            "pong",
            junk(r#"{"type":"pong","nonce":1,"j":["#, "]}", size),
        ),
        (
            "unknown kind",
            junk(r#"{"j":["#, r#"],"type":"from_the_future"}"#, size),
        ),
    ];
    let mut costs = Vec::new();
    for (name, json) in &server {
        costs.push((name, json.len(), peak(|| drop(ServerMessage::read(json)))));
    }

    let peer = junk(
        &format!(r#"{{"type":"peer_request","key":"{KEY}","request":["#),
        "]}",
        size,
    );
    let cost = peak(|| drop(ClientMessage::read_peer(&peer)));
    costs.push((&"peer request", peer.len(), cost));

    // A guest's holder lines may take a whole frame, anything else 4 MiB.
    let frame = 16 * MIB - 8;
    let guest = [
        (
            "holder line",
            junk(
                r#"{"type":"holder","conn":1,"event":{"type":"line","seq":1,"at":0,"text":"x","replayed":false,"j":["#,
                "]}}",
                frame,
            ),
        ),
        (
            "holder hello",
            junk(
                r#"{"type":"holder","conn":1,"event":{"type":"hello","holder_pid":1,"child":{"state":"running","pid":2},"first_seq":1,"last_seq":1,"pending":[{"seq":1,"at":0,"text":"x","replayed":false,"j":["#,
                "]}]}}",
                frame,
            ),
        ),
        (
            "holder hello with a large child",
            junk(
                r#"{"type":"holder","conn":1,"event":{"type":"hello","holder_pid":1,"child":{"state":"running","pid":2,"j":["#,
                r#"]},"first_seq":1,"last_seq":1,"pending":[{"seq":1,"at":0,"text":"x","replayed":false}]}}"#,
                frame,
            ),
        ),
        (
            "ports past 4 MiB",
            junk(r#"{"type":"ports","listening":[],"j":["#, "]}", frame),
        ),
        (
            "history",
            items(
                r#"{"type":"history_items","req":1,"items":["#,
                "]}",
                size - 1024,
            ),
        ),
    ];
    for (name, json) in &guest {
        costs.push((name, json.len(), peak(|| drop(GuestToHost::parse(json)))));
    }

    // Read whole, these took 40 to 140 times their size.
    for (name, len, cost) in costs {
        assert!(
            cost < 4 * len,
            "a {name} message of {len} bytes took {cost} bytes to read"
        );
    }

    // One piece is read whole up to a million values, which take about 100
    // bytes each at most.
    let under = junk(ENTITY, "]}]}", 2 * MIB);
    let cost = peak(|| drop(ServerMessage::read(&under)));
    assert!(cost < 128 * MIB, "one entity took {cost} bytes to read");
}
