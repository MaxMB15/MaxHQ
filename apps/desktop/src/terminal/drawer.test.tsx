// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { browserNative, ComposerNativeProvider } from "@/composer/native";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { mockSource } from "@/data/mock-source";
import type { MenuEntry } from "@/native/menu";
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

/** The context menus the app asked for, as the native menu would show them. */
const menus: (readonly MenuEntry[])[] = [];
const native = {
	...browserNative,
	contextMenu: (entries: readonly MenuEntry[]) => {
		menus.push(entries);
		return true;
	},
};

const renderApp = () => {
	const source: DataSource = mockSource({ now: () => NOW, tick: false });
	const closeTerminal = vi.spyOn(source, "closeTerminal");
	render(
		<FixedNow value={NOW}>
			<ComposerNativeProvider value={native}>
				<App source={source} ui={createUiStore()} />
			</ComposerNativeProvider>
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
	menus.length = 0;
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

	it("sends picked output to the agent's chat as an Output pill with its lines", async () => {
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun("pnpm dev");
		const panel = panels()[0]!;
		act(() => view.pick({ text: "ready in 412 ms", lines: { first: 2, last: 3 } }));
		fireEvent.pointerUp(view.element, { clientX: 40, clientY: 20 });
		fireEvent.click(within(panel).getByRole("button", { name: "Send to agent" }));

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
		expect(within(panel).queryByRole("button", { name: "Send to agent" })).toBeNull();
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

	it("marks a tab you named while a command runs in it, and not by its name", async () => {
		renderApp();
		const view = await openAndRun();
		fireEvent.doubleClick((await tabs())[0]!);
		const name = screen.getByRole("textbox", { name: "Rename" });
		fireEvent.change(name, { target: { value: "server" } });
		fireEvent.keyDown(name, { key: "Enter" });
		const running = async () => {
			const [tab] = await tabs();
			expect(tab).toHaveTextContent("server");
			return within(tab!).queryByRole("img", { name: "Running" });
		};
		await waitFor(async () => expect((await tabs())[0]).toHaveTextContent("server"));
		expect(await running()).toBeNull();

		await type(view, "pnpm dev\r");
		await waitFor(async () => expect(await running()).not.toBeNull());
		fireEvent.contextMenu(await row());
		fireEvent.click(
			within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Delete" })
		);
		const dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent(
			"Deleting also stops what still runs in a terminal: server."
		);
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

		await type(view, "\x03");
		await waitFor(async () => expect(await running()).toBeNull());
	});
});

describe("a terminal whose shell marks its commands", () => {
	const failed = { command: "pnpm test", exit: 1, running: false };
	const output = { text: "1 failed", lines: { first: 3, last: 4 }, mark: failed };

	/** Clicks command 1's mark and answers with its menu. */
	const openMenu = async (view: FakeView) => {
		const dot = view.element.appendChild(document.createElement("div"));
		act(() => view.clickMark(1, dot));
		return screen.findByRole("menu");
	};

	it("opens a command's menu from its mark, which sends what it printed to the agent or copies it", async () => {
		const send = vi.spyOn(quoteInbox, "send");
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		renderApp();
		const view = await openAndRun();
		view.mark([{ id: 1, ...failed, output }]);

		let menu = await openMenu(view);
		expect(menu).toHaveTextContent("pnpm test");
		expect(menu).toHaveTextContent("Exit 1");
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy output" }));
		expect(writeText).toHaveBeenLastCalledWith("1 failed");
		await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

		menu = await openMenu(view);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy command" }));
		expect(writeText).toHaveBeenLastCalledWith("pnpm test");
		await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

		menu = await openMenu(view);
		fireEvent.click(within(menu).getByRole("menuitem", { name: "Send to agent" }));
		expect(send).toHaveBeenCalledWith(
			AGENT,
			expect.objectContaining({
				type: "output",
				command: "pnpm test",
				device_name: "Mac",
				cwd: FOLDER,
				text: "1 failed",
				lines: { first: 3, last: 4 },
				running: false,
				exit_code: 1,
			}),
			true
		);
	});

	it("can't copy the output of a command that printed nothing yet", async () => {
		renderApp();
		const view = await openAndRun();
		const running = { command: "sleep 5", exit: null, running: true };
		view.mark([
			{
				id: 1,
				...running,
				output: { text: "", lines: { first: 5, last: 5 }, mark: running },
			},
		]);
		const menu = await openMenu(view);
		expect(menu).toHaveTextContent("Running");
		expect(within(menu).getByRole("menuitem", { name: "Copy output" })).toHaveAttribute(
			"aria-disabled",
			"true"
		);
	});

	it("jumps between commands on Control+Shift and Up or Down off a Mac", async () => {
		renderApp();
		const view = await openAndRun();
		fireEvent.keyDown(view.element, { key: "ArrowUp", ctrlKey: true, shiftKey: true });
		fireEvent.keyDown(view.element, { key: "ArrowDown", ctrlKey: true, shiftKey: true });
		// Control alone with an arrow is the shell's, and so is Command.
		fireEvent.keyDown(view.element, { key: "ArrowUp", ctrlKey: true });
		fireEvent.keyDown(view.element, { key: "ArrowUp", metaKey: true });
		expect(view.jumps).toEqual([true, false]);
	});

	// The fake view's element stands in for the terminal's own text field.
	const focusIn = (view: FakeView) =>
		act(() => {
			view.element.tabIndex = 0;
			view.element.focus();
		});
	const finding = () => screen.queryByRole("search", { name: "Find in the terminal" });

	it("opens find on Control+Shift+F off a Mac, where Control+F is the shell's", async () => {
		renderApp();
		const view = await openAndRun();
		focusIn(view);
		fireEvent.keyDown(view.element, { key: "f", code: "KeyF", ctrlKey: true });
		expect(finding()).toBeNull();
		fireEvent.keyDown(view.element, { key: "F", code: "KeyF", ctrlKey: true, shiftKey: true });
		expect(await screen.findByRole("search", { name: "Find in the terminal" })).toBeVisible();
	});

	it("opens find on Command+F on a Mac", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		renderApp();
		const view = await openAndRun();
		focusIn(view);
		fireEvent.keyDown(view.element, { key: "F", code: "KeyF", metaKey: true, shiftKey: true });
		expect(finding()).toBeNull();
		fireEvent.keyDown(view.element, { key: "f", code: "KeyF", metaKey: true });
		expect(await screen.findByRole("search", { name: "Find in the terminal" })).toBeVisible();
	});

	it("jumps between commands on Command and Up or Down on a Mac", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		renderApp();
		const view = await openAndRun();
		fireEvent.keyDown(view.element, { key: "ArrowUp", metaKey: true });
		fireEvent.keyDown(view.element, { key: "ArrowDown", metaKey: true });
		fireEvent.keyDown(view.element, { key: "ArrowUp", ctrlKey: true, shiftKey: true });
		fireEvent.keyDown(view.element, { key: "ArrowUp", metaKey: true, altKey: true });
		expect(view.jumps).toEqual([true, false]);
	});
});

