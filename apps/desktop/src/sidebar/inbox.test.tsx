// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { Agent } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import { DEFAULT_INBOX } from "@maxhq/shared/view/inbox";

import { readShelves, SHELVES_KEY } from "./open-shelves";
import { PREFS_KEY } from "./prefs";

const MIN = 60_000;

// The sample source with one agent changed.
const changed = (id: string | readonly string[], over: Partial<Agent>): DataSource => {
	const ids = new Set([id].flat());
	const source = mockSource({ now: () => NOW, tick: false });
	const snapshot = source.snapshot;
	const patched: DataSource["snapshot"] = async () => {
		const state = await snapshot();
		return {
			...state,
			entities: state.entities.map((e) =>
				e.entity === "agent" && ids.has(e.id) ? { ...e, ...over } : e
			),
		};
	};
	return Object.assign(source, { snapshot: patched });
};

const renderApp = (
	scenario?: string,
	source: DataSource = mockSource({ now: () => NOW, tick: false, scenario })
) => {
	const ui = createUiStore();
	const setInbox = vi.spyOn(source, "setInbox");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui, setInbox };
};

const nav = () => screen.getByRole("navigation", { name: "Agents" });
// An agent's row in the sidebar's list, not the Needs you box, which has no menu.
const row = (name: RegExp) => within(nav()).findByRole("option", { name });
const region = (name: string) => within(nav()).getByRole("region", { name });
const shelf = (name: string) =>
	within(region(name)).getByRole("button", { name: new RegExp(`^${name}`) });
const titles = (name: string) =>
	within(region(name))
		.queryAllByRole("option")
		.map((b) => b.getAttribute("data-pin-row") ?? b.textContent);
const notice = () => within(nav()).getByRole("status");
const alert = () => within(nav()).findByRole("alert");

// A key pressed with Control, which is Command's place off a Mac.
const press = (code: string, key: string, shiftKey = false, on: Element = document.body) =>
	act(() => {
		fireEvent.keyDown(on, { code, key, ctrlKey: true, shiftKey });
	});

const menuFor = async (name: RegExp) => {
	fireEvent.contextMenu(await row(name));
	return screen.findByRole("menu");
};

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});

