// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { DRAFT_CLIPBOARD_MIME } from "@maxhq/shared/pills/clipboard";
import { type AgentRef, type GitItemRef, OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import type { ActionResult } from "@maxhq/shared/data/source";
import type { Agent } from "@maxhq/shared/bindings";
import { agent, entities, MIN, NOW, workspace } from "@maxhq/shared/test/build";
import { NodeSelection } from "@tiptap/pm/state";

import { agentDrops } from "./agent-drop";
import {
	AGENT,
	attachment,
	deferred,
	DEVICE,
	editorOf,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	queuedMessage,
	REMOTE,
	remoteLink,
	renderComposer,
	staged,
	textIn,
	type,
} from "./testing";

beforeAll(installBrowserFakes);

const pr: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 412,
	kind: "pull_request",
	title: "Cache the session key",
	state: "open",
	url: "https://github.com/owner/repo/pull/412",
	head: "cache-key",
	base: "main",
	body: "",
	checked_at: NOW,
};

const other = agent({ id: "a2", title: "Sidebar sort", vendor: "codex" });

const otherRef: AgentRef = {
	type: "agent",
	agent: "a2",
	title: "Sidebar sort",
	vendor: "codex",
	device: DEVICE,
	excerpt: null,
};

/** Drops main.rs into the composer and waits for its pill. */
const withFile = async (before = "") => {
	const composer = renderComposer();
	const textbox = await findTextbox();
	if (before !== "") await type(textbox, before);
	act(() => composer.drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
	await waitFor(() => expect(screen.getByText(/^1 attachment,/)).toBeInTheDocument());
	return { ...composer, textbox };
};

const caretAt = (textbox: HTMLElement, pos: number) =>
	act(() => {
		editorOf(textbox).commands.setTextSelection(pos);
	});

const caret = (textbox: HTMLElement) => editorOf(textbox).state.selection.from;

/** A clipboard that keeps what a copy writes, for a paste to read back. */
const fakeClipboard = () => {
	const data = new Map<string, string>();
	return {
		data,
		setData: (format: string, value: string) => data.set(format, value),
		clearData: () => data.clear(),
		getData: (format: string) => data.get(format) ?? "",
		files: [] as File[],
		get types() {
			return [...data.keys()];
		},
	};
};

describe("a pill in the text", () => {
	it("is one step for the arrow keys", async () => {
		const { textbox } = await withFile();
		caretAt(textbox, 2);
		fireEvent.keyDown(textbox, { key: "ArrowLeft" });
		expect(caret(textbox)).toBe(1);
		fireEvent.keyDown(textbox, { key: "ArrowRight" });
		expect(caret(textbox)).toBe(2);
	});

	it("goes with the word before it on Option+Backspace", async () => {
		const { textbox } = await withFile("Fix the");
		expect(textIn(textbox)).toBe(`Fix the ${MARK} `);
		caretAt(textbox, 10);
		fireEvent.keyDown(textbox, { key: "Backspace", altKey: true });
		expect(textIn(textbox)).toBe("Fix  ");
		expect(pillsIn(textbox)).toEqual([]);
	});

	it("comes back on undo with its file, without staging it again", async () => {
		const { source, textbox } = await withFile();
		caretAt(textbox, 2);
		fireEvent.keyDown(textbox, { key: "Backspace" });
		expect(pillsIn(textbox)).toEqual([]);
		expect(screen.queryByText(/^1 attachment,/)).toBeNull();

		act(() => {
			editorOf(textbox).commands.undo();
		});
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]));
		expect(screen.getByText(/^1 attachment,/)).toBeInTheDocument();
		expect(source.stageAttachments).toHaveBeenCalledTimes(1);

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: MARK,
					attachments: ["att-main.rs"],
					refs: [{ type: "attachment", id: "att-main.rs" }],
				},
				"queue"
			)
		);
	});

	it("copies as its marker for other apps, and cut and paste keep the pill", async () => {
		const { source, textbox } = await withFile("Read");
		const clipboard = fakeClipboard();
		act(() => {
			editorOf(textbox).commands.selectAll();
		});
		fireEvent.copy(textbox, { clipboardData: clipboard });
		expect(clipboard.data.get("text/plain")).toBe("Read [File: main.rs] ");
		expect(clipboard.data.get(DRAFT_CLIPBOARD_MIME)).toContain("main.rs");
		expect(clipboard.data.get("text/html")).toContain("main.rs");

		fireEvent.cut(textbox, { clipboardData: clipboard });
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		expect(textIn(textbox)).toBe("");

		fireEvent.paste(textbox, { clipboardData: clipboard });
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]));
		expect(textIn(textbox)).toBe(`Read ${MARK} `);
		expect(source.stageAttachments).toHaveBeenCalledTimes(1);
	});

	it("copies plain text as plain text", async () => {
		const { textbox } = await withFile("Read");
		const clipboard = fakeClipboard();
		act(() => {
			editorOf(textbox).commands.setTextSelection({ from: 1, to: 5 });
		});
		fireEvent.copy(textbox, { clipboardData: clipboard });
		expect(clipboard.data.has(DRAFT_CLIPBOARD_MIME)).toBe(false);
	});
});

