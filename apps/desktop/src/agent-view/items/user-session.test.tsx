// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { DeviceLink, Entity, SessionBlock, Terminal, Workspace } from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import type { UserSessionItem } from "@maxhq/shared/terminal/session";
import { entities, link, workspace } from "@maxhq/shared/test/build";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { ServicesProvider } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { Composer } from "@/composer/composer";
import { ComposerNativeProvider } from "@/composer/native";
import { forgetNewest, startSessionRun } from "@/composer/run-command";
import {
	AGENT,
	DEVICE,
	DONE,
	editorOf,
	fakeNative,
	fakeSource,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
} from "@/composer/testing";
import type { LiveTerminalProps } from "@/terminal";

import { IGNORED_AFTER } from "./run-input";
import { UserSessionCard } from "./user-session";

// The terminal module, as the card reaches it.
const seam = vi.hoisted(() => ({
	sendInput: vi.fn<(device: string, terminal: string, data: string) => void>(),
	showInDrawer: vi.fn<(agent: string, terminal: string) => void>(),
	live: [] as LiveTerminalProps[],
	shown: false,
}));

vi.mock("@/terminal", () => ({
	useShownInPanel: () => seam.shown,
	LiveTerminal: (props: LiveTerminalProps) => {
		seam.live.push(props);
		return <div data-testid="live-terminal" />;
	},
	useTerminalActions: () => ({
		sendInput: seam.sendInput,
		text: () => null,
		runningText: () => null,
		cellSize: () => ({ width: 8, height: 17 }),
		pasteIntoDrawer: () => {},
		showInDrawer: seam.showInDrawer,
		openAgentShell: () => {},
	}),
}));

beforeAll(installBrowserFakes);
beforeEach(() => {
	seam.sendInput.mockReset();
	seam.showInDrawer.mockReset();
	seam.live = [];
	seam.shown = false;
});
afterEach(() => {
	cleanup();
	clearKept();
	quoteInbox.take(AGENT);
	localStorage.clear();
	sessionStorage.clear();
	vi.restoreAllMocks();
});

const WITH_TERMINALS = { transfers: true, tunnels: true, workspaces: true, terminals: true };

const macLink = (): DeviceLink =>
	link({ id: "mac", name: "Mac", device: DEVICE, protocol: 48, capabilities: WITH_TERMINALS });

const FOLDER = "/Users/dev/code/example-app";

const home = (): Workspace => workspace({ id: "w1", location: { type: "folder", path: FOLDER } });

const EXITED = (code: number) => ({ state: "exited", code, signal: null }) as const;

/** A command in the session, which ended well unless a test says otherwise. */
const block = (id: number, over: Partial<SessionBlock> = {}): SessionBlock => ({
	id,
	command: "true",
	cwd: FOLDER,
	cwd_after: null,
	private: false,
	process: EXITED(0),
	started_at: Date.now() - 60_000,
	duration_ms: 1200,
	lines: 0,
	output: "",
	output_truncated: false,
	secret_prompts: 0,
	start: null,
	...over,
});

/** A `cd`, an install, a private `printenv` and a test run that fails. */
const BLOCKS: readonly SessionBlock[] = [
	block(1, { command: "cd apps/web", cwd_after: `${FOLDER}/apps/web`, duration_ms: 10 }),
	block(2, {
		command: "pnpm install",
		cwd: `${FOLDER}/apps/web`,
		output: "Packages: +212\nDone in 4.2s\n",
		lines: 2,
		duration_ms: 4200,
	}),
	block(3, {
		command: "printenv API_TOKEN",
		cwd: `${FOLDER}/apps/web`,
		private: true,
		output: "tok_example_123\n",
		lines: 1,
	}),
	block(4, {
		command: "pnpm test",
		cwd: `${FOLDER}/apps/web`,
		process: EXITED(1),
		output: "\u001b[31mFAIL\u001b[0m src/cache.test.ts\nTests 1 failed\n",
		lines: 2,
		duration_ms: 9100,
	}),
];

// Each card gets an ID of its own, since the composer remembers the
// cards whose pill it took.
let serial = 0;

/** A session card, open unless a test says otherwise. */
const card = (over: Partial<UserSessionItem> = {}): UserSessionItem =>
	timelineItem(
		{
			kind: "user_session",
			terminal: "s1",
			device: DEVICE,
			blocks: [...BLOCKS],
			sealed: false,
			sent: false,
			...over,
		},
		{
			id: `card-${(serial += 1)}`,
			agent: AGENT,
			ordinal: 1,
			status: "completed",
			started_at: Date.now() - 60_000,
		}
	) as UserSessionItem;

