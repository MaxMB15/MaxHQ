// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Builders for PR tests. Each fills in an open PR that merges cleanly and
// takes the fields a test cares about.

import type {
	CredentialKind,
	DeviceId,
	Entity,
	PullDetail,
	PullLink,
	PullSnapshot,
	PullSummary,
} from "../bindings";
import { MIN, NOW } from "./build";

/** The repository the builders' PRs are in. */
export const REPO = "owner/repo";

/** An open PR with passing checks, no review yet, ten lines changed. */
export const snapshot = (over: Partial<PullSnapshot> = {}): PullSnapshot => ({
	title: "A change",
	state: "open",
	draft: false,
	author: "someone",
	head: "topic",
	base: "main",
	head_sha: "abc123",
	checks: "passing",
	mergeable: "yes",
	review: "none",
	additions: 8,
	deletions: 2,
	updated_at: NOW - 10 * MIN,
	merged_at: null,
	...over,
});

/** PR `number` in `REPO` as a list shows it, linked to no agent. */
export const summary = (number: number, over: Partial<PullSnapshot> = {}): PullSummary => ({
	key: { repo: REPO, number },
	pull: snapshot(over),
	agents: [],
});

/** PR `number` in full, allowing a squash or a merge commit. */
export const detail = (
	number: number,
	over: Partial<PullSnapshot> = {},
	rest: Partial<Omit<PullDetail, "summary">> = {}
): PullDetail => ({
	summary: summary(number, over),
	body: null,
	checks: [],
	reviews: [],
	merge_methods: ["squash", "merge"],
	...rest,
});

/** A link to PR `number` in `REPO`. */
export const pullLink = (number: number, source: PullLink["source"] = "created"): PullLink => ({
	key: { repo: REPO, number },
	source,
	at: NOW - 5 * MIN,
	snapshot: null,
});

/** A credential a device holds, tagged for a snapshot. */
export const credential = (device: DeviceId, kind: CredentialKind): Entity => ({
	entity: "credential",
	id: `cred-${kind.type}-${device}`,
	device,
	kind,
	stored_at: NOW - 60 * MIN,
	problem: null,
});

/** Your GitHub login on a device. */
export const githubUser = (device: DeviceId, login = "you"): Entity =>
	credential(device, { type: "github_user", login });

/** A GitHub App on a device. */
export const githubApp = (device: DeviceId): Entity =>
	credential(device, { type: "github_app", app_id: 1, slug: null, key_sha256: "00", repos: [] });