describe("a drop", () => {
	it("goes in at the caret the box kept, wherever the pointer lets go", async () => {
		const composer = renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "Read now");
		caretAt(textbox, 6);
		act(() => {
			editorOf(textbox).commands.blur();
		});
		// Were the pointer to count, it would land at the start.
		editorOf(textbox).view.posAtCoords = () => ({ pos: 1, inside: -1 });

		act(() => composer.drop().onDrop(["/Users/max/main.rs"], { x: 10, y: 10 }));
		await waitFor(() => expect(textIn(textbox)).toBe(`Read ${MARK} now`));
		expect(caret(textbox)).toBe(8);
	});

	it("keeps the selected text and goes in after it", async () => {
		const composer = renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "Read now");
		act(() => {
			editorOf(textbox).commands.setTextSelection({ from: 1, to: 5 });
		});

		act(() => composer.drop().onDrop(["/Users/max/main.rs"], { x: 10, y: 10 }));
		await waitFor(() => expect(textIn(textbox)).toBe(`Read ${MARK} now`));
		expect(caret(textbox)).toBe(8);
	});
});

describe("dragging a pill", () => {
	afterEach(() => {
		vi.restoreAllMocks();
	});

	/** Lays the text box out, which jsdom can't, so a point over it lands at `pos`. */
	const landAt = (textbox: HTMLElement, pos: number) => {
		vi.spyOn(textbox, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 400, 40));
		// The editor scrolls the caret into view, and jsdom can't scroll the window.
		vi.spyOn(window, "scrollBy").mockImplementation(() => {});
		const { view } = editorOf(textbox);
		view.posAtCoords = () => ({ pos, inside: -1 });
		view.coordsAtPos = () => ({ left: 120, right: 120, top: 10, bottom: 28 });
	};

	/** "Read", main.rs and "now", with the pill at 6 and the end at 11. */
	const readNow = async () => {
		const composer = await withFile("Read");
		await type(composer.textbox, "now");
		expect(textIn(composer.textbox)).toBe(`Read ${MARK} now`);
		return composer;
	};

	const press = (textbox: HTMLElement) =>
		fireEvent.mouseDown(textbox.querySelector(".pill")!, {
			button: 0,
			clientX: 50,
			clientY: 10,
		});
	const moveTo = (x: number, y: number) =>
		fireEvent.mouseMove(window, { clientX: x, clientY: y });
	const letGo = (x: number, y: number) =>
		fireEvent.mouseUp(window, { button: 0, clientX: x, clientY: y });
	const dropCaret = () => document.querySelector<HTMLElement>("[data-pill-caret]");

	it("moves to where it lets go, with a caret showing where, as one undo step", async () => {
		const { source, textbox } = await readNow();
		landAt(textbox, 11);

		press(textbox);
		moveTo(150, 12);
		expect(dropCaret()).toHaveStyle({ left: "119px", top: "10px", height: "18px" });
		letGo(150, 12);
		expect(dropCaret()).toBeNull();
		expect(textIn(textbox)).toBe(`Read now ${MARK} `);
		expect(caret(textbox)).toBe(12);
		// The click that ends the drag goes nowhere.
		expect(fireEvent.click(textbox)).toBe(false);

		act(() => {
			editorOf(textbox).commands.undo();
		});
		expect(textIn(textbox)).toBe(`Read ${MARK} now`);
		act(() => {
			editorOf(textbox).commands.redo();
		});
		expect(textIn(textbox)).toBe(`Read now ${MARK} `);

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `Read now ${MARK}`,
					attachments: ["att-main.rs"],
					refs: [{ type: "attachment", id: "att-main.rs" }],
				},
				"queue"
			)
		);
		expect(source.stageAttachments).toHaveBeenCalledTimes(1);
	});

	it("stays where it was on Escape", async () => {
		const { textbox } = await readNow();
		landAt(textbox, 11);
		const before = editorOf(textbox).state.doc;

		press(textbox);
		moveTo(150, 12);
		expect(dropCaret()).not.toBeNull();
		fireEvent.keyDown(window, { key: "Escape" });
		expect(dropCaret()).toBeNull();
		moveTo(160, 12);
		expect(dropCaret()).toBeNull();
		letGo(160, 12);
		expect(editorOf(textbox).state.doc).toBe(before);
	});

	it("stays where it was when let go off the text", async () => {
		const { textbox } = await readNow();
		landAt(textbox, 11);
		const before = editorOf(textbox).state.doc;

		press(textbox);
		moveTo(150, 12);
		moveTo(150, 300);
		expect(dropCaret()).toBeNull();
		letGo(150, 300);
		expect(editorOf(textbox).state.doc).toBe(before);
	});

	it("is selected by a press that doesn't move, without a drag of the text", async () => {
		const { textbox } = await readNow();
		landAt(textbox, 11);

		// The press is the pill's own, so the box neither selects text nor drags.
		expect(press(textbox)).toBe(false);
		moveTo(52, 11);
		expect(dropCaret()).toBeNull();
		letGo(52, 11);
		const { selection } = editorOf(textbox).state;
		expect(selection).toBeInstanceOf(NodeSelection);
		expect(selection.from).toBe(6);
		expect(textbox).toHaveFocus();
		expect(textIn(textbox)).toBe(`Read ${MARK} now`);

		// Its keys work as before: Escape leaves it, and Backspace takes it.
		fireEvent.keyDown(textbox, { key: "Escape" });
		expect(caret(textbox)).toBe(7);
		fireEvent.keyDown(textbox, { key: "Backspace" });
		expect(pillsIn(textbox)).toEqual([]);
	});

	it("moves by cut and paste too, keeping its file", async () => {
		const { source, textbox } = await readNow();
		const clipboard = fakeClipboard();
		act(() => {
			editorOf(textbox).commands.setNodeSelection(6);
		});
		fireEvent.cut(textbox, { clipboardData: clipboard });
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		expect(textIn(textbox)).toBe("Read  now");

		caretAt(textbox, 10);
		fireEvent.paste(textbox, { clipboardData: clipboard });
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]));
		expect(textIn(textbox)).toBe(`Read  now ${MARK} `);
		expect(source.stageAttachments).toHaveBeenCalledTimes(1);

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `Read  now ${MARK}`,
					attachments: ["att-main.rs"],
					refs: [{ type: "attachment", id: "att-main.rs" }],
				},
				"queue"
			)
		);
	});
});