describe("the inbox sections", () => {
	it("pins agents above the projects and keeps the rest on closed shelves", async () => {
		localStorage.setItem(PREFS_KEY, JSON.stringify({ version: 1, workingShelf: true }));
		renderApp();
		await row(/Sidebar sort order/);
		expect(titles("Pinned")).toEqual(["agent-sidebar-sort"]);
		expect(
			within(region("MaxHQ")).queryByRole("option", { name: /Sidebar sort order/ })
		).toBeNull();

		for (const [name, count] of [
			["Working", /\d+$/],
			["Snoozed", /1$/],
			["Settled", /2$/],
		] as const) {
			expect(shelf(name)).toHaveAttribute("aria-expanded", "false");
			expect(shelf(name)).toHaveTextContent(count);
		}
		expect(within(nav()).queryByRole("option", { name: /M0 skeleton review/ })).toBeNull();

		fireEvent.click(shelf("Settled"));
		expect(shelf("Settled")).toHaveAttribute("aria-expanded", "true");
		const settled = region("Settled");
		expect(
			within(settled).getByRole("option", { name: /M0 skeleton review/ })
		).toBeInTheDocument();
		expect(
			within(settled).getByRole("option", { name: /License allowlist/ })
		).toBeInTheDocument();

		fireEvent.click(shelf("Snoozed"));
		const snoozed = within(region("Snoozed")).getByRole("option", {
			name: /Subtitle font fallback/,
		});
		expect(snoozed).toHaveTextContent(/2h|\d:\d\d/);
		expect(
			within(region("Comet")).queryByRole("option", { name: /Subtitle font fallback/ })
		).toBeNull();
	});

	it("opens the shelves you left open", async () => {
		localStorage.setItem(SHELVES_KEY, JSON.stringify({ version: 1, open: ["settled"] }));
		render(
			<FixedNow value={NOW}>
				<App source={mockSource({ now: () => NOW, tick: false })} />
			</FixedNow>
		);
		await row(/Sidebar sort order/);
		expect(shelf("Settled")).toHaveAttribute("aria-expanded", "true");
		expect(shelf("Snoozed")).toHaveAttribute("aria-expanded", "false");

		fireEvent.click(shelf("Settled"));
		fireEvent.click(shelf("Snoozed"));
		expect(readShelves(localStorage.getItem(SHELVES_KEY))).toEqual(new Set(["snoozed"]));
	});

	it("keeps the agent you're looking at on a closed shelf", async () => {
		renderApp();
		await row(/Sidebar sort order/);
		fireEvent.click(shelf("Settled"));
		fireEvent.click(
			within(region("Settled")).getByRole("option", { name: /M0 skeleton review/ })
		);
		fireEvent.click(shelf("Settled"));

		const settled = region("Settled");
		expect(within(settled).getByRole("option", { name: /M0 skeleton review/ })).toHaveAttribute(
			"aria-current",
			"true"
		);
		expect(within(settled).queryByRole("option", { name: /License allowlist/ })).toBeNull();
	});

	it("marks an agent whose change waits for its device", async () => {
		renderApp("peer_offline");
		const menu = await menuFor(/GTFS feed refresh/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Pin/ }));

		const pinned = await within(region("Pinned")).findByRole("option", {
			name: /GTFS feed refresh/,
		});
		expect(pinned).toHaveAccessibleName(/waiting for devbox/);
		expect(within(pinned).getByTitle("Waiting for devbox")).toBeInTheDocument();
		expect(
			within(nav()).getByRole("option", { name: /Sidebar sort order/ })
		).not.toHaveAccessibleName(/waiting/);
	});

	it("marks a woken agent until you open it", async () => {
		const { setInbox } = renderApp();
		const bindings = await row(/Bindings check/);
		expect(bindings).toHaveTextContent("Woke");

		fireEvent.click(bindings);
		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], { change: "dismiss_woke" });
		await vi.waitFor(async () =>
			expect(await row(/Bindings check/)).not.toHaveTextContent("Woke")
		);
	});

	it("marks a quiet agent asleep once its idle CLI stopped", async () => {
		renderApp(undefined, changed("agent-sidebar-sort", { asleep: true }));
		expect(await row(/Sidebar sort order/)).toHaveTextContent(/Asleep$/);
	});

	it("shows unread before asleep", async () => {
		const unread = { asleep: true, activity: { state: "done_unread" } } as const;
		renderApp(undefined, changed("agent-sidebar-sort", unread));
		expect(await row(/Sidebar sort order/)).not.toHaveTextContent("Asleep");
	});
});

describe("picking agents", () => {
	it("adds with Control, picks a range with Shift and clears with Escape", async () => {
		const { setInbox } = renderApp();
		await row(/Bindings check/);
		const rows = within(region("MaxHQ")).getAllByRole("option");
		// The two agents the release checklist started sit under it.
		expect(rows.map((b) => b.textContent)).toEqual([
			expect.stringMatching(/^Weekly report page/),
			expect.stringMatching(/^Release checklist/),
			expect.stringMatching(/^Write the release notes/),
			expect.stringMatching(/^Build a signed bundle/),
			expect.stringMatching(/^Tidy old claude\.ai pages/),
			expect.stringMatching(/^Document the sync API/),
			expect.stringMatching(/^Show the release plan/),
			expect.stringMatching(/^M1 observe milestone/),
			expect.stringMatching(/^Usage meter popover/),
			expect.stringMatching(/^Bindings check/),
			// The launch agent's six fold into one row, all but the one
			// that needs you.
			expect.stringMatching(/^Prepare the launch/),
			expect.stringMatching(/^Tag the release/),
		]);

		fireEvent.click(rows[0]!, { ctrlKey: true });
		expect(screen.getByText("1 agent selected")).toBeInTheDocument();
		expect(rows[0]).toHaveAttribute("aria-selected", "true");
		fireEvent.keyDown(document.body, { key: "Escape" });
		expect(screen.queryByText(/selected$/)).toBeNull();

		fireEvent.click(rows[0]!, { ctrlKey: true });
		fireEvent.click(rows[4]!, { shiftKey: true });
		expect(screen.getByText("5 agents selected")).toBeInTheDocument();
		// None of the clicks opened an agent.
		expect(screen.queryAllByRole("tab")).toEqual([]);

		fireEvent.click(rows[1]!, { ctrlKey: true });
		expect(screen.getByText("4 agents selected")).toBeInTheDocument();
		expect(rows[1]).toHaveAttribute("aria-selected", "false");
		fireEvent.click(screen.getByRole("button", { name: "Clear the selection" }));
		expect(screen.queryByText(/selected$/)).toBeNull();
		expect(setInbox).not.toHaveBeenCalled();
	});

	it("settles the picked agents in one call from the Actions menu", async () => {
		const { setInbox } = renderApp();
		fireEvent.click(await row(/Bindings check/), { ctrlKey: true });
		fireEvent.click(await row(/Sidebar sort order/), { ctrlKey: true });

		fireEvent.click(screen.getByRole("button", { name: "Actions" }));
		const menu = await screen.findByRole("menu");
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Settle \(2\)/ }));

		expect(setInbox).toHaveBeenCalledTimes(1);
		expect(setInbox).toHaveBeenCalledWith(["agent-bindings", "agent-sidebar-sort"], {
			change: "settle",
		});
		expect(await within(notice()).findByText("Settled 2 agents")).toBeInTheDocument();
		expect(screen.queryByText(/selected$/)).toBeNull();
		expect(region("Settled")).toHaveTextContent(/4$/);
	});

	it("opens the selection's menu from a picked row", async () => {
		renderApp();
		fireEvent.click(await row(/Bindings check/), { ctrlKey: true });
		fireEvent.click(await row(/Sidebar sort order/), { ctrlKey: true });
		const menu = await menuFor(/Bindings check/);
		expect(within(menu).getByRole("menuitem", { name: /^Settle \(2\)/ })).toBeInTheDocument();
		// One is pinned and one isn't, so it pins the other.
		expect(within(menu).getByRole("menuitem", { name: /^Pin \(1\)/ })).toBeInTheDocument();
	});
});

