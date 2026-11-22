// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { DeviceLink, Entity, Terminal, TimelineItem, Workspace } from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { startedOutput } from "@maxhq/shared/terminal/output";
import { entities, link, workspace } from "@maxhq/shared/test/build";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { Composer } from "@/composer/composer";
import { ComposerNativeProvider } from "@/composer/native";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { forgetNewest, startRun } from "@/composer/run-command";
import { draftsOf } from "@/composer/saved-drafts";
import { settingsOf } from "@/settings/store";
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
	textIn,
} from "@/composer/testing";
import type { LiveTerminalProps } from "@/terminal";

import { IGNORED_AFTER } from "./run-input";
import { UserRunCard } from "./user-run";

// The terminal module, as the card reaches it.
const seam = vi.hoisted(() => ({
	sendInput: vi.fn<(device: string, terminal: string, data: string) => void>(),
	text: vi.fn<(terminal: string, lines: number) => { text: string; total: number } | null>(
		() => null
	),
	pasteIntoDrawer: vi.fn<(agent: string, text: string) => void>(),
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
		text: seam.text,
		runningText: () => null,
		cellSize: () => ({ width: 8, height: 17 }),
		pasteIntoDrawer: seam.pasteIntoDrawer,
		showInDrawer: seam.showInDrawer,
		openAgentShell: () => {},
	}),
}));