describe("a pull request's number", () => {
	const found = (source: ReturnType<typeof renderComposer>["source"]) =>
		source.findGitItems.mockResolvedValue({
			repo: "owner/repo",
			items: [pr],
			unavailable: null,
		});

	it("draws as its pill once GitHub finds it, and goes out as a ref", async () => {
		const { source } = renderComposer();
		found(source);
		const textbox = await findTextbox();
		await type(textbox, "See #412 now");

		const drawn = await waitFor(() => {
			const element = textbox.querySelector("[data-git-number]");
			expect(element).not.toBeNull();
			return element!;
		});
		expect(drawn).toHaveTextContent("#412");
		expect(drawn).toHaveAttribute("data-family", "git-open");
		expect(textIn(textbox)).toBe("See #412 now");
		expect(source.findGitItems).toHaveBeenCalledWith(AGENT, "412");

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "See #412 now", attachments: [], refs: [pr] },
				"queue"
			)
		);
	});

	it("stays text when GitHub doesn't know it", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "Step #3 first");
		await waitFor(() => expect(source.findGitItems).toHaveBeenCalledWith(AGENT, "3"));
		expect(textbox.querySelector("[data-git-number]")).toBeNull();

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "Step #3 first", attachments: [] },
				"queue"
			)
		);
	});

	describe("picked a while ago", () => {
		const merged: GitItemRef = { ...pr, state: "merged", checked_at: NOW + 10 * MIN };
		const drawn = async () => {
			const { source } = renderComposer();
			found(source);
			const textbox = await findTextbox();
			await type(textbox, "See #412 now");
			await waitFor(() => expect(textbox.querySelector("[data-git-number]")).not.toBeNull());
			return { source, textbox };
		};

		afterEach(() => {
			vi.restoreAllMocks();
		});

		it("is looked up again on send, and goes out as it is now", async () => {
			vi.spyOn(Date, "now").mockReturnValue(NOW + 10 * MIN);
			const { source, textbox } = await drawn();
			source.findGitItems.mockResolvedValue({
				repo: "owner/repo",
				items: [merged],
				unavailable: null,
			});

			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenCalledWith(
					AGENT,
					{ text: "See #412 now", attachments: [], refs: [merged] },
					"queue"
				)
			);
			expect(source.findGitItems).toHaveBeenCalledTimes(2);
			expect(source.findGitItems).toHaveBeenLastCalledWith(AGENT, "412");
		});

		it("goes out as it was when the lookup fails", async () => {
			vi.spyOn(Date, "now").mockReturnValue(NOW + 10 * MIN);
			const { source, textbox } = await drawn();
			source.findGitItems.mockRejectedValue(new Error("No network"));

			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenCalledWith(
					AGENT,
					{ text: "See #412 now", attachments: [], refs: [pr] },
					"queue"
				)
			);
			expect(source.findGitItems).toHaveBeenCalledTimes(2);
			expect(screen.queryByRole("alert")).toBeNull();
		});

		it("isn't looked up again within five minutes", async () => {
			vi.spyOn(Date, "now").mockReturnValue(NOW + 4 * MIN);
			const { source, textbox } = await drawn();

			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenCalledWith(
					AGENT,
					{ text: "See #412 now", attachments: [], refs: [pr] },
					"queue"
				)
			);
			expect(source.findGitItems).toHaveBeenCalledTimes(1);
		});
	});

	it("previews on hover, and Keep as text draws it as text again", async () => {
		const { source } = renderComposer();
		found(source);
		const textbox = await findTextbox();
		await type(textbox, "See #412 now");
		const drawn = await waitFor(() => {
			const element = textbox.querySelector("[data-git-number]");
			expect(element).not.toBeNull();
			return element!;
		});

		fireEvent.mouseOver(drawn);
		const preview = await screen.findByRole("dialog", {}, { timeout: 2000 });
		expect(preview).toHaveTextContent("Pull request #412");
		expect(preview).toHaveTextContent("Cache the session key");
		fireEvent.click(within(preview).getByRole("button", { name: "Keep as text" }));

		await waitFor(() => expect(textbox.querySelector("[data-git-number]")).toBeNull());
		expect(textIn(textbox)).toBe("See #412 now");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "See #412 now", attachments: [] },
				"queue"
			)
		);
	});
});