describe("the row menu", () => {
	it("pins an agent after the others", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		expect(within(menu).getByRole("menuitem", { name: /^Settle/ })).toBeInTheDocument();
		expect(
			within(menu).getByRole("menuitemcheckbox", { name: "Auto-settle when idle" })
		).toHaveAttribute("aria-checked", "true");
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Pin/ }));

		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], {
			change: "pin",
			key: expect.any(String),
		});
		expect(await within(notice()).findByText("Pinned Bindings check")).toBeInTheDocument();
		expect(titles("Pinned")).toEqual(["agent-sidebar-sort", "agent-bindings"]);
	});

	it("turns auto-settle off for an agent", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(
			within(menu).getByRole("menuitemcheckbox", { name: "Auto-settle when idle" })
		);
		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], {
			change: "auto_settle",
			on: false,
		});
		expect(
			await within(notice()).findByText("Auto-settle off for Bindings check")
		).toBeInTheDocument();
	});

	it("says when an automation keeps an agent active", async () => {
		renderApp();
		const bindings = await menuFor(/Bindings check/);
		expect(within(bindings).queryByText("Kept active by an automation")).toBeNull();
		fireEvent.keyDown(bindings, { key: "Escape" });

		const menu = await menuFor(/Release checklist/);
		const item = within(menu).getByRole("menuitemcheckbox", {
			name: /^Auto-settle when idle/,
		});
		expect(await within(item).findByText("Kept active by an automation")).toBeInTheDocument();
	});

	it("turns continuing after the limit on for an agent MaxHQ holds", async () => {
		const { setInbox } = renderApp();
		const bindings = await menuFor(/Bindings check/);
		expect(
			within(bindings).queryByRole("menuitemcheckbox", { name: "Continue after the limit" })
		).toBeNull();
		fireEvent.keyDown(bindings, { key: "Escape" });

		const menu = await menuFor(/Release checklist/);
		const item = within(menu).getByRole("menuitemcheckbox", {
			name: "Continue after the limit",
		});
		expect(item).toHaveAttribute("aria-checked", "false");
		fireEvent.click(item);
		expect(setInbox).toHaveBeenCalledWith(["agent-release"], {
			change: "continue_at_limit",
			on: true,
		});
		expect(
			await within(notice()).findByText("Continue after the limit on for Release checklist")
		).toBeInTheDocument();
	});

	it("snoozes for a preset's time", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Snooze" }));
		const presets = (await screen.findAllByRole("menu")).at(-1)!;
		expect(
			within(presets).queryByRole("menuitem", { name: /Until the limit resets/ })
		).toBeNull();
		fireEvent.click(within(presets).getByRole("menuitem", { name: /^In 1 hour/ }));

		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], {
			change: "snooze",
			until: NOW + 60 * MIN,
		});
		expect(
			await within(notice()).findByText(/^Snoozed Bindings check until/)
		).toBeInTheDocument();
		expect(shelf("Snoozed")).toHaveTextContent(/2$/);
	});

	it("snoozes for a custom time", async () => {
		vi.useFakeTimers({ toFake: ["Date"] });
		vi.setSystemTime(NOW);
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Snooze" }));
		const presets = (await screen.findAllByRole("menu")).at(-1)!;
		fireEvent.click(within(presets).getByRole("menuitem", { name: "Custom…" }));

		const dialog = await screen.findByRole("dialog", { name: "Custom snooze" });
		fireEvent.click(within(dialog).getByRole("button", { name: "Duration" }));
		fireEvent.change(within(dialog).getByLabelText("Snooze for"), { target: { value: "30" } });
		fireEvent.change(within(dialog).getByLabelText("Unit"), { target: { value: "minutes" } });
		fireEvent.click(within(dialog).getByRole("button", { name: "Snooze" }));

		await vi.waitFor(() =>
			expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], {
				change: "snooze",
				until: NOW + 30 * MIN,
			})
		);
		expect(screen.queryByRole("dialog")).toBeNull();
	});

	it("offers the limit's reset at the limit and shows why it can't snooze", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/HDR tone mapping/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Snooze" }));
		const presets = (await screen.findAllByRole("menu")).at(-1)!;
		fireEvent.click(within(presets).getByRole("menuitem", { name: /^Until the limit resets/ }));

		expect(setInbox).toHaveBeenCalledWith(["agent-hdr"], {
			change: "snooze",
			until: expect.any(Number),
		});
		expect(await alert()).toHaveTextContent(
			"Couldn't snooze HDR tone mapping. A message is waiting for the agent. Take it back or let it go before you snooze it."
		);
	});

	it("shows a refusal until you dismiss it", async () => {
		renderApp();
		const menu = await menuFor(/Release checklist/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Settle/ }));

		const failure = await alert();
		expect(failure).toHaveTextContent(
			"Couldn't settle Release checklist. The agent needs your input. Answer it before you settle it."
		);
		expect(within(notice()).queryByText(/Settled/)).toBeNull();
		fireEvent.click(within(failure.parentElement!).getByRole("button", { name: "Dismiss" }));
		expect(within(nav()).queryByRole("alert")).toBeNull();
	});

	it("says when a device's maxd is too old", async () => {
		const { setInbox } = renderApp();
		setInbox.mockResolvedValue([
			{
				status: "ok",
				data: {
					status: "refused",
					reason: "Mac's maxd is too old to settle, snooze or pin its agents. Update it first",
				},
			},
		]);
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Settle/ }));
		expect(await alert()).toHaveTextContent(
			"Couldn't settle Bindings check. Mac's maxd is too old to settle, snooze or pin its agents. Update it first."
		);
	});

	it("copies one agent's folder path, branch and session ID, and opens its folder", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			const source = changed("agent-bindings", {
				branch: "fix/bindings",
				vendor_session_id: "session-1",
			});
			const openInEditor = vi.spyOn(source, "openInEditor");
			renderApp(undefined, source);
			const open = await within(await menuFor(/Bindings check/)).findByRole("menuitem", {
				name: /^Open in Cursor/,
			});
			fireEvent.click(open);
			await waitFor(() =>
				expect(openInEditor).toHaveBeenCalledWith(
					"cursor",
					expect.any(String),
					undefined,
					undefined
				)
			);
			await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

			const menu = await menuFor(/Bindings check/);
			fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy" }));
			const copies = (await screen.findAllByRole("menu")).at(-1)!;
			expect(
				within(copies)
					.getAllByRole("menuitem")
					.map((item) => item.textContent)
			).toEqual([
				expect.stringMatching(/^Folder path\//),
				"Branchfix/bindings",
				"Session IDsession-1",
			]);
			const branch = within(copies).getByRole("menuitem", { name: /^Branch/ });
			fireEvent.click(branch);
			expect(writeText).toHaveBeenCalledWith("fix/bindings");
			await waitFor(() => expect(branch).toHaveTextContent("Copied"));
			fireEvent.click(within(copies).getByRole("menuitem", { name: /^Session ID/ }));
			expect(writeText).toHaveBeenLastCalledWith("session-1");
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("asks about the SSH setup once the menu closes, then opens the editor", async () => {
		const { source } = renderApp();
		const openInEditor = vi.spyOn(source, "openInEditor");
		const menu = await menuFor(/Stop times cache/);
		const open = await within(menu).findByRole("menuitem", { name: /^Open in Cursor/ });
		expect(open).toHaveTextContent("after one line of setup");
		fireEvent.click(open);
		const dialog = await screen.findByRole("dialog");
		expect(dialog).toHaveTextContent("Let editors reach this container");
		expect(screen.queryByRole("menu")).toBeNull();
		fireEvent.click(within(dialog).getByRole("button", { name: "Add the line and open" }));
		await waitFor(() =>
			expect(openInEditor).toHaveBeenCalledWith("cursor", "ws-alpha", undefined, undefined)
		);
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
	});

	it("opens with Shift+F10 on a focused row and from the ⋯, and gives the row its focus back", async () => {
		const { ui } = renderApp();
		const target = await row(/Bindings check/);
		// The ⋯ is hidden from assistive tech, which reaches the menu with Shift+F10.
		const more = () => target.parentElement!.querySelector<HTMLElement>('[title="More"]')!;
		// The ⋯ is the pointer's way in, out of the tab order.
		expect(more()).toHaveAttribute("tabindex", "-1");
		expect(more()).toHaveClass("opacity-0", "group-hover/agent-row:opacity-100");

		target.focus();
		fireEvent.keyDown(target, { key: "F10", shiftKey: true });
		const menu = await screen.findByRole("menu");
		expect(within(menu).getByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
		await waitFor(() => expect(target).toHaveFocus());

		fireEvent.click(more());
		expect(await screen.findByRole("menu")).toBeInTheDocument();
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

		// The selected row keeps its ⋯ showing, with room for it.
		act(() => ui.getState().selectAgent("agent-bindings", false));
		const selected = await row(/Bindings check/);
		expect(selected).toHaveClass("pr-7");
		expect(selected.parentElement!.querySelector('[title="More"]')).toHaveClass("opacity-100");
	});
});

describe("undo", () => {
	it("puts the agents back as they were", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Settle/ }));
		await within(notice()).findByText("Settled Bindings check");
		expect(
			within(region("MaxHQ")).queryByRole("option", { name: /Bindings check/ })
		).toBeNull();

		fireEvent.click(within(notice()).getByRole("button", { name: "Undo" }));
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], {
			change: "restore",
			inbox: { ...DEFAULT_INBOX, woke_at: NOW - 10 * MIN },
		});
		expect(within(notice()).queryByText("Settled Bindings check")).toBeNull();
		expect(
			await within(region("MaxHQ")).findByRole("option", { name: /Bindings check/ })
		).toHaveTextContent("Woke");
	});

	it("offers to unsettle what auto-settle settled while you watched", async () => {
		const { source, setInbox } = renderApp();
		await row(/Bindings check/);
		await act(() =>
			source.setInbox(["agent-bindings"], {
				change: "restore",
				inbox: { ...DEFAULT_INBOX, settled: { at: NOW, by: "auto" } },
			})
		);
		expect(within(notice()).getByText("Auto-settled 1 idle agent")).toBeInTheDocument();

		fireEvent.click(within(notice()).getByRole("button", { name: "Undo" }));
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], { change: "unsettle" });
		expect(
			await within(region("MaxHQ")).findByRole("option", { name: /Bindings check/ })
		).toBeInTheDocument();
	});

	it("lets the notice go after five seconds", async () => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
		renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Pin/ }));
		await within(notice()).findByText("Pinned Bindings check");
		act(() => {
			vi.advanceTimersByTime(5000);
		});
		expect(notice()).toBeEmptyDOMElement();
	});
});

