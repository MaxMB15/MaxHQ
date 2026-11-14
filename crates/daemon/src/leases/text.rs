// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the lease tools say to an agent: names, states, why a lease ended,
//! what a command printed and how to drive a machine.

use maxhq_core::leases::MAX_OUTPUT_BYTES;
use maxhq_protocol::leases::{
    LeaseEnd, LeaseState, RunOutput, TestHost, TestHostBackend, TestHostState, TestKind, TestLease,
};

/// What a kind is called, as the app calls it.
pub fn kind_label(kind: TestKind) -> &'static str {
    match kind {
        TestKind::IosSimulator => "iOS Simulator",
        TestKind::AndroidEmulator => "Android emulator",
        TestKind::Linux => "Linux",
        TestKind::Macos => "macOS VM",
    }
}

/// What a backend is called, as the app calls it.
fn backend_label(backend: TestHostBackend) -> &'static str {
    match backend {
        TestHostBackend::IncusTest => "test container",
        TestHostBackend::MacVm => "Mac VM",
        TestHostBackend::MacHost => "this Mac",
        TestHostBackend::HostEmulator => "emulator",
    }
}

/// Why a lease ended, as one sentence.
pub fn end_reason(end: LeaseEnd) -> &'static str {
    match end {
        LeaseEnd::Closed => "It was closed.",
        LeaseEnd::TimeUp => "It reached its time limit.",
        LeaseEnd::Idle => "It went too long without a call.",
        LeaseEnd::HolderGone => "Its agent was archived or deleted.",
        LeaseEnd::EndedByYou => "It was ended in the MaxHQ app.",
        LeaseEnd::HostFailed => "Its test host failed, or MaxHQ restarted while it ran.",
    }
}

/// What a caller reads about a lease it can't use now.
pub fn not_ready(lease: &TestLease) -> String {
    let id = lease.id;
    match lease.state {
        LeaseState::Queued => format!("Lease {id} is still waiting for a host."),
        LeaseState::Starting => format!("Lease {id} is still starting. Try again shortly."),
        LeaseState::Ready => format!("Lease {id} is ready."),
        LeaseState::Ending => format!("Lease {id} is ending."),
        LeaseState::Ended => format!(
            "Lease {id} has ended. {} Call lease_open for a new one.",
            lease.ended.map_or("", end_reason)
        ),
    }
}

/// One host as a line of `list_test_hosts`.
pub fn host_line(host: &TestHost) -> String {
    let state = match host.state {
        TestHostState::Unavailable => "not available",
        TestHostState::Off => "not built yet; build it in the MaxHQ app",
        TestHostState::Building => "being built",
        TestHostState::Warm => "ready",
        TestHostState::Leased => "leased",
        TestHostState::Resetting => "going back to a clean state",
        TestHostState::Failed => "failed",
    };
    let mut line = format!(
        "- {} on the {}: {state}",
        kind_label(host.kind),
        backend_label(host.backend)
    );
    if host.queue > 0 {
        line.push_str(&format!(", {} waiting", host.queue));
    }
    line.push('.');
    for note in [&host.setup, &host.error].into_iter().flatten() {
        line.push(' ');
        line.push_str(note);
    }
    line
}

/// What one command printed, for the agent.
pub fn run_output(output: &RunOutput, timeout_seconds: u32) -> String {
    let mut text = match (output.timed_out, output.exit_code) {
        (true, _) => format!("Stopped after {timeout_seconds} seconds."),
        (false, Some(code)) => format!("Exit code {code}."),
        (false, None) => "Ended by a signal.".to_owned(),
    };
    for (name, out) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        if !out.is_empty() {
            text.push_str(&format!("\n{name}:\n{out}"));
        }
    }
    if output.truncated {
        text.push_str(&format!(
            "\nThe output was cut at {} KiB.",
            MAX_OUTPUT_BYTES / 1024
        ));
    }
    text
}

/// How to drive a ready lease's machine.
pub fn quick_start(lease: &TestLease, minutes: u16, idle_minutes: u16) -> String {
    let id = lease.id;
    let target = lease.target.as_deref().unwrap_or_default();
    let head = format!(
        "Lease {id} on the {} {target} is ready. It lasts {minutes} minutes and ends after \
         {idle_minutes} minutes without a call. Call lease_close with it when you're done.",
        kind_label(lease.kind)
    );
    let body = if lease.kind == TestKind::Linux {
        "Run one program at a time with lease_run, such as [\"uname\", \"-a\"]. Copy a build in \
         with lease_install."
            .to_owned()
    } else {
        format!(
            "Drive it with lease_run and one agent-device command, without --platform, --udid, \
             --serial, --config or --session, which MaxHQ adds:\n\
             - [\"open\", \"<bundle-or-package-id>\"] launches the app and starts the session \
             the other commands need.\n\
             - [\"snapshot\", \"-i\"] lists what's on screen with @eN refs.\n\
             - [\"press\", \"@e3\"] and [\"fill\", \"@e5\", \"text\"] act on a ref.\n\
             - [\"alert\", \"accept\"], [\"back\"] and [\"home\"] do what they say.\n\
             Prefer refs from a snapshot to coordinates. Install a build with lease_install and \
             look at the screen with lease_screenshot. Don't use simctl or adb for these; they \
             don't reach lease {id}."
        )
    };
    format!("{head}\n{body}")
}
