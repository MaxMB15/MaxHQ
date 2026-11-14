// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the history
// recorder, with its retention, snapshot, entry and byte caps and its
// tests. Samples come from this crate's collector, so they carry no request
// IDs.

//! The last hour of samples.

use std::collections::VecDeque;

use super::collect::Sample;

/// How long samples are kept.
pub const HISTORY_RETENTION_MS: u64 = 60 * 60_000;
/// The most samples kept.
pub const MAX_HISTORY_SNAPSHOTS: usize = 3_600;
/// The most processes and roots kept across every sample.
pub const MAX_HISTORY_RETAINED_ENTRIES: usize = 20_000;
/// The most bytes kept, roughly.
pub const MAX_HISTORY_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// Samples in the order they were taken, within the caps.
#[derive(Default)]
pub struct HistoryRecorder {
    snapshots: VecDeque<Sample>,
    retained_entry_count: usize,
    retained_bytes: usize,
}

impl HistoryRecorder {
    /// Keeps `snapshot`, dropping the oldest past the caps.
    pub fn record(&mut self, snapshot: &Sample) {
        self.record_with_limits(
            snapshot,
            MAX_HISTORY_SNAPSHOTS,
            MAX_HISTORY_RETAINED_ENTRIES,
            MAX_HISTORY_RETAINED_BYTES,
        );
    }

    fn record_with_limits(
        &mut self,
        snapshot: &Sample,
        max_snapshots: usize,
        max_retained_entries: usize,
        max_retained_bytes: usize,
    ) {
        let clock_moved_backward = self
            .snapshots
            .back()
            .is_some_and(|previous| previous.sampled_at_ms > snapshot.sampled_at_ms);
        let retained = snapshot.clone();
        self.retained_entry_count = self
            .retained_entry_count
            .saturating_add(retained.retained_entry_count());
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(retained.estimated_history_bytes());
        self.snapshots.push_back(retained);
        self.trim_to_limits(
            snapshot.sampled_at_ms,
            max_snapshots,
            max_retained_entries,
            max_retained_bytes,
            clock_moved_backward,
        );
    }

    fn trim_to_limits(
        &mut self,
        now_ms: u64,
        max_snapshots: usize,
        max_retained_entries: usize,
        max_retained_bytes: usize,
        clock_moved_backward: bool,
    ) {
        if clock_moved_backward {
            let mut future_entry_count = 0usize;
            let mut future_bytes = 0usize;
            self.snapshots.retain(|snapshot| {
                let keep = snapshot.sampled_at_ms <= now_ms;
                if !keep {
                    future_entry_count =
                        future_entry_count.saturating_add(snapshot.retained_entry_count());
                    future_bytes = future_bytes.saturating_add(snapshot.estimated_history_bytes());
                }
                keep
            });
            self.retained_entry_count =
                self.retained_entry_count.saturating_sub(future_entry_count);
            self.retained_bytes = self.retained_bytes.saturating_sub(future_bytes);
        }

        while self.snapshots.front().is_some_and(|snapshot| {
            snapshot.sampled_at_ms < now_ms.saturating_sub(HISTORY_RETENTION_MS)
                || self.snapshots.len() > max_snapshots
                || self.retained_entry_count > max_retained_entries
                || self.retained_bytes > max_retained_bytes
        }) {
            if let Some(removed) = self.snapshots.pop_front() {
                self.retained_entry_count = self
                    .retained_entry_count
                    .saturating_sub(removed.retained_entry_count());
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(removed.estimated_history_bytes());
            }
        }
    }

    /// The samples in the `window_ms` up to `now_ms`.
    pub fn read(&self, window_ms: u64, now_ms: u64) -> Vec<Sample> {
        let started_at_ms = now_ms.saturating_sub(window_ms.min(HISTORY_RETENTION_MS));
        self.snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.sampled_at_ms >= started_at_ms && snapshot.sampled_at_ms <= now_ms
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::collect::{ProcessSample, Root};
    use super::*;

    fn sample(sampled_at_ms: u64) -> Sample {
        Sample {
            sampled_at_ms,
            roots: Vec::new(),
            processes: Vec::new(),
            inaccessible: 0,
        }
    }

    #[test]
    fn retains_bounded_history() {
        let mut history = HistoryRecorder::default();
        for sequence in 0..=MAX_HISTORY_SNAPSHOTS {
            history.record(&Sample {
                roots: vec![Root {
                    pid: 7,
                    start_time_ms: Some(1_000),
                }],
                ..sample(sequence as u64 * 1_000)
            });
        }

        assert_eq!(history.snapshots.len(), MAX_HISTORY_SNAPSHOTS);
        assert!(history.snapshots.iter().all(|snapshot| {
            snapshot.roots.len() == 1
                && snapshot.roots[0].pid == 7
                && snapshot.roots[0].start_time_ms == Some(1_000)
        }));
        assert_eq!(
            history
                .read(10_000, MAX_HISTORY_SNAPSHOTS as u64 * 1_000)
                .len(),
            11
        );
    }

    #[test]
    fn excludes_and_trims_future_history_after_the_clock_moves_backward() {
        let mut history = HistoryRecorder::default();
        history.record(&sample(2_000));

        assert!(history.read(0, 1_000).is_empty());

        history.record(&sample(1_000));
        assert_eq!(history.snapshots.len(), 1);
        assert_eq!(
            history.snapshots.front().map(|entry| entry.sampled_at_ms),
            Some(1_000)
        );
    }

    #[test]
    fn bounds_history_by_estimated_process_bytes() {
        let mut history = HistoryRecorder::default();
        let process = ProcessSample {
            pid: 1,
            ppid: 0,
            root: 1,
            start_time_ms: 0,
            command: "x".repeat(128),
            cpu_percent: 0.0,
            resident_bytes: 0,
        };
        let snapshot_bytes = std::mem::size_of::<Sample>() + process.estimated_history_bytes();
        for sequence in 0..3u64 {
            history.record_with_limits(
                &Sample {
                    processes: vec![ProcessSample {
                        pid: sequence as u32 + 1,
                        start_time_ms: sequence * 1_000,
                        ..process.clone()
                    }],
                    ..sample(sequence * 1_000)
                },
                3,
                3,
                snapshot_bytes * 2,
            );
        }

        assert!(history.retained_bytes <= snapshot_bytes * 2);
        assert_eq!(history.snapshots.len(), 2);
        assert_eq!(
            history
                .snapshots
                .front()
                .map(|snapshot| snapshot.sampled_at_ms),
            Some(1_000)
        );
    }

    #[test]
    fn counts_roots_toward_history_limits() {
        let mut history = HistoryRecorder::default();
        let roots = (1..=128)
            .map(|pid| Root {
                pid,
                start_time_ms: Some(u64::from(pid) * 1_000),
            })
            .collect::<Vec<_>>();
        let snapshot = Sample { roots, ..sample(0) };
        let snapshot_bytes = snapshot.estimated_history_bytes();
        let snapshot_entries = snapshot.retained_entry_count();

        for sequence in 0..3u64 {
            history.record_with_limits(
                &Sample {
                    sampled_at_ms: sequence * 1_000,
                    ..snapshot.clone()
                },
                3,
                snapshot_entries * 2,
                snapshot_bytes * 2,
            );
        }

        assert_eq!(history.retained_entry_count, snapshot_entries * 2);
        assert!(history.retained_bytes <= snapshot_bytes * 2);
        assert_eq!(history.snapshots.len(), 2);
        assert_eq!(
            history
                .snapshots
                .front()
                .map(|snapshot| snapshot.sampled_at_ms),
            Some(1_000)
        );
    }
}
