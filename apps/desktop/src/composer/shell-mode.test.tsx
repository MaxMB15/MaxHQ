// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { DeviceLink, Entity, TimelineItem, Workspace } from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { NO_TERMINALS } from "@maxhq/shared/data/mock-terminals";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { startedOutput } from "@maxhq/shared/terminal/output";
import { entities, link, workspace } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";

import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { RUN_ROWS, runSize } from "./run-command";
import { putCommand } from "./shell-mode";
import {
	AGENT,
	clipboard,
	DEVICE,
	editorOf,
	findTextbox,
	installBrowserFakes,
	pillsIn,
	REMOTE,
	renderComposer,
	textIn,
	type,
} from "./testing";

// What the terminal module answers for a live terminal's last lines.
const seam = vi.hoisted(() => ({ live: null as { text: string; total: number } | null }));

vi.mock("@/terminal", () => ({
	LiveTerminal: () => null,
	useTerminalActions: () => ({
		sendInput: () => {},
		text: () => seam.live,
		runningText: () => null,
		cellSize: () => ({ width: 8, height: 17 }),
		pasteIntoDrawer: () => {},
		showInDrawer: () => {},
	}),
}));

beforeAll(installBrowserFakes);
beforeEach(() => {
	seam.live = null;
});
afterEach(() => {
	cleanup();
	clearKept();
	quoteInbox.take(AGENT);
	localStorage.clear();
});

const WITH_TERMINALS = { transfers: true, tunnels: true, workspaces: true, terminals: true };

/** The Mac's link, its maxd speaking protocol 45 with terminals. */
const macLink = (over: Partial<DeviceLink> = {}): DeviceLink =>
	link({
		id: "mac",
		name: "Mac",
		device: DEVICE,
		protocol: 45,
		capabilities: WITH_TERMINALS,
		...over,
	});

/** The agent's folder on the Mac. */
const home = (over: Partial<Workspace> = {}): Workspace =>
	workspace({
		id: "w1",
		location: { type: "folder", path: "/Users/dev/code/example-app" },
		...over,
	});

const OPENED: ActionResult = { status: "ok", data: { status: "terminal_opened", terminal: "t1" } };

/** The composer of an agent whose Mac has terminals, with opening one answering t1. */
const renderShell = ({
	links = [macLink()],
	more = entities({ workspaces: [home()] }),
	...rest
}: Parameters<typeof renderComposer>[0] & { links?: DeviceLink[]; more?: Entity[] } = {}) => {
	const rendered = renderComposer({ links, more, ...rest });
	const openTerminal = vi.fn<DataSource["openTerminal"]>(async () => OPENED);
	Object.assign(rendered.source, { openTerminal });
	return { ...rendered, openTerminal };
};

/** `!` typed in the text box. */
const bang = (textbox: HTMLElement) => fireEvent.keyDown(textbox, { key: "!" });

/** The command field, in shell mode. */
const commandField = () =>
	screen.findByRole<HTMLTextAreaElement>("textbox", { name: "Command to run" });

/** Types a command into the field, the caret after it. */
const typeCommand = (field: HTMLTextAreaElement, command: string) =>
	fireEvent.change(field, { target: { value: command } });

/** A key at the very start of the field. */
const keyAtStart = (field: HTMLTextAreaElement, key: string) => {
	field.setSelectionRange(0, 0);
	fireEvent.keyDown(field, { key });
};

type UserRun = Extract<TimelineItem, { kind: "user_run" }>;

/** A command you ran in this agent's timeline. */
const ran = (id: string, command: string, ordinal: number, over: Partial<UserRun> = {}) =>
	timelineItem(
		{
			kind: "user_run",
			terminal: `t-${id}`,
			command,
			cwd: "~/code/example-app",
			device: DEVICE,
			private: false,
			process: { state: "exited", code: 0, signal: null },
			duration_ms: 100,
			lines: 0,
			output: "",
			output_truncated: false,
			moved: false,
			...over,
		},
		{ id, agent: AGENT, ordinal }
	);

