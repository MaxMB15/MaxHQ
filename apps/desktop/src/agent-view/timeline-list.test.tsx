// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import type { DataSource } from "@maxhq/shared/data/source";
import type { TimelineItem, TimelineUpdate } from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { openTimeline } from "@maxhq/shared/timeline/store";

import { askForRequest, clearRequests } from "./request";
import { clearSnapshots, TimelineList } from "./timeline-list";

const ROW = 100;

// jsdom lays nothing out, so give the list a window and every row a height.
// A hidden pane is out of the page, where everything measures zero.
let height = 600;

// Whatever watches the scroller's size, called back when the test resizes it.
const watching = new Map<Element, Set<() => void>>();

class FakeResizeObserver {
	private readonly targets = new Set<Element>();
	private readonly notify: () => void;

	constructor(callback: ResizeObserverCallback) {
		this.notify = () => callback([], this as unknown as ResizeObserver);
	}

	observe(target: Element) {
		this.targets.add(target);
		const set = watching.get(target) ?? new Set();
		set.add(this.notify);
		watching.set(target, set);
	}

	unobserve(target: Element) {
		this.targets.delete(target);
		watching.get(target)?.delete(this.notify);
	}

	disconnect() {
		for (const target of this.targets) this.unobserve(target);
	}
}

const resized = (target: Element) => {
	for (const notify of [...(watching.get(target) ?? [])]) notify();
};

// Like a browser: a write past either end stops there, a write that moves fires
// a scroll event, and a hidden element ignores it.
const scrollTo = function (this: Element, options?: ScrollToOptions | number) {
	if (height === 0 || typeof options !== "object") return;
	const max = Math.max(this.scrollHeight - this.clientHeight, 0);
	const top = Math.min(Math.max(options.top ?? this.scrollTop, 0), max);
	if (top === this.scrollTop) return;
	this.scrollTop = top;
	this.dispatchEvent(new Event("scroll"));
};

beforeAll(() => {
	vi.stubGlobal("ResizeObserver", FakeResizeObserver);
	vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(() => height);
	vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(800);
	vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(ROW);
	// The scroller's content is the sizer the virtualizer sets the height of.
	vi.spyOn(Element.prototype, "scrollHeight", "get").mockImplementation(function (this: Element) {
		if (height === 0) return 0;
		const sizer = this.firstElementChild;
		const size = sizer instanceof HTMLElement ? parseFloat(sizer.style.height) : 0;
		return Math.max(height, size || 0);
	});
	Element.prototype.scrollTo = scrollTo as Element["scrollTo"];
});

afterAll(() => {
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
	Reflect.deleteProperty(Element.prototype, "scrollTo");
});

afterEach(() => {
	height = 600;
	watching.clear();
	clearSnapshots();
	clearRequests();
});

const say = (n: number): TimelineItem =>
	timelineItem(
		{ kind: "assistant_message", text: `Message ${n}`, streaming: false },
		{ id: `m${n}`, ordinal: n }
	);

const reset = (count: number, hasOlder: boolean, seq = 1): TimelineUpdate => ({
	type: "reset",
	seq,
	items: Array.from({ length: count }, (_, n) => say(1000 + n)),
	has_older: hasOlder,
});

const openList = () => {
	let push: (update: TimelineUpdate) => void = () => {};
	const loadOlder = vi.fn(async () => ({ status: "ok" as const, data: null }));
	const source = {
		...mockSource({ tick: false }),
		openTimeline: async (
			_agent: string,
			onUpdate: (update: TimelineUpdate, live: boolean) => void
		) => {
			push = (update) => onUpdate(update, false);
			return () => {};
		},
		loadOlder,
	} satisfies DataSource;
	const timeline = openTimeline(source, "a1", { schedule: (flush) => flush() });
	// What the cards that wait on you need from the view around them.
	const row: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline: timeline.store,
		readOnly: false,
		remote: false,
		answer: async () => ({ status: "ok", data: { status: "done" } }),
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	const mount = (container?: HTMLElement) =>
		render(
			<ServicesProvider source={source}>
				<RowProvider value={row}>
					<TimelineList
						agent="a1"
						timeline={timeline}
						observed={false}
						startedAt={NOW}
						ended={false}
						endedAt={null}
					/>
				</RowProvider>
			</ServicesProvider>,
			{ container }
		);
	return { mount, push: (update: TimelineUpdate) => act(() => push(update)), loadOlder };
};

// Lets the virtualizer finish a scroll it is still checking, a few frames on.
const settle = () => act(() => new Promise<void>((done) => setTimeout(done, 120)));

const atEnd = (scroller: HTMLElement) =>
	scroller.scrollTop > 0 && scroller.scrollTop === scroller.scrollHeight - scroller.clientHeight;

