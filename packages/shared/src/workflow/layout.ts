// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the Workflow tab draws, worked out from a run: the rows with their
// lanes and groups, where loops go, and which agent a node opens. One row
// per node down a main lane, with nodes the workflow puts aside on a
// second, which keeps a run readable in a narrow column.

import type {
	AgentId,
	LoopState,
	NodeShape,
	NodeState,
	NodeStatus,
	RunShape,
	Tone,
} from "../bindings";

/** Whether a node counts as finished in a group's count. */
export const isSettled = (status: NodeStatus): boolean => status === "done" || status === "skipped";

/** Whether the lane into a node is drawn solid, because the run got there. */
export const isReached = (status: NodeStatus): boolean =>
	status !== "queued" && status !== "ready" && status !== "will_skip" && status !== "omitted";

/** The main lane, and the one beside it for nodes the workflow puts aside. */
export type Lane = 0 | 1;

/** One row of the tab. */
export type TabRow =
	| {
			readonly kind: "node";
			readonly node: NodeShape;
			readonly lane: Lane;
			/** Whether it sits in an expanded group, with a bracket beside it. */
			readonly grouped: boolean;
	  }
	| {
			readonly kind: "group";
			readonly id: string;
			readonly label: string;
			/** Its members, in the workflow's order. */
			readonly members: readonly NodeShape[];
			readonly open: boolean;
	  };

/**
 * The tab's rows in the workflow's order. Omitted nodes are left out. A
 * group takes the place of its first member, as one row while folded and
 * then one row per member while open. A node with a lane of its own sits
 * on the second lane.
 */
export const tabRows = (
	shape: RunShape,
	status: (id: string) => NodeStatus | undefined,
	open: ReadonlySet<string>
): TabRow[] => {
	const shown = shape.nodes.filter((node) => status(node.id) !== "omitted");
	const groups = new Map(shape.groups.map((group) => [group.id, group]));
	const rows: TabRow[] = [];
	const placed = new Set<string>();
	for (const node of shown) {
		const group = node.group === null ? undefined : groups.get(node.group);
		if (!group) {
			rows.push({ kind: "node", node, lane: node.lane === null ? 0 : 1, grouped: false });
			continue;
		}
		if (placed.has(group.id)) continue;
		placed.add(group.id);
		const members = shown.filter((member) => member.group === group.id);
		const isOpen = open.has(group.id);
		rows.push({ kind: "group", id: group.id, label: group.label, members, open: isOpen });
		if (isOpen) {
			for (const member of members) {
				rows.push({ kind: "node", node: member, lane: 0, grouped: true });
			}
		}
	}
	return rows;
};

// The order a folded group's glyph picks its members' statuses in, most
// pressing first.
const PRESSING: readonly NodeStatus[] = [
	"waiting_human",
	"failed",
	"blocked",
	"busy",
	"waiting_child",
	"stale",
	"unknown",
	"ready",
	"queued",
	"will_skip",
];

/** The status a folded group shows: its most pressing member's, or done once all are settled. */
export const groupStatus = (statuses: readonly NodeStatus[]): NodeStatus =>
	PRESSING.find((status) => statuses.includes(status)) ??
	(statuses.includes("done") ? "done" : "skipped");

/** The groups open at first: the ones the workflow doesn't fold. */
export const openAtFirst = (shape: RunShape): Set<string> =>
	new Set(shape.groups.filter((group) => !group.collapsed).map((group) => group.id));

/** The row a node shows on, which is its group's while that's folded. */
export const rowOf = (rows: readonly TabRow[], node: string): number =>
	rows.findIndex((row) =>
		row.kind === "node"
			? row.node.id === node
			: !row.open && row.members.some((member) => member.id === node)
	);

/** A loop drawn in the right gutter, from the row it went back from up to the row it went to. */
export type LoopArc = {
	readonly loop: LoopState;
	readonly from: number;
	readonly to: number;
	readonly tone: Tone;
};

/** How a loop's counter reads: amber at its last allowed loop, red once spent. */
export const loopTone = (loop: LoopState): Tone => {
	if (loop.budget === null) return "info";
	if (loop.count >= loop.budget) return "bad";
	if (loop.count === loop.budget - 1) return "warn";
	return "info";
};

/** The loops that have run, placed on the rows. A loop inside one folded row isn't drawn. */
export const loopArcs = (rows: readonly TabRow[], loops: readonly LoopState[]): LoopArc[] =>
	loops.flatMap((loop) => {
		if (loop.count === 0) return [];
		const from = rowOf(rows, loop.from);
		const to = rowOf(rows, loop.to);
		if (from < 0 || to < 0 || from === to) return [];
		return [{ loop, from, to, tone: loopTone(loop) }];
	});

/** A loop's counter, such as "2/3" or "2". */
export const loopCount = (loop: LoopState): string =>
	loop.budget === null ? `${loop.count}` : `${loop.count}/${loop.budget}`;

const ORCHESTRATED: ReadonlySet<NodeStatus> = new Set(["busy", "waiting_child", "waiting_human"]);

/**
 * The agent a node opens: the one behind its latest attempt, else the last
 * one bound to it. A node with no agent of its own opens the run's
 * orchestrator while it's under way, and nothing otherwise.
 */
export const nodeAgent = (node: NodeState, orchestrator: AgentId | null): AgentId | null => {
	for (let i = node.attempts.length - 1; i >= 0; i--) {
		const agent = node.attempts[i]!.agent;
		if (agent !== null) return agent;
	}
	const bound = node.agents.at(-1);
	if (bound !== undefined) return bound;
	return ORCHESTRATED.has(node.status) ? orchestrator : null;
};

/** Whether a node is the one the selected agent works on. */
export const holdsAgent = (node: NodeState, agent: AgentId | null): boolean =>
	agent !== null &&
	(node.agents.includes(agent) || node.attempts.some((attempt) => attempt.agent === agent));