beforeAll(installBrowserFakes);
beforeEach(() => {
	seam.sendInput.mockReset();
	seam.text.mockReset();
	seam.text.mockReturnValue(null);
	seam.pasteIntoDrawer.mockReset();
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

type UserRun = Extract<TimelineItem, { kind: "user_run" }>;

const WITH_TERMINALS = { transfers: true, tunnels: true, workspaces: true, terminals: true };

const macLink = (): DeviceLink =>
	link({ id: "mac", name: "Mac", device: DEVICE, protocol: 45, capabilities: WITH_TERMINALS });

const home = (): Workspace =>
	workspace({ id: "w1", location: { type: "folder", path: "/Users/dev/code/example-app" } });

const FAILED_OUTPUT = "\u001b[31mFAIL\u001b[0m src/cache.test.ts\nTests 1 failed\ndone\n";

const RUNNING = { state: "running" } as const;

/** A command you ran, failed with exit 1 after 4.2 seconds unless a test says otherwise. */
const ran = (over: Partial<UserRun> = {}, status: TimelineItem["status"] = "failed"): UserRun =>
	timelineItem(
		{
			kind: "user_run",
			terminal: "t1",
			command: "pnpm test",
			cwd: "~/code/example-app",
			device: DEVICE,
			private: false,
			process: { state: "exited", code: 1, signal: null },
			duration_ms: 4200,
			lines: 3,
			output: FAILED_OUTPUT,
			output_truncated: false,
			moved: false,
			...over,
		},
		{ id: "r1", agent: AGENT, ordinal: 1, status, started_at: Date.now() - 8000 }
	) as UserRun;

/** A command that still runs. */
const running = (over: Partial<UserRun> = {}) =>
	ran({ process: RUNNING, duration_ms: null, lines: 0, output: "", ...over }, "running");

/** The terminal a command runs in, as the view holds it. */
const terminalOf = (over: Partial<Terminal> = {}): Entity => ({
	entity: "terminal",
	id: "t1",
	agent: AGENT,
	device: DEVICE,
	kind: { type: "run", command: "pnpm test", private: false },
	moved: false,
	title: "pnpm test",
	cwd: "~/code/example-app",
	status: RUNNING,
	password_prompt: false,
	started_at: Date.now() - 8000,
	cols: 100,
	rows: 24,
	...over,
});

const OPENED: ActionResult = { status: "ok", data: { status: "terminal_opened", terminal: "t2" } };

/** The card in its agent's timeline, with the agent's composer below it. */
const renderCard = (
	item: UserRun,
	{ terminal, readOnly = false }: { terminal?: Entity; readOnly?: boolean } = {}
) => {
	const fake = fakeSource({
		agent: heldAgent(),
		links: [macLink()],
		more: [...entities({ workspaces: [home()] }), ...(terminal ? [terminal] : [])],
	});
	const calls = {
		openTerminal: vi.fn<DataSource["openTerminal"]>(async () => OPENED),
		interruptTerminal: vi.fn<DataSource["interruptTerminal"]>(async () => DONE),
		closeTerminal: vi.fn<DataSource["closeTerminal"]>(async () => DONE),
		moveTerminal: vi.fn<DataSource["moveTerminal"]>(async () => DONE),
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
	let services: Services | null = null;
	const Probe = () => {
		services = useServices();
		return null;
	};
	const view = render(
		<ServicesProvider source={fake.source}>
			<Probe />
			<RowProvider value={row}>
				<UserRunCard item={item} />
			</RowProvider>
			<ComposerNativeProvider value={fakeNative().native}>
				<Composer agent={AGENT} device={DEVICE} queued={[]} timeline={timeline} />
			</ComposerNativeProvider>
			<ConfirmHost />
		</ServicesProvider>
	);
	return {
		...fake,
		...calls,
		timeline,
		container: view.container,
		services: (): Services => {
			if (!services) throw new Error("The services haven't started");
			return services;
		},
	};
};

/** The card, once the view knows the device's name. */
const findCard = async (command = "pnpm test") => {
	const card = await screen.findByRole("region", { name: `You ran ${command}` });
	await within(card).findByText(/ · Mac$/);
	return card;
};

/** The card's own button, by its name. */
const button = (card: HTMLElement, name: string | RegExp) =>
	within(card).getByRole("button", { name });

/**
 * Whether `secret` is anywhere in `value`: its fields, items, keys and
 * entries, all the way down, but not in the page, which a test reads on
 * its own, and not in functions.
 */
const holds = (value: unknown, secret: string, seen = new Set<unknown>()): boolean => {
	if (typeof value === "string") return value.includes(secret);
	if (value === null || typeof value !== "object") return false;
	if (seen.has(value) || value === globalThis || value instanceof Node) return false;
	seen.add(value);
	try {
		if (value instanceof Map)
			return [...value].some(
				([key, each]) => holds(key, secret, seen) || holds(each, secret, seen)
			);
		if (value instanceof Set) return [...value].some((each) => holds(each, secret, seen));
		return Object.values(value).some((each) => holds(each, secret, seen));
	} catch {
		return false;
	}
};

/** The root of React's tree for a container: every component's props, state and hooks hang off it. */
const reactRoot = (container: HTMLElement): unknown => {
	const key = Object.keys(container).find((name) => name.startsWith("__reactContainer$"));
	if (!key) throw new Error("No React root in the container");
	return (container as unknown as Record<string, unknown>)[key];
};

/** Everywhere outside the field itself that holds `secret`. */
const placesHolding = (
	card: ReturnType<typeof renderCard>,
	secret: string,
	logs: readonly { mock: { calls: unknown[][] } }[]
): string[] => {
	const services = card.services();
	const page = [
		document.documentElement.outerHTML,
		...[...document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input, textarea")]
			.filter((field) => field !== document.activeElement)
			.map((field) => field.value),
	];
	const storage = [localStorage, sessionStorage].flatMap((store) =>
		Object.keys(store).map((key) => `${key}=${store.getItem(key)}`)
	);
	const places: [string, unknown][] = [
		["React's state and props", reactRoot(card.container)],
		["the view", services.view.getState()],
		["the window's state", services.ui.getState()],
		["the timeline", card.timeline.getState()],
		["the draft", draftsOf(services).get(AGENT)],
		["the page", page],
		["storage", storage],
		["a log", logs.map((log) => log.mock.calls)],
	];
	return places.filter(([, value]) => holds(value, secret)).map(([name]) => name);
};

describe("a running command", () => {
	it("shows its live terminal, how long it has run, and where", async () => {
		renderCard(running(), { terminal: terminalOf() });
		const card = await findCard();

		expect(card).toHaveAttribute("data-user-run", "running");
		expect(card).toHaveTextContent(/You ran pnpm test/);
		expect(card).toHaveTextContent(/Running 0:0[89]/);
		expect(within(card).getByText("~/code/example-app · Mac")).toBeInTheDocument();
		expect(within(card).getByTestId("live-terminal")).toBeInTheDocument();
		expect(seam.live.at(-1)).toEqual({
			device: DEVICE,
			terminal: "t1",
			maxRows: 24,
			interactive: false,
		});
		expect(button(card, "Stop")).toHaveTextContent("Stop⌃C");
		expect(button(card, "Open in terminal")).toBeInTheDocument();
		expect(
			within(card).getByPlaceholderText("Type to send input to the command")
		).toBeInTheDocument();
		expect(within(card).queryByRole("button", { name: "Ask about this" })).toBeNull();
	});

	it("stops it with Ctrl-C, and offers Force stop once it ignored that", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		try {
			const python = terminalOf({
				kind: { type: "run", command: "python3", private: false },
			});
			const { interruptTerminal, closeTerminal } = renderCard(
				running({ command: "python3" }),
				{
					terminal: python,
				}
			);
			const card = await findCard("python3");

			fireEvent.click(button(card, "Stop"));
			await waitFor(() => expect(interruptTerminal).toHaveBeenCalledWith("t1"));
			// Stop stays Stop, and another press types Control-C again.
			expect(button(card, "Stop")).toHaveTextContent("Stop⌃C");
			fireEvent.click(button(card, "Stop"));
			await waitFor(() => expect(interruptTerminal).toHaveBeenCalledTimes(2));
			expect(within(card).queryByRole("button", { name: "Force stop" })).toBeNull();
			expect(card).not.toHaveTextContent("ignored Stop");

			await act(async () => {
				await vi.advanceTimersByTimeAsync(IGNORED_AFTER);
			});

			expect(card).toHaveTextContent("python3 ignored Stop");
			expect(button(card, "Stop")).toHaveTextContent(/^Stop$/);
			fireEvent.click(button(card, "Force stop"));
			await waitFor(() => expect(closeTerminal).toHaveBeenCalledExactlyOnceWith("t1", true));
			expect(interruptTerminal).toHaveBeenCalledTimes(2);
		} finally {
			vi.useRealTimers();
		}
	});

	it("says nothing was ignored when it ends soon after Stop", async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		try {
			const rendered = renderCard(running(), { terminal: terminalOf() });
			const card = await findCard();

			fireEvent.click(button(card, "Stop"));
			await waitFor(() => expect(rendered.interruptTerminal).toHaveBeenCalledWith("t1"));
			rendered.upsert([
				terminalOf({ status: { state: "exited", code: null, signal: 2, stopped: true } }),
			]);
			await act(async () => {
				await vi.advanceTimersByTimeAsync(IGNORED_AFTER);
			});

			expect(card).not.toHaveTextContent("ignored Stop");
			expect(within(card).queryByRole("button", { name: "Force stop" })).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});

	it("moves it to the terminal panel and shows its tab there", async () => {
		const { moveTerminal } = renderCard(running(), { terminal: terminalOf() });
		const card = await findCard();

		fireEvent.click(button(card, "Open in terminal"));

		await waitFor(() => expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "t1"));
		expect(moveTerminal).toHaveBeenCalledWith("t1");
	});

	it("says why when moving it to the terminal doesn't go", async () => {
		const { moveTerminal } = renderCard(running(), { terminal: terminalOf() });
		const card = await findCard();
		moveTerminal.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "The terminal has ended" },
		});

		fireEvent.click(button(card, "Open in terminal"));

		expect(await within(card).findByRole("alert")).toHaveTextContent(
			"Couldn't move the command to the terminal. The terminal has ended."
		);
		expect(moveTerminal).toHaveBeenCalledWith("t1");
		expect(seam.showInDrawer).not.toHaveBeenCalled();
	});

	it("types a line into it, and Ctrl-C in the line stops it", async () => {
		const { interruptTerminal } = renderCard(running(), { terminal: terminalOf() });
		const card = await findCard();
		const line = within(card).getByRole<HTMLInputElement>("textbox", {
			name: "Input for the command",
		});

		fireEvent.change(line, { target: { value: "yes" } });
		fireEvent.keyDown(line, { key: "Enter" });

		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "t1", "yes\r", undefined);
		expect(line.value).toBe("");

		fireEvent.keyDown(line, { key: "c", ctrlKey: true });
		await waitFor(() => expect(interruptTerminal).toHaveBeenCalledWith("t1"));
	});

	it("keeps a line you type in it nowhere once it went", async () => {
		const logs = (["log", "info", "warn", "error", "debug"] as const).map((method) =>
			vi.spyOn(console, method)
		);
		const rendered = renderCard(running(), { terminal: terminalOf() });
		const card = await findCard();
		const line = within(card).getByRole<HTMLInputElement>("textbox", {
			name: "Input for the command",
		});
		const typed = "token-for-the-prompt-81f3";

		fireEvent.change(line, { target: { value: typed } });
		fireEvent.keyDown(line, { key: "Enter" });

		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(
			DEVICE,
			"t1",
			`${typed}\r`,
			undefined
		);
		seam.sendInput.mockClear();
		line.blur();
		expect(placesHolding(rendered, typed, logs)).toEqual([]);
	});
});

