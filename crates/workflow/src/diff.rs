// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What changed between two snapshots of a run.

use maxhq_protocol::workflow::{RunPatch, RunSnapshot};

/// Diffing for [`RunSnapshot`], which lives in the protocol crate.
pub trait Diff {
    /// The patch from this snapshot to `newer`. `None` when nothing changed
    /// (the same `rev`) and when a patch can't say it: another key, shape
    /// or notice. Compare `rev` to tell the two apart, and send `newer`
    /// whole as a reset in the second case.
    fn diff(&self, newer: &RunSnapshot) -> Option<RunPatch>;
}

impl Diff for RunSnapshot {
    fn diff(&self, newer: &RunSnapshot) -> Option<RunPatch> {
        if self.rev == newer.rev
            || self.key != newer.key
            || self.shape.hash != newer.shape.hash
            || self.notice != newer.notice
        {
            return None;
        }
        let nodes = newer
            .nodes
            .iter()
            .filter(|node| !self.nodes.iter().any(|old| old == *node))
            .cloned()
            .collect();
        Some(RunPatch {
            key: newer.key.clone(),
            rev: newer.rev,
            nodes,
            loops: (self.loops != newer.loops).then(|| newer.loops.clone()),
            header: (self.header != newer.header).then(|| newer.header.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::{DeviceId, ProjectId};
    use maxhq_protocol::workflow::{NodeStatus, RunNotice, Tone};

    use crate::fake::{FakeAgents, MemFiles};
    use crate::{load, Run, RunRef};

    const FILE: &str = "/p/runs/r1/run.yaml";

    fn derive(contents: &str) -> (RunSnapshot, RunSnapshot) {
        let desc =
            load(b"format: maxhq.workflow/1.0\nid: example\nruns: { glob: runs/*/run.yaml }\n")
                .unwrap();
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: busy }, { id: b, needs: [a] }, { id: c }]",
        );
        let run_ref = RunRef {
            workflow: "example".into(),
            file: FILE.into(),
            project: "/p".into(),
            root: "/p".into(),
        };
        let mut run = Run::new(DeviceId::derive("d"), ProjectId::derive("p"), run_ref);
        let one = run.derive(&desc, &files, &FakeAgents::default());
        files.put(FILE, contents);
        (one, run.derive(&desc, &files, &FakeAgents::default()))
    }

    #[test]
    fn a_patch_carries_only_changed_nodes() {
        let (one, two) =
            derive("nodes: [{ id: a, status: done }, { id: b, needs: [a] }, { id: c }]");
        let patch = one.diff(&two).unwrap();
        assert_eq!(patch.rev, one.rev + 1);
        let ids: Vec<_> = patch.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "b went ready");
        assert_eq!(patch.nodes[1].status, NodeStatus::Ready);
        assert!(patch.header.is_some(), "progress moved");
        assert_eq!(patch.loops, None);
    }

    #[test]
    fn nothing_changed_and_a_new_shape_give_no_patch() {
        let (one, _) = derive("nodes: []");
        assert_eq!(one.diff(&one), None);
        let (one, two) = derive("nodes: [{ id: a, status: busy }]");
        assert_ne!(one.rev, two.rev);
        assert_eq!(one.diff(&two), None, "a reset");
        let mut noticed = one.clone();
        noticed.rev += 1;
        noticed.notice = Some(RunNotice {
            text: "x".into(),
            tone: Tone::Warn,
        });
        assert_eq!(one.diff(&noticed), None);
    }
}