describe("the Agent shell", () => {
	it("is the panel's first tab, titled Agent shell, and a shell you open goes after it", async () => {
		renderApp();
		fireEvent.click(await within(nav()).findByRole("option", { name: /Usage meter popover/ }));
		await screen.findAllByRole("region", { name: "Usage meter popover" });

		toggle();

		const [session] = await tabs();
		expect(session).toHaveTextContent("Agent shell");
		expect(session).toHaveAttribute("aria-selected", "true");
		await waitFor(() => expect(viewIn(panels()[0]!).written.join("")).toContain("pnpm test"));

		// A shell you open goes after it.
		fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
		await waitFor(async () => expect(await tabs()).toHaveLength(2));
		const [first, second] = await tabs();
		expect(first).toHaveTextContent("Agent shell");
		expect(second).toHaveAttribute("aria-selected", "true");
	});
});

describe("the strip under the composer", () => {
	const strip = () => document.querySelector("[data-terminal-strip]");

	it("names what runs in the hidden panel, and opens the panel on that tab", async () => {
		renderApp();
		await openAndRun("pnpm dev");
		await waitFor(async () => expect((await tabs())[0]).toHaveTextContent("pnpm dev"));
		fireEvent.click(screen.getByRole("button", { name: "New terminal" }));
		await waitFor(async () => expect(await tabs()).toHaveLength(2));
		expect(strip()).toBeNull();

		fireEvent.click(screen.getByRole("button", { name: "Hide" }));

		await waitFor(() => expect(strip()).toHaveTextContent("pnpm dev running·2 tabs"));
		fireEvent.click(strip()!);
		const [dev] = await tabs();
		expect(dev).toHaveAttribute("aria-selected", "true");
		expect(strip()).toBeNull();
	});

	it("isn't there when nothing runs in the hidden panel", async () => {
		renderApp();
		await openAndRun();

		fireEvent.click(screen.getByRole("button", { name: "Hide" }));

		expect(screen.queryByRole("tablist", { name: "Terminals" })).toBeNull();
		expect(strip()).toBeNull();
	});
});