describe("a command waiting for you", () => {
	/** The terminal `ssh devbox` runs in, at its host key prompt or not. */
	const sshTerminal = (waiting: boolean) =>
		terminalOf({
			kind: { type: "run", command: "ssh devbox", private: false },
			waiting,
			choices: waiting ? ["yes", "no"] : [],
		});
	const ssh = (waiting = true) =>
		renderCard(running({ command: "ssh devbox" }), { terminal: sshTerminal(waiting) });

	/** Puts focus in the composer's text box, and waits until it's there. */
	const focusComposer = async () => {
		const message = await findTextbox();
		act(() => {
			editorOf(message).commands.focus();
		});
		await waitFor(() => expect(message).toHaveFocus());
		return message;
	};

	it("says so in place of its clock, and offers the prompt's answers", async () => {
		ssh();
		const card = await findCard("ssh devbox");

		expect(card).toHaveAttribute("data-waiting", "true");
		expect(card).toHaveTextContent("Waiting for you");
		expect(card).not.toHaveTextContent(/Running \d/);
		const line = within(card).getByRole("textbox", { name: "Input for the command" });
		expect(line).toHaveAttribute("placeholder", "Or type a reply");

		fireEvent.click(button(card, "Answer yes"));
		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "t1", "yes\r", undefined);
		fireEvent.click(button(card, "Answer no"));
		expect(seam.sendInput).toHaveBeenLastCalledWith(DEVICE, "t1", "no\r", undefined);
	});

	it("drops the answers and the chip once it goes on", async () => {
		const rendered = ssh();
		const card = await findCard("ssh devbox");
		rendered.upsert([sshTerminal(false)]);

		await waitFor(() => expect(card).toHaveTextContent(/Running \d/));
		expect(card).not.toHaveAttribute("data-waiting");
		expect(within(card).queryByRole("button", { name: "Answer yes" })).toBeNull();
	});

	it("takes focus from an empty composer once, as the wait starts", async () => {
		const rendered = ssh(false);
		const card = await findCard("ssh devbox");
		const message = await focusComposer();

		rendered.upsert([sshTerminal(true)]);
		const line = within(card).getByRole("textbox", { name: "Input for the command" });
		await waitFor(() => expect(line).toHaveFocus());

		// The same wait, told again, leaves focus where you put it.
		await focusComposer();
		rendered.upsert([sshTerminal(true)]);
		expect(message).toHaveFocus();
	});

	it("never takes focus while you type in the composer", async () => {
		const rendered = ssh(false);
		const card = await findCard("ssh devbox");
		const message = await focusComposer();
		act(() => {
			editorOf(message).commands.insertContent("half a thought");
		});

		rendered.upsert([sshTerminal(true)]);
		await within(card).findByRole("button", { name: "Answer yes" });
		expect(message).toHaveFocus();
		expect(textIn(message)).toBe("half a thought");
	});
});

