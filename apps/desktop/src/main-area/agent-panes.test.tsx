// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { takeRequest, watchRequests } from "@/agent-view/request";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { followLead } from "@maxhq/shared/agent-view/outside";
import type { AgentId } from "@maxhq/shared/bindings";
import { pillsIn } from "@/composer/testing";
import { mockSource } from "@/data/mock-source";
import { CLAUDE } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { revealTab } from "@/layout/tab-strip";

// The real helper, watched to see which tab each pane asks to show.
vi.mock("@/layout/tab-strip", async (original) => {
	const real = await original<typeof import("@/layout/tab-strip")>();
	return { revealTab: vi.fn(real.revealTab) };
});

const renderApp = (source = mockSource({ now: () => NOW, tick: false })) => {
	const ui = createUiStore();
	const rendered = render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { ui, ...rendered };
};

// An agent's row in the sidebar's list.
const row = (name: RegExp) =>
	within(screen.getByRole("navigation", { name: "Agents" })).findByRole("option", { name });
const tabs = () => screen.queryAllByRole("tab").map((t) => t.getAttribute("aria-label"));
const heading = () => screen.getByRole("heading", { level: 1 });

// A key pressed with Command, as on a Mac.
const press = (code: string, key: string, mods: { shiftKey?: boolean; altKey?: boolean } = {}) =>
	act(() => {
		fireEvent.keyDown(document.body, { code, key, metaKey: true, ...mods });
	});
const onMac = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
const splitButtons = () => screen.getAllByRole("button", { name: "Split pane" });

afterEach(() => {
	vi.restoreAllMocks();
	clearKept();
});

describe("agent panes", () => {
	it("opens an agent once and focuses its pane after that", async () => {
		const { ui } = renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/M1 observe milestone/));
		expect(tabs()).toEqual(["M1 observe milestone"]);

		fireEvent.click(await row(/Release checklist/));
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(heading()).toHaveTextContent("Release checklist");

		fireEvent.click(await row(/M1 observe milestone/));
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(heading()).toHaveTextContent("M1 observe milestone");
		const m1 = ui.getState().selectedAgent;

		// Picking a tab selects its agent, as a sidebar click would.
		const tab = screen.getByRole("tab", { name: "Release checklist" });
		fireEvent.pointerDown(tab, { button: 0 });
		fireEvent.pointerUp(tab, { button: 0 });
		fireEvent.click(tab);
		expect(heading()).toHaveTextContent("Release checklist");
		expect(ui.getState().selectedAgent).not.toBe(m1);
	});

	it("unmounts a closed pane's view and moves the selection to the next pane", async () => {
		const { ui } = renderApp();
		fireEvent.click(await row(/Release checklist/));
		const release = ui.getState().selectedAgent;
		fireEvent.click(await row(/M1 observe milestone/));

		fireEvent.click(screen.getByRole("button", { name: "Close M1 observe milestone" }));
		expect(tabs()).toEqual(["Release checklist"]);
		expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
		expect(heading()).toHaveTextContent("Release checklist");
		expect(ui.getState().selectedAgent).toBe(release);

		fireEvent.click(screen.getByRole("button", { name: "Close Release checklist" }));
		expect(tabs()).toEqual([]);
		expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
		expect(screen.getByText("Pick an agent to continue")).toBeInTheDocument();
		expect(ui.getState().selectedAgent).toBeNull();
	});

	it("lists what waits on you, what you closed and the keys with no pane open", async () => {
		onMac();
		renderApp();
		await row(/Release checklist/);
		const list = (name: string) =>
			within(screen.getByRole("region", { name }))
				.getAllByRole("button")
				.map((b) => b.textContent);
		expect(list("Needs you")).toEqual([
			"Subtitle sync driftQuestion",
			"Release checklistAllow?",
		]);
		expect(screen.queryByRole("region", { name: "Recently closed" })).not.toBeInTheDocument();
		expect(list("Keys")).toEqual([
			"New agent⌘N",
			"Next agent waiting⌘J",
			"Reopen the last closed pane⇧⌘T",
		]);
		expect(screen.getByRole("button", { name: /Reopen the last closed pane/ })).toBeDisabled();

		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		fireEvent.click(screen.getByRole("button", { name: "Close Release checklist" }));
		fireEvent.click(screen.getByRole("button", { name: "Close M1 observe milestone" }));
		expect(tabs()).toEqual([]);
		expect(list("Recently closed")).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(screen.getByRole("button", { name: /Reopen the last closed pane/ })).toBeEnabled();

		fireEvent.click(
			within(screen.getByRole("region", { name: "Recently closed" })).getByRole("button", {
				name: "Release checklist",
			})
		);
		expect(tabs()).toEqual(["Release checklist"]);
		expect(heading()).toHaveTextContent("Release checklist");
	});

	it("brings the window's panes back after a restart", async () => {
		const first = renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		first.unmount();

		const { ui } = renderApp();
		await row(/Release checklist/);
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(heading()).toHaveTextContent("Release checklist");
		expect(ui.getState().mode).toBe("projects");
		expect(ui.getState().selectedAgent).not.toBeNull();
	});

	it("keeps the panes while the devices view shows", async () => {
		const { ui } = renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		act(() => ui.getState().setMode("devices"));
		expect(tabs()).toEqual([]);
		act(() => ui.getState().setMode("projects"));
		expect(tabs()).toEqual(["M1 observe milestone"]);
	});

	it("points a sub-agent gone with its session to where the session went", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		let focus: (agent: AgentId) => void = () => {};
		Object.assign(source, {
			onFocusAgent: async (onFocus: typeof focus) => {
				focus = onFocus;
				return () => {};
			},
		});
		// Its banner saw the lead before the session left the terminal.
		followLead("sub-gone", "agent-release");
		renderApp(source);
		await row(/Release checklist/);
		act(() => focus("sub-gone"));
		const moved = await screen.findByRole("region", { name: "Session continues in MaxHQ" });
		fireEvent.click(within(moved).getByRole("button", { name: "Open session" }));
		expect(heading()).toHaveTextContent("Release checklist");
	});
});

