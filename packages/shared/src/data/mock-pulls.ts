// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR calls of the sample data and of tests. The sample repository has
// an open PR ready to merge, one with failing checks, a draft, one that
// conflicts, one with checks still running, and a merged and a closed one
// for the Closed tab. Some are linked to sample agents, the way `maxd`
// links an agent's own PRs. Merges, approvals and links change it in
// memory, and a sign-in gives a code and, a few seconds later, signs you
// in on the device you picked.

import type {
	AgentId,
	CheckState,
	DeviceId,
	MergeMethod,
	PullCheck,
	PullComment,
	PullDetail,
	PullFile,
	PullKey,
	PullLink,
	PullReview,
	PullSnapshot,
	PullSummary,
	Timestamp,
} from "../bindings";
import { NOTES } from "./mock-started";
import type { ActionResult, DataSource } from "./source";

/** The PR calls of a data source. */
export type PullCalls = Pick<
	DataSource,
	| "listPulls"
	| "listBranches"
	| "getPull"
	| "pullFiles"
	| "pullComments"
	| "pullChecks"
	| "pullAction"
	| "pullsDue"
	| "linkPull"
	| "unlinkPull"
	| "beginGithubSignIn"
	| "cancelGithubSignIn"
	| "checkGithubSignIn"
>;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const NO_PULL = "GitHub has no such PR.";

/** The PR calls with no PRs behind them and no GitHub sign-in to give. */
export const noPulls: PullCalls = {
	listPulls: async () => ({ status: "ok", data: { status: "pulls", pulls: [] } }),
	listBranches: async () => ({
		status: "ok",
		data: { status: "branches", branches: [], default_branch: null },
	}),
	getPull: async () => refused(NO_PULL),
	pullFiles: async () => refused(NO_PULL),
	pullComments: async () => refused(NO_PULL),
	pullChecks: async () => refused(NO_PULL),
	pullAction: async () => refused(NO_PULL),
	pullsDue: async () => DONE,
	linkPull: async () => DONE,
	unlinkPull: async () => DONE,
	beginGithubSignIn: async () => refused("This device can't sign in to GitHub."),
	cancelGithubSignIn: async () => DONE,
	checkGithubSignIn: async () => DONE,
};

/** The sample repository, as `maxd` keys it. */
export const MOCK_REPO = "example-user/maxhq";

/** The project whose PRs the sample has. */
export const MOCK_PULLS_PROJECT = "proj-maxhq";

/** The login a sample sign-in gives. */
export const MOCK_GITHUB_LOGIN = "example-user";

/** How long a sample sign-in waits before it signs you in. */
export const SIGN_IN_MS = 3000;

/** How many files a page of the sample's changed files holds. */
export const FILES_PAGE = 3;

/** How long a sample code lasts, as GitHub's do. */
const CODE_LIFETIME_MS = 15 * 60_000;

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

// The daemon's words for a merge or an approval it refuses.
const MOVED_MERGE = "The PR changed since you looked. Reload it and merge again.";
const MOVED_APPROVE = "The PR changed since you looked. Reload it and approve again.";
const NOT_OPEN = "This PR isn't open anymore.";
const YOUR_OWN = "GitHub doesn't let you approve your own PR.";
const NO_SIGN_IN = "Sign in to GitHub in Settings to merge and approve here.";

/** One sample PR and everything read of it. */
type MockPull = {
	number: number;
	pull: PullSnapshot;
	body: string;
	checks: PullCheck[];
	reviews: PullReview[];
	files: PullFile[];
	comments: PullComment[];
};

const check = (name: string, state: CheckState, run: number): PullCheck => ({
	name,
	state,
	url: `https://github.com/${MOCK_REPO}/actions/runs/${run}`,
});

const passing = (run: number): PullCheck[] => [
	check("Lint", "passing", run),
	check("Unit tests", "passing", run + 1),
	check("Build macOS", "passing", run + 2),
	check("Release notes", "skipped", run + 3),
];