describe("a command asking for a password", () => {
	/** The terminal `sudo apt upgrade` runs in, asking for a password or not. */
	const sudoTerminal = (asking: boolean) =>
		terminalOf({
			kind: { type: "run", command: "sudo apt upgrade", private: false },
			password_prompt: asking,
		});
	const sudo = () =>
		renderCard(running({ command: "sudo apt upgrade" }), { terminal: sudoTerminal(true) });

	/** Starts `sudo apt upgrade` in `terminal` from this window, as the composer does. */
	const startHere = async (terminal: string) => {
		const opened: ActionResult = {
			status: "ok",
			data: { status: "terminal_opened", terminal },
		};
		const started = await startRun(
			{ openTerminal: async () => opened },
			() => {
				throw new Error("A private run doesn't read the view");
			},
			{
				agent: AGENT,
				command: "sudo apt upgrade",
				private: true,
				size: { cols: 80, rows: 24 },
			}
		);
		expect(started).toEqual({ terminal });
	};

	/** Puts focus in the composer's text box, and waits until it's there. */
	const focusComposer = async () => {
		const message = await findTextbox();
		act(() => {
			editorOf(message).commands.focus();
		});
		await waitFor(() => expect(message).toHaveFocus());
		return message;
	};

	it("takes focus from the composer the first time a command run here asks, and gives it back after", async () => {
		await startHere("t1");
		const rendered = renderCard(running({ command: "sudo apt upgrade" }), {
			terminal: sudoTerminal(false),
		});
		const card = await findCard("sudo apt upgrade");
		const message = await focusComposer();

		rendered.upsert([sudoTerminal(true)]);
		const field = await within(card).findByLabelText<HTMLInputElement>(
			"Password for sudo on Mac"
		);
		expect(field).toHaveFocus();

		// A wrong password asks again, in a fresh field that keeps focus.
		fireEvent.change(field, { target: { value: "wrong" } });
		fireEvent.keyDown(field, { key: "Enter" });
		const fresh = within(card).getByLabelText<HTMLInputElement>("Password for sudo on Mac");
		expect(fresh).not.toBe(field);
		await waitFor(() => expect(fresh).toHaveFocus());

		rendered.upsert([sudoTerminal(false)]);
		expect(within(card).queryByLabelText("Password for sudo on Mac")).toBeNull();
		expect(message).toHaveFocus();
		expect(textIn(message)).toBe("");

		// A later ask leaves focus where it is.
		rendered.upsert([sudoTerminal(true)]);
		await within(card).findByLabelText("Password for sudo on Mac");
		expect(message).toHaveFocus();
	});

	it("leaves focus alone for a command run elsewhere, or while you type in another field", async () => {
		const other = document.createElement("input");
		document.body.append(other);
		try {
			const elsewhere = renderCard(running({ command: "sudo apt upgrade" }), {
				terminal: sudoTerminal(false),
			});
			const card = await findCard("sudo apt upgrade");
			const message = await focusComposer();
			elsewhere.upsert([sudoTerminal(true)]);
			await within(card).findByLabelText("Password for sudo on Mac");
			expect(message).toHaveFocus();
			cleanup();

			await startHere("t1");
			const here = renderCard(running({ command: "sudo apt upgrade" }), {
				terminal: sudoTerminal(false),
			});
			const again = await findCard("sudo apt upgrade");
			act(() => other.focus());
			here.upsert([sudoTerminal(true)]);
			await within(again).findByLabelText("Password for sudo on Mac");
			expect(other).toHaveFocus();
		} finally {
			other.remove();
		}
	});

	it("swaps the input line for a secure field, with only Stop", async () => {
		sudo();
		const card = await findCard("sudo apt upgrade");
		const field = await within(card).findByLabelText<HTMLInputElement>(
			"Password for sudo on Mac"
		);

		expect(card).toHaveAttribute("data-user-run", "password");
		expect(card).toHaveTextContent("Waiting for your password");
		expect(field).toHaveAttribute("type", "password");
		expect(field).toHaveAttribute("autocomplete", "off");
		expect(field).not.toHaveAttribute("value");
		expect(field.closest("form")).toBeNull();
		expect(card).toHaveTextContent(
			"Goes straight to this command over the encrypted link. MaxHQ doesn't keep or log it, and no agent can see it or type into this command."
		);
		expect(
			within(card)
				.getAllByRole("button")
				.map((each) => each.textContent)
		).toEqual(["Stop", "Send"]);
		expect(within(card).queryByPlaceholderText("Type to send input to the command")).toBeNull();
		expect(within(card).queryByRole("button", { name: "More for this command" })).toBeNull();
	});

	it("sends the password straight to the command and keeps it nowhere: no state, store, draft, log or page", async () => {
		const logs = (["log", "info", "warn", "error", "debug"] as const).map((method) =>
			vi.spyOn(console, method)
		);
		const rendered = sudo();
		const card = await findCard("sudo apt upgrade");
		const field = await within(card).findByLabelText<HTMLInputElement>(
			"Password for sudo on Mac"
		);
		const secret = "correct-horse-battery-staple-7d2c";

		field.focus();
		fireEvent.change(field, { target: { value: secret } });
		// While you type, only the field itself has it.
		expect(placesHolding(rendered, secret, logs)).toEqual([]);

		fireEvent.keyDown(field, { key: "Enter" });

		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "t1", `${secret}\r`, {
			secret: true,
		});
		seam.sendInput.mockClear();
		// A new, empty field takes its place, so no undo brings it back.
		const fresh = within(card).getByLabelText<HTMLInputElement>("Password for sudo on Mac");
		expect(fresh).not.toBe(field);
		expect(field.isConnected).toBe(false);
		expect(field.value).toBe("");
		expect(fresh.value).toBe("");
		fresh.blur();
		expect(placesHolding(rendered, secret, logs)).toEqual([]);
		expect(textIn(await findTextbox())).toBe("");
		expect(draftsOf(rendered.services()).get(AGENT)).toBeNull();
		expect(rendered.source.sendInput).not.toHaveBeenCalled();
	});

	it("sends with the button too", async () => {
		sudo();
		const card = await findCard("sudo apt upgrade");
		const field = await within(card).findByLabelText<HTMLInputElement>(
			"Password for sudo on Mac"
		);

		fireEvent.change(field, { target: { value: "hunter2" } });
		fireEvent.click(button(card, "Send"));

		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "t1", "hunter2\r", {
			secret: true,
		});
		expect(
			within(card).getByLabelText<HTMLInputElement>("Password for sudo on Mac").value
		).toBe("");
	});
});