// Opens a long conversation the way the app does: the list mounts first, and
// the items arrive after.
const showList = async (hasOlder = false, container?: HTMLElement) => {
	const { mount, push, loadOlder } = openList();
	mount(container);
	// Let the source open, then send the newest page.
	await act(async () => {});
	push(reset(200, hasOlder));
	const scroller = screen.getByRole("region", { name: "Conversation" });
	await waitFor(() => expect(atEnd(scroller)).toBe(true));
	return { scroller, push, loadOlder };
};

const scrollUp = (scroller: HTMLElement, by: number) =>
	act(() => {
		fireEvent.wheel(scroller, { deltaY: -by });
		scroller.scrollTop -= by;
		fireEvent.scroll(scroller);
	});

const scrollDown = (scroller: HTMLElement, by: number) =>
	act(() => {
		fireEvent.wheel(scroller, { deltaY: by });
		scroller.scrollTop += by;
		fireEvent.scroll(scroller);
	});

// What dockview does to the pane of a background tab: it takes it out of the
// page, and the browser forgets how far it was scrolled.
const hide = (scroller: HTMLElement) =>
	act(() => {
		height = 0;
		scroller.scrollTop = 0;
		resized(scroller);
	});

const showAgain = (scroller: HTMLElement) =>
	act(() => {
		height = 600;
		resized(scroller);
	});

// A scroll nobody asked for here: the list moved, with no wheel or key behind it.
const moves = (scroller: HTMLElement, by: number) =>
	act(() => {
		scroller.scrollTop -= by;
		fireEvent.scroll(scroller);
	});

// The list loses rows, and the browser takes the view back to its new end.
const getsShorter = (scroller: HTMLElement, push: (update: TimelineUpdate) => void) => {
	push(reset(190, false, 2));
	act(() => {
		const max = scroller.scrollHeight - scroller.clientHeight;
		if (scroller.scrollTop <= max) return;
		scroller.scrollTop = max;
		fireEvent.scroll(scroller);
	});
};

const click = (element: Element) => {
	fireEvent.pointerDown(element);
	fireEvent.pointerUp(element);
	fireEvent.click(element);
};

const askFirst = () =>
	timelineItem(
		{
			kind: "approval",
			attention: "att-1",
			approval_kind: "command",
			title: "Run `pnpm install`",
			detail: null,
			cwd: null,
			options: [{ id: "allow_once", label: "Allow once" }],
			decided: null,
		},
		{ id: "ask", ordinal: 1200, status: "waiting" }
	);

const following = () => screen.queryByRole("button", { name: "Jump to latest" }) === null;