const comment = (
	number: number,
	id: number,
	author: string,
	body: string,
	at: Timestamp,
	path: string | null = null,
	line: number | null = null
): PullComment => ({
	id,
	author,
	body,
	at,
	path,
	line,
	url: `https://github.com/${MOCK_REPO}/pull/${number}#issuecomment-${id}`,
});

const METER_PATCH = `@@ -12,9 +12,15 @@ export const UsageMeter = ({ usage }: Props) => {
 	const five = usage.windows.find((w) => w.kind === "five_hour");
+	const week = usage.windows.find((w) => w.kind === "weekly");
 	if (!five) return null;
 	return (
 		<div className="usage-meter">
 			<Bar fill={five.used} label="5h" />
+			{week ? <Bar fill={week.used} label="Week" thin /> : null}
 		</div>
 	);
 };`;

const METER_TEST_PATCH = `@@ -30,6 +30,18 @@ describe("UsageMeter", () => {
 		expect(screen.getByText("5h")).toBeInTheDocument();
 	});
+
+	it("shows the week under the 5-hour bar", () => {
+		render(<UsageMeter usage={withWeek(0.4)} />);
+		expect(screen.getByText("Week")).toBeInTheDocument();
+	});
+
+	it("leaves the week out when the plan has none", () => {
+		render(<UsageMeter usage={fiveHourOnly} />);
+		expect(screen.queryByText("Week")).toBeNull();
+	});
 });`;

const POPOVER_PATCH = `@@ -1,8 +1,10 @@
 import { useState } from "react";
+import { resetIn } from "./reset";

 export const UsagePopover = ({ account }: Props) => {
 	const [open, setOpen] = useState(false);
-	const reset = account.usage?.resets_at;
+	const reset = resetIn(account.usage?.resets_at, Date.now());
 	return (
 		<Popover open={open} onOpenChange={setOpen}>
-			<span>Resets {reset}</span>
+			<span>{reset ? \`Resets in \${reset}\` : "No reset yet"}</span>`;

const RESET_PATCH = `@@ -0,0 +1,9 @@
+/** How long until \`at\`, in hours and minutes, or null once it passed. */
+export const resetIn = (at: number | null | undefined, now: number): string | null => {
+	if (!at || at <= now) return null;
+	const minutes = Math.ceil((at - now) / 60_000);
+	const hours = Math.floor(minutes / 60);
+	return hours > 0 ? \`\${hours}h \${minutes % 60}m\` : \`\${minutes}m\`;
+};`;

const SORT_PATCH = `@@ -44,7 +44,11 @@ const byOrder = (order: SortOrder) => (a: Row, b: Row) => {
-	return a.name.localeCompare(b.name);
+	if (order === "activity") return b.lastActivity - a.lastActivity;
+	if (order === "started") return b.startedAt - a.startedAt;
+	return a.name.localeCompare(b.name);
 };`;

const CHANGELOG_PATCH = `@@ -1,5 +1,11 @@
 # Changelog

+## 0.3.0
+
+- Agents can open and watch their own pull requests.
+- The header meter shows the week under the 5-hour bar.
+- Settings has a default editor for Open in editor.
+
 ## 0.2.0`;

const VERSION_PATCH = `@@ -1,6 +1,6 @@
 {
 	"name": "maxhq",
-	"version": "0.2.0",
+	"version": "0.3.0",
 	"private": true,`;

const file = (
	path: string,
	patch: string | null,
	additions: number,
	deletions: number,
	status: PullFile["status"] = "modified"
): PullFile => ({ path, status, additions, deletions, patch });

const snapshot = (
	now: Timestamp,
	fields: Partial<PullSnapshot> & Pick<PullSnapshot, "title" | "head" | "head_sha">
): PullSnapshot => ({
	state: "open",
	draft: false,
	author: MOCK_GITHUB_LOGIN,
	base: "main",
	checks: "passing",
	mergeable: "yes",
	review: "none",
	additions: 0,
	deletions: 0,
	updated_at: now - HOUR,
	merged_at: null,
	...fields,
});