describe("a command you stopped", () => {
	it("says Stopped after how long, in grey, and not the signal", async () => {
		renderCard(
			ran({
				process: { state: "exited", code: null, signal: 1, stopped: true },
				duration_ms: 33_400,
			})
		);
		const card = await findCard();

		const ended = card.querySelector<HTMLElement>("[data-ended]")!;
		expect(ended).toHaveAttribute("data-ended", "stopped");
		expect(ended).toHaveTextContent(/^Stopped after 33 s$/);
		expect(ended).toHaveClass("text-muted-foreground");
	});

	it("names the signal that ended it when you didn't", async () => {
		renderCard(ran({ process: { state: "exited", code: null, signal: 9 }, duration_ms: 4200 }));
		const card = await findCard();

		expect(card.querySelector("[data-ended]")).toHaveTextContent(
			"Ended by signal 9 (killed) · 4.2 s"
		);
	});
});

describe("a finished command", () => {
	it("says how it ended and shows its output in color", async () => {
		renderCard(ran());
		const card = await findCard();

		expect(card).toHaveAttribute("data-user-run", "finished");
		expect(card).toHaveTextContent("exit 1 · 4.2 s");
		expect(within(card).getByText("FAIL")).toHaveStyle({ color: "var(--terminal-red)" });
		expect(card).toHaveTextContent("Tests 1 failed");
		expect(button(card, "Ask about this")).toBeInTheDocument();
		expect(button(card, "Run again")).toBeInTheDocument();
		expect(button(card, "Copy")).toBeInTheDocument();
		expect(within(card).queryByTestId("live-terminal")).toBeNull();
		expect(within(card).queryByRole("textbox")).toBeNull();
	});

	it("folds long output to its last 20 lines until you ask for all", async () => {
		const output = `${Array.from({ length: 30 }, (_, index) => `line ${index + 1}`).join("\n")}\n`;
		renderCard(ran({ output, lines: 30, process: { state: "exited", code: 0, signal: null } }));
		const card = await findCard();

		expect(card).toHaveTextContent("exit 0 · 4.2 s");
		const shown = () => card.querySelector("pre")?.textContent ?? "";
		expect(shown().startsWith("line 11\n")).toBe(true);
		expect(shown()).not.toMatch(/line 10\n/);
		expect(shown().endsWith("line 30")).toBe(true);

		fireEvent.click(button(card, "Show all 30 lines"));

		expect(shown().startsWith("line 1\nline 2\n")).toBe(true);
		expect(within(card).queryByRole("button", { name: /^Show all/ })).toBeNull();
	});

	it("asks the agent about it at once, with its output pill", async () => {
		const { source } = renderCard(ran());
		const card = await findCard();

		fireEvent.click(button(card, "Ask about this"));

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		const [agent, input, mode] = source.sendInput.mock.calls[0]!;
		expect(agent).toBe(AGENT);
		expect(mode).toBe("queue");
		expect(input.text).toBe(`Look at this output. ${MARK}`);
		expect(input.refs).toEqual([
			expect.objectContaining({
				type: "output",
				command: "pnpm test",
				exit_code: 1,
				running: false,
				lines_total: 3,
				text: "FAIL src/cache.test.ts\nTests 1 failed\ndone",
			}),
		]);
	});

	it("runs it again, its output pill going to the composer", async () => {
		const { openTerminal } = renderCard(ran());
		const card = await findCard();

		fireEvent.click(button(card, "Run again"));

		await waitFor(() =>
			expect(openTerminal).toHaveBeenCalledWith(
				AGENT,
				{ type: "run", command: "pnpm test", private: false },
				{ cols: 20, rows: 24 }
			)
		);
		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, running"]));
	});

	it("says its output goes with the next message, and Keep private takes it back", async () => {
		renderCard(ran());
		const card = await findCard();
		const textbox = await findTextbox();
		act(() =>
			quoteInbox.send(
				AGENT,
				startedOutput({
					command: "pnpm test",
					deviceName: "Mac",
					cwd: "~/code/example-app",
					terminal: "t1",
					at: Date.now() - 8000,
				}),
				false
			)
		);
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, exit 1"]));
		expect(await within(card).findByText("Goes with your next message")).toBeInTheDocument();

		fireEvent.click(button(card, "Keep private"));

		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		expect(within(card).queryByText("Goes with your next message")).toBeNull();
	});

	it("has Copy command and Attach to next message in its menu", async () => {
		renderCard(ran());
		const card = await findCard();

		fireEvent.click(button(card, "More for this command"));
		const menu = await screen.findByRole("menu");
		expect(
			within(menu)
				.getAllByRole("menuitem")
				.map((each) => each.textContent)
		).toEqual(["Copy command", "Attach to next message"]);

		fireEvent.click(within(menu).getByRole("menuitem", { name: "Attach to next message" }));

		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, exit 1"]));
	});

	it("shows only the command and its output in a read-only timeline", async () => {
		renderCard(ran(), { readOnly: true });
		const card = await findCard();

		expect(within(card).queryAllByRole("button")).toEqual([]);
		expect(card).toHaveTextContent("Tests 1 failed");
	});
});

