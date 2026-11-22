// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import {
	act,
	cleanup,
	fireEvent,
	renderHook,
	screen,
	waitFor,
	within,
} from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { DeviceLink, Entity, SessionBlock, Workspace } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { sessionPill, type UserSessionItem } from "@maxhq/shared/terminal/session";
import { entities, link, workspace } from "@maxhq/shared/test/build";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { useSessionFill } from "./session-pill";

import {
	AGENT,
	DEVICE,
	DONE,
	editorOf,
	findTextbox,
	installBrowserFakes,
	pillsIn,
	renderComposer,
	type,
} from "./testing";

// The terminal module, as the composer reaches it.
const seam = vi.hoisted(() => ({
	runningText: vi.fn<
		(terminal: string, start: number | null) => { text: string; total: number } | null
	>(() => null),
	openAgentShell: vi.fn<(agent: string) => void>(),
}));

vi.mock("@/terminal", () => ({
	useShownInPanel: () => false,
	LiveTerminal: () => null,
	useTerminalActions: () => ({
		sendInput: () => {},
		text: () => null,
		runningText: seam.runningText,
		cellSize: () => ({ width: 8, height: 17 }),
		pasteIntoDrawer: () => {},
		showInDrawer: () => {},
		openAgentShell: seam.openAgentShell,
	}),
}));

beforeAll(installBrowserFakes);
afterEach(() => {
	cleanup();
	clearKept();
	quoteInbox.take(AGENT);
	seam.runningText.mockReset();
	seam.runningText.mockReturnValue(null);
	seam.openAgentShell.mockReset();
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

const block = (id: number, command: string, over: Partial<SessionBlock> = {}): SessionBlock => ({
	id,
	command,
	cwd: FOLDER,
	cwd_after: null,
	private: false,
	process: EXITED(0),
	started_at: Date.now() - 60_000,
	duration_ms: 1200,
	lines: 1,
	output: `${command} done\n`,
	output_truncated: false,
	secret_prompts: 0,
	start: null,
	...over,
});

const INSTALL = block(1, "pnpm install");
const SECRET = block(2, "printenv API_TOKEN", { private: true, output: "tok_example_123\n" });
const TEST = block(3, "pnpm test", { process: EXITED(1), output: "Tests 1 failed\n" });

// Each card gets an ID of its own, since the composer remembers the
// cards whose pill it took.
let serial = 0;

const card = (blocks: readonly SessionBlock[], over: Partial<UserSessionItem> = {}) =>
	timelineItem(
		{
			kind: "user_session",
			terminal: "s1",
			device: DEVICE,
			blocks: [...blocks],
			sealed: false,
			sent: false,
			...over,
		},
		{ id: `card-${(serial += 1)}`, agent: AGENT, ordinal: 1, started_at: Date.now() - 60_000 }
	) as UserSessionItem;

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

/** The composer below a timeline holding this session card. */
const renderWith = (item: UserSessionItem) => {
	const composer = renderComposer({
		sent: [item],
		links: [macLink()],
		more: [...entities({ workspaces: [home()] }), sessionTerminal],
	});
	const sessionShare = vi.fn<DataSource["sessionShare"]>(async () => DONE);
	Object.assign(composer.source, { sessionShare });
	/** Changes the card, as the device does. */
	const amend = (next: UserSessionItem) =>
		act(() => {
			composer.timeline.setState(
				applyUpdate(composer.timeline.getState(), {
					type: "events",
					seq: composer.timeline.getState().seq + 1,
					events: [{ type: "upsert", item: next }],
				})
			);
		});
	return { ...composer, sessionShare, amend };
};

const pillOf = (textbox: HTMLElement) => textbox.querySelector<HTMLElement>(".pill");

describe("the session pill", () => {
	it("goes in by itself once the open card shares a command, red when one failed", async () => {
		renderWith(card([INSTALL, SECRET, TEST]));
		const textbox = await findTextbox();

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Agent shell · 1 failed on Mac"]));
		expect(pillOf(textbox)).toHaveAttribute("data-family", "output-failed");
	});

	it("opens the Agent shell in the terminal panel on a click", async () => {
		renderWith(card([INSTALL, TEST]));
		const textbox = await findTextbox();
		await waitFor(() => expect(pillOf(textbox)).not.toBeNull());

		fireEvent.click(pillOf(textbox)!);

		expect(seam.openAgentShell).toHaveBeenCalledExactlyOnceWith(AGENT);
	});

	it("stays plain while nothing shared failed, and spins while a shared command runs", async () => {
		const running = block(4, "pnpm dev", { process: { state: "running" }, duration_ms: null });
		const item = card([INSTALL, SECRET]);
		const { amend } = renderWith(item);
		const textbox = await findTextbox();

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Agent shell on Mac"]));
		expect(pillOf(textbox)).toHaveAttribute("data-family", "output");
		expect(pillOf(textbox)!.querySelector(".pill-spin")).toBeNull();

		amend({ ...item, blocks: [INSTALL, SECRET, running] });

		await waitFor(() => expect(pillsIn(textbox)).toEqual(["Agent shell on Mac, running"]));
		expect(pillOf(textbox)!.querySelector(".pill-spin")).not.toBeNull();
	});

	it("stays out while every command is private, and leaves once none is shared", async () => {
		const item = card([INSTALL, SECRET]);
		const { amend } = renderWith({ ...item, blocks: [{ ...INSTALL, private: true }, SECRET] });
		const textbox = await findTextbox();
		await act(async () => {});
		expect(pillsIn(textbox)).toEqual([]);

		amend(item);
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));

		amend({ ...item, blocks: [{ ...INSTALL, private: true }, SECRET] });
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));

		// Shared again, it comes back.
		amend(item);
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
	});

	it("stays out once you removed it, through a restart too", async () => {
		const item = card([INSTALL, TEST]);
		renderWith(item);
		let textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));

		act(() => {
			editorOf(textbox).commands.clearContent(true);
		});
		await act(async () => {});
		expect(pillsIn(textbox)).toEqual([]);

		cleanup();
		clearKept();
		renderWith(item);
		textbox = await findTextbox();
		// The composer has had its chance to put it back.
		await act(async () => {});
		await act(async () => {});
		expect(pillsIn(textbox)).toEqual([]);
	});

	it("waits for its card while the timeline loads, and leaves once the loaded one lacks it", async () => {
		const { timeline } = renderWith(card([INSTALL], { sealed: true, sent: true }));
		const textbox = await findTextbox();
		act(() => timeline.setState(EMPTY_TIMELINE, true));

		act(() => quoteInbox.send(AGENT, sessionPill({ id: "card-gone" }, "Mac"), false));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		await act(async () => {});
		expect(pillsIn(textbox)).toHaveLength(1);

		act(() =>
			timeline.setState(
				applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 2, items: [], has_older: false }),
				true
			)
		);
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
	});

	it("leaves once a message closed its card", async () => {
		const item = card([INSTALL, TEST]);
		const { amend } = renderWith(item);
		const textbox = await findTextbox();
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));

		amend({ ...item, sealed: true, sent: false });

		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
	});

	it("previews the commands with ticks, and unticking one keeps it from the agent", async () => {
		const item = card([INSTALL, SECRET, TEST]);
		const { sessionShare } = renderWith(item);
		const textbox = await findTextbox();
		await waitFor(() => expect(pillOf(textbox)).not.toBeNull());

		const pill = pillOf(textbox)!;
		fireEvent.pointerEnter(pill, { pointerType: "mouse" });
		fireEvent.mouseEnter(pill);
		fireEvent.mouseMove(pill);
		const preview = await screen.findByRole("dialog", {}, { timeout: 2000 });
		expect(preview).toHaveTextContent("Agent shell on Mac");
		expect(preview).toHaveTextContent("· 2 of 3 go with your next message");
		const tick = (command: string) =>
			within(preview).getByRole("checkbox", { name: `Send ${command} to the agent` });
		expect(tick("pnpm install")).toBeChecked();
		expect(tick("printenv API_TOKEN")).not.toBeChecked();
		expect(tick("pnpm test")).toBeChecked();
		expect(preview).toHaveTextContent("exit 1");
		expect(preview).toHaveTextContent("private");

		fireEvent.click(tick("pnpm test"));

		await waitFor(() =>
			expect(sessionShare).toHaveBeenCalledExactlyOnceWith(AGENT, item.id, 3, true)
		);
	});

	it("takes the shared commands' last lines with the message, never the private ones", async () => {
		const running = block(4, "pnpm dev", {
			process: { state: "running" },
			duration_ms: null,
			output: "",
			lines: 0,
		});
		const item = card([INSTALL, SECRET, TEST, running]);
		const { source } = renderWith(item);
		seam.runningText.mockReturnValue({ text: "ready on :3000", total: 4 });
		const textbox = await findTextbox();
		await waitFor(() => expect(pillOf(textbox)).not.toBeNull());

		await type(textbox, "Why does this fail?");
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() => expect(source.sendInput).toHaveBeenCalledOnce());
		const [, input] = source.sendInput.mock.calls[0]!;
		expect(input.text).toBe(`${MARK} Why does this fail?`);
		expect(input.refs).toEqual([
			{
				type: "session",
				device_name: "Mac",
				item: item.id,
				blocks: [
					expect.objectContaining({ command: "pnpm install", text: "pnpm install done" }),
					expect.objectContaining({
						command: "pnpm test",
						exit_code: 1,
						text: "Tests 1 failed",
					}),
					expect.objectContaining({
						command: "pnpm dev",
						running: true,
						exit_code: null,
						lines_total: 4,
						text: "ready on :3000",
					}),
				],
			},
		]);
		expect(JSON.stringify(input)).not.toContain("tok_example_123");
	});
});