describe("the inbox shortcuts", () => {
	it("settle, pin and undo for the agent you're looking at", async () => {
		const { setInbox } = renderApp();
		fireEvent.click(await row(/Bindings check/));
		setInbox.mockClear();

		press("KeyS", "S", true);
		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], { change: "settle" });
		await within(notice()).findByText("Settled Bindings check");

		// Undo is the field's own while you type in one.
		const field = document.createElement("textarea");
		document.body.append(field);
		field.focus();
		press("KeyZ", "z", false, field);
		expect(setInbox).toHaveBeenCalledTimes(1);
		field.remove();

		press("KeyZ", "z");
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], {
			change: "restore",
			inbox: DEFAULT_INBOX,
		});
		// Settling went on to an agent waiting on you, so come back first.
		fireEvent.click(
			await within(region("MaxHQ")).findByRole("option", { name: /Bindings check/ })
		);

		press("KeyP", "P", true);
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], {
			change: "pin",
			key: expect.any(String),
		});
		await within(notice()).findByText("Pinned Bindings check");
		press("KeyP", "P", true);
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], { change: "unpin" });
	});

	it("go on from the agent you settle to the next one waiting on you", async () => {
		const { setInbox, ui } = renderApp();
		fireEvent.click(await row(/Bindings check/));

		press("KeyS", "S", true);
		expect(setInbox).toHaveBeenLastCalledWith(["agent-bindings"], { change: "settle" });
		const waiting = [
			"agent-release",
			"agent-sync-drift",
			"agent-launch-tag",
			"agent-weekly-report",
			"agent-tidy-pages",
		];
		const first = ui.getState().selectedAgent;
		expect(waiting).toContain(first);

		// The one just settled is passed over, so another opens.
		await within(notice()).findByText("Settled Bindings check");
		press("KeyS", "S", true);
		expect(setInbox).toHaveBeenLastCalledWith([first], { change: "settle" });
		const second = ui.getState().selectedAgent;
		expect(waiting).toContain(second);
		expect(second).not.toBe(first);
	});

	it("stay on the agent you settle when no other waits", async () => {
		const { setInbox, ui } = renderApp(
			undefined,
			changed(
				["agent-sync-drift", "agent-launch-tag", "agent-weekly-report", "agent-tidy-pages"],
				{ activity: { state: "idle" } }
			)
		);
		fireEvent.click(await row(/Release checklist/));

		press("KeyS", "S", true);
		expect(setInbox).toHaveBeenLastCalledWith(["agent-release"], { change: "settle" });
		expect(ui.getState().selectedAgent).toBe("agent-release");
	});

	it("act on the picked agents before the open one, and stay", async () => {
		const { setInbox, ui } = renderApp();
		fireEvent.click(await row(/Bindings check/));
		fireEvent.click(await row(/Sidebar sort order/), { ctrlKey: true });
		setInbox.mockClear();

		press("KeyS", "S", true);
		expect(setInbox).toHaveBeenCalledWith(["agent-sidebar-sort"], { change: "settle" });
		expect(screen.queryByText(/selected$/)).toBeNull();
		expect(ui.getState().selectedAgent).toBe("agent-bindings");
	});

	it("leave the key alone when there's nothing to undo", async () => {
		const { setInbox } = renderApp();
		await row(/Bindings check/);
		const key = new KeyboardEvent("keydown", {
			code: "KeyZ",
			key: "z",
			ctrlKey: true,
			bubbles: true,
			cancelable: true,
		});
		act(() => {
			document.body.dispatchEvent(key);
		});
		expect(key.defaultPrevented).toBe(false);
		expect(setInbox).not.toHaveBeenCalled();
	});
});

