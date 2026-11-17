// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { quoteInbox } from "@/composer/quote-inbox";
import { mockSource } from "@/data/mock-source";
import type { FakeView } from "@/test/fake-term-view";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// jsdom can't draw a terminal, so each one gets a view that keeps what it printed.
const views = vi.hoisted((): FakeView[] => []);
vi.mock("@/terminal/term-view-xterm", async () => {
	const { fakeView } = await import("@/test/fake-term-view");
	return {
		FONT_SIZE: 12,
		LINE_HEIGHT: 1.25,
		monoFamily: () => "monospace",
		keyForApp: () => false,
		xtermView: () => {
			const view = fakeView();
			views.push(view);
			return view;
		},
	};
});

const AGENT = "agent-release";
const FOLDER = "/Users/me/Code/Github/Multi/MaxAIManager";

const renderApp = () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const closeTerminal = vi.spyOn(source, "closeTerminal");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return { closeTerminal };
};

const toggle = () =>
	act(() => {
		fireEvent.keyDown(document.body, { code: "Backquote", key: "`", ctrlKey: true });
	});

const tablist = () => screen.findByRole("tablist", { name: "Terminals" });
const tabs = async () => within(await tablist()).getAllByRole("tab");
// The agent's pane is a tab panel too, so only look in the drawer.
const panels = () =>
	within(screen.getByRole("region", { name: "Terminal" })).getAllByRole("tabpanel");
/** The view showing in a drawer panel. */
const viewIn = (panel: HTMLElement) => {
	const view = views.find((one) => panel.contains(one.element));
	if (!view) throw new Error("no terminal in the panel");
	return view;
};
const type = (view: FakeView, text: string) => act(() => view.type(text));

const nav = () => screen.getByRole("navigation", { name: "Agents" });
const row = () => within(nav()).findByRole("option", { name: /Release checklist/ });

/** Opens the agent and its drawer, waits for its first shell, and runs `command` in it. */
const openAndRun = async (command?: string) => {
	fireEvent.click(await row());
	await screen.findAllByRole("region", { name: "Release checklist" });
	toggle();
	await waitFor(async () => expect(await tabs()).toHaveLength(1));
	const view = viewIn(panels()[0]!);
	await waitFor(() => expect(view.written.join("")).toContain("%"));
	if (command) await type(view, `${command}\r`);
	return view;
};

afterEach(() => {
	vi.restoreAllMocks();
	views.length = 0;
	localStorage.clear();
});

describe("the terminal drawer", () => {
	it("opens on ⌃` with a shell in the agent's folder, names where it runs, and hides again", async () => {
		renderApp();
		const view = await openAndRun();
		expect(screen.getByText(`${FOLDER} · Mac`)).toBeInTheDocument();
		const [shell] = await tabs();
		expect(shell).toHaveTextContent("zsh");
		expect(within(shell!).queryByRole("img", { name: "Running" })).toBeNull();

		fireEvent.click(screen.getByRole("button", { name: "Hide" }));
		expect(screen.queryByRole("tablist", { name: "Terminals" })).toBeNull();
		// It comes back on the same shell, still running.
		toggle();
		expect(await tabs()).toHaveLength(1);
		expect(viewIn(panels()[0]!)).toBe(view);
	});

	it("marks a tab running a command, opens another, and shows two side by side", async () => {
		renderApp();
		await openAndRun("pnpm dev");
		const [dev] = await tabs();
		await waitFor(() => expect(dev).toHaveTextContent("pnpm dev"));
		expect(within(dev!).getByRole("img", { name: "Running" })).toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
		await waitFor(async () => expect(await tabs()).toHaveLength(2));
		expect((await tabs())[1]).toHaveAttribute("aria-selected", "true");
		expect(panels()).toHaveLength(1);

		const split = screen.getByRole("button", { name: "Split" });
		fireEvent.click(split);
		expect(split).toHaveAttribute("aria-pressed", "true");
		expect(panels()).toHaveLength(2);
		fireEvent.click(split);
		expect(panels()).toHaveLength(1);
	});

	it("asks before closing a tab that runs a command, and stops it once you say so", async () => {
		const { closeTerminal } = renderApp();
		await openAndRun("pnpm dev");
		const close = await screen.findByRole("button", { name: "Close pnpm dev" });

		fireEvent.click(close);
		let dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent("Stop pnpm dev and close the tab?");
		expect(dialog).toHaveTextContent("It's still running, and closing the tab stops it.");
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(closeTerminal).not.toHaveBeenCalled();

		fireEvent.click(close);
		dialog = await screen.findByRole("alertdialog");
		fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
		// The drawer hides with its last tab.
		await waitFor(() =>
			expect(screen.queryByRole("tablist", { name: "Terminals" })).toBeNull()
		);
		expect(closeTerminal).toHaveBeenCalledWith(expect.any(String), true);
	});

	it("says what you type at a password prompt goes only to the terminal, and never shows it", async () => {
		renderApp();
		const view = await openAndRun("sudo true");
		const bar = await screen.findByText(/^Password prompt\./);
		expect(bar.closest("[role=status]")).toHaveTextContent(
			"Password prompt. What you type goes only to this terminal and isn't shown or recorded."
		);
		await type(view, "hunter2\r");
		await waitFor(() => expect(screen.queryByText(/^Password prompt\./)).toBeNull());
		expect(view.written.join("")).not.toContain("hunter2");
	});

	it("adds picked output to the agent's chat as an Output pill with its lines", async () => {
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun("pnpm dev");
		const panel = panels()[0]!;
		act(() => view.pick({ text: "ready in 412 ms", lines: { first: 2, last: 3 } }));
		fireEvent.pointerUp(view.element, { clientX: 40, clientY: 20 });
		fireEvent.click(within(panel).getByRole("button", { name: "Add to chat" }));

		expect(send).toHaveBeenCalledWith(
			AGENT,
			expect.objectContaining({
				type: "output",
				command: "pnpm dev",
				device_name: "Mac",
				cwd: FOLDER,
				text: "ready in 412 ms",
				lines: { first: 2, last: 3 },
				running: true,
				exit_code: null,
			}),
			true
		);
		expect(view.selection()).toBeNull();
		expect(within(panel).queryByRole("button", { name: "Add to chat" })).toBeNull();
	});

	it("names what still runs when you delete the agent", async () => {
		renderApp();
		await openAndRun("pnpm dev");
		fireEvent.contextMenu(await row());
		fireEvent.click(
			within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Delete" })
		);
		expect(await screen.findByRole("alertdialog")).toHaveTextContent(
			"Deleting also stops what still runs in a terminal: pnpm dev."
		);
	});
});