/** Sizes a PR from its files, as GitHub counts them. */
const sized = (pull: MockPull): MockPull => ({
	...pull,
	pull: {
		...pull.pull,
		additions: pull.files.reduce((sum, f) => sum + f.additions, 0),
		deletions: pull.files.reduce((sum, f) => sum + f.deletions, 0),
	},
});

const samplePulls = (now: Timestamp): MockPull[] =>
	(
		[
			{
				number: 141,
				pull: snapshot(now, {
					title: "Show the week under the 5-hour bar",
					author: "sam-reviewer",
					head: "usage-week",
					head_sha: "a41c09e7d2f35b18c6e0942f7ab1d3e5c8f60b21",
					review: "approved",
					updated_at: now - 25 * MIN,
				}),
				body: "The header meter shows only the 5-hour window. This adds the week as a thin bar under it, for plans that have one.\n\n- Codex Pro and Claude Max show both.\n- Cursor shows neither, as before.",
				checks: passing(9100),
				reviews: [
					{
						author: MOCK_GITHUB_LOGIN,
						verdict: "commented",
						commit_sha: "a41c09e7d2f35b18c6e0942f7ab1d3e5c8f60b21",
						at: now - 2 * HOUR,
					},
					{
						author: "alex-maintainer",
						verdict: "approved",
						commit_sha: "a41c09e7d2f35b18c6e0942f7ab1d3e5c8f60b21",
						at: now - 40 * MIN,
					},
				],
				files: [
					file("apps/desktop/src/usage/meter.tsx", METER_PATCH, 2, 0),
					file("apps/desktop/src/usage/meter.test.tsx", METER_TEST_PATCH, 10, 0),
				],
				comments: [
					comment(
						141,
						7001,
						MOCK_GITHUB_LOGIN,
						"Should the week bar hide on Cursor?",
						now - 2 * HOUR
					),
					comment(
						141,
						7002,
						"sam-reviewer",
						"Yes, Cursor has no week. I left it out there.",
						now - 90 * MIN
					),
					comment(141, 7003, "alex-maintainer", "Looks good to me.", now - 40 * MIN),
				],
			},
			{
				number: 142,
				pull: snapshot(now, {
					title: "Usage meter popover",
					head: "usage-popover",
					head_sha: "b52d1af8e3046c29d7f1a53b8bc2e4f6d9071c32",
					checks: "failing",
					review: "changes_requested",
					updated_at: now - 12 * MIN,
				}),
				body: "Adds a popover on the meter with the reset time.",
				checks: [
					check("Lint", "passing", 9200),
					check("Unit tests", "failing", 9201),
					check("Build macOS", "passing", 9202),
				],
				reviews: [
					{
						author: "alex-maintainer",
						verdict: "changes_requested",
						commit_sha: "b52d1af8e3046c29d7f1a53b8bc2e4f6d9071c32",
						at: now - 30 * MIN,
					},
				],
				files: [
					file("apps/desktop/src/usage/popover.tsx", POPOVER_PATCH, 3, 2),
					file("apps/desktop/src/usage/reset.ts", RESET_PATCH, 7, 0, "added"),
				],
				comments: [
					comment(
						142,
						7101,
						"alex-maintainer",
						"The reset test fails when the clock passes the hour.",
						now - 30 * MIN,
						"apps/desktop/src/usage/reset.ts",
						4
					),
				],
			},
			{
				number: 143,
				pull: snapshot(now, {
					title: "Sidebar sort order",
					draft: true,
					head: "sidebar-sort",
					head_sha: "c63e2b09f4157d3ae8025c4a9cd3f5071ae82d43",
					checks: "passing",
					updated_at: now - 3 * HOUR,
				}),
				body: "Work in progress. Sorts the sidebar by activity or start time.",
				checks: passing(9300),
				reviews: [],
				files: [file("apps/desktop/src/sidebar/sort.ts", SORT_PATCH, 3, 1)],
				comments: [],
			},
			{
				number: 144,
				pull: snapshot(now, {
					title: "Move settings into tabs",
					author: "sam-reviewer",
					head: "settings-tabs",
					head_sha: "d74f3c1a05268e4bf9136d5bade4061820b93e54",
					mergeable: "conflicting",
					review: "approved",
					updated_at: now - DAY,
				}),
				body: "Splits the settings page into tabs.",
				checks: passing(9400),
				reviews: [
					{
						author: "alex-maintainer",
						verdict: "approved",
						commit_sha: "d74f3c1a05268e4bf9136d5bade4061820b93e54",
						at: now - DAY - HOUR,
					},
				],
				files: [file("apps/desktop/src/settings/tabs.tsx", null, 180, 64)],
				comments: [],
			},
			{
				number: 145,
				pull: snapshot(now, {
					title: "Release 0.3.0",
					head: "release-0.3.0",
					head_sha: "e8504d2b16379f5ca0247e6cbef5172931ca4f65",
					checks: "pending",
					updated_at: now - 4 * MIN,
				}),
				body: "Bumps the version and writes the changelog for 0.3.0.",
				checks: [
					check("Lint", "passing", 9500),
					check("Unit tests", "pending", 9501),
					check("Build macOS", "pending", 9502),
					check("Build Linux", "passing", 9503),
					check("Release notes", "skipped", 9504),
				],
				reviews: [],
				files: [
					file("CHANGELOG.md", CHANGELOG_PATCH, 6, 0),
					file("package.json", VERSION_PATCH, 1, 1),
					file("apps/desktop/package.json", VERSION_PATCH, 1, 1),
					file("crates/daemon/Cargo.toml", null, 1, 1),
					file("apps/desktop/src-tauri/icons/icon.png", null, 0, 0),
				],
				comments: [
					comment(
						145,
						7501,
						MOCK_GITHUB_LOGIN,
						"Waiting on the macOS build.",
						now - 4 * MIN
					),
				],
			},
			{
				number: 138,
				pull: snapshot(now, {
					title: "Default editor setting",
					state: "merged",
					head: "default-editor",
					head_sha: "f9615e3c2748a06db1358f7dcf06283a42d5a076",
					review: "approved",
					updated_at: now - 2 * DAY,
					merged_at: now - 2 * DAY,
				}),
				body: "Adds a setting for which editor Open in editor uses.",
				checks: passing(9800),
				reviews: [],
				files: [file("apps/desktop/src/editors/default.ts", null, 24, 3)],
				comments: [],
			},
			{
				number: 136,
				pull: snapshot(now, {
					title: "Try a tray icon",
					state: "closed",
					author: "sam-reviewer",
					head: "tray-icon",
					head_sha: "0a726f4d3859b17ec2469a8ed017394b53e6b187",
					checks: "none",
					updated_at: now - 5 * DAY,
				}),
				body: "An experiment we didn't keep.",
				checks: [],
				reviews: [],
				files: [file("apps/desktop/src-tauri/src/tray.rs", null, 88, 0, "added")],
				comments: [
					comment(
						136,
						7601,
						"sam-reviewer",
						"Closing, the menu bar covers this.",
						now - 5 * DAY
					),
				],
			},
		] satisfies MockPull[]
	).map(sized);

