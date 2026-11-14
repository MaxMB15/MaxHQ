// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The CPU benchmark. It replays a busy few seconds through the whole window,
// as the app core sends them: four agents stream long answers with tool
// calls, one of them shown, and maxd sends each agent four times a second.
// It counts the work for each streamed second: time in JS, what React
// renders, and how much Markdown the lexer reads. The limits sit well above
// what it takes now, so only a real step back fails. To print the numbers,
// run it with VITE_MAXHQ_REPLAY=1 and --silent=false.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { Profiler } from "react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import { mockSource } from "@/data/mock-source";
import type { AgentId, Delta, TimelineUpdate } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { countLexing, FRAME_MS, replay } from "@maxhq/shared/test/replay";
import { createUiStore } from "@maxhq/shared/ui/store";
import type { EntityOf } from "@maxhq/shared/view/replica";

import App from "./App";
import { countRenders } from "./test/renders";

const hooks = vi.hoisted(() => ({ draws: 0 }));

// Mermaid needs a real page to draw, so this counts the draws instead.
vi.mock("./agent-view/diagram-render", () => ({
	drawnDiagram: () => undefined,
	allowDiagram: () => {},
	drawDiagram: async () => {
		hooks.draws++;
		return { state: "failed", reason: "Not drawn in tests", heldBack: false };
	},
}));

/** The agents that stream, the shown one first. */
const STREAMING = ["agent-gtfs", "agent-w1", "agent-w2", "agent-w3"];

const SECONDS = 3;

// Frames run when the replay says, the way the browser runs them before it paints.
let queued = new Map<number, FrameRequestCallback>();
let next = 0;
const runFrame = (time: number) => {
	const due = queued;
	queued = new Map();
	for (const callback of due.values()) callback(time);
};

beforeAll(() => {
	vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
		queued.set(++next, callback);
		return next;
	});
	vi.stubGlobal("cancelAnimationFrame", (id: number) => queued.delete(id));
	vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(800);
	vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(1000);
	vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(120);
	// The scroller's content is the sizer the virtualizer sets the height of,
	// and a scroll stops at either end, so the list follows the newest row.
	vi.spyOn(Element.prototype, "scrollHeight", "get").mockImplementation(function (this: Element) {
		const sizer = this.firstElementChild;
		const size = sizer instanceof HTMLElement ? parseFloat(sizer.style.height) : 0;
		return Math.max(800, size || 0);
	});
	Element.prototype.scrollTo = function (this: Element, options?: ScrollToOptions | number) {
		if (typeof options !== "object") return;
		const max = Math.max(this.scrollHeight - this.clientHeight, 0);
		const top = Math.min(Math.max(options.top ?? this.scrollTop, 0), max);
		if (top === this.scrollTop) return;
		this.scrollTop = top;
		this.dispatchEvent(new Event("scroll"));
	} as Element["scrollTo"];
});

afterAll(() => {
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
	Reflect.deleteProperty(Element.prototype, "scrollTo");
});

/**
 * The sample source with the view's deltas and the timelines in the
 * replay's hands. Its own deltas take the next numbers, so they fit in.
 */
const replaySource = () => {
	const base = mockSource({ now: () => NOW, tick: false });
	let seq = 1;
	let deliver: (delta: Delta) => void = () => {};
	const timelines = new Map<AgentId, (update: TimelineUpdate, live: boolean) => void>();
	let opened: TimelineUpdate | null = null;
	const source: DataSource = {
		...base,
		snapshot: async () => ({ ...(await base.snapshot()), seq }),
		subscribe: async (onDelta) => {
			deliver = onDelta;
			return base.subscribe((delta) => onDelta({ ...delta, seq: ++seq }));
		},
		openTimeline: async (agent, onUpdate) => {
			timelines.set(agent, onUpdate);
			if (opened) onUpdate(opened, false);
			return () => {
				timelines.delete(agent);
			};
		},
	};
	return {
		source,
		/** Sends a delta of `changes`, numbered next. */
		send: (changes: Delta["changes"]) => deliver({ seq: ++seq, changes }),
		/** The open timeline's listener for `agent`. */
		timeline: (agent: AgentId) => timelines.get(agent),
		/** What each open sends first. */
		opensWith: (update: TimelineUpdate) => void (opened = update),
	};
};