describe("shell mode", () => {
	it("turns the composer into a command on ! at the very start, with Run for Send", async () => {
		renderShell();
		const textbox = await findTextbox();
		expect(screen.getByRole("button", { name: /^Send/ })).toBeInTheDocument();

		bang(textbox);

		const field = await commandField();
		expect(field).toHaveFocus();
		expect(field.value).toBe("");
		expect(screen.getByText("Shell")).toBeInTheDocument();
		expect(textIn(textbox)).toBe("");
		expect(screen.queryByRole("button", { name: /^Send/ })).toBeNull();
		expect(screen.getByRole("button", { name: "Run" })).toBeDisabled();

		typeCommand(field, "pnpm test");
		expect(screen.getByRole("button", { name: "Run" })).toBeEnabled();
	});

	it("says where the command runs and where its output goes", async () => {
		renderShell();
		bang(await findTextbox());
		await commandField();

		expect(screen.getByText(/^Runs in/)).toHaveTextContent("Runs in ~/code/example-app on Mac");
		expect(screen.getByText("Output goes with your next message")).toBeInTheDocument();
		expect(screen.getByText(/keeps it private$/)).toHaveTextContent("!! keeps it private");
		expect(screen.getByText(/earlier commands$/)).toHaveTextContent("↑ earlier commands");
	});

	it("names the container an agent in one runs in", async () => {
		renderShell({
			on: REMOTE,
			links: [macLink({ id: "devbox", name: "devbox", device: REMOTE })],
			more: entities({
				workspaces: [
					home({
						device: REMOTE,
						location: {
							type: "container",
							instance: "example-app",
							path: "/home/dev/code/example-app",
						},
					}),
				],
			}),
		});
		bang(await findTextbox());
		await commandField();

		expect(screen.getByText(/^Runs in/)).toHaveTextContent(
			"Runs in ~/code/example-app in example-app on devbox"
		);
	});

	it("goes private on a second ! and says the agent won't see the output", async () => {
		renderShell();
		bang(await findTextbox());
		const field = await commandField();
		typeCommand(field, "cat .env");

		keyAtStart(field, "!");

		const secret = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});
		expect(secret.value).toBe("cat .env");
		expect(screen.getByText("Private")).toBeInTheDocument();
		expect(screen.getByText("The agent won't see the output")).toBeInTheDocument();
		expect(screen.queryByText("Output goes with your next message")).toBeNull();
	});

	it("steps back one ! at a time on Backspace, and keeps the command as the message", async () => {
		renderShell();
		const textbox = await findTextbox();
		bang(textbox);
		const field = await commandField();
		typeCommand(field, "pnpm lint");
		keyAtStart(field, "!");
		const secret = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});

		keyAtStart(secret, "Backspace");
		keyAtStart(await commandField(), "Backspace");

		await waitFor(() => expect(textIn(textbox)).toBe("pnpm lint"));
		expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
		expect(screen.getByRole("button", { name: /^Send/ })).toBeInTheDocument();
	});

	it("takes one line already typed as the command", async () => {
		renderShell();
		const textbox = await findTextbox();
		await type(textbox, "git status --short");
		act(() => {
			editorOf(textbox).commands.setTextSelection(1);
		});

		bang(textbox);

		expect((await commandField()).value).toBe("git status --short");
		expect(textIn(textbox)).toBe("");
	});

	it("leaves a ! later in the text alone", async () => {
		renderShell();
		const textbox = await findTextbox();
		await type(textbox, "Ship it");

		bang(textbox);

		await act(async () => {});
		expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
	});

	it("doesn't switch for a paste that starts with !", async () => {
		renderShell();
		const textbox = await findTextbox();

		fireEvent.paste(textbox, { clipboardData: clipboard({ text: "!important: read this" }) });

		expect(textIn(textbox)).toBe("!important: read this");
		expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
	});

	it("has no shell mode while the device's maxd has no terminals", async () => {
		for (const links of [
			[macLink({ protocol: 44 })],
			[macLink({ capabilities: { transfers: true, tunnels: true, workspaces: true } })],
		]) {
			renderShell({ links });
			const textbox = await findTextbox();
			act(() => putCommand(AGENT, "pnpm test", false));

			bang(textbox);

			await act(async () => {});
			expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
			expect(screen.getByRole("button", { name: /^Send/ })).toBeInTheDocument();
			cleanup();
			clearKept();
		}
	});
});