describe("dragging a pin", () => {
	const place = (element: Element, top: number) =>
		vi.spyOn(element, "getBoundingClientRect").mockReturnValue({
			left: 0,
			top,
			x: 0,
			y: top,
			width: 200,
			height: 28,
			right: 200,
			bottom: top + 28,
			toJSON: () => ({}),
		});

	it("moves it among the pins", async () => {
		const { setInbox } = renderApp();
		const menu = await menuFor(/Bindings check/);
		fireEvent.click(within(menu).getByRole("menuitem", { name: /^Pin/ }));
		await within(notice()).findByText("Pinned Bindings check");
		const [sort, bindings] = within(region("Pinned")).getAllByRole("option");
		place(sort!, 100);
		place(bindings!, 128);
		setInbox.mockClear();

		fireEvent.pointerDown(bindings!, {
			button: 0,
			pointerType: "mouse",
			clientX: 50,
			clientY: 140,
		});
		fireEvent.pointerMove(window, { pointerType: "mouse", clientX: 50, clientY: 120 });
		fireEvent.pointerMove(window, { pointerType: "mouse", clientX: 50, clientY: 104 });
		fireEvent.pointerUp(window, { pointerType: "mouse", clientX: 50, clientY: 104 });
		fireEvent.click(bindings!);

		expect(setInbox).toHaveBeenCalledTimes(1);
		expect(setInbox).toHaveBeenCalledWith(["agent-bindings"], {
			change: "pin",
			key: expect.any(String),
		});
		expect(await within(notice()).findByText("Moved Bindings check")).toBeInTheDocument();
		expect(titles("Pinned")).toEqual(["agent-bindings", "agent-sidebar-sort"]);
		expect(screen.queryAllByRole("tab")).toEqual([]);
	});
});
