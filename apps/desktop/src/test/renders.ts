// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Counts what React renders, by component, for the benchmarks. Put a
// Profiler at the top of the tree and call `counted` from its onRender: it
// walks the part of the fiber tree the commit touched, the way React's own
// developer tools do, and counts each component that ran.

/** The few fields of a React fiber this reads. */
type Fiber = {
	readonly tag: number;
	readonly type: unknown;
	readonly flags: number;
	readonly child: Fiber | null;
	readonly sibling: Fiber | null;
	readonly alternate: Fiber | null;
	readonly stateNode: unknown;
};

// React's fiber tags for components, and the flag a component that ran gets.
const FUNCTION = 0;
const CLASS = 1;
const FORWARD_REF = 11;
const SIMPLE_MEMO = 15;
const PERFORMED_WORK = 1;

const nameOf = (fiber: Fiber): string => {
	const type = fiber.type as {
		displayName?: string;
		name?: string;
		render?: { displayName?: string; name?: string };
	} | null;
	return (
		type?.displayName ||
		type?.name ||
		type?.render?.displayName ||
		type?.render?.name ||
		"anonymous"
	);
};

/** The root fiber of what React rendered into `container`. */
const rootOf = (container: Element): Fiber | null => {
	const key = Object.keys(container).find((k) => k.startsWith("__reactContainer$"));
	if (!key) return null;
	const created = (container as unknown as Record<string, Fiber>)[key];
	return (created?.stateNode as { current: Fiber } | undefined)?.current ?? null;
};

/** Renders by component name, and commits. */
export type Renders = { commits: number; byName: Map<string, number> };

/** A counter for what renders into `container`, with `counted` for the Profiler. */
export const countRenders = (container: Element) => {
	const renders: Renders = { commits: 0, byName: new Map() };
	const add = (fiber: Fiber) => {
		const name = nameOf(fiber);
		renders.byName.set(name, (renders.byName.get(name) ?? 0) + 1);
	};
	// A fiber the commit made or ran has a new copy. Where a parent bailed
	// out, its child is the same object as before and nothing under it ran.
	const walk = (next: Fiber, mounted: boolean) => {
		const prev = mounted ? null : next.alternate;
		const component =
			next.tag === FUNCTION ||
			next.tag === CLASS ||
			next.tag === FORWARD_REF ||
			next.tag === SIMPLE_MEMO;
		if (component && (prev === null || (next.flags & PERFORMED_WORK) !== 0)) add(next);
		if (prev !== null && next.child === prev.child) return;
		for (let child = next.child; child; child = child.sibling)
			walk(child, child.alternate === null);
	};
	const counted = () => {
		renders.commits++;
		const root = rootOf(container);
		if (root) walk(root, false);
	};
	const reset = () => {
		renders.commits = 0;
		renders.byName.clear();
	};
	return { renders, counted, reset };
};
