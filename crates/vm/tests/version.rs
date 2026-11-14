// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The built helper answers `version` and exits on `stop`, with no VM.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use maxhq_vm::{Event, Op, Request, VERSION};

#[test]
fn answers_its_version_and_stops() {
    let mut helper = Command::new(env!("CARGO_BIN_EXE_maxd-vm"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = helper.stdin.take().unwrap();
    let mut lines = BufReader::new(helper.stdout.take().unwrap()).lines();
    let ask = |stdin: &mut std::process::ChildStdin, id, op| {
        stdin
            .write_all(Request { id, op }.line().as_bytes())
            .unwrap();
    };

    ask(&mut stdin, 1, Op::Version);
    let answer: Event = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(
        answer,
        Event::Done {
            id: 1,
            version: Some(VERSION.to_owned())
        }
    );
    ask(&mut stdin, 2, Op::Stop);
    let answer: Event = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(
        answer,
        Event::Done {
            id: 2,
            version: None
        }
    );
    assert!(helper.wait().unwrap().success());
}