/** The sample agents' links, the way their devices made them. */
const SAMPLE_LINKS: readonly (readonly [AgentId, number, PullLink["source"]])[] = [
	["agent-release", 141, "agent"],
	["agent-usage-popover", 142, "created"],
	["agent-sidebar-sort", 143, "branch"],
	[NOTES, 145, "created"],
	["agent-release", 145, "you"],
	["agent-bindings", 144, "dismissed"],
];

/** The sample's PR calls, and what they change. */
export type MockPulls = {
	readonly calls: PullCalls;
	/** An agent's links, dismissed ones too, as its device publishes them. */
	readonly linksOf: (agent: AgentId) => PullLink[] | undefined;
};

/**
 * Sample PRs in memory. `changed` runs when a link or a PR changes, so
 * the source can publish the agents' links again. `repoOf` gives an
 * agent's repository, `login` the login you're signed in to GitHub as, if
 * any, and `signedIn` signs you in on a device once a sample code is
 * "entered".
 */
export const mockPulls = ({
	now,
	changed = () => {},
	repoOf,
	login,
	signedIn,
}: {
	now: () => Timestamp;
	changed?: () => void;
	repoOf: (agent: AgentId) => string | null;
	login: () => string | null;
	signedIn: (device: DeviceId, login: string) => void;
}): MockPulls => {
	const pulls = new Map(samplePulls(now()).map((p) => [p.number, p]));
	const links = new Map<
		AgentId,
		{ number: number; source: PullLink["source"]; at: Timestamp }[]
	>();
	for (const [agent, number, source] of SAMPLE_LINKS) {
		links.set(agent, [...(links.get(agent) ?? []), { number, source, at: now() - HOUR }]);
	}

	const keyOf = (number: number): PullKey => ({ repo: MOCK_REPO, number });
	const ours = (repo: string) => repo.toLowerCase() === MOCK_REPO;
	const find = (repo: string, number: number) => (ours(repo) ? pulls.get(number) : undefined);

	const linkedTo = (number: number): AgentId[] =>
		[...links]
			.filter(([, held]) => held.some((l) => l.number === number && l.source !== "dismissed"))
			.map(([agent]) => agent)
			.sort();

	const summary = (p: MockPull): PullSummary => ({
		key: keyOf(p.number),
		pull: p.pull,
		agents: linkedTo(p.number),
	});

	const detail = (p: MockPull): PullDetail => ({
		summary: summary(p),
		body: p.body,
		checks: p.checks,
		reviews: p.reviews,
		merge_methods: ["squash", "merge"],
	});

	const change = (number: number, edit: (p: MockPull) => MockPull) => {
		const p = pulls.get(number);
		if (p) pulls.set(number, edit(p));
		changed();
	};

	const merge = (p: MockPull, method: MergeMethod, expectSha: string): ActionResult => {
		if (p.pull.state !== "open") return refused(NOT_OPEN);
		if (p.pull.head_sha !== expectSha) return refused(MOVED_MERGE);
		if (p.pull.draft) return refused("GitHub won't merge a draft PR.");
		if (p.pull.mergeable === "conflicting")
			return refused("This PR has conflicts to resolve first.");
		if (!detail(p).merge_methods.includes(method))
			return refused("This repository doesn't allow that merge method.");
		const at = now();
		change(p.number, (q) => ({
			...q,
			pull: { ...q.pull, state: "merged", merged_at: at, updated_at: at },
		}));
		return { status: "ok", data: { status: "pull_merged", queued: false } };
	};

	const approve = (p: MockPull, you: string, expectSha: string): ActionResult => {
		if (p.pull.state !== "open") return refused(NOT_OPEN);
		if (p.pull.head_sha !== expectSha) return refused(MOVED_APPROVE);
		if (p.pull.author.toLowerCase() === you.toLowerCase()) return refused(YOUR_OWN);
		const at = now();
		change(p.number, (q) => ({
			...q,
			pull: { ...q.pull, review: "approved", updated_at: at },
			reviews: [
				...q.reviews,
				{ author: you, verdict: "approved", commit_sha: expectSha, at },
			],
		}));
		return DONE;
	};

	const waiting = new Map<string, ReturnType<typeof setTimeout>>();
	let signIns = 0;

	const calls: PullCalls = {
		listPulls: async (project, state) => {
			const rows = project === MOCK_PULLS_PROJECT ? [...pulls.values()] : [];
			const shown = rows
				.filter((p) =>
					state === "open" ? p.pull.state === "open" : p.pull.state !== "open"
				)
				.sort((a, b) => b.pull.updated_at - a.pull.updated_at)
				.map(summary);
			return { status: "ok", data: { status: "pulls", pulls: shown } };
		},
		listBranches: async (project) => {
			const heads =
				project === MOCK_PULLS_PROJECT ? [...pulls.values()].map((p) => p.pull.head) : [];
			const branches = [...new Set(["main", ...heads])].sort();
			return { status: "ok", data: { status: "branches", branches, default_branch: "main" } };
		},
		getPull: async (_device, repo, number) => {
			const p = find(repo, number);
			return p
				? { status: "ok", data: { status: "pull", pull: detail(p) } }
				: refused(NO_PULL);
		},
		pullFiles: async (_device, repo, number, page) => {
			const p = find(repo, number);
			if (!p) return refused(NO_PULL);
			const start = (Math.max(page, 1) - 1) * FILES_PAGE;
			const files = p.files.slice(start, start + FILES_PAGE);
			const more = p.files.length > start + FILES_PAGE;
			return { status: "ok", data: { status: "pull_files", files, more } };
		},
		pullComments: async (_device, repo, number) => {
			const p = find(repo, number);
			return p
				? { status: "ok", data: { status: "pull_comments", comments: p.comments } }
				: refused(NO_PULL);
		},
		pullChecks: async (_device, repo, number) => {
			const p = find(repo, number);
			return p
				? {
						status: "ok",
						data: { status: "pull_checks", state: p.pull.checks, checks: p.checks },
					}
				: refused(NO_PULL);
		},
		pullAction: async (repo, number, action, expectSha) => {
			const you = login();
			if (!you) return { status: "error", error: { kind: "failed", message: NO_SIGN_IN } };
			const p = find(repo, number);
			if (!p) return refused(NO_PULL);
			return action.action === "merge"
				? merge(p, action.method, expectSha)
				: approve(p, you, expectSha);
		},
		pullsDue: async () => DONE,
		linkPull: async (agent, number) => {
			const repo = repoOf(agent);
			if (!repo) return refused("This agent isn't in a GitHub repository.");
			if (!ours(repo) || !pulls.has(number)) return refused(NO_PULL);
			const held = (links.get(agent) ?? []).filter((l) => l.number !== number);
			links.set(agent, [...held, { number, source: "you", at: now() }]);
			changed();
			return DONE;
		},
		unlinkPull: async (agent, repo, number) => {
			if (!ours(repo)) return DONE;
			const held = (links.get(agent) ?? []).filter((l) => l.number !== number);
			links.set(agent, [...held, { number, source: "dismissed", at: now() }]);
			changed();
			return DONE;
		},
		beginGithubSignIn: async (device) => {
			signIns += 1;
			const signIn = `sign-in-${signIns}`;
			waiting.set(
				signIn,
				setTimeout(() => {
					waiting.delete(signIn);
					signedIn(device, MOCK_GITHUB_LOGIN);
				}, SIGN_IN_MS)
			);
			return {
				status: "ok",
				data: {
					status: "device_code",
					sign_in: signIn,
					user_code: "WDJB-MJHT",
					verification_uri: "https://github.com/login/device",
					expires_at: now() + CODE_LIFETIME_MS,
				},
			};
		},
		cancelGithubSignIn: async (_device, signIn) => {
			clearTimeout(waiting.get(signIn));
			waiting.delete(signIn);
			return DONE;
		},
		// The sample's sign-ins always go through, so none is refused.
		checkGithubSignIn: async () => DONE,
	};

	const linksOf = (agent: AgentId): PullLink[] | undefined =>
		links.get(agent)?.map((l) => ({
			key: keyOf(l.number),
			source: l.source,
			at: l.at,
			snapshot: pulls.get(l.number)?.pull ?? null,
			watch: null,
		}));

	return { calls, linksOf };
};