describe("a command run privately", () => {
	it("starts folded, shows its output on request, and attaches it only when asked", async () => {
		renderCard(ran({ private: true }));
		const card = await findCard();

		expect(card).toHaveTextContent(/You ran privately pnpm test/);
		expect(card).toHaveTextContent("exit 1 · 4.2 s");
		expect(card).not.toHaveTextContent("Tests 1 failed");
		expect(within(card).queryByRole("button", { name: "Ask about this" })).toBeNull();

		fireEvent.click(button(card, "Show output"));
		expect(card).toHaveTextContent("Tests 1 failed");
		fireEvent.click(button(card, "Hide output"));
		expect(card).not.toHaveTextContent("Tests 1 failed");

		fireEvent.click(button(card, "Attach to next message"));

		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Output of pnpm test, exit 1"]));
		await waitFor(() =>
			expect(
				within(card).queryByRole("button", { name: "Attach to next message" })
			).toBeNull()
		);
		expect(card).toHaveTextContent("Goes with your next message");
	});
});

describe("a command moved to the terminal", () => {
	it("shrinks to a line that opens its tab", async () => {
		renderCard(running({ command: "pnpm dev", moved: true }), {
			terminal: terminalOf({
				kind: { type: "run", command: "pnpm dev", private: false },
				moved: true,
			}),
		});
		const stub = await waitFor(() => {
			const found = document.querySelector<HTMLElement>("[data-user-run='moved']");
			if (!found) throw new Error("No stub yet");
			return found;
		});

		expect(stub).toHaveTextContent("You ran pnpm dev· moved to the terminal, still running");
		expect(screen.queryByTestId("live-terminal")).toBeNull();

		fireEvent.click(within(stub).getByRole("button", { name: "Open in terminal" }));

		expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "t1");
	});

	it("goes back to a card once it ends", async () => {
		renderCard(ran({ moved: true }));
		const card = await findCard();

		expect(card).toHaveTextContent("exit 1 · 4.2 s");
	});
});

