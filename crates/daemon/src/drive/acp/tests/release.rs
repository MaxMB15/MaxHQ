// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! When an idle ACP agent's CLI may stop to free its memory.

use super::*;

#[test]
fn a_cli_that_loads_sessions_can_stop_between_turns() {
    let run = Run::ready();
    assert!(run.session.can_release(lead()));
    let mut run = Run::turning(PROMPTS[0]);
    assert!(!run.session.can_release(lead()), "a turn runs");
    run.feed_all(&live_turns()[0]);
    assert!(run.session.can_release(lead()));
}

#[test]
fn a_cli_that_cant_load_its_session_keeps_running() {
    let mut run = Run::with(None, PermissionMode::Ask);
    run.connect();
    run.feed_all(&fixture("handshake-no-image.jsonl"));
    assert!(!run.session.can_release(lead()));
}