describe("an agent dragged from the sidebar", () => {
	const elementFromPoint = document.elementFromPoint;
	afterEach(() => {
		document.elementFromPoint = elementFromPoint;
	});

	it("lights the composer and drops in as a pill at the caret", async () => {
		const { source, upsert } = renderComposer();
		const textbox = await findTextbox();
		upsert(entities({ agents: [other] }));
		await type(textbox, "Ask now");
		caretAt(textbox, 5);
		document.elementFromPoint = () => textbox;
		// Were the pointer to count, it would land at the start.
		editorOf(textbox).view.posAtCoords = () => ({ pos: 1, inside: -1 });

		act(() => {
			agentDrops.aim([other], { x: 10, y: 10 });
		});
		expect(screen.getByText("Drop to add Sidebar sort at the cursor")).toBeInTheDocument();
		act(() => {
			agentDrops.drop([other], { x: 10, y: 10 });
		});
		expect(screen.queryByText(/^Drop to add Sidebar sort/)).toBeNull();

		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Sidebar sort, Codex/)])
		);
		expect(textIn(textbox)).toBe(`Ask ${MARK} now`);
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Ask ${MARK} now`, attachments: [], refs: [otherRef] },
				"queue"
			)
		);
	});

	it("drops every picked agent in, past its own", async () => {
		const { upsert } = renderComposer();
		const textbox = await findTextbox();
		const third = agent({ id: "a4", title: "Login flow", vendor: "claude" });
		upsert(entities({ agents: [other, third] }));
		document.elementFromPoint = () => textbox;
		act(() => {
			agentDrops.aim([other, heldAgent(), third], { x: 10, y: 10 });
		});
		expect(screen.getByText(/^Drop to add 2 agents/)).toBeInTheDocument();
		act(() => {
			agentDrops.drop([other, heldAgent(), third], { x: 10, y: 10 });
		});
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([
				expect.stringMatching(/^Agent Sidebar sort/),
				expect.stringMatching(/^Agent Login flow/),
			])
		);
	});

	it("doesn't take the agent it belongs to", async () => {
		renderComposer();
		const textbox = await findTextbox();
		document.elementFromPoint = () => textbox;
		let taken = true;
		act(() => {
			taken = agentDrops.drop([heldAgent()], { x: 10, y: 10 });
		});
		expect(taken).toBe(false);
		expect(pillsIn(textbox)).toEqual([]);
	});
});

describe("an agent on another device", () => {
	const elementFromPoint = document.elementFromPoint;
	afterEach(() => {
		document.elementFromPoint = elementFromPoint;
	});

	/** An agent on devbox, while the composer's agent is on this Mac. */
	const far = agent({ id: "a3", title: "Login flow", vendor: "claude", workspace: "w2" });
	const farRef = (excerpt: string | null): AgentRef => ({
		type: "agent",
		agent: "a3",
		title: "Login flow",
		vendor: "claude",
		device: REMOTE,
		excerpt,
	});
	const EXCERPT = "att-agent-a3.md";

	/** Drops an agent into the text at `pos`, the way a drag from the sidebar does. */
	const dropAgent = (textbox: HTMLElement, which: Agent, pos: number) => {
		document.elementFromPoint = () => textbox;
		caretAt(textbox, pos);
		act(() => {
			agentDrops.drop([which], { x: 10, y: 10 });
		});
	};

	/** The composer with "Ask " and the agent on devbox after it. */
	const asking = async (options: Parameters<typeof renderComposer>[0] = {}) => {
		const composer = renderComposer(options);
		const textbox = await findTextbox();
		composer.upsert(
			entities({
				agents: [far, other],
				workspaces: [
					workspace({ id: "w1", device: DEVICE }),
					workspace({ id: "w2", device: REMOTE }),
				],
			})
		);
		await type(textbox, "Ask ");
		dropAgent(textbox, far, 5);
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Login flow/)])
		);
		return { ...composer, textbox };
	};

	it("goes with an excerpt, staged here and listed with the attachments", async () => {
		const { source, textbox } = await asking();
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Ask ${MARK}`, attachments: [EXCERPT], refs: [farRef(EXCERPT)] },
				"queue"
			)
		);
		expect(source.excerptAgent).toHaveBeenCalledWith("a3", 40_000);
		expect(source.stagePasted).toHaveBeenCalledWith(
			DEVICE,
			"agent-a3.md",
			"text/markdown",
			new TextEncoder().encode("# Agent a3\n")
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));
	});

	it("takes one excerpt for an agent named twice", async () => {
		const { source, textbox } = await asking();
		await type(textbox, "and ");
		dropAgent(textbox, far, 11);
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `Ask ${MARK} and ${MARK}`,
					attachments: [EXCERPT],
					refs: [farRef(EXCERPT), farRef(EXCERPT)],
				},
				"queue"
			)
		);
		expect(source.excerptAgent).toHaveBeenCalledTimes(1);
	});

	it("next to an agent on this device, which goes live with no excerpt", async () => {
		const { source, textbox } = await asking();
		await type(textbox, "and ");
		dropAgent(textbox, other, 11);
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `Ask ${MARK} and ${MARK}`,
					attachments: [EXCERPT],
					refs: [farRef(EXCERPT), otherRef],
				},
				"queue"
			)
		);
		expect(source.excerptAgent).toHaveBeenCalledTimes(1);
		expect(source.excerptAgent).toHaveBeenCalledWith("a3", 40_000);
	});

	it("holds the send while the excerpt readies, and typing meanwhile keeps it", async () => {
		const { source, textbox } = await asking();
		const staging = deferred<ActionResult>();
		source.stagePasted.mockReturnValueOnce(staging.promise);
		fireEvent.keyDown(textbox, { key: "Enter" });
		await screen.findByText("Sends when the files are ready");

		await type(textbox, "now");
		staging.resolve(staged(attachment("agent-a3.md")));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Ask ${MARK} now`, attachments: [EXCERPT], refs: [farRef(EXCERPT)] },
				"queue"
			)
		);
	});

	it("doesn't send when the excerpt can't be taken, and says why", async () => {
		const { source, textbox } = await asking();
		source.excerptAgent.mockRejectedValueOnce(new Error("devbox isn't connected"));
		fireEvent.keyDown(textbox, { key: "Enter" });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't get an excerpt of Login flow. Devbox isn't connected. Send again, or remove the agent."
		);
		expect(source.sendInput).not.toHaveBeenCalled();
		expect(textIn(textbox)).toBe(`Ask ${MARK} `);

		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Ask ${MARK}`, attachments: [EXCERPT], refs: [farRef(EXCERPT)] },
				"queue"
			)
		);
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("doesn't send when the excerpt can't be staged", async () => {
		const { source, textbox } = await asking();
		source.stagePasted.mockResolvedValueOnce({
			status: "error",
			error: { kind: "failed", message: "The disk is full" },
		});
		fireEvent.keyDown(textbox, { key: "Enter" });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			/^Couldn't get an excerpt of Login flow\. .*Send again, or remove the agent\.$/
		);
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("uploads the excerpt to a remote agent, and Cancel upload stops the send", async () => {
		const { source, textbox } = await asking({ links: [remoteLink()], on: REMOTE });
		// From devbox, the agent on this Mac is the one on another device.
		dropAgent(textbox, other, 7);
		const upload = deferred<ActionResult>();
		source.uploadPasted.mockReturnValueOnce(upload.promise);
		fireEvent.keyDown(textbox, { key: "Enter" });

		await screen.findByText("Sends when the upload finishes");
		expect(source.excerptAgent).toHaveBeenCalledWith("a2", 40_000);
		expect(source.uploadPasted).toHaveBeenCalledWith(
			REMOTE,
			{ type: "agent", agent: AGENT },
			"agent-a2.md",
			"text/markdown",
			expect.any(Uint8Array),
			expect.any(Function)
		);
		const strip = screen.getByRole("group", { name: "Upload" });
		fireEvent.click(within(strip).getByRole("button", { name: "Cancel upload" }));

		await waitFor(() =>
			expect(screen.queryByText("Sends when the upload finishes")).toBeNull()
		);
		expect(screen.queryByRole("group", { name: "Upload" })).toBeNull();
		expect(source.sendInput).not.toHaveBeenCalled();
		expect(pillsIn(textbox)).toHaveLength(2);
	});
});

