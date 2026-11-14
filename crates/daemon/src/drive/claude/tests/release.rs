// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! When an idle CLI may stop to free its memory.

use maxhq_protocol::processes::AgentWait;

use super::*;

/// The recorded session after its last turn, before the CLI exits.
fn idle() -> Run {
    let mut run = Run::new();
    run.play(0..=LAST - 1);
    run
}

#[test]
fn an_idle_session_can_stop_and_resume_later() {
    let run = idle();
    assert!(run.driver.can_release(lead()));
}

#[test]
fn a_turn_or_an_open_prompt_keeps_the_cli() {
    let mut run = Run::new();
    run.play(0..=14);
    assert!(!run.driver.can_release(lead()), "a turn runs");
    run.play(15..=84);
    assert!(!run.driver.can_release(lead()), "a prompt waits on you");
}

#[test]
fn background_tasks_keep_the_cli() {
    let mut run = idle();
    let tasks = json!([{"id": "b1", "type": "local_bash", "status": "running"}]);
    run.feed(system(
        "background_tasks_changed",
        json!({ "tasks": tasks }),
    ));
    assert!(!run.driver.can_release(lead()), "a background command runs");
    run.feed(system("background_tasks_changed", json!({ "tasks": [] })));
    assert!(run.driver.can_release(lead()));
}

#[test]
fn an_ended_cli_has_nothing_to_release() {
    let mut run = idle();
    run.step(LAST);
    assert!(!run.driver.can_release(lead()));
}

#[test]
fn a_call_still_running_keeps_the_cli() {
    let mut run = idle();
    run.feed(tool_use("toolu_x", "Read", json!({"file_path": "/a"})));
    assert!(!run.driver.can_release(lead()), "a call runs");
    run.feed(tool_result("toolu_x", "a", json!({})));
    assert!(run.driver.can_release(lead()));
}

#[test]
fn a_wakeup_keeps_the_cli_until_it_is_due() {
    let mut run = idle();
    let wake = json!({"delaySeconds": 600, "prompt": "check", "reason": "wait"});
    run.feed(tool_use("toolu_w", "ScheduleWakeup", wake));
    run.feed(tool_result("toolu_w", "Scheduled", json!({})));
    assert!(
        !run.driver.can_release(lead()),
        "the wakeup is still to fire"
    );
    run.clock.fetch_add(600_000, Ordering::SeqCst);
    assert!(run.driver.can_release(lead()));
}

#[test]
fn a_wakeup_counts_after_its_call_is_forgotten() {
    let mut run = idle();
    let wake = json!({"delaySeconds": 600, "prompt": "check", "reason": "wait"});
    run.feed(tool_use("toolu_w", "ScheduleWakeup", wake));
    run.feed(tool_result("toolu_w", "Scheduled", json!({})));
    // Enough calls after it that the wakeup's item leaves memory.
    for at in 0..400 {
        let call = format!("toolu_r{at}");
        run.feed(tool_use(&call, "Read", json!({"file_path": "/a"})));
        run.feed(tool_result(&call, "a", json!({})));
    }
    assert!(
        !run.driver.can_release(lead()),
        "the wakeup is still to fire"
    );
    run.clock.fetch_add(600_000, Ordering::SeqCst);
    assert!(run.driver.can_release(lead()));
}

#[test]
fn a_cron_job_keeps_the_cli_until_it_is_deleted() {
    let mut run = idle();
    let cron = json!({"cron": "*/5 * * * *", "prompt": "check", "recurring": true});
    run.feed(tool_use("toolu_c", "CronCreate", cron));
    run.feed(tool_result("toolu_c", "Scheduled job c1", json!({})));
    assert!(
        !run.driver.can_release(lead()),
        "the job fires while the CLI runs"
    );
    run.feed(tool_use("toolu_d", "CronDelete", json!({"id": "c1"})));
    run.feed(tool_result("toolu_d", "Deleted c1", json!({})));
    assert!(run.driver.can_release(lead()));
}

#[test]
fn an_idle_agent_says_it_waits_on_a_background_task_until_it_ends() {
    let mut run = idle();
    assert_eq!(run.lead().running, None);
    let started = json!({"task_id": "b1", "tool_use_id": "toolu_bg", "description": "pnpm dev",
        "task_type": "local_bash"});
    run.feed(system("task_started", started));
    let waiting = run
        .lead()
        .running
        .as_ref()
        .and_then(|running| running.waiting.clone());
    assert_eq!(
        waiting,
        Some(AgentWait::Task {
            name: "pnpm dev".into()
        })
    );
    run.feed(system(
        "task_notification",
        json!({"task_id": "b1", "status": "completed", "summary": "Done"}),
    ));
    assert_eq!(run.lead().running, None);
}

#[test]
fn an_idle_agent_says_when_its_wakeup_is_due() {
    let mut run = idle();
    let now = run.clock.load(Ordering::SeqCst);
    let wake = json!({"delaySeconds": 600, "prompt": "check", "reason": "wait"});
    run.feed(tool_use("toolu_w", "ScheduleWakeup", wake));
    run.feed(tool_result("toolu_w", "Scheduled", json!({})));
    let waiting = run
        .lead()
        .running
        .as_ref()
        .and_then(|running| running.waiting.clone());
    // The clock moves a little with each line fed.
    let Some(AgentWait::Wakeup { at }) = waiting else {
        panic!("no wakeup: {waiting:?}");
    };
    assert!((now + 600_000..now + 601_000).contains(&at.0), "{at:?}");
}

#[test]
fn a_cron_job_counts_for_a_week_at_most() {
    let mut run = idle();
    let cron = json!({"cron": "0 9 * * *", "prompt": "check", "recurring": true});
    run.feed(tool_use("toolu_c", "CronCreate", cron));
    run.feed(tool_result("toolu_c", "Scheduled job c1", json!({})));
    assert!(!run.driver.can_release(lead()));
    run.clock
        .fetch_add(7 * 24 * 60 * 60 * 1000, Ordering::SeqCst);
    assert!(run.driver.can_release(lead()));
}