describe("pane shortcuts", () => {
	it("switch tabs, split, maximize, close and reopen panes", async () => {
		onMac();
		const { ui } = renderApp();
		const selected = () => ui.getState().selectedAgent;
		fireEvent.click(await row(/M1 observe milestone/));
		const m1 = selected();
		fireEvent.click(await row(/Release checklist/));
		const release = selected();
		expect(release).not.toBe(m1);

		press("BracketLeft", "{", { shiftKey: true });
		expect(selected()).toBe(m1);
		press("Digit9", "9");
		expect(selected()).toBe(release);
		press("Digit1", "1");
		expect(selected()).toBe(m1);
		expect(heading()).toHaveTextContent("M1 observe milestone");

		press("KeyD", "d");
		expect(splitButtons()).toHaveLength(2);
		expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(2);
		expect(selected()).toBe(m1);

		press("KeyM", "M", { shiftKey: true });
		expect(screen.getByRole("button", { name: "Restore group" })).toBeInTheDocument();
		press("KeyM", "M", { shiftKey: true });
		expect(screen.queryByRole("button", { name: "Restore group" })).not.toBeInTheDocument();

		press("KeyW", "W", { shiftKey: true });
		expect(tabs()).toEqual(["Release checklist"]);
		press("KeyT", "T", { shiftKey: true });
		expect(tabs()).toHaveLength(2);
		expect(splitButtons()).toHaveLength(2);
		expect(selected()).toBe(m1);
	});

	it("goes to the agents waiting on you in turn, from any mode", async () => {
		onMac();
		const { ui } = renderApp();
		await row(/Release checklist/);
		act(() => ui.getState().setMode("devices"));

		press("KeyJ", "j");
		expect(ui.getState().mode).toBe("projects");
		const first = heading().textContent;
		press("KeyJ", "j");
		const second = heading().textContent;
		expect(second).not.toBe(first);
		expect([first, second].sort()).toEqual(["Release checklist", "Subtitle sync drift"]);
		press("KeyJ", "j");
		expect(heading().textContent).toBe(first);
	});

	it("asks the agent ⌘J opens to go to the request it waits on", async () => {
		onMac();
		const { ui } = renderApp();
		await row(/Release checklist/);
		const heard: [string, string | null][] = [];
		const unwatch = ["agent-release", "agent-sync-drift"].map((agent) =>
			watchRequests(agent, () => heard.push([agent, takeRequest(agent)?.attention ?? null]))
		);

		press("KeyJ", "j");
		unwatch.forEach((undo) => undo());

		const opened = ui.getState().selectedAgent;
		expect(heard).toEqual([
			[opened, opened === "agent-release" ? "att-release" : "att-sync-drift"],
		]);
	});

	it("stop the active pane's turn on Command and period with focus outside the panes", async () => {
		onMac();
		const source = mockSource({ now: () => NOW, tick: false });
		const interrupt = vi.spyOn(source, "interrupt");
		renderApp(source);
		fireEvent.click(await row(/Release checklist/));
		expect(screen.getByRole("button", { name: "Stop" })).toHaveAttribute(
			"aria-keyshortcuts",
			"Meta+."
		);
		act(() => {
			(document.activeElement as HTMLElement | null)?.blur();
		});

		press("Period", ".");
		await waitFor(() => expect(interrupt).toHaveBeenCalledWith("agent-release"));
	});

	it("leave keys the composer or a dialog took alone", async () => {
		onMac();
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		const event = new KeyboardEvent("keydown", {
			code: "Digit1",
			key: "1",
			metaKey: true,
			bubbles: true,
			cancelable: true,
		});
		event.preventDefault();
		act(() => {
			document.body.dispatchEvent(event);
		});
		expect(heading()).toHaveTextContent("Release checklist");
		// Control isn't the shortcut key on a Mac, since the composer edits with it.
		act(() => {
			fireEvent.keyDown(document.body, { code: "Digit1", key: "1", ctrlKey: true });
		});
		expect(heading()).toHaveTextContent("Release checklist");
	});

	it("show in the buttons that do the same", async () => {
		onMac();
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		const split = splitButtons()[0]!;
		expect(split).toHaveAttribute("title", "Split pane ⌘D");
		expect(split).toHaveAttribute("aria-keyshortcuts", "Meta+D");
		expect(screen.getByRole("button", { name: "Maximize group" })).toHaveAttribute(
			"title",
			"Maximize group ⇧⌘M"
		);
	});
});

