// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The usage reading Claude Code keeps in a login's `.claude.json` after its
//! own checks, under `cachedUsageUtilization`.

use std::time::Duration;

use maxhq_protocol::model::{Usage, UsageSource};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;

use super::windows::windows;

/// How long a cached reading stays good.
pub const FRESH_FOR: Duration = Duration::from_secs(60 * 60);

/// The cached reading in a state file's JSON, when it's under an hour old
/// and belongs to the login signed in now. A cache left from an earlier
/// login names a different account, so it's never used.
pub fn cached(state: &[u8], now: Timestamp) -> Option<Usage> {
    let state: Value = serde_json::from_slice(state).ok()?;
    let cache = state.get("cachedUsageUtilization")?;
    let signed_in = state
        .get("oauthAccount")?
        .get("accountUuid")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    if cache.get("accountUuid")?.as_str()? != signed_in {
        return None;
    }
    let fetched = Timestamp(cache.get("fetchedAtMs")?.as_u64()?);
    if now >= fetched.after(FRESH_FOR) {
        return None;
    }
    let windows = windows(cache.get("utilization")?);
    (!windows.is_empty()).then_some(Usage {
        checked_at: fetched,
        source: UsageSource::ClaudeCache,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::observe::claude::tests::fixture;

    /// When the fixture's reading was fetched.
    const FETCHED: Timestamp = Timestamp(1_792_889_000_000);

    fn state() -> Vec<u8> {
        fs::read(fixture("config/.claude.json")).unwrap()
    }

    #[test]
    fn a_fresh_reading_for_this_login_is_used() {
        let now = FETCHED.after(Duration::from_secs(59 * 60));
        let usage = cached(&state(), now).expect("a cached reading");
        assert_eq!(usage.checked_at, FETCHED);
        assert_eq!(usage.source, UsageSource::ClaudeCache);
        let minutes: Vec<(Option<&str>, u32)> = usage
            .windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.window_minutes))
            .collect();
        assert_eq!(
            minutes,
            [(None, 300), (None, 10_080), (Some("opus"), 10_080)]
        );
        assert_eq!(usage.windows[0].used_percent, 22.0);
    }

    #[test]
    fn a_reading_an_hour_old_is_stale() {
        assert_eq!(cached(&state(), FETCHED.after(FRESH_FOR)), None);
    }

    #[test]
    fn a_reading_for_another_login_is_ignored() {
        let text = String::from_utf8(state()).unwrap();
        let other = text.replacen(
            "\"accountUuid\": \"00000000-0000-4000-8000-0000000000a1\"",
            "\"accountUuid\": \"00000000-0000-4000-8000-0000000000b2\"",
            1,
        );
        assert_ne!(text, other);
        assert_eq!(cached(other.as_bytes(), FETCHED), None);

        let logged_out = text.replacen("\"oauthAccount\"", "\"formerAccount\"", 1);
        assert_eq!(cached(logged_out.as_bytes(), FETCHED), None);
        assert_eq!(cached(b"{}", FETCHED), None);
        assert_eq!(cached(b"not json", FETCHED), None);
    }
}