/** The session's terminal, as the view holds it. */
const sessionTerminal = (over: Partial<Terminal> = {}): Entity => ({
	entity: "terminal",
	id: "s1",
	agent: AGENT,
	device: DEVICE,
	kind: { type: "session" },
	moved: false,
	title: "zsh",
	cwd: `${FOLDER}/apps/web`,
	status: { state: "running" },
	busy: false,
	marks: true,
	password_prompt: false,
	started_at: Date.now() - 60_000,
	cols: 100,
	rows: 24,
	...over,
});

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

/** The card in its agent's timeline, with the agent's composer below it. */
const renderCard = (
	item: UserSessionItem,
	{
		terminal = sessionTerminal(),
		readOnly = false,
	}: { terminal?: Entity | null; readOnly?: boolean } = {}
) => {
	const fake = fakeSource({
		agent: heldAgent(),
		links: [macLink()],
		more: [...entities({ workspaces: [home()] }), ...(terminal ? [terminal] : [])],
	});
	const calls = {
		sessionShare: vi.fn<DataSource["sessionShare"]>(async () => DONE),
		sessionEnd: vi.fn<DataSource["sessionEnd"]>(async () => DONE),
	};
	Object.assign(fake.source, calls);
	const timeline = createTimelineStore();
	timeline.setState(
		applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 1, items: [item], has_older: false }),
		true
	);
	const row: RowContext = {
		agent: AGENT,
		vendor: "claude",
		timeline,
		readOnly,
		remote: false,
		answer: async () => DONE,
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	render(
		<ServicesProvider source={fake.source}>
			<RowProvider value={row}>
				<UserSessionCard item={item} />
			</RowProvider>
			<ComposerNativeProvider value={fakeNative().native}>
				<Composer agent={AGENT} device={DEVICE} queued={[]} timeline={timeline} />
			</ComposerNativeProvider>
			<ConfirmHost />
		</ServicesProvider>
	);
	return { ...fake, ...calls, timeline };
};

/** The card, once the view knows the device's name. */
const findCard = async () => {
	const found = await screen.findByRole("region", { name: "Commands you ran on Mac" });
	await within(found).findByText("on Mac");
	return found;
};

const button = (within_: HTMLElement, name: string | RegExp) =>
	within(within_).getByRole("button", { name });

/** Opens the card's ⋯ menu and clicks Close shell. */
const closeShell = async (found: HTMLElement) => {
	fireEvent.click(button(found, "More for this shell"));
	fireEvent.click(await screen.findByRole("menuitem", { name: "Close shell" }));
};

const blockOf = (found: HTMLElement, id: number) =>
	found.querySelector<HTMLElement>(`[data-session-block="${id}"]`)!;