describe("find in the agent view", () => {
	const findBox = () =>
		screen.getByRole<HTMLInputElement>("textbox", { name: "Find in conversation" });
	const count = () => screen.getByRole("search", { name: "Find in conversation" }).textContent;

	it("opens on Command F, steps with Enter and Shift Enter, and closes on Escape", async () => {
		onMac();
		const source = mockSource({ now: () => NOW, tick: false });
		const ask = vi.spyOn(source, "findInTimeline");
		renderApp(source);
		fireEvent.click(await row(/Release checklist/));
		// Focus goes back where it was when the bar closes.
		const button = within(heading().closest("section")!).getAllByRole("button")[0]!;
		act(() => button.focus());
		press("KeyF", "f");
		const input = findBox();
		expect(input).toHaveFocus();
		fireEvent.change(input, { target: { value: "the" } });
		expect(ask).not.toHaveBeenCalled();
		await waitFor(() => expect(ask).toHaveBeenCalledTimes(1));
		expect(ask.mock.calls[0]?.[1]).toBe("the");
		const { total } = await ask.mock.results[0]!.value;
		expect(total).toBeGreaterThan(2);
		await waitFor(() => expect(count()).toContain(`1 of ${total}`));

		fireEvent.keyDown(input, { key: "Enter" });
		await waitFor(() => expect(count()).toContain(`2 of ${total}`));
		fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
		fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
		await waitFor(() => expect(count()).toContain(`${total} of ${total}`));

		// Command F again picks the query, to type over it.
		act(() => input.setSelectionRange(3, 3));
		press("KeyF", "f");
		expect(input).toHaveFocus();
		expect([input.selectionStart, input.selectionEnd]).toEqual([0, 3]);

		fireEvent.keyDown(input, { key: "Escape" });
		expect(
			screen.queryByRole("search", { name: "Find in conversation" })
		).not.toBeInTheDocument();
		expect(button).toHaveFocus();
	});

	it("shows no matches for an agent maxd can't read yet", async () => {
		onMac();
		const source = mockSource({ now: () => NOW, tick: false });
		vi.spyOn(source, "findInTimeline").mockRejectedValue(
			new Error("MaxHQ can't read that agent's timeline.")
		);
		renderApp(source);
		fireEvent.click(await row(/Release checklist/));
		press("KeyF", "f");
		fireEvent.change(findBox(), { target: { value: "anything" } });
		await waitFor(() => expect(count()).toContain("No matches"));
		expect(screen.getByRole("button", { name: "Next match" })).toBeDisabled();
	});
});