describe("running a command", () => {
	it("opens a terminal for it, 24 rows high, then clears and holds its output pill", async () => {
		const { openTerminal, source } = renderShell();
		const textbox = await findTextbox();
		bang(textbox);
		typeCommand(await commandField(), "pnpm test");

		fireEvent.click(screen.getByRole("button", { name: "Run" }));

		await waitFor(() =>
			expect(openTerminal).toHaveBeenCalledWith(
				AGENT,
				{ type: "run", command: "pnpm test", private: false },
				{ cols: 20, rows: RUN_ROWS }
			)
		);
		await waitFor(() =>
			expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull()
		);
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("starts the next command on ! after the last one's pill, and keeps the pill", async () => {
		renderShell();
		const textbox = await findTextbox();
		bang(textbox);
		typeCommand(await commandField(), "pnpm test");
		fireEvent.click(screen.getByRole("button", { name: "Run" }));
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));
		act(() => {
			editorOf(textbox).commands.focus("end");
		});

		bang(textbox);

		expect((await commandField()).value).toBe("");
		expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]);
	});

	it("leaves a ! after text and a pill alone", async () => {
		renderShell();
		const textbox = await findTextbox();
		await type(textbox, "Why did ");
		act(() => putCommand(AGENT, "pnpm test", false));
		const field = await commandField();
		fireEvent.keyDown(field, { key: "Enter" });
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));
		act(() => {
			editorOf(textbox).commands.focus("end");
		});

		bang(textbox);

		await act(async () => {});
		expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
	});

	it("runs on Enter, and a private command leaves no pill", async () => {
		const { openTerminal } = renderShell();
		const textbox = await findTextbox();
		bang(textbox);
		const field = await commandField();
		typeCommand(field, "cat .env");
		keyAtStart(field, "!");
		const secret = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});

		fireEvent.keyDown(secret, { key: "Enter" });

		await waitFor(() =>
			expect(openTerminal).toHaveBeenCalledWith(
				AGENT,
				{ type: "run", command: "cat .env", private: true },
				{ cols: 20, rows: RUN_ROWS }
			)
		);
		await waitFor(() => expect(screen.queryByRole("textbox", { name: /^Command/ })).toBeNull());
		expect(pillsIn(textbox)).toEqual([]);
	});

	it("keeps the command and says why when it doesn't run", async () => {
		const { openTerminal } = renderShell();
		openTerminal.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: NO_TERMINALS },
		});
		bang(await findTextbox());
		const field = await commandField();
		typeCommand(field, "pnpm test");

		fireEvent.keyDown(field, { key: "Enter" });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't run the command. Terminals aren't available on this maxd yet."
		);
		expect((await commandField()).value).toBe("pnpm test");
	});

	it("sizes the terminal to the card", () => {
		expect(runSize(826, 8)).toEqual({ cols: 100, rows: RUN_ROWS });
		expect(runSize(0, 8)).toEqual({ cols: 20, rows: RUN_ROWS });
	});

	it("takes a command from a code block's Run menu, now or once it opens", async () => {
		act(() => putCommand(AGENT, "pnpm dev", true));
		renderShell();
		await findTextbox();
		const secret = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});
		expect(secret.value).toBe("pnpm dev");

		act(() => putCommand(AGENT, "pnpm build", false));

		expect((await commandField()).value).toBe("pnpm build");
	});
});