describe("an open session card", () => {
	it("says what you ran where, with each command's end, folder and output", async () => {
		renderCard(card());
		const found = await findCard();

		expect(found).toHaveAttribute("data-user-session", "open");
		expect(found).toHaveTextContent("You ran 4 commands");
		expect(blockOf(found, 1)).toHaveTextContent("now in ~/code/example-app/apps/web");
		expect(blockOf(found, 2)).toHaveTextContent("Done in 4.2s");
		expect(blockOf(found, 3)).toHaveAttribute("data-private", "true");
		expect(blockOf(found, 4)).toHaveTextContent("exit 1");
		expect(within(blockOf(found, 4)).getByText("FAIL")).toHaveStyle({
			color: "var(--terminal-red)",
		});
		expect(within(found).queryByTestId("live-terminal")).toBeNull();
	});

	it("switches a command between Shared and Private", async () => {
		const item = card();
		const { sessionShare } = renderCard(item);
		const found = await findCard();
		const shared = button(found, "Keep pnpm test private");
		const kept = button(found, "Share printenv API_TOKEN with the agent");
		expect(shared).toHaveAttribute("aria-pressed", "true");
		expect(shared).toHaveTextContent("Shared");
		expect(kept).toHaveAttribute("aria-pressed", "false");
		expect(kept).toHaveTextContent("Private");

		fireEvent.click(shared);
		await waitFor(() =>
			expect(sessionShare).toHaveBeenCalledExactlyOnceWith(AGENT, item.id, 4, true)
		);
		fireEvent.click(kept);

		await waitFor(() => expect(sessionShare).toHaveBeenCalledTimes(2));
		expect(sessionShare).toHaveBeenLastCalledWith(AGENT, item.id, 3, false);
	});

	it("says why a switch didn't go", async () => {
		const { sessionShare } = renderCard(card());
		const found = await findCard();
		sessionShare.mockResolvedValueOnce(refused("That message went"));

		fireEvent.click(button(found, "Keep pnpm test private"));

		expect(await within(found).findByRole("alert")).toHaveTextContent(
			"Couldn't change who sees the command. That message went."
		);
	});

	it("shows a running command live, with a line to type into it", async () => {
		const running = block(5, {
			command: "pnpm dev",
			process: { state: "running" },
			duration_ms: null,
		});
		renderCard(card({ blocks: [...BLOCKS, running] }));
		const found = await findCard();

		expect(within(blockOf(found, 5)).getByTestId("live-terminal")).toBeInTheDocument();
		expect(seam.live.at(-1)).toMatchObject({
			device: DEVICE,
			terminal: "s1",
			interactive: false,
		});
		expect(blockOf(found, 5)).toHaveTextContent(/Running/);
		expect(within(blockOf(found, 5)).getByRole("textbox")).toBeInTheDocument();
	});

	describe("a command that runs in it", () => {
		const python = block(5, {
			command: "python3",
			process: { state: "running" },
			duration_ms: null,
		});

		/** The card with `python3` running in its session, on a device that answers or not. */
		const renderPython = (answers: boolean) => {
			const rendered = renderCard(card({ blocks: [...BLOCKS, python] }), {
				terminal: sessionTerminal({ busy: true, answers }),
			});
			const interruptTerminal = vi.fn<DataSource["interruptTerminal"]>(async () => DONE);
			Object.assign(rendered.source, { interruptTerminal });
			return { ...rendered, interruptTerminal };
		};

		/** Presses Stop on `python3`, and lets the moment it gets go by. */
		const stopAndWait = async (found: HTMLElement, interrupt: ReturnType<typeof vi.fn>) => {
			fireEvent.click(button(blockOf(found, 5), "Stop"));
			await waitFor(() => expect(interrupt).toHaveBeenCalledWith("s1", false));
			await act(async () => {
				await vi.advanceTimersByTimeAsync(IGNORED_AFTER);
			});
		};

		beforeEach(() => {
			vi.useFakeTimers({ shouldAdvanceTime: true });
		});
		afterEach(() => {
			vi.useRealTimers();
		});

		it("stops it, and once it ignored that ends only it, leaving the shell", async () => {
			const { interruptTerminal } = renderPython(true);
			const found = await findCard();
			expect(
				within(blockOf(found, 5)).queryByRole("button", { name: "Force stop" })
			).toBeNull();

			await stopAndWait(found, interruptTerminal);

			expect(blockOf(found, 5)).toHaveTextContent("python3 ignored Stop");
			fireEvent.click(button(blockOf(found, 5), "Force stop"));
			await waitFor(() => expect(interruptTerminal).toHaveBeenLastCalledWith("s1", true));
		});

		it("has no Force stop on a device that can't end only the command", async () => {
			const { interruptTerminal } = renderPython(false);
			const found = await findCard();

			await stopAndWait(found, interruptTerminal);

			expect(blockOf(found, 5)).toHaveTextContent("python3 ignored Stop");
			expect(
				within(blockOf(found, 5)).queryByRole("button", { name: "Force stop" })
			).toBeNull();
		});

		it("opens the Agent shell's tab from the command, with no second button", async () => {
			renderPython(true);
			const found = await findCard();

			expect(within(found).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(
				1
			);
			fireEvent.click(button(blockOf(found, 5), "Open in terminal"));

			expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "s1");
		});

		it("says it waits for you, with the prompt's answers", async () => {
			renderCard(card({ blocks: [...BLOCKS, python] }), {
				terminal: sessionTerminal({ busy: true, waiting: true, choices: ["y", "n"] }),
			});
			const found = await findCard();

			expect(blockOf(found, 5)).toHaveTextContent("Waiting for you");
			fireEvent.click(button(blockOf(found, 5), "Answer y"));
			expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "s1", "y\r", undefined);
		});
	});

	it("says Stopped after how long for a command you stopped", async () => {
		const stopped = block(5, {
			command: "pnpm dev",
			process: { state: "exited", code: null, signal: 2, stopped: true },
			duration_ms: 33_000,
		});
		renderCard(card({ blocks: [...BLOCKS, stopped] }));
		const found = await findCard();

		expect(blockOf(found, 5)).toHaveTextContent("Stopped after 33 s");
	});

	it("opens the Agent shell's tab in the terminal panel", async () => {
		renderCard(card());
		const found = await findCard();

		fireEvent.click(button(found, "Open in terminal"));

		expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "s1");
	});

	it("drops Open in terminal and the ⋯ menu once the shell ended", async () => {
		renderCard(card(), {
			terminal: sessionTerminal({ status: { state: "exited", code: 0, signal: null } }),
		});
		const found = await findCard();

		expect(within(found).queryByRole("button", { name: "Open in terminal" })).toBeNull();
		expect(within(found).queryByRole("button", { name: "More for this shell" })).toBeNull();
		expect(button(found, "Send now")).toBeEnabled();
	});

	it("closes the shell from its ⋯ menu, asking first while a command runs", async () => {
		const { sessionEnd } = renderCard(card(), { terminal: sessionTerminal({ busy: true }) });
		const found = await findCard();
		sessionEnd.mockResolvedValueOnce(refused("A command still runs in the session"));

		await closeShell(found);

		const ask = await screen.findByRole("alertdialog");
		expect(ask).toHaveTextContent("Close the shell?");
		expect(ask).toHaveTextContent("A command still runs in the session");
		fireEvent.click(within(ask).getByRole("button", { name: "Close shell" }));
		await waitFor(() => expect(sessionEnd).toHaveBeenCalledTimes(2));
		expect(sessionEnd.mock.calls[0]).toEqual([AGENT]);
		expect(sessionEnd.mock.calls[1]).toEqual([AGENT, true]);
		expect(within(found).queryByRole("alert")).toBeNull();
	});

	it("asks first while one of its commands runs, too", async () => {
		const running = block(5, { command: "pnpm dev", process: { state: "running" } });
		const { sessionEnd } = renderCard(card({ blocks: [...BLOCKS, running] }));
		const found = await findCard();
		sessionEnd.mockResolvedValueOnce(refused("A command still runs in the session"));

		await closeShell(found);

		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", {
				name: "Close shell",
			})
		);
		await waitFor(() => expect(sessionEnd).toHaveBeenLastCalledWith(AGENT, true));
	});

	it("only says why it couldn't close the shell when nothing runs", async () => {
		const { sessionEnd } = renderCard(card());
		const found = await findCard();
		sessionEnd.mockResolvedValueOnce(refused("No session to end"));

		await closeShell(found);

		expect(await within(found).findByRole("alert")).toHaveTextContent(
			"Couldn't close the shell. No session to end."
		);
		expect(screen.queryByRole("alertdialog")).toBeNull();
		expect(sessionEnd).toHaveBeenCalledExactlyOnceWith(AGENT);
	});

	it("doesn't ask when the device is away, even while a command runs", async () => {
		const { sessionEnd } = renderCard(card(), { terminal: sessionTerminal({ busy: true }) });
		const found = await findCard();
		sessionEnd.mockResolvedValueOnce({
			status: "error",
			error: { kind: "failed", message: "The device is offline" },
		});

		await closeShell(found);

		expect(await within(found).findByRole("alert")).toHaveTextContent(
			"Couldn't close the shell. The device is offline."
		);
		expect(screen.queryByRole("alertdialog")).toBeNull();
		expect(sessionEnd).toHaveBeenCalledExactlyOnceWith(AGENT);
	});

	it("sends the shared commands on their own, as a message holding only the pill", async () => {
		const item = card();
		const { source } = renderCard(item);
		const found = await findCard();

		fireEvent.click(button(found, "Send now"));

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		const [agent, input, mode] = source.sendInput.mock.calls[0]!;
		expect(agent).toBe(AGENT);
		expect(mode).toBe("queue");
		expect(input.text).toBe(MARK);
		expect(input.refs).toEqual([
			{
				type: "session",
				device_name: "Mac",
				item: item.id,
				blocks: [
					expect.objectContaining({ command: "cd apps/web", text: "" }),
					expect.objectContaining({ command: "pnpm install", exit_code: 0 }),
					expect.objectContaining({
						command: "pnpm test",
						exit_code: 1,
						running: false,
						lines_total: 2,
						text: "FAIL src/cache.test.ts\nTests 1 failed",
					}),
				],
			},
		]);
		expect(JSON.stringify(input)).not.toContain("tok_example_123");
		expect(JSON.stringify(input)).not.toContain("printenv");
	});

	it("takes its pill out of the composer once it sent them, so the next message doesn't too", async () => {
		const { source } = renderCard(card());
		const found = await findCard();
		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));

		fireEvent.click(button(found, "Send now"));

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		// The composer has had its chance to put it back.
		await act(async () => {});
		expect(pillsIn(textbox)).toEqual([]);
		expect(found).toHaveTextContent("Not in your next message");
	});

	it("keeps its pill in the composer when sending them failed", async () => {
		const { source } = renderCard(card());
		const found = await findCard();
		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		source.sendInput.mockResolvedValueOnce(refused("The agent is gone"));

		fireEvent.click(button(found, "Send now"));

		expect(await within(found).findByRole("alert")).toHaveTextContent(
			"Couldn't send the commands. The agent is gone."
		);
		expect(pillsIn(textbox)).toHaveLength(1);
	});

	it("can't send when every command is private", async () => {
		renderCard(card({ blocks: BLOCKS.map((each) => ({ ...each, private: true })) }));
		const found = await findCard();

		expect(button(found, "Send now")).toBeDisabled();
		expect(found).toHaveTextContent("Not in your next message");
		expect(within(found).queryByRole("button", { name: "Add to next message" })).toBeNull();
	});

	it("says its commands go with your next message while the composer holds its pill", async () => {
		const item = card();
		renderCard(item);
		const found = await findCard();
		const textbox = await findTextbox();

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Agent shell · 1 failed on Mac"]));
		expect(
			await within(found).findByText("3 of 4 go with your next message")
		).toBeInTheDocument();
		expect(within(found).queryByRole("button", { name: "Add to next message" })).toBeNull();

		act(() => {
			editorOf(textbox).commands.clearContent(true);
		});

		expect(await within(found).findByText("Not in your next message")).toBeInTheDocument();
		// A pill you removed stays out.
		expect(pillsIn(textbox)).toEqual([]);

		fireEvent.click(button(found, "Add to next message"));

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Agent shell · 1 failed on Mac"]));
		expect(
			await within(found).findByText("3 of 4 go with your next message")
		).toBeInTheDocument();
	});

	it("shows the commands without buttons in a read-only timeline", async () => {
		renderCard(card(), { readOnly: true });
		const found = await findCard();

		expect(found).toHaveTextContent("3 of 4 go with your next message");
		expect(
			within(found).queryByRole("button", { name: /Send now|More for this shell/ })
		).toBeNull();
		expect(found.querySelector("[aria-pressed]")).toBeNull();
		expect(blockOf(found, 3)).toHaveTextContent("Private");
	});
});