// Rows that cover some of the window.
const rowsInView = (scroller: HTMLElement) =>
	[...scroller.querySelectorAll<HTMLElement>("[data-index]")].filter((row) => {
		const top = Number(
			/translate3d\(0(?:px)?, ([\d.]+)px/.exec(row.style.transform)?.[1] ?? NaN
		);
		return top < scroller.scrollTop + height && top + ROW > scroller.scrollTop;
	});

describe("timeline list", () => {
	it("is back where you left it when its pane shows again", async () => {
		const { scroller } = await showList();
		scrollUp(scroller, 3000);
		const place = scroller.scrollTop;
		expect(rowsInView(scroller)).not.toHaveLength(0);

		hide(scroller);
		showAgain(scroller);

		expect(scroller.scrollTop).toBe(place);
		expect(rowsInView(scroller)).not.toHaveLength(0);
		expect(screen.getByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
	});

	it("is back at the end when its pane shows again while following", async () => {
		const { scroller } = await showList();

		hide(scroller);
		showAgain(scroller);

		expect(scroller.scrollTop).toBe(scroller.scrollHeight - scroller.clientHeight);
		expect(rowsInView(scroller)).not.toHaveLength(0);
	});

	it("is back where you left it when its pane is taken out and put straight back", async () => {
		// dockview's box for a pane's content, in its group's content box.
		const box = document.body.appendChild(document.createElement("div"));
		const part = box.appendChild(document.createElement("div"));
		part.className = "dv-react-part";
		const { scroller } = await showList(false, part);
		scrollUp(scroller, 3000);
		const place = scroller.scrollTop;

		// What a click on the pane's tab does, which no resize shows: the
		// browser forgets how far the list was scrolled.
		await act(async () => {
			part.remove();
			box.appendChild(part);
			scroller.scrollTop = 0;
			await new Promise((done) => requestAnimationFrame(done));
		});

		expect(scroller.scrollTop).toBe(place);
		expect(rowsInView(scroller)).not.toHaveLength(0);
		box.remove();
	});

	it("doesn't load earlier items while its pane is hidden", async () => {
		const { scroller, push, loadOlder } = await showList(true);
		scrollUp(scroller, 3000);

		hide(scroller);
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: say(1200) }] });
		expect(loadOlder).not.toHaveBeenCalled();

		showAgain(scroller);
		expect(loadOlder).not.toHaveBeenCalled();
	});

	it("opens at the end when the conversation is already loaded", async () => {
		const { mount, push } = openList();
		// Another pane opened this agent first, so its newest page is here before the list.
		await act(async () => {});
		push(reset(200, false));

		mount();

		const scroller = screen.getByRole("region", { name: "Conversation" });
		await waitFor(() => expect(atEnd(scroller)).toBe(true));
		expect(rowsInView(scroller)).not.toHaveLength(0);
	});

	it("stays at the end when a short list gets shorter", async () => {
		height = 400;
		const { scroller } = await showList();

		await settle();

		// Something above or below the list takes some of its room.
		act(() => {
			height = 250;
			resized(scroller);
		});

		await waitFor(() => expect(atEnd(scroller)).toBe(true));
		expect(rowsInView(scroller)).not.toHaveLength(0);
		expect(screen.queryByRole("button", { name: "Jump to latest" })).not.toBeInTheDocument();
	});

	it("keeps your place when the list gets shorter after you scroll up", async () => {
		const { scroller } = await showList();
		scrollUp(scroller, 3000);
		const place = scroller.scrollTop;

		act(() => {
			height = 400;
			resized(scroller);
		});

		expect(scroller.scrollTop).toBe(place);
		expect(screen.getByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
	});

	it("lets you scroll up from the end a little at a time", async () => {
		const { scroller } = await showList();
		const end = scroller.scrollTop;

		// A trackpad starts a scroll with small steps, each still near the end.
		for (let step = 0; step < 5; step++) scrollUp(scroller, 10);

		expect(scroller.scrollTop).toBe(end - 50);
		expect(screen.getByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
	});

	it("goes to the card waiting on you and focuses it when asked", async () => {
		const { scroller, push } = await showList();
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: askFirst() }] });
		scrollUp(scroller, 3000);

		act(() => askForRequest("a1", "att-1"));

		const card = await screen.findByRole("region", { name: "Permission request" });
		await waitFor(() => expect(card).toHaveFocus());
	});

	it("follows again once you scroll back down near the end", async () => {
		const { scroller, push } = await showList();
		scrollUp(scroller, 3000);

		scrollDown(scroller, 3000 - 20);
		expect(screen.queryByRole("button", { name: "Jump to latest" })).not.toBeInTheDocument();

		push({ type: "events", seq: 2, events: [{ type: "upsert", item: say(1200) }] });
		await waitFor(() =>
			expect(scroller.scrollTop).toBe(scroller.scrollHeight - scroller.clientHeight)
		);
	});

	it("keeps following when you answer a card and the list gets shorter", async () => {
		const { scroller, push } = await showList();
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: askFirst() }] });
		const allow = await screen.findByRole("button", { name: /Allow once/ });

		click(allow);
		moves(scroller, 300);

		expect(following()).toBe(true);
	});

	it("doesn't take Space on a button for scrolling", async () => {
		const { scroller, push } = await showList();
		push({ type: "events", seq: 2, events: [{ type: "upsert", item: askFirst() }] });
		const allow = await screen.findByRole("button", { name: /Allow once/ });

		fireEvent.keyDown(allow, { key: " " });
		moves(scroller, 300);

		expect(following()).toBe(true);
	});

	it("keeps following when the list gets shorter just after you scroll down", async () => {
		const { scroller, push } = await showList();

		fireEvent.wheel(scroller, { deltaY: 10 });
		getsShorter(scroller, push);

		expect(following()).toBe(true);
		expect(atEnd(scroller)).toBe(true);
	});

	it("still stops following when you drag the scroll bar up", async () => {
		const { scroller } = await showList();

		fireEvent.pointerDown(scroller);
		moves(scroller, 300);

		expect(following()).toBe(false);
	});

	it("counts a scroll bar let go of outside the list as let go", async () => {
		const { scroller } = await showList();

		fireEvent.pointerDown(scroller);
		fireEvent.pointerUp(document.body);
		// Past the moment a scroll still counts as yours.
		await act(() => new Promise<void>((done) => setTimeout(done, 300)));
		moves(scroller, 300);

		expect(following()).toBe(true);
	});

	it("still stops following on a step of one pixel up from the end", async () => {
		const { scroller } = await showList();

		scrollUp(scroller, 1);

		expect(following()).toBe(false);
	});
});

const ask = (id: string, ordinal: number, text: string): TimelineItem =>
	timelineItem(
		{ kind: "user_message", text, attachments: [], delivery: "sent" },
		{ id, ordinal }
	);

// A long conversation with a prompt every `every` rows, from the first.
const withPrompts = (every: number, hasOlder = false, count = 200): TimelineUpdate => ({
	type: "reset",
	seq: 1,
	items: Array.from({ length: count }, (_, n) =>
		n % every === 0 ? ask(`p${n}`, 1000 + n, `Prompt ${n / every + 1}\nmore`) : say(1000 + n)
	),
	has_older: hasOlder,
});