describe("earlier commands", () => {
	const sent = [
		ran("r1", "pnpm lint", 1),
		timelineItem(
			{ kind: "assistant_message", text: "Run pnpm test next.", streaming: false },
			{ id: "m2", agent: AGENT, ordinal: 2 }
		),
		ran("r3", "pnpm test", 3),
		ran("r4", "pnpm lint", 4),
	];

	it("walks the commands you ran here, newest first, each once, then back to what you typed", async () => {
		renderShell({ sent });
		bang(await findTextbox());
		const field = await commandField();
		typeCommand(field, "pn");

		fireEvent.keyDown(field, { key: "ArrowUp" });
		expect(field.value).toBe("pnpm lint");
		fireEvent.keyDown(field, { key: "ArrowUp" });
		expect(field.value).toBe("pnpm test");
		fireEvent.keyDown(field, { key: "ArrowUp" });
		expect(field.value).toBe("pnpm test");

		fireEvent.keyDown(field, { key: "ArrowDown" });
		expect(field.value).toBe("pnpm lint");
		fireEvent.keyDown(field, { key: "ArrowDown" });
		expect(field.value).toBe("pn");
	});
});

describe("output going with a message", () => {
	const lines = (count: number) =>
		Array.from({ length: count }, (_, index) => `line ${index + 1}`).join("\n");

	const pending = startedOutput({
		command: "pnpm test",
		deviceName: "Mac",
		cwd: "~/code/example-app",
		terminal: "t-r1",
		at: 1_000,
	});

	it("takes the last 200 lines of an ended command when the message goes", async () => {
		const output = `\u001b[31m${lines(250)}\u001b[0m\n`;
		const { source } = renderShell({
			sent: [
				ran("r1", "pnpm test", 1, {
					process: { state: "exited", code: 1, signal: null },
					duration_ms: 4200,
					lines: 250,
					output,
				}),
			],
		});
		const textbox = await findTextbox();
		act(() => quoteInbox.send(AGENT, pending, true));
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, exit 1"]));
		await type(textbox, "Why does this fail?");

		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		const [, input] = source.sendInput.mock.calls[0]!;
		expect(input.text).toBe(`${MARK} Why does this fail?`);
		expect(input.refs).toEqual([
			expect.objectContaining({
				type: "output",
				command: "pnpm test",
				running: false,
				exit_code: 1,
				duration_ms: 4200,
				lines_total: 250,
				text: lines(250).split("\n").slice(50).join("\n"),
			}),
		]);
	});

	it("takes the live terminal's last lines while the command runs", async () => {
		seam.live = { text: "Compiled\nwatching for changes", total: 41 };
		const { source } = renderShell({
			more: [
				...entities({ workspaces: [home()] }),
				{
					entity: "terminal",
					id: "t-r1",
					agent: AGENT,
					device: DEVICE,
					kind: { type: "run", command: "pnpm test", private: false },
					moved: false,
					title: "pnpm test",
					cwd: "~/code/example-app",
					status: { state: "running" },
					password_prompt: false,
					started_at: 1_000,
					cols: 100,
					rows: RUN_ROWS,
				},
			],
		});
		const textbox = await findTextbox();
		act(() => quoteInbox.send(AGENT, pending, true));
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));

		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		const [, input] = source.sendInput.mock.calls[0]!;
		expect(input.refs).toEqual([
			expect.objectContaining({
				type: "output",
				running: true,
				lines_total: 41,
				text: "Compiled\nwatching for changes",
			}),
		]);
	});
});

