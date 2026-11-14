// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs the built `fake-agent` binary.

use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_fake-agent");
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.cast");
const SILENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silent.cast");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("fake-agent runs")
}

/// Runs for at most `limit`, then kills it. None means it was still running.
fn run_for(args: &[&str], limit: Duration) -> Option<ExitStatus> {
    let mut child = Command::new(BIN)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("fake-agent runs");
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    None
}

#[test]
fn replay_writes_the_recorded_output() {
    let output = run(&["replay", FIXTURE, "--speed", "50"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "\x1b[1mfake-agent\x1b[0m ready\r\nReading Cargo.toml\r\n\x1b[32mDone.\x1b[0m\r\n"
    );
}

#[test]
fn info_summarises_a_recording() {
    let output = run(&["info", FIXTURE]);
    assert!(output.status.success(), "{output:?}");
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["version"], 3);
    assert_eq!(info["cols"], 80);
    assert_eq!(info["events"]["output"], 3);
    assert_eq!(info["events"]["exit"], 1);
    assert_eq!(info["idle_time_limit_secs"], 1.0);
}

#[test]
fn flood_writes_exactly_the_bytes_asked_for() {
    let output = run(&["flood", "--bytes", "100K", "--line", "abc"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout.len(), 100 * 1024);
    assert!(output.stdout.starts_with(b"abc\nabc\n"));
}

#[test]
fn spinner_stops_after_its_time() {
    let output = run(&[
        "spinner",
        "--hz",
        "100",
        "--cols",
        "40",
        "--seconds",
        "0.05",
    ]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.trim_end_matches("\r\n").split('\r').skip(1).collect();
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| line.len() == 40), "{lines:?}");
}

#[test]
fn bad_arguments_print_usage() {
    let output = run(&["replay"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing FILE"));
    assert_eq!(run(&["flood", "--bytes", "lots"]).status.code(), Some(2));
    assert_eq!(run(&["spinner", "--extra"]).status.code(), Some(2));
}

#[test]
fn float_flags_out_of_range_are_usage_errors() {
    for args in [
        &["spinner", "--seconds", "-1"][..],
        &["spinner", "--seconds", "nan"],
        &["spinner", "--hz", "1e-30"],
        &["replay", FIXTURE, "--idle-limit", "1e20"],
        &["replay", FIXTURE, "--idle-limit", "inf"],
    ] {
        assert_eq!(run(args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn loop_refuses_a_recording_that_prints_nothing() {
    let status = run_for(&["replay", SILENT, "--loop"], Duration::from_secs(5));
    assert_eq!(status.and_then(|status| status.code()), Some(2));
}

#[test]
fn a_tiny_speed_waits_instead_of_panicking() {
    let status = run_for(
        &["replay", FIXTURE, "--speed", "1e-300"],
        Duration::from_millis(300),
    );
    assert_eq!(
        status, None,
        "it should still be waiting for the first event"
    );
}