describe("a sealed session card", () => {
	it("says its commands went with your message, and drops the switches and buttons", async () => {
		renderCard(card({ sealed: true, sent: true }));
		const found = await findCard();

		expect(found).toHaveAttribute("data-user-session", "sealed");
		expect(found.querySelector("[data-session-sealed]")).toHaveAttribute(
			"data-session-sealed",
			"sent"
		);
		expect(found).toHaveTextContent("Sent with your message");
		expect(found.querySelector("[aria-pressed]")).toBeNull();
		expect(within(found).queryByRole("button", { name: "Send now" })).toBeNull();
		expect(within(found).queryByRole("button", { name: "More for this shell" })).toBeNull();
		expect(blockOf(found, 3)).toHaveTextContent("Private");
		expect(blockOf(found, 4)).not.toHaveTextContent("Shared");
	});

	it("says Not sent when the message went without its pill", async () => {
		renderCard(card({ sealed: true, sent: false }));
		const found = await findCard();

		expect(found.querySelector("[data-session-sealed]")).toHaveAttribute(
			"data-session-sealed",
			"not-sent"
		);
		expect(found).toHaveTextContent("Not sent");
	});

	it("puts no pill in the composer", async () => {
		renderCard(card({ sealed: true, sent: true }));
		await findCard();
		const textbox = await findTextbox();

		// The composer has had its chance to add one.
		await act(async () => {});
		expect(pillsIn(textbox)).toEqual([]);
	});
});