describe("tabs that wait on you", () => {
	it("count on the group header while they're hidden", async () => {
		renderApp();
		fireEvent.click(await row(/Release checklist/));
		expect(screen.queryByRole("button", { name: /hidden tab/ })).not.toBeInTheDocument();

		fireEvent.click(await row(/M1 observe milestone/));
		const count = screen.getByRole("button", { name: "1 hidden tab needs you" });
		expect(count).toHaveTextContent("1");

		fireEvent.click(count);
		expect(heading()).toHaveTextContent("Release checklist");
		expect(screen.queryByRole("button", { name: /hidden tab/ })).not.toBeInTheDocument();
	});
});

describe("the tab strip", () => {
	it("keeps the shown tab in sight once its title draws", async () => {
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		const shown = vi.mocked(revealTab).mock.lastCall![0];
		expect(shown.closest(".dv-tabs-container > .dv-tab")).toBe(
			screen.getByRole("tab", { name: "Release checklist" })
		);

		fireEvent.click(await row(/M1 observe milestone/));
		expect(vi.mocked(revealTab).mock.lastCall![0].closest(".dv-tab")).toBe(
			screen.getByRole("tab", { name: "M1 observe milestone" })
		);
	});
});

describe("the tab menu", () => {
	it("lists the shortcuts and reopens the last closed pane", async () => {
		onMac();
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		fireEvent.click(screen.getByRole("button", { name: "Close M1 observe milestone" }));
		expect(tabs()).toEqual(["Release checklist"]);

		fireEvent.contextMenu(screen.getByRole("tab", { name: "Release checklist" }));
		const menu = await screen.findByRole("menu");
		const reopen = within(menu).getByRole("menuitem", { name: /Reopen closed pane/ });
		expect(reopen).toHaveTextContent("⇧⌘T");
		expect(within(menu).getByRole("menuitem", { name: /Split right/ })).toHaveAttribute(
			"aria-disabled",
			"true"
		);

		fireEvent.click(reopen);
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(screen.queryByRole("menu")).not.toBeInTheDocument();
	});
});