describe("a queued message with pills", () => {
	const busy = heldAgent({ activity: { state: "busy" } });
	const queued = [{ ...queuedMessage("q1", `Ask ${MARK} about it`, 3), refs: [otherRef] }];
	const bubble = () =>
		within(screen.getByRole("list", { name: "Queued messages" })).getAllByRole("listitem")[0]!;

	it("shows each pill's words in the queue", async () => {
		renderComposer({ agent: busy, queued });
		await findTextbox();
		expect(screen.getByText("Ask Sidebar sort about it")).toBeInTheDocument();
	});

	it("comes back for editing with its pills", async () => {
		const { source, upsert } = renderComposer({ agent: busy, queued });
		const textbox = await findTextbox();
		upsert(entities({ agents: [other] }));
		fireEvent.click(within(bubble()).getByRole("button", { name: "Edit" }));

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q1"));
		await waitFor(() => expect(textIn(textbox)).toBe(`Ask ${MARK} about it`));
		expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Sidebar sort/)]);
	});

	it("comes back without the excerpt it went with", async () => {
		const excerpt = attachment("agent-a2.md");
		const withExcerpt = [
			{
				...queuedMessage("q1", `Ask ${MARK} about it`, 3, [excerpt]),
				refs: [{ ...otherRef, device: "dev-other", excerpt: excerpt.id }],
			},
		];
		const { source, upsert } = renderComposer({ agent: busy, queued: withExcerpt });
		const textbox = await findTextbox();
		upsert(entities({ agents: [other] }));
		expect(within(bubble()).queryByText(/attachment/)).toBeNull();
		fireEvent.click(within(bubble()).getByRole("button", { name: "Edit" }));

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q1"));
		await waitFor(() => expect(textIn(textbox)).toBe(`Ask ${MARK} about it`));
		expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Sidebar sort/)]);
	});

	it("steers with its refs", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		await findTextbox();
		fireEvent.click(within(bubble()).getByRole("button", { name: "Steer now" }));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Ask ${MARK} about it`, attachments: [], refs: [otherRef] },
				"steer"
			)
		);
	});
});