/** What the replay measured for each streamed second. */
type Work = {
	/** Time in JS for the view's deltas, and for the timeline's events and frames. */
	readonly viewMs: number;
	readonly timelineMs: number;
	readonly commits: number;
	readonly renders: number;
	readonly lexCalls: number;
	readonly lexChars: number;
	readonly draws: number;
	readonly top: readonly (readonly [string, number])[];
};

const measure = async (): Promise<Work> => {
	const { source, send, timeline, opensWith } = replaySource();
	const agents = (await source.snapshot()).entities.filter(
		(e): e is EntityOf<"agent"> => e.entity === "agent" && STREAMING.includes(e.id)
	);
	agents.sort((a, b) => STREAMING.indexOf(a.id) - STREAMING.indexOf(b.id));
	const shown = agents[0];
	if (!shown) throw new Error("No agent to show");
	const { items, frames } = replay({
		agents,
		seconds: SECONDS,
		now: NOW,
		streamed: 12_000,
		history: 60,
	});
	opensWith({ type: "reset", seq: 1, items: [...items], has_older: true });

	const counter = { counted: () => {} };
	const view = render(
		<Profiler id="window" onRender={() => counter.counted()}>
			<FixedNow value={NOW}>
				<App source={source} ui={createUiStore()} />
			</FixedNow>
		</Profiler>
	);
	const { renders, counted, reset } = countRenders(view.container);
	counter.counted = counted;
	fireEvent.click(await screen.findByRole("option", { name: new RegExp(shown.title) }));
	await screen.findByRole("heading", { level: 1, name: shown.title });
	// Let the open settle: its reset, its first measures and its first frames.
	for (let n = 0; n < 10; n++) {
		await act(async () => runFrame(n * FRAME_MS));
	}
	const push = timeline(shown.id);
	if (!push) throw new Error("The shown agent's timeline didn't open");

	reset();
	hooks.draws = 0;
	const lexing = countLexing();
	let timelineSeq = 1;
	const spent = { view: 0, timeline: 0 };
	const timed = (part: keyof typeof spent, work: () => void) => {
		const start = performance.now();
		act(work);
		spent[part] += performance.now() - start;
	};
	for (const [n, frame] of frames.entries()) {
		// A frame's view changes come in one event, so they're one task.
		if (frame.view.length > 0) {
			timed("view", () => {
				for (const changes of frame.view) send([...changes]);
			});
		}
		if (frame.events.length > 0) {
			timed("timeline", () =>
				push({ type: "events", seq: ++timelineSeq, events: [...frame.events] }, true)
			);
		}
		timed("timeline", () => runFrame((n + 10) * FRAME_MS));
	}
	lexing.stop();

	const per = (value: number) => Math.round(value / SECONDS);
	const total = [...renders.byName.values()].reduce((sum, count) => sum + count, 0);
	return {
		viewMs: per(spent.view),
		timelineMs: per(spent.timeline),
		commits: per(renders.commits),
		renders: per(total),
		lexCalls: per(lexing.lexing.calls),
		lexChars: per(lexing.lexing.chars),
		draws: per(hooks.draws),
		top: [...renders.byName]
			.sort((a, b) => b[1] - a[1])
			.slice(0, 12)
			.map(([name, count]) => [name, per(count)] as const),
	};
};

describe("the replay benchmark", () => {
	it(
		"streams four agents for a few seconds within its budget",
		{ timeout: 120_000 },
		async () => {
			const work = await measure();
			if (import.meta.env.VITE_MAXHQ_REPLAY) console.info(JSON.stringify(work));
			// It takes about 50 commits, 16000 renders and 6000 characters lexed
			// a second. Sending each agent with every chunk took 104 commits and
			// 86000 renders, each change to the view as its own task twice that,
			// and lexing every text whole took 330000.
			expect(work.commits).toBeGreaterThan(0);
			expect(work.commits).toBeLessThan(80);
			expect(work.renders).toBeLessThan(40_000);
			expect(work.lexChars).toBeLessThan(20_000);
			expect(work.draws).toBeLessThanOrEqual(2);
		}
	);
});