describe("Send to agent", () => {
	const macKeys = () =>
		vi
			.spyOn(navigator, "userAgent", "get")
			.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");

	/** The entries of the menu a right-click on the view asks for. */
	const rightClick = (view: FakeView) => {
		fireEvent.contextMenu(view.element, { clientX: 40, clientY: 20 });
		const entries = menus.at(-1);
		if (!entries) throw new Error("no menu");
		return entries;
	};
	const item = (entries: readonly MenuEntry[], text: string) => {
		const found = entries.find((entry) => "text" in entry && entry.text === text);
		if (!found || found.kind !== "item") throw new Error(`no ${text}`);
		return found;
	};

	it("sends the picked text on ⌘↩, named after the tab", async () => {
		macKeys();
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun("pnpm dev");
		act(() => view.pick({ text: "ready in 412 ms\nlistening", lines: { first: 2, last: 3 } }));

		fireEvent.keyDown(view.element, { key: "Enter", metaKey: true });

		expect(send).toHaveBeenCalledExactlyOnceWith(
			AGENT,
			expect.objectContaining({
				type: "output",
				command: "pnpm dev",
				text: "ready in 412 ms\nlistening",
				lines: { first: 2, last: 3 },
			}),
			true
		);
		expect(view.selection()).toBeNull();
	});

	it("takes Control, Shift and Return off a Mac, and leaves Return alone", async () => {
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun();
		act(() => view.pick({ text: "ok", lines: { first: 1, last: 1 } }));

		fireEvent.keyDown(view.element, { key: "Enter", metaKey: true });
		fireEvent.keyDown(view.element, { key: "Enter", ctrlKey: true });
		expect(send).not.toHaveBeenCalled();

		fireEvent.keyDown(view.element, { key: "Enter", ctrlKey: true, shiftKey: true });
		expect(send).toHaveBeenCalledOnce();
	});

	it("offers Send to agent and Copy on a right-click over picked text", async () => {
		macKeys();
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun();
		act(() => view.pick({ text: "ok", lines: { first: 1, last: 1 } }));

		const entries = rightClick(view);

		expect(entries.map((entry) => ("text" in entry ? entry.text : entry.kind))).toEqual([
			"Send to agent",
			"Send to another agent",
			"separator",
			"ok",
		]);
		expect(entries.at(-1)).toEqual({ kind: "copy", text: "ok" });
		const sending = item(entries, "Send to agent");
		expect(sending).toMatchObject({ enabled: true, accelerator: "Cmd+Enter" });
		act(() => sending.action());
		expect(send).toHaveBeenCalledWith(
			AGENT,
			expect.objectContaining({ text: "ok", lines: { first: 1, last: 1 } }),
			true
		);
	});

	it("sends the last command's output with nothing picked, and is dimmed with none", async () => {
		const send = vi.spyOn(quoteInbox, "send");
		renderApp();
		const view = await openAndRun();

		let entries = rightClick(view);
		expect(item(entries, "Send to agent").enabled).toBe(false);
		expect(entries.some((entry) => entry.kind === "copy")).toBe(false);
		fireEvent.keyDown(view.element, { key: "Enter", ctrlKey: true, shiftKey: true });
		expect(send).not.toHaveBeenCalled();

		const failed = { command: "pnpm test", exit: 1, running: false };
		view.mark([
			{
				id: 1,
				...failed,
				output: { text: "1 failed", lines: { first: 3, last: 4 }, mark: failed },
			},
		]);
		entries = rightClick(view);
		expect(item(entries, "Send to agent").enabled).toBe(true);
		act(() => item(entries, "Send to agent").action());

		expect(send).toHaveBeenCalledExactlyOnceWith(
			AGENT,
			expect.objectContaining({
				command: "pnpm test",
				text: "1 failed",
				lines: { first: 3, last: 4 },
				exit_code: 1,
			}),
			true
		);
	});
});