describe("a command that ran in a shell of its own", () => {
	it("says Shell integration is off, and its link opens that setting", async () => {
		const { services } = renderCard(ran({ fresh: "integration_off" }));
		const card = await findCard();

		expect(card.querySelector("[data-fresh]")).toHaveTextContent(
			"Sessions need Shell integration, which is off on Mac."
		);
		fireEvent.click(button(card, "Shell integration"));

		// Settings opens on the device's tab, scrolled to its Terminals card.
		expect(settingsOf(services()).getState()).toMatchObject({
			open: true,
			tab: DEVICE,
			reveal: "device-card-terminals",
		});
	});

	it("says quietly why the session couldn't take it", async () => {
		for (const [fresh, note] of [
			["no_marks", "Your shell didn't report its commands, so this ran on its own."],
			["busy", "The session was busy, so this ran on its own."],
			["lines", "This spans more than one line, so it ran on its own."],
			["exec", "This starts with exec, so it ran on its own."],
		] as const) {
			renderCard(ran({ fresh }));
			const card = await findCard();

			expect(card.querySelector("[data-fresh]")).toHaveTextContent(note);
			expect(within(card).queryByRole("button", { name: "Shell integration" })).toBeNull();
			cleanup();
			clearKept();
		}
	});

	it("says nothing when you asked for a fresh shell, or the session wasn't there", async () => {
		for (const fresh of ["asked", null] as const) {
			renderCard(ran({ fresh }));
			const card = await findCard();

			expect(card.querySelector("[data-fresh]")).toBeNull();
			cleanup();
			clearKept();
		}
	});
});