describe("on a device with sessions", () => {
	const FOLDER = "/Users/dev/code/example-app";

	const RAN: ActionResult = {
		status: "ok",
		data: { status: "session_ran", terminal: "s1", item: "card-1", block: 1 },
	};

	/** The composer of an agent whose Mac's maxd has sessions. */
	const renderSession = (over: Parameters<typeof renderShell>[0] = {}) => {
		const rendered = renderShell({ links: [macLink({ protocol: 48 })], ...over });
		const sessionRun = vi.fn<DataSource["sessionRun"]>(async () => RAN);
		const sessionCd = vi.fn<DataSource["sessionCd"]>(async () => OPENED);
		const listFolder = vi.fn<DataSource["listFolder"]>(async (_device, path) => ({
			status: "ok",
			data: {
				status: "listed",
				listing: { path, home: "/Users/dev", exists: true, entries: [], truncated: false },
			},
		}));
		Object.assign(rendered.source, { sessionRun, sessionCd, listFolder });
		return { ...rendered, sessionRun, sessionCd, listFolder };
	};

	const chip = () => screen.getByRole("button", { name: "Pick the folder the session runs in" });

	/** The session's terminal, in the folder it started in. */
	const sessionTerminal: Entity = {
		entity: "terminal",
		id: "s1",
		agent: AGENT,
		device: DEVICE,
		kind: { type: "session" },
		moved: false,
		title: "zsh",
		cwd: FOLDER,
		status: { state: "running" },
		busy: false,
		marks: true,
		password_prompt: false,
		started_at: Date.now() - 60_000,
		cols: 100,
		rows: 24,
	};

	/** The open card, its two commands having moved the session to apps/web. */
	const openCard = timelineItem(
		{
			kind: "user_session",
			terminal: "s1",
			device: DEVICE,
			blocks: [1, 2].map((id) => ({
				id,
				command: id === 1 ? "cd apps/web" : "pnpm install",
				cwd: id === 1 ? FOLDER : `${FOLDER}/apps/web`,
				cwd_after: id === 1 ? `${FOLDER}/apps/web` : null,
				private: false,
				process: { state: "exited", code: 0, signal: null } as const,
				started_at: Date.now() - 30_000,
				duration_ms: 100,
				lines: 0,
				output: "",
				output_truncated: false,
				secret_prompts: 0,
				start: null,
			})),
			sealed: false,
			sent: false,
		},
		{ id: "card-1", agent: AGENT, ordinal: 1 }
	);

	it("shows the agent's folder in a chip, the device, and the session's keys", async () => {
		renderSession();
		bang(await findTextbox());
		await commandField();

		expect(chip()).toHaveTextContent("~/code/example-app");
		expect(chip().parentElement).toHaveTextContent(/^~\/code\/example-appon Mac$/);
		expect(screen.getByText(/run in the session$/)).toHaveTextContent("↩ run in the session");
		expect(screen.getByText(/run in a fresh shell$/)).toHaveTextContent(
			"⌥↩ run in a fresh shell"
		);
		expect(screen.getByText(/keeps this one private$/)).toHaveTextContent(
			"!! keeps this one private"
		);
		expect(screen.queryByText(/^Runs in/)).toBeNull();
		expect(screen.queryByText(/^Session · /)).toBeNull();
	});

	it("runs on ↩ in the session, and on ⌥↩ in a fresh shell", async () => {
		const { sessionRun, openTerminal } = renderSession();
		const textbox = await findTextbox();
		bang(textbox);
		typeCommand(await commandField(), "pnpm test");

		fireEvent.keyDown(await commandField(), { key: "Enter" });

		await waitFor(() =>
			expect(sessionRun).toHaveBeenCalledExactlyOnceWith(AGENT, "pnpm test", {
				private: false,
				fresh: false,
				size: { cols: 20, rows: RUN_ROWS },
			})
		);
		await waitFor(() =>
			expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull()
		);
		// The session's pill stands for its commands, so none of the command's own.
		expect(pillsIn(textbox)).toEqual([]);

		bang(textbox);
		typeCommand(await commandField(), "pnpm dev");
		fireEvent.keyDown(await commandField(), { key: "Enter", altKey: true });

		await waitFor(() => expect(sessionRun).toHaveBeenCalledTimes(2));
		expect(sessionRun).toHaveBeenLastCalledWith(AGENT, "pnpm dev", {
			private: false,
			fresh: true,
			size: { cols: 20, rows: RUN_ROWS },
		});
		expect(openTerminal).not.toHaveBeenCalled();
	});

	it("sends a private command as private", async () => {
		const { sessionRun } = renderSession();
		bang(await findTextbox());
		const field = await commandField();
		typeCommand(field, "cat .env");
		keyAtStart(field, "!");
		const secret = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});
		expect(screen.getByText("The agent won't see the output")).toBeInTheDocument();

		fireEvent.keyDown(secret, { key: "Enter" });

		await waitFor(() =>
			expect(sessionRun).toHaveBeenCalledWith(
				AGENT,
				"cat .env",
				expect.objectContaining({ private: true, fresh: false })
			)
		);
	});

	it("holds a fresh shell's output pill, as a command of its own does", async () => {
		const { sessionRun } = renderSession();
		sessionRun.mockResolvedValue(OPENED);
		const textbox = await findTextbox();
		bang(textbox);
		typeCommand(await commandField(), "pnpm test");

		fireEvent.keyDown(await commandField(), { key: "Enter", altKey: true });

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));
	});

	it("keeps the command and says why when the session doesn't take it", async () => {
		const { sessionRun } = renderSession();
		sessionRun.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "Sessions aren't ready yet." },
		});
		bang(await findTextbox());
		typeCommand(await commandField(), "pnpm test");

		fireEvent.keyDown(await commandField(), { key: "Enter" });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't run the command. Sessions aren't ready yet."
		);
		expect((await commandField()).value).toBe("pnpm test");
	});

	it("counts the open card's commands, and shows the folder the session is in now", async () => {
		renderSession({
			sent: [openCard],
			more: [...entities({ workspaces: [home()] }), sessionTerminal],
		});
		bang(await findTextbox());
		await commandField();

		expect(chip()).toHaveTextContent("~/code/example-app/apps/web");
		expect(screen.getByText(/^Session · /)).toHaveTextContent("Session · 2 commands");
	});

	it("moves the session to the folder you choose on the agent's device", async () => {
		const { sessionCd, listFolder } = renderSession();
		bang(await findTextbox());
		await commandField();

		fireEvent.click(chip());

		const dialog = await screen.findByRole("dialog", { name: "Choose a folder on Mac" });
		await waitFor(() => expect(listFolder).toHaveBeenCalled());
		expect(listFolder.mock.calls[0]!.slice(0, 2)).toEqual([DEVICE, FOLDER]);
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose" }));

		await waitFor(() => expect(sessionCd).toHaveBeenCalledExactlyOnceWith(AGENT, FOLDER));
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("says why the session didn't move", async () => {
		const { sessionCd } = renderSession();
		sessionCd.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "A command still runs in the session" },
		});
		bang(await findTextbox());
		await commandField();
		fireEvent.click(chip());
		const dialog = await screen.findByRole("dialog", { name: "Choose a folder on Mac" });

		await waitFor(() =>
			expect(within(dialog).getByRole("button", { name: "Choose" })).toBeEnabled()
		);
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose" }));

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't move the session. A command still runs in the session."
		);
	});

	it("stays as before, with no chip, on a maxd before sessions", async () => {
		const { sessionRun, openTerminal } = renderSession({ links: [macLink({ protocol: 47 })] });
		bang(await findTextbox());
		const field = await commandField();

		expect(
			screen.queryByRole("button", { name: "Pick the folder the session runs in" })
		).toBeNull();
		expect(screen.getByText(/^Runs in/)).toHaveTextContent("Runs in ~/code/example-app on Mac");
		expect(screen.queryByText(/run in a fresh shell$/)).toBeNull();
		typeCommand(field, "pnpm test");

		fireEvent.keyDown(field, { key: "Enter" });

		await waitFor(() => expect(openTerminal).toHaveBeenCalledOnce());
		expect(sessionRun).not.toHaveBeenCalled();
	});
});