describe("dragging an agent from the sidebar", () => {
	// Puts an element at a box on screen, since jsdom lays nothing out.
	const place = (element: Element, left: number, top: number, w: number, h: number) =>
		vi.spyOn(element, "getBoundingClientRect").mockReturnValue({
			left,
			top,
			x: left,
			y: top,
			width: w,
			height: h,
			right: left + w,
			bottom: top + h,
			toJSON: () => ({}),
		});
	// The grid right of a 200 pixel sidebar, with one group in it.
	const layOut = () => {
		place(document.querySelector(".dv-dockview")!, 200, 0, 800, 600);
		place(document.querySelector(".dv-groupview")!, 204, 4, 792, 592);
	};
	const spot = () => document.querySelector<HTMLElement>("[data-drop-spot]");
	const drag = (from: Element, to: { clientX: number; clientY: number }) => {
		fireEvent.pointerDown(from, { button: 0, pointerType: "mouse", clientX: 50, clientY: 80 });
		fireEvent.pointerMove(window, { pointerType: "mouse", clientX: 52, clientY: 81 });
		expect(spot()).toBeNull();
		fireEvent.pointerMove(window, { pointerType: "mouse", ...to });
	};
	const settle = () => act(() => new Promise((done) => setTimeout(done, 0)));

	it("lights up the spot and opens the pane there", async () => {
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		layOut();
		const release = await row(/Release checklist/);

		drag(release, { clientX: 950, clientY: 300 });
		expect(spot()).toHaveStyle({ left: "400px", top: "4px", width: "396px", height: "592px" });
		fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 950, clientY: 300 });
		fireEvent.click(release);
		expect(spot()).toBeNull();
		expect(splitButtons()).toHaveLength(2);
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
		expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(2);
	});

	it("drops nothing outside the panes or after Escape", async () => {
		renderApp();
		fireEvent.click(await row(/M1 observe milestone/));
		layOut();
		const release = await row(/Release checklist/);

		drag(release, { clientX: 120, clientY: 300 });
		expect(spot()).toBeNull();
		fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 120, clientY: 300 });
		// The click that ends a drag doesn't open the agent either.
		fireEvent.click(release);
		expect(tabs()).toEqual(["M1 observe milestone"]);
		await settle();

		drag(release, { clientX: 600, clientY: 300 });
		expect(spot()).not.toBeNull();
		fireEvent.keyDown(window, { key: "Escape" });
		expect(spot()).toBeNull();
		fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 600, clientY: 300 });
		fireEvent.click(release);
		expect(tabs()).toEqual(["M1 observe milestone"]);
		await settle();

		// A plain click still opens it as a tab.
		fireEvent.click(release);
		expect(tabs()).toEqual(["M1 observe milestone", "Release checklist"]);
	});

	it("puts an agent dropped on another agent's composer into its message", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		await source.startAgent({
			vendor: "claude",
			account: CLAUDE,
			place: { type: "workspace", workspace: "ws-mac-maxhq" },
			model: null,
			permission: null,
			title: "Write the notes",
			first: null,
			branch: null,
		});
		renderApp(source);
		fireEvent.click(await row(/Write the notes/));
		const textbox = await screen.findByRole("textbox", { name: /^Message / });
		const release = await row(/Release checklist/);
		const elementFromPoint = document.elementFromPoint;
		document.elementFromPoint = () => textbox;
		try {
			drag(release, { clientX: 600, clientY: 300 });
			expect(screen.getByText("Drop to add Release checklist")).toBeInTheDocument();
			expect(spot()).toBeNull();
			fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 600, clientY: 300 });
			fireEvent.click(release);
		} finally {
			document.elementFromPoint = elementFromPoint;
		}

		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Release checklist/)])
		);
		expect(screen.queryByText("Drop to add Release checklist")).toBeNull();
		expect(tabs()).toEqual(["Write the notes"]);
	});

	it("carries every picked agent onto a composer", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		await source.startAgent({
			vendor: "claude",
			account: CLAUDE,
			place: { type: "workspace", workspace: "ws-mac-maxhq" },
			model: null,
			permission: null,
			title: "Write the notes",
			first: null,
			branch: null,
		});
		renderApp(source);
		fireEvent.click(await row(/Write the notes/));
		const textbox = await screen.findByRole("textbox", { name: /^Message / });
		// The rows in the MaxHQ group, past the "Needs you" box.
		const maxhq = within(screen.getByRole("region", { name: "MaxHQ" }));
		const release = maxhq.getByRole("option", { name: /Release checklist/ });
		fireEvent.click(release, { ctrlKey: true });
		fireEvent.click(maxhq.getByRole("option", { name: /Usage meter popover/ }), {
			ctrlKey: true,
		});
		const elementFromPoint = document.elementFromPoint;
		document.elementFromPoint = () => textbox;
		try {
			drag(release, { clientX: 600, clientY: 300 });
			expect(screen.getByText("Drop to add 2 agents")).toBeInTheDocument();
			expect(screen.getByText("+1")).toBeInTheDocument();
			fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 600, clientY: 300 });
			fireEvent.click(release);
		} finally {
			document.elementFromPoint = elementFromPoint;
		}

		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([
				expect.stringMatching(/^Agent Release checklist/),
				expect.stringMatching(/^Agent Usage meter popover/),
			])
		);
	});
});