describe("filling session pills for a message", () => {
	const fillWith = (items: UserSessionItem[]) => {
		const timeline = createTimelineStore();
		timeline.setState(
			applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 1, items, has_older: false }),
			true
		);
		return renderHook(() => useSessionFill(timeline)).result.current;
	};

	it("drops a pill whose card the timeline doesn't hold, so no empty ref goes", () => {
		const fill = fillWith([card([INSTALL])]);

		const segments = fill([
			{ type: "pill", pill: sessionPill({ id: "card-gone" }, "Mac") },
			{ type: "text", text: " Why?" },
		]);

		expect(segments).toEqual([{ type: "text", text: " Why?" }]);
	});

	it("reads a running command's live text by where its output starts", () => {
		const running = block(4, "nix develop", {
			process: { state: "running" },
			duration_ms: null,
			output: "",
			lines: 0,
			start: 120,
		});
		const item = card([INSTALL, running]);
		seam.runningText.mockReturnValue({ text: "entering", total: 1 });
		const fill = fillWith([item]);

		const [segment] = fill([{ type: "pill", pill: sessionPill(item, "Mac") }]);

		expect(seam.runningText).toHaveBeenCalledExactlyOnceWith("s1", 120);
		expect(segment).toMatchObject({
			pill: { blocks: [{}, { command: "nix develop", running: true, text: "entering" }] },
		});
	});
});