describe("a program that takes the whole screen", () => {
	const commit = () => running({ command: "git commit" });
	const full = (over: Partial<Terminal> = {}) =>
		terminalOf({
			kind: { type: "run", command: "git commit", private: false },
			title: "git commit",
			full_screen: true,
			...over,
		});

	afterEach(forgetNewest);

	it("says it's in the terminal, with one Open in terminal that moves it there", async () => {
		const { moveTerminal } = renderCard(commit(), { terminal: full() });
		const card = await findCard("git commit");

		expect(card).toHaveAttribute("data-handoff", "in_terminal");
		expect(card).toHaveTextContent("In the terminal");
		expect(card).toHaveTextContent(
			"git is open in the terminal below. The result comes back here when it closes."
		);
		expect(within(card).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
		expect(button(card, "Stop")).toBeInTheDocument();
		expect(within(card).queryByTestId("live-terminal")).toBeNull();
		expect(within(card).queryByRole("textbox")).toBeNull();

		fireEvent.click(button(card, "Open in terminal"));

		await waitFor(() => expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "t1"));
		expect(moveTerminal).toHaveBeenCalledWith("t1");
	});

	it("moves to the terminal by itself when you just ran it here", async () => {
		const here = {
			openTerminal: async (): Promise<ActionResult> => ({
				status: "ok",
				data: { status: "terminal_opened", terminal: "t1" },
			}),
		};
		const noView = (): never => {
			throw new Error("A private command doesn't read the view");
		};
		await startRun(here, noView, {
			agent: AGENT,
			command: "git commit",
			private: true,
			size: { cols: 100, rows: 24 },
		});

		const { moveTerminal } = renderCard(commit(), { terminal: full() });
		await findCard("git commit");

		await waitFor(() => expect(seam.showInDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "t1"));
		expect(moveTerminal).toHaveBeenCalledExactlyOnceWith("t1");
	});

	it("stays put for a command this window didn't just run", async () => {
		const { moveTerminal } = renderCard(commit(), { terminal: full() });
		await findCard("git commit");
		await act(async () => {});

		expect(moveTerminal).not.toHaveBeenCalled();
		expect(seam.showInDrawer).not.toHaveBeenCalled();
	});

	it("says so once moved, offering its tab only while the panel doesn't show it", async () => {
		renderCard(running({ command: "git commit", moved: true }), {
			terminal: full({ moved: true }),
		});
		const stub = await waitFor(() => {
			const found = document.querySelector<HTMLElement>("[data-user-run='moved']");
			if (!found) throw new Error("No stub yet");
			return found;
		});

		expect(stub).toHaveAttribute("data-handoff", "in_terminal");
		expect(stub).toHaveTextContent("In the terminal");
		expect(stub).toHaveTextContent("git is open in the terminal below.");
		expect(within(stub).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
	});

	it("drops the tab's button once moved while the panel shows it", async () => {
		seam.shown = true;
		renderCard(running({ command: "git commit", moved: true }), {
			terminal: full({ moved: true }),
		});
		const stub = await waitFor(() => {
			const found = document.querySelector<HTMLElement>("[data-user-run='moved']");
			if (!found) throw new Error("No stub yet");
			return found;
		});

		expect(stub).toHaveTextContent("git is open in the terminal below.");
		expect(within(stub).queryByRole("button")).toBeNull();
	});
});

describe("a picker that reads keys", () => {
	const reads = (over: Partial<Terminal> = {}) =>
		terminalOf({ reads_keys: true, waiting: true, answers: true, ...over });

	it("takes your keys once you click its output, and gives them back with ⌘.", async () => {
		renderCard(running(), { terminal: reads() });
		const card = await findCard();
		const box = card.querySelector<HTMLElement>("[data-keys]")!;

		expect(card.querySelector("[data-handoff]")).toHaveTextContent("Waiting for keys");
		expect(box).toHaveAttribute("data-keys", "offered");

		act(() => box.focus());

		expect(box).toHaveAttribute("data-keys", "held");
		expect(card).toHaveTextContent("go to the program");
		expect(within(card).getAllByRole("button", { name: "Open in terminal" })).toHaveLength(1);
		expect(within(card).getAllByRole("button", { name: "Stop" })).toHaveLength(1);
		expect(within(card).queryByRole("textbox")).toBeNull();

		fireEvent.keyDown(box, { key: "ArrowUp" });
		expect(seam.sendInput).toHaveBeenCalledExactlyOnceWith(DEVICE, "t1", "\x1b[A");

		const command = navigator.userAgent.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
		fireEvent.keyDown(box, { ...command, key: ".", code: "Period" });

		expect(box).toHaveAttribute("data-keys", "offered");
		expect(within(card).getByRole("textbox")).toBeInTheDocument();
	});

	it("stays view-only with a device that doesn't answer for the terminal", async () => {
		renderCard(running(), { terminal: terminalOf({ waiting: true }) });
		const card = await findCard();

		expect(card.querySelector("[data-keys]")).toBeNull();
		expect(card).toHaveTextContent("Waiting for you");
		expect(within(card).getByRole("textbox")).toBeInTheDocument();
	});

	it("takes no keys in a read-only timeline, or while its output is folded", async () => {
		renderCard(running(), { terminal: reads(), readOnly: true });
		expect((await findCard()).querySelector("[data-keys]")).toBeNull();
		cleanup();

		renderCard(running({ private: true }), { terminal: reads() });
		const card = await screen.findByRole("region", { name: "You ran pnpm test" });
		expect(card.querySelector("[data-keys]")).toBeNull();
	});
});