const showPrompts = async (every: number, hasOlder = false) => {
	const { mount, push } = openList();
	mount();
	await act(async () => {});
	push(withPrompts(every, hasOlder));
	const scroller = screen.getByRole("region", { name: "Conversation" });
	await waitFor(() => expect(atEnd(scroller)).toBe(true));
	return scroller;
};

// Where a row starts: the padding, the row above the items, then the rows before it.
const rowTop = (row: number) => 18 + ROW * (row + 1);

// The prompt row that starts last above a place, a prompt every 20 rows.
const promptAbove = (top: number) => {
	let row = 0;
	while (rowTop(row + 20) < top - 8) row += 20;
	return row;
};

describe("prompt steps", () => {
	it("steps to the prompt before the view's top from the pill and lights it", async () => {
		const scroller = await showPrompts(20);
		scrollUp(scroller, 3000);
		const rail = screen.getByRole("navigation", { name: "Your prompts" });
		expect(rail.querySelectorAll("[aria-current]")).toHaveLength(1);

		const row = promptAbove(scroller.scrollTop);

		const pill = screen.getAllByRole("button", { name: "Earlier prompt" }).at(-1)!;
		act(() => pill.click());
		await waitFor(() => expect(scroller.scrollTop).toBe(rowTop(row)));
		await waitFor(() =>
			expect(scroller.querySelector(`[data-quote-item='p${row}']`)).toHaveAttribute(
				"data-quote-flash"
			)
		);
		expect(screen.getByText(`${row / 20 + 1} / 10`)).toBeInTheDocument();
	});

	it("takes Option+Up and Option+Down, and goes to the latest past the last prompt", async () => {
		const scroller = await showPrompts(20);
		scrollUp(scroller, 5000);
		const row = promptAbove(scroller.scrollTop);
		fireEvent.keyDown(scroller, { key: "ArrowUp", altKey: true });
		await waitFor(() => expect(scroller.scrollTop).toBe(rowTop(row)));
		// The last prompt is row 180, of 200.
		for (let next = row + 20; next <= 180; next += 20) {
			fireEvent.keyDown(scroller, { key: "ArrowDown", altKey: true });
			await waitFor(() => expect(scroller.scrollTop).toBe(rowTop(next)));
		}
		fireEvent.keyDown(scroller, { key: "ArrowDown", altKey: true });
		await waitFor(() => expect(atEnd(scroller)).toBe(true));
		await waitFor(() => expect(following()).toBe(true));
	});

	it("shows the steps while older pages may hold a prompt, without a count", async () => {
		const scroller = await showPrompts(1000, true);
		scrollUp(scroller, 3000);
		const rail = screen.getByRole("navigation", { name: "Your prompts" });
		expect(within(rail).getByRole("button", { name: "Prompt 1" })).toBeInTheDocument();
		expect(screen.getAllByRole("button", { name: "Earlier prompt" })).toHaveLength(2);
		expect(screen.queryByText(/^\d+ \/ \d+$/)).toBeNull();
	});

	it("loads older pages until one has a prompt, and lights it", async () => {
		const { scroller, push, loadOlder } = await showList(true);
		scrollUp(scroller, 3000);
		expect(screen.getByRole("navigation", { name: "Your prompts" })).toBeInTheDocument();

		const pill = screen.getAllByRole("button", { name: "Earlier prompt" }).at(-1)!;
		act(() => pill.click());
		await waitFor(() => expect(loadOlder).toHaveBeenCalledTimes(1));
		push({
			type: "older",
			items: Array.from({ length: 10 }, (_, n) => say(990 + n)),
			has_older: true,
		});
		await waitFor(() => expect(loadOlder).toHaveBeenCalledTimes(2));
		push({
			type: "older",
			items: Array.from({ length: 10 }, (_, n) =>
				n === 5 ? ask("p985", 985, "Prompt 1") : say(980 + n)
			),
			has_older: true,
		});

		await waitFor(() => expect(scroller.scrollTop).toBe(rowTop(5)));
		await waitFor(() =>
			expect(scroller.querySelector("[data-quote-item='p985']")).toHaveAttribute(
				"data-quote-flash"
			)
		);
	});

	it("leaves the rail and the pill's arrows out with fewer than two prompts", async () => {
		const scroller = await showPrompts(1000);
		scrollUp(scroller, 3000);
		expect(screen.getByRole("button", { name: "Jump to latest" })).toBeInTheDocument();
		expect(screen.queryByRole("navigation", { name: "Your prompts" })).toBeNull();
		expect(screen.queryByRole("button", { name: "Earlier prompt" })).toBeNull();
	});
});
