// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// how a key is made and how long replay keys and coalescing windows last.

//! The keys that drop repeated requests. A replay key drops a request
//! whose key a task saw in the last week. A coalescing key gathers the
//! requests that share it within a window into one run, which sees each
//! of them. A window may restart with each request, up to a longest wait.

use maxhq_protocol::tasks::HookRules;
use maxhq_protocol::time::Timestamp;
use ring::digest;

use super::template::render_raw;
use super::Incoming;

/// How long a replay key is kept.
pub const REPLAY_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// How long a coalescing window stays open when the task doesn't say.
pub const COALESCE_SECS: u32 = 120;

/// The key `template` makes from `request`: the SHA-256 of its raw
/// filling, in hex. `None` when there's no template, a field it names is
/// missing or empty, or it fills in to nothing, so such requests are never
/// dropped.
pub fn key(template: Option<&str>, request: &Incoming) -> Option<String> {
    let rendered = render_raw(template?, request);
    if !rendered.missing.is_empty() || rendered.prompt.is_empty() {
        return None;
    }
    let hash = digest::digest(&digest::SHA256, rendered.prompt.as_bytes());
    Some(hash.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether a replay key seen at `seen` still drops a request at `now`.
pub fn replayed(seen: Timestamp, now: Timestamp) -> bool {
    now.0 < seen.0.saturating_add(REPLAY_MS)
}

/// The longest a window that restarts waits after its first request, in
/// seconds, when the task doesn't say.
pub const MAX_WAIT_SECS: u32 = 2 * 60 * 60;

/// When a coalescing window opened at `first` runs, until a later request
/// moves it.
pub fn due(first: Timestamp, secs: Option<u32>) -> Timestamp {
    Timestamp(first.0 + u64::from(secs.unwrap_or(COALESCE_SECS)) * 1000)
}

/// When a window that runs at `runs`, opened at `first`, runs once a
/// request joins it at `now`. With `rules.coalesce_restart` it's a window
/// after `now`, but no later than the longest wait after `first`. Without,
/// it doesn't move.
pub fn rejoined(rules: &HookRules, runs: Timestamp, first: Timestamp, now: Timestamp) -> Timestamp {
    if !rules.coalesce_restart {
        return runs;
    }
    let latest = due(
        first,
        Some(rules.coalesce_max_secs.unwrap_or(MAX_WAIT_SECS)),
    );
    due(now, rules.coalesce_secs).min(latest).max(runs)
}

/// Whether a window that runs at `due` still takes requests at `now`.
pub fn open(due: Timestamp, now: Timestamp) -> bool {
    now < due
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::template::tests::headers;

    fn key_of(template: Option<&str>, body: &str) -> Option<String> {
        let headers = headers(&[("x-github-delivery", "d-1")]);
        let request = Incoming {
            method: "POST",
            path: "/maxhq/task",
            query: "",
            headers: &headers,
            body,
        };
        key(template, &request)
    }

    #[test]
    fn a_key_is_the_hash_of_what_its_template_fills_in() {
        let one = key_of(Some("{{body.id}}"), r#"{"id":"abc"}"#).unwrap();
        // SHA-256 of "abc".
        assert_eq!(
            one,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            key_of(Some("{{body.id}}"), r#"{"id":"abc","other":1}"#).unwrap(),
            one
        );
        assert_ne!(
            key_of(Some("{{headers.x-github-delivery}}"), "").unwrap(),
            one
        );
    }

    #[test]
    fn no_key_without_a_template_or_with_a_field_missing_or_empty() {
        assert_eq!(key_of(None, r#"{"id":"abc"}"#), None);
        assert_eq!(
            key_of(Some("{{body.id}}-{{body.nope}}"), r#"{"id":"abc"}"#),
            None
        );
        assert_eq!(key_of(Some("{{body.id}}"), r#"{"id":""}"#), None);
        assert_eq!(key_of(Some(""), "{}"), None);
    }

    #[test]
    fn replay_keys_last_a_week_and_windows_end_on_time() {
        let seen = Timestamp(1_000);
        assert!(replayed(seen, Timestamp(1_000 + REPLAY_MS - 1)));
        assert!(!replayed(seen, Timestamp(1_000 + REPLAY_MS)));
        assert_eq!(due(seen, None), Timestamp(121_000));
        assert_eq!(due(seen, Some(5)), Timestamp(6_000));
        assert!(open(Timestamp(6_000), Timestamp(5_999)));
        assert!(!open(Timestamp(6_000), Timestamp(6_000)));
    }

    #[test]
    fn one_text_with_no_placeholder_is_one_key_for_every_request() {
        let one = key_of(Some("all"), r#"{"pr":4}"#).unwrap();
        assert_eq!(key_of(Some("all"), r#"{"pr":7}"#), Some(one));
    }

    #[test]
    fn a_window_that_restarts_moves_with_each_request_up_to_its_longest_wait() {
        const MIN: u64 = 60_000;
        let rules = HookRules {
            coalesce_secs: Some(900),
            coalesce_restart: true,
            ..HookRules::default()
        };
        let first = Timestamp(0);
        let runs = due(first, rules.coalesce_secs);
        assert_eq!(runs, Timestamp(15 * MIN));
        let moved = rejoined(&rules, runs, first, Timestamp(5 * MIN));
        assert_eq!(moved, Timestamp(20 * MIN));
        // Two hours after the first, however late the last request.
        assert_eq!(
            rejoined(&rules, moved, first, Timestamp(110 * MIN)),
            Timestamp(120 * MIN)
        );
        let short = HookRules {
            coalesce_max_secs: Some(1_200),
            ..rules.clone()
        };
        assert_eq!(
            rejoined(&short, runs, first, Timestamp(10 * MIN)),
            Timestamp(20 * MIN)
        );
        // Without a restart it keeps its time.
        let fixed = HookRules {
            coalesce_restart: false,
            ..rules
        };
        assert_eq!(rejoined(&fixed, runs, first, Timestamp(5 * MIN)), runs);
    }
}