describe("lines you typed in the terminal", () => {
	const typed = (id: number, over: Partial<SessionBlock> = {}) =>
		block(id, {
			command: `ls ${id}`,
			cwd: `${FOLDER}/apps/web`,
			typed: true,
			private: true,
			output: `README-${id}.md\n`,
			lines: 1,
			...over,
		});

	it("shows each as a Private line you can share, which opens to its output", async () => {
		const item = card({ blocks: [...BLOCKS, typed(5, { command: "ls -la" })] });
		const { sessionShare } = renderCard(item);
		const found = await findCard();
		const line = blockOf(found, 5);

		expect(line).toHaveAttribute("data-typed");
		expect(line).toHaveTextContent("ls -latyped in the terminalPrivate");
		expect(line).not.toHaveTextContent("README-5.md");
		expect(found).toHaveTextContent("3 of 5 go with your next message");

		fireEvent.click(button(line, "ls -la"));
		expect(line).toHaveTextContent("README-5.md");

		fireEvent.click(button(line, "Share ls -la with the agent"));
		await waitFor(() =>
			expect(sessionShare).toHaveBeenCalledExactlyOnceWith(AGENT, item.id, 5, false)
		);
	});

	it("folds more than three in a row into one line that opens to them", async () => {
		const run = [typed(5), typed(6, { private: false }), typed(7), typed(8)];
		renderCard(card({ blocks: [...BLOCKS, ...run] }));
		const found = await findCard();
		const fold = found.querySelector<HTMLElement>("[data-typed-run]")!;

		expect(fold).toHaveAttribute("data-typed-run", "4");
		expect(fold).toHaveTextContent("4 commands typed in the terminal1 of 4 shared");
		expect(found.querySelectorAll("[data-typed]")).toHaveLength(0);

		fireEvent.click(button(fold, /4 commands typed in the terminal/));

		expect(found.querySelectorAll("[data-typed]")).toHaveLength(4);
		expect(blockOf(found, 6)).toHaveTextContent("Shared");
	});

	it("shows three in a row one by one", async () => {
		renderCard(card({ blocks: [...BLOCKS, typed(5), typed(6), typed(7)] }));
		const found = await findCard();

		expect(found.querySelector("[data-typed-run]")).toBeNull();
		expect(found.querySelectorAll("[data-typed]")).toHaveLength(3);
	});

	it("keeps the header's Open in terminal while one runs, with no view of its own", async () => {
		const running = typed(5, { process: { state: "running" }, duration_ms: null, output: "" });
		renderCard(card({ blocks: [...BLOCKS, running] }), {
			terminal: sessionTerminal({ busy: true }),
		});
		const found = await findCard();

		expect(within(found).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
		expect(within(blockOf(found, 5)).queryByTestId("live-terminal")).toBeNull();
		expect(within(blockOf(found, 5)).queryByRole("textbox")).toBeNull();
		expect(within(blockOf(found, 5)).queryByRole("button", { name: "Stop" })).toBeNull();
	});
});

describe("a program that takes the whole screen", () => {
	const vim = block(5, {
		command: "vim notes.md",
		process: { state: "running" },
		duration_ms: null,
	});
	const full = () => sessionTerminal({ busy: true, full_screen: true });
	// A private command puts no pill in, so starting it never reads the view.
	const noView = (): never => {
		throw new Error("A private command doesn't read the view");
	};

	afterEach(forgetNewest);

	it("says it's in the terminal, with one Open in terminal and no view", async () => {
		renderCard(card({ blocks: [...BLOCKS, vim] }), { terminal: full() });
		const found = await findCard();
		const line = blockOf(found, 5);

		expect(line.querySelector("[data-handoff]")).toHaveTextContent("In the terminal");
		expect(line).toHaveTextContent(
			"vim is open in the terminal below. The result comes back here when it closes."
		);
		expect(within(found).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
		expect(button(line, "Stop")).toBeInTheDocument();
		expect(within(line).queryByTestId("live-terminal")).toBeNull();
		expect(within(line).queryByRole("textbox")).toBeNull();

		fireEvent.click(button(line, "Open in terminal"));
		expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "s1");
	});

	it("has no Open in terminal while the panel shows it", async () => {
		seam.shown = true;
		renderCard(card({ blocks: [...BLOCKS, vim] }), { terminal: full() });
		const found = await findCard();

		expect(within(found).queryByRole("button", { name: "Open in terminal" })).toBeNull();
		expect(blockOf(found, 5)).toHaveTextContent("vim is open in the terminal below.");
	});

	it("opens the Agent shell's tab by itself for the command you just ran here", async () => {
		const here = {
			sessionRun: async (): Promise<ActionResult> => ({
				status: "ok",
				data: { status: "session_ran", terminal: "s1", item: "card-1", block: 5 },
			}),
		};
		await startSessionRun(here, noView, {
			agent: AGENT,
			command: "vim notes.md",
			private: true,
			fresh: false,
			size: { cols: 100, rows: 24 },
		});

		renderCard(card({ blocks: [...BLOCKS, vim] }), { terminal: full() });
		await findCard();

		await waitFor(() => expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "s1"));
	});

	it("leaves the panel alone for a command this window didn't just run", async () => {
		renderCard(card({ blocks: [...BLOCKS, vim] }), { terminal: full() });
		await findCard();
		await act(async () => {});

		expect(seam.showInDrawer).not.toHaveBeenCalled();
	});

	it("has no buttons of its own in a read-only timeline", async () => {
		renderCard(card({ blocks: [...BLOCKS, vim] }), { terminal: full(), readOnly: true });
		const found = await findCard();
		const line = blockOf(found, 5);

		expect(line).toHaveTextContent("vim is open in the terminal below.");
		expect(within(line).queryByRole("button")).toBeNull();
		// The header keeps its Open in terminal, as it does in any read-only card.
		expect(within(found).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
	});
});

describe("a picker that reads keys", () => {
	const picker = block(5, {
		command: "pnpm create vite",
		process: { state: "running" },
		duration_ms: null,
	});
	const reads = (over: Partial<Terminal> = {}) =>
		sessionTerminal({
			busy: true,
			reads_keys: true,
			waiting: true,
			answers: true,
			app_cursor: true,
			...over,
		});

	it("waits for keys, and takes yours once you click its output", async () => {
		renderCard(card({ blocks: [...BLOCKS, picker] }), { terminal: reads() });
		const found = await findCard();
		const line = blockOf(found, 5);
		const box = line.querySelector<HTMLElement>("[data-keys]")!;

		expect(line.querySelector("[data-handoff]")).toHaveTextContent("Waiting for keys");
		expect(box).toHaveAttribute("data-keys", "offered");

		act(() => box.focus());

		expect(box).toHaveAttribute("data-keys", "held");
		expect(line).toHaveTextContent("go to the program");
		expect(within(found).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
		expect(within(found).getAllByRole("button", { name: "Stop" })).toHaveLength(1);
		expect(within(line).queryByRole("textbox")).toBeNull();

		fireEvent.keyDown(box, { key: "ArrowDown" });
		fireEvent.keyDown(box, { key: "Enter" });
		expect(seam.sendInput.mock.calls).toEqual([
			[DEVICE, "s1", "\x1bOB"],
			[DEVICE, "s1", "\r"],
		]);
	});

	it("keeps waiting for keys while you hold them, also just after one", async () => {
		// Right after a key it prints, so the device doesn't say it waits yet.
		renderCard(card({ blocks: [...BLOCKS, picker] }), { terminal: reads({ waiting: false }) });
		const found = await findCard();
		const line = blockOf(found, 5);
		const box = line.querySelector<HTMLElement>("[data-keys]")!;
		expect(line.querySelector("[data-handoff]")).toBeNull();

		act(() => box.focus());

		expect(line.querySelector("[data-handoff]")).toHaveTextContent("Waiting for keys");
	});

	it("stays view-only with a device that doesn't answer for the terminal", async () => {
		renderCard(card({ blocks: [...BLOCKS, picker] }), {
			terminal: sessionTerminal({ busy: true, waiting: true }),
		});
		const found = await findCard();
		const line = blockOf(found, 5);

		expect(line.querySelector("[data-keys]")).toBeNull();
		expect(line).toHaveTextContent("Waiting for you");
		expect(within(line).getByRole("textbox")).toBeInTheDocument();
	});

	it("takes no keys in a read-only timeline", async () => {
		renderCard(card({ blocks: [...BLOCKS, picker] }), { terminal: reads(), readOnly: true });
		const found = await findCard();

		expect(blockOf(found, 5).querySelector("[data-keys]")).toBeNull();
	});
});
