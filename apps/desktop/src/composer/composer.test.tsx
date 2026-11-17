// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { ActionResult } from "@maxhq/shared/data/source";
import { takeRequest, watchRequests } from "@/agent-view/request";
import { NativeFailure } from "@/native";
import { AUTO_CONSENT_KEY } from "@/settings/auto-consent";
import { OBJECT_MARK as MARK, type QuoteRef } from "@maxhq/shared/pills/draft";
import { QUOTE_MIME } from "@maxhq/shared/pills/quote";
import { account, entities, workspace } from "@maxhq/shared/test/build";

import { attachFailure, intakeFailure } from "./failure";
import { PASTED_TEXT_ATTACHMENT_THRESHOLD_BYTES } from "./paste";
import { quoteInbox } from "./quote-inbox";
import { failureOf } from "@maxhq/shared/composer/send";
import { DEFAULT_TYPING, writeTypingPrefs } from "@maxhq/shared/editor/typing";
import {
	AGENT,
	attachment,
	clipboard,
	DEVICE,
	DONE,
	deferred,
	editorOf,
	fakeNative,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	queuedMessage,
	REMOTE,
	remoteLink,
	renderComposer,
	SHOT_DRAG,
	staged,
	textIn,
	type,
} from "./testing";
import { TYPING_KEY } from "./typing-prefs";

const busy = heldAgent({ activity: { state: "busy" } });
const prompted = heldAgent({ activity: { state: "needs_input", attention: "att-1" } });

/** The composer's box, around its text box. */
const boxAround = (textbox: HTMLElement): HTMLElement => {
	const box = [...document.querySelectorAll<HTMLElement>(".composer > div")].find((each) =>
		each.contains(textbox)
	);
	if (!box) throw new Error("The text box has no box");
	return box;
};

beforeAll(installBrowserFakes);

describe("sending", () => {
	it("sends on Enter while the agent is idle, then clears", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		expect(screen.getByRole("button", { name: /^Send/ })).toBeDisabled();

		await type(textbox, "Run the tests");
		expect(screen.getByRole("button", { name: /^Send/ })).toBeEnabled();
		fireEvent.keyDown(textbox, { key: "Enter" });

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "Run the tests", attachments: [] },
				"queue"
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));
	});

	it("breaks the line on Shift+Enter instead of sending", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "First");
		fireEvent.keyDown(textbox, { key: "Enter", shiftKey: true });
		await type(textbox, "Second");

		expect(textIn(textbox)).toBe("First\nSecond");
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("passes Command+F on to find and keeps the text", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "Half a thought");
		const passed = fireEvent.keyDown(textbox, { key: "f", code: "KeyF", metaKey: true });

		expect(passed).toBe(true);
		expect(textIn(textbox)).toBe("Half a thought");
	});

	it("writes its keys with Control off a Mac", async () => {
		renderComposer({ agent: busy });
		await findTextbox();
		const attach = screen.getByRole("button", { name: "Attach files" });
		act(() => attach.focus());
		expect(await screen.findByText("Attach files or folders (Ctrl+U)")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: /^Steer/ })).toHaveTextContent("SteerCtrl+Enter");
	});

	it("queues on Enter during a turn, and steers from the button or Command+Enter", async () => {
		const { source } = renderComposer({ agent: busy });
		const textbox = await findTextbox();
		expect(screen.queryByRole("button", { name: /^Send/ })).not.toBeInTheDocument();

		await type(textbox, "Also fix the header");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(AGENT, expect.anything(), "queue")
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));

		await type(textbox, "Use the new font");
		fireEvent.click(screen.getByRole("button", { name: /^Steer/ }));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(
				AGENT,
				{ text: "Use the new font", attachments: [] },
				"steer"
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));

		await type(textbox, "And the footer");
		fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(
				AGENT,
				{ text: "And the footer", attachments: [] },
				"steer"
			)
		);
		expect(source.sendInput).toHaveBeenCalledTimes(3);
	});

	describe("with Steer picked for Enter", () => {
		const pickSteer = () =>
			localStorage.setItem(
				TYPING_KEY,
				writeTypingPrefs({ ...DEFAULT_TYPING, busySend: "steer" })
			);

		it("steers on Enter during a turn, and queues from the button or Command+Enter", async () => {
			pickSteer();
			const { source } = renderComposer({ agent: busy });
			const textbox = await findTextbox();
			const sent = (text: string, mode: string) =>
				waitFor(() =>
					expect(source.sendInput).toHaveBeenLastCalledWith(
						AGENT,
						{ text, attachments: [] },
						mode
					)
				);

			await type(textbox, "Use the new font");
			fireEvent.keyDown(textbox, { key: "Enter" });
			await sent("Use the new font", "steer");
			await waitFor(() => expect(textIn(textbox)).toBe(""));

			await type(textbox, "And the footer");
			fireEvent.keyDown(textbox, { key: "Enter", ctrlKey: true });
			await sent("And the footer", "queue");
			await waitFor(() => expect(textIn(textbox)).toBe(""));

			await type(textbox, "Then lint");
			fireEvent.click(screen.getByRole("button", { name: /^Queue/ }));
			await sent("Then lint", "queue");
			expect(source.sendInput).toHaveBeenCalledTimes(3);
		});

		it("still queues on Enter and sends now on Command+Enter while a prompt waits", async () => {
			pickSteer();
			const { source } = renderComposer({ agent: prompted });
			const textbox = await findTextbox();
			await type(textbox, "Then run the linter");
			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenLastCalledWith(AGENT, expect.anything(), "queue")
			);
			await waitFor(() => expect(textIn(textbox)).toBe(""));
			await type(textbox, "Skip it");
			fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenLastCalledWith(
					AGENT,
					expect.anything(),
					"interrupt"
				)
			);
		});

		it("sends on Enter and Command+Enter alike while the agent is idle", async () => {
			pickSteer();
			const { source } = renderComposer();
			const textbox = await findTextbox();
			await type(textbox, "Hello");
			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenLastCalledWith(AGENT, expect.anything(), "queue")
			);
			await waitFor(() => expect(textIn(textbox)).toBe(""));
			await type(textbox, "Again");
			fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
			await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(2));
			expect(source.sendInput).toHaveBeenLastCalledWith(AGENT, expect.anything(), "queue");
		});
	});

	it("stops the turn", async () => {
		const { source, update } = renderComposer();
		await findTextbox();
		expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();

		update(busy);
		fireEvent.click(screen.getByRole("button", { name: "Stop" }));
		await waitFor(() => expect(source.interrupt).toHaveBeenCalledWith(AGENT));
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("stops the turn on Command and period from anywhere in the agent's pane", async () => {
		const mac = vi
			.spyOn(navigator, "userAgent", "get")
			.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
		const pane = document.createElement("section");
		const inside = document.createElement("button");
		pane.append(inside);
		document.body.append(pane);
		const elsewhere = document.createElement("button");
		document.body.append(elsewhere);
		const stopKey = (on: Element, extra: { repeat?: boolean } = {}) =>
			fireEvent.keyDown(on, { key: ".", code: "Period", metaKey: true, ...extra });
		try {
			const { source, update } = renderComposer({ dropTarget: pane });
			await findTextbox();
			inside.focus();
			// Nothing runs, so the key passes on.
			expect(stopKey(inside)).toBe(true);

			update(busy);
			act(() => screen.getByRole("button", { name: "Stop" }).focus());
			expect(await screen.findByText("Stop this turn ⌘.")).toBeInTheDocument();
			elsewhere.focus();
			stopKey(elsewhere);
			expect(source.interrupt).not.toHaveBeenCalled();

			inside.focus();
			stopKey(inside, { repeat: true });
			expect(source.interrupt).not.toHaveBeenCalled();
			expect(stopKey(inside)).toBe(false);
			await waitFor(() => expect(source.interrupt).toHaveBeenCalledWith(AGENT));
			expect(source.interrupt).toHaveBeenCalledOnce();
		} finally {
			pane.remove();
			elsewhere.remove();
			mac.mockRestore();
		}
	});

	it("queues while a permission prompt waits, and sends now past it from the button or Command+Enter", async () => {
		const { source } = renderComposer({ agent: prompted });
		const textbox = await findTextbox();
		expect(
			screen.getByText("Anything you send now waits until Claude's next turn.")
		).toBeInTheDocument();
		// The link asks the agent's list to go to the card waiting on you.
		const heard = vi.fn();
		const unwatch = watchRequests(AGENT, heard);
		fireEvent.click(screen.getByRole("button", { name: "Go to the request" }));
		unwatch();
		expect(heard).toHaveBeenCalledOnce();
		expect(takeRequest(AGENT)).toMatchObject({ attention: "att-1" });
		expect(screen.getByText("⏎ to queue, Ctrl+Enter to send now")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /^Steer/ })).not.toBeInTheDocument();

		await type(textbox, "Then run the linter");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(
				AGENT,
				{ text: "Then run the linter", attachments: [] },
				"queue"
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));

		await type(textbox, "Skip that, run the tests");
		const now = screen.getByRole("button", { name: "Send now" });
		act(() => now.focus());
		expect(
			await screen.findByText("Skips what's waiting and sends this now")
		).toBeInTheDocument();
		fireEvent.click(now);
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(
				AGENT,
				{ text: "Skip that, run the tests", attachments: [] },
				"interrupt"
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));

		await type(textbox, "And the linter");
		fireEvent.keyDown(textbox, { key: "Enter", metaKey: true });
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenLastCalledWith(
				AGENT,
				{ text: "And the linter", attachments: [] },
				"interrupt"
			)
		);
		expect(source.sendInput).toHaveBeenCalledTimes(3);
		expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
	});

	it("offers Send now only while a prompt waits", async () => {
		renderComposer({ agent: busy });
		await findTextbox();
		expect(screen.queryByRole("button", { name: "Send now" })).not.toBeInTheDocument();
		expect(screen.queryByText(/to send now/)).not.toBeInTheDocument();
	});

	it("keeps the text and says why when sending fails", async () => {
		const { source } = renderComposer();
		source.sendInput.mockResolvedValueOnce({
			status: "error",
			error: { kind: "not_connected", device: "Mac" },
		});
		const textbox = await findTextbox();
		await type(textbox, "Ship it");
		fireEvent.keyDown(textbox, { key: "Enter" });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't send the message. Mac isn't connected."
		);
		expect(textIn(textbox)).toBe("Ship it");
	});

	it("says what didn't happen and why, for each reason", () => {
		const error = (e: Extract<ActionResult, { status: "error" }>["error"]): ActionResult => ({
			status: "error",
			error: e,
		});
		const outcome = (data: Extract<ActionResult, { status: "ok" }>["data"]): ActionResult => ({
			status: "ok",
			data,
		});

		expect(failureOf(error({ kind: "not_connected", device: "devbox" }), "steer")).toBe(
			"Couldn't steer the turn. devbox isn't connected."
		);
		expect(failureOf(error({ kind: "not_found", what: "agent a1" }), "stop")).toBe(
			"Couldn't stop the turn. Agent a1 isn't there anymore."
		);
		expect(failureOf(error({ kind: "failed", message: "broken pipe." }), "model")).toBe(
			"Couldn't change the model. Broken pipe."
		);
		expect(
			failureOf(outcome({ status: "refused", reason: "it already went out" }), "unqueue")
		).toBe("Couldn't take the message back. It already went out.");
		// maxd's reason reads as a sentence of its own.
		expect(
			failureOf(
				outcome({
					status: "refused",
					reason: "couldn't restart Cursor: the folder /tmp/project is gone",
				}),
				"send"
			)
		).toBe(
			"Couldn't send the message. Couldn't restart Cursor: the folder /tmp/project is gone."
		);
		expect(failureOf(outcome({ status: "unknown_agent" }), "permission")).toBe(
			"Couldn't change the permission mode. MaxHQ no longer knows this agent."
		);
		expect(failureOf(DONE, "send")).toBeNull();
		// A chip shows the reason alone.
		expect(
			failureOf(outcome({ status: "refused", reason: "the pasted bytes are over 8 MiB" }))
		).toBe("The pasted bytes are over 8 MiB");
	});

	it("can't send while the device is out of reach", async () => {
		const { source } = renderComposer({
			agent: heldAgent({ activity: { state: "not_accessible" } }),
		});
		const textbox = await findTextbox();
		await type(textbox, "Hello");
		expect(screen.getByRole("button", { name: /^Send/ })).toBeDisabled();
		fireEvent.keyDown(textbox, { key: "Enter" });
		await act(async () => {});
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("shows nothing for an agent started in your own terminal", async () => {
		renderComposer({ agent: heldAgent({ control: "observed" }) });
		await act(async () => {});
		expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
	});

	it("leaves spelling to the webview without native spell check", async () => {
		renderComposer();
		expect(await findTextbox()).toHaveAttribute("spellcheck", "true");
	});

	it("turns the webview's spell check off when the system's runs", async () => {
		renderComposer({ native: fakeNative({ spelling: true }) });
		expect(await findTextbox()).toHaveAttribute("spellcheck", "false");
	});
});

describe("the draft", () => {
	it("keeps the text and files when the pane closes and opens again", async () => {
		const { drop } = renderComposer();
		let textbox = await findTextbox();
		await type(textbox, "What's wrong in ");
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]));
		await type(textbox, "@ma");
		expect(await screen.findByRole("listbox")).toBeInTheDocument();

		cleanup();
		const { source } = renderComposer();
		textbox = await findTextbox();
		expect(textIn(textbox)).toBe(`What's wrong in ${MARK} @ma`);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]);
		expect(screen.getByText(/^1 attachment,/)).toBeInTheDocument();
		// It comes back without taking the focus, an undo step or its menu.
		expect(textbox).not.toHaveFocus();
		expect(editorOf(textbox).can().undo()).toBe(false);
		expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `What's wrong in ${MARK} @ma`,
					attachments: ["att-main.rs"],
					refs: [{ type: "attachment", id: "att-main.rs" }],
				},
				"queue"
			)
		);
	});

	it("comes back after a restart as typed, with the files maxd still holds", async () => {
		const { source, drop } = renderComposer();
		// maxd names a staged copy with a UUIDv7, which carries when it staged.
		const hex = Date.now().toString(16).padStart(12, "0");
		const fresh = `${hex.slice(0, 8)}-${hex.slice(8)}-7abc-8def-0123456789ab`;
		source.stageAttachments.mockImplementation(async (_device, paths) =>
			paths.map((path) => {
				const name = path.split("/").at(-1) ?? path;
				return staged(attachment(name, name === "main.rs" ? { id: fresh } : {}));
			})
		);
		let textbox = await findTextbox();
		await type(textbox, "What's wrong in ");
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await type(textbox, "and ");
		act(() => drop().onDrop(["/Users/max/notes.txt"], { x: 0, y: 0 }));
		await waitFor(() => expect(source.stageAttachments).toHaveBeenCalledTimes(2));
		await act(() => Promise.resolve());
		await type(textbox, "@ma");
		expect(textIn(textbox)).toBe(`What's wrong in ${MARK} and ${MARK} @ma`);

		cleanup();
		clearKept();
		const restarted = renderComposer();
		textbox = await findTextbox();
		// The copy maxd may have deleted comes back as its name, to drop again.
		expect(textIn(textbox)).toBe(`What's wrong in ${MARK} and [File: notes.txt] @ma`);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]);
		expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
		expect(restarted.source.stageAttachments).not.toHaveBeenCalled();

		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		await waitFor(() =>
			expect(restarted.source.sendInput).toHaveBeenCalledWith(
				AGENT,
				expect.objectContaining({ attachments: [fresh] }),
				"queue"
			)
		);
	});

	it("keeps each agent's draft apart", async () => {
		renderComposer();
		await type(await findTextbox(), "For Fix login");

		cleanup();
		renderComposer({ agent: heldAgent({ id: "a2", title: "Tidy docs" }) });
		expect(textIn(await findTextbox())).toBe("");
	});

	it("forgets the draft once it's sent", async () => {
		renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "Run the tests");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() => expect(textIn(textbox)).toBe(""));

		cleanup();
		renderComposer();
		expect(textIn(await findTextbox())).toBe("");

		cleanup();
		clearKept();
		renderComposer();
		expect(textIn(await findTextbox())).toBe("");
	});

	it("forgets the draft that went out after its pane closed", async () => {
		const { source } = renderComposer();
		const sending = deferred<ActionResult>();
		source.sendInput.mockReturnValueOnce(sending.promise);
		const textbox = await findTextbox();
		await type(textbox, "Run the tests");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() => expect(source.sendInput).toHaveBeenCalled());

		cleanup();
		await act(async () => sending.resolve(DONE));
		renderComposer();
		expect(textIn(await findTextbox())).toBe("");

		cleanup();
		clearKept();
		renderComposer();
		expect(textIn(await findTextbox())).toBe("");
	});
});

describe("queued messages", () => {
	const queued = [
		queuedMessage("q1", "Then update the docs", 4),
		queuedMessage("q2", "And bump the version", 6, [attachment("CHANGELOG.md")]),
	];

	const bubbles = () =>
		within(screen.getByRole("list", { name: "Queued messages" })).getAllByRole("listitem");

	it("lists what waits, oldest first", async () => {
		renderComposer({ agent: busy, queued: [queued[1]!, queued[0]!] });
		await findTextbox();
		expect(bubbles().map((bubble) => bubble.textContent)).toEqual([
			expect.stringContaining("Then update the docs"),
			expect.stringContaining("And bump the version1 attachment"),
		]);
		expect(bubbles()[0]).toHaveTextContent("Queued, sends when this turn ends");
	});

	it("takes a message back for editing, attachments and all", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		const textbox = await findTextbox();
		await type(textbox, "draft");

		fireEvent.click(within(bubbles()[1]!).getByRole("button", { name: "Edit" }));

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q2"));
		await waitFor(() => expect(textIn(textbox)).toBe(`And bump the version ${MARK}\n\ndraft`));
		expect(bubbles()).toHaveLength(1);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("CHANGELOG.md")]);
	});

	it("shows the preview maxd made for a picture it takes back", async () => {
		const preview = "data:image/jpeg;base64,/9j/4AAQSkZJRg==";
		const shot = attachment("shot.png", {
			kind: "image",
			mime: "image/png",
			thumbnail: preview,
		});
		renderComposer({ agent: busy, queued: [queuedMessage("q3", "Like this", 4, [shot])] });
		await findTextbox();

		fireEvent.click(within(bubbles()[0]!).getByRole("button", { name: "Edit" }));

		const pill = await screen.findByRole("button", { name: /^Image 1, shot\.png/ });
		expect(pill.querySelector("img")).toHaveAttribute("src", preview);
	});

	it("steers a queued message into the turn now", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		await findTextbox();

		fireEvent.click(within(bubbles()[1]!).getByRole("button", { name: "Steer now" }));

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "And bump the version", attachments: ["att-CHANGELOG.md"] },
				"steer"
			)
		);
		expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q2");
		expect(bubbles()).toHaveLength(1);
	});

	it("sends a queued message now past the prompt that waits, in place of Steer now", async () => {
		const { source } = renderComposer({ agent: prompted, queued });
		await findTextbox();
		expect(within(bubbles()[0]!).queryByRole("button", { name: "Steer now" })).toBeNull();

		fireEvent.click(within(bubbles()[1]!).getByRole("button", { name: "Send now" }));

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: "And bump the version", attachments: ["att-CHANGELOG.md"] },
				"interrupt"
			)
		);
		expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q2");
		expect(bubbles()).toHaveLength(1);
	});

	it("offers neither while the agent idles", async () => {
		renderComposer({ queued });
		await findTextbox();
		const [bubble] = bubbles();
		expect(within(bubble!).queryByRole("button", { name: "Steer now" })).toBeNull();
		expect(within(bubble!).queryByRole("button", { name: "Send now" })).toBeNull();
	});

	it("removes a queued message", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		await findTextbox();

		fireEvent.click(within(bubbles()[0]!).getByRole("button", { name: "Remove" }));

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q1"));
		await waitFor(() => expect(bubbles()).toHaveLength(1));
		expect(bubbles()[0]).toHaveTextContent("And bump the version");
	});

	it("keeps a message that couldn't be taken back", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		source.cancelQueued.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "It already went out" },
		});
		const textbox = await findTextbox();

		fireEvent.click(within(bubbles()[0]!).getByRole("button", { name: "Edit" }));

		expect(await screen.findByRole("alert")).toHaveTextContent("It already went out");
		expect(bubbles()).toHaveLength(2);
		expect(textIn(textbox)).toBe("");
	});

	it("edits the last queued message on Up in an empty composer", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		const textbox = await findTextbox();

		fireEvent.keyDown(textbox, { key: "ArrowUp" });

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q2"));
		await waitFor(() => expect(textIn(textbox)).toBe(`And bump the version ${MARK}`));
	});

	it("leaves Up alone once there's text", async () => {
		const { source } = renderComposer({ agent: busy, queued });
		const textbox = await findTextbox();
		await type(textbox, "draft");

		fireEvent.keyDown(textbox, { key: "ArrowUp" });
		await act(async () => {});
		expect(source.cancelQueued).not.toHaveBeenCalled();
	});
});

describe("attachments", () => {
	it("says under an image that a local ACP agent gets it as a file path", async () => {
		const note = "Gemini doesn't take images, so they go as file paths in the message.";
		const attach = async (images: boolean) => {
			const gemini = account({
				id: "a-acp",
				vendor: "acp",
				label: "Gemini",
				acp: { images },
			});
			const { drop, source } = renderComposer({
				agent: heldAgent({ vendor: "acp", account: "a-acp" }),
				account: gemini,
			});
			source.stageAttachments.mockReturnValueOnce(new Promise(() => {}));
			const textbox = await findTextbox();
			act(() => drop().onDrop(["/Users/max/shot.png"], { x: 0, y: 0 }));
			await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		};
		await attach(false);
		expect(screen.getByText(note)).toBeInTheDocument();
		cleanup();
		await attach(true);
		expect(screen.queryByText(note)).not.toBeInTheDocument();
	});

	it("says under an image that a model that can't read images gets its file path", async () => {
		const note = "GLM 5.3 Flash can't see images. The agent gets the file path instead.";
		const cursor = account({
			id: "a-cursor",
			vendor: "cursor",
			label: "Cursor",
			models: [
				{ id: "composer-2.5", label: "Composer 2.5", default: true },
				{ id: "glm-5p3-flash", label: "GLM 5.3 Flash", default: false, no_images: true },
			],
		});
		const attach = async (model: string) => {
			const { drop, source } = renderComposer({
				agent: heldAgent({ vendor: "cursor", account: "a-cursor", model }),
				account: cursor,
			});
			source.stageAttachments.mockReturnValueOnce(new Promise(() => {}));
			const textbox = await findTextbox();
			expect(screen.queryByText(note)).not.toBeInTheDocument();
			act(() => drop().onDrop(["/Users/max/shot.png"], { x: 0, y: 0 }));
			await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		};
		await attach("glm-5p3-flash");
		expect(screen.getByText(note)).toBeInTheDocument();
		cleanup();
		await attach("composer-2.5");
		expect(screen.queryByText(note)).not.toBeInTheDocument();
	});

	it("takes a drop, shows the overlay while over, and sends once staged", async () => {
		const { source, drop } = renderComposer();
		const staging = deferred<ActionResult[]>();
		source.stageAttachments.mockReturnValueOnce(staging.promise);
		const textbox = await findTextbox();

		act(() => drop().onHover?.(true));
		expect(screen.getByText("Drop to attach")).toBeInTheDocument();
		expect(screen.getByText("For Fix login on Mac")).toBeInTheDocument();
		act(() => drop().onHover?.(false));
		expect(screen.queryByText("Drop to attach")).not.toBeInTheDocument();

		act(() => drop().onDrop(["/Users/max/shot.png", "/Users/max/main.rs"], { x: 0, y: 0 }));
		expect(source.stageAttachments).toHaveBeenCalledWith(DEVICE, [
			"/Users/max/shot.png",
			"/Users/max/main.rs",
		]);
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([
				"Image 1, shot.png, getting ready",
				expect.stringMatching(/^File, main\.rs, .*getting ready$/),
			])
		);
		expect(screen.getByText("2 attachments")).toBeInTheDocument();

		await type(textbox, "What's wrong here?");
		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		expect(await screen.findByText("Sends when the files are ready")).toBeInTheDocument();
		expect(source.sendInput).not.toHaveBeenCalled();

		await act(async () => {
			staging.resolve([
				staged(attachment("shot.png", { kind: "image" })),
				staged(attachment("main.rs")),
			]);
		});
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `${MARK} ${MARK} What's wrong here?`,
					attachments: ["att-shot.png", "att-main.rs"],
					refs: [
						{ type: "attachment", id: "att-shot.png" },
						{ type: "attachment", id: "att-main.rs" },
					],
				},
				"queue"
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));
		expect(pillsIn(textbox)).toEqual([]);
	});

	it("attaches what the picker returns, from the button or Command+U", async () => {
		const native = fakeNative();
		native.native.pickAttachments
			.mockResolvedValueOnce(["/Users/max/spec.pdf"])
			.mockResolvedValueOnce(["/Users/max/src/"]);
		const { source } = renderComposer({ native });
		const textbox = await findTextbox();

		fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		fireEvent.keyDown(textbox, { key: "u", metaKey: true });
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(2));

		expect(source.stageAttachments).toHaveBeenNthCalledWith(1, DEVICE, ["/Users/max/spec.pdf"]);
		expect(source.stageAttachments).toHaveBeenNthCalledWith(2, DEVICE, ["/Users/max/src/"]);
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([
				expect.stringContaining("spec.pdf"),
				expect.stringContaining("src"),
			])
		);
		expect(textIn(textbox)).toBe(`${MARK} ${MARK} `);
	});

	it("leaves out a file whose pill Backspace removed", async () => {
		const { source, drop } = renderComposer();
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/Users/max/a.rs", "/Users/max/b.rs"], { x: 0, y: 0 }));
		await waitFor(() => expect(screen.getByText("2 attachments")).toBeInTheDocument());

		// Right after the second pill, past the space between them.
		act(() => {
			editorOf(textbox).commands.setTextSelection(4);
		});
		fireEvent.keyDown(textbox, { key: "Backspace" });
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("a.rs")]);
		expect(screen.getByText(/^1 attachment,/)).toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: MARK,
					attachments: ["att-a.rs"],
					refs: [{ type: "attachment", id: "att-a.rs" }],
				},
				"queue"
			)
		);
	});

	it("won't send with a chip that failed", async () => {
		const { source, drop } = renderComposer();
		source.stageAttachments.mockResolvedValueOnce([
			{ status: "ok", data: { status: "refused", reason: "No such file" } },
		]);
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/Users/max/gone.txt"], { x: 0, y: 0 }));
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringContaining("failed, No such file")])
		);

		await type(textbox, "Read this");
		fireEvent.keyDown(textbox, { key: "Enter" });
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't attach gone.txt. No such file. Remove it, then send again."
		);
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("names the shared reason when several files couldn't be attached", () => {
		const chip = (name: string, error: string | null) => ({ name, error });
		expect(
			attachFailure([
				chip("a.rs", "Mac isn't connected"),
				chip("b.rs", "Mac isn't connected"),
			])
		).toBe("Couldn't attach 2 files. Mac isn't connected. Remove them, then send again.");
		expect(attachFailure([chip("a.rs", "Too big to send"), chip("b.rs", null)])).toBe(
			"Couldn't attach 2 files. Remove them, then send again."
		);
		expect(attachFailure([chip("a.rs", null)])).toBe(
			"Couldn't attach a.rs. Remove it, then send again."
		);
	});

	it("takes drops on the element it's given, such as the agent's pane", async () => {
		const pane = document.createElement("section");
		const { native, source, drop } = renderComposer({ dropTarget: pane });
		await findTextbox();

		expect(native.registerDropTarget).toHaveBeenCalledWith(pane, expect.anything());
		act(() => drop().onHover?.(true));
		expect(screen.getByText("Drop to attach")).toBeInTheDocument();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		expect(source.stageAttachments).toHaveBeenCalledWith(DEVICE, ["/Users/max/main.rs"]);
	});

	it("waits for a drop target that hasn't mounted yet", async () => {
		const { native } = renderComposer({ dropTarget: null });
		await findTextbox();
		expect(native.registerDropTarget).not.toHaveBeenCalled();
	});

	it("attaches a file dragged out of a timeline, again each time", async () => {
		const { source, drop } = renderComposer();
		source.attachFromAgent.mockResolvedValue(
			staged(attachment("shot.png", { kind: "image", mime: "image/png" }))
		);
		const textbox = await findTextbox();
		expect(drop().takesPageDrag?.(SHOT_DRAG, { x: 0, y: 0 })).toBe(true);
		const quote = new Map([["application/x-maxhq-quote", "{}"]]);
		expect(drop().takesPageDrag?.(quote, { x: 0, y: 0 })).toBe(false);

		act(() => drop().onPageDrop?.(SHOT_DRAG, { x: 0, y: 0 }));
		expect(source.attachFromAgent).toHaveBeenCalledWith(
			DEVICE,
			null,
			"a2",
			"/attachments/a2/shot.png",
			expect.any(Function)
		);
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Image 1, shot\.png/)])
		);

		// The same file dropped again goes in again, so the drop never looks lost.
		act(() => drop().onPageDrop?.(SHOT_DRAG, { x: 0, y: 0 }));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(2));
		expect(source.attachFromAgent).toHaveBeenCalledTimes(2);
		expect(source.stageAttachments).not.toHaveBeenCalled();
	});

	it("sends a dragged file on to an agent on another device", async () => {
		const { source, drop } = renderComposer({ links: [remoteLink()], on: REMOTE });
		await findTextbox();
		act(() => drop().onPageDrop?.(SHOT_DRAG, { x: 0, y: 0 }));
		expect(source.attachFromAgent).toHaveBeenCalledWith(
			REMOTE,
			{ type: "agent", agent: AGENT },
			"a2",
			"/attachments/a2/shot.png",
			expect.any(Function)
		);
	});

	it("takes a file from its own pane's timeline only over the box", async () => {
		const pane = document.createElement("section");
		const { drop } = renderComposer({ dropTarget: pane });
		const textbox = await findTextbox();
		vi.spyOn(boxAround(textbox), "getBoundingClientRect").mockReturnValue(
			new DOMRect(0, 400, 600, 120)
		);
		expect(drop().takesPageDrag?.(SHOT_DRAG, { x: 300, y: 200 })).toBe(false);
		expect(drop().takesPageDrag?.(SHOT_DRAG, { x: 300, y: 450 })).toBe(true);
	});

	it("says why the picker didn't open, and what to do instead", async () => {
		const native = fakeNative();
		native.native.pickAttachments
			.mockRejectedValueOnce(
				new NativeFailure({ kind: "unsupported", what: "the file and folder picker" })
			)
			.mockRejectedValueOnce(
				new NativeFailure({
					kind: "failed",
					message: "the picker closed without an answer",
				})
			);
		renderComposer({ native });
		await findTextbox();

		fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"The file and folder picker isn't available on this system. Drop or paste the files instead."
		);
		fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
		await waitFor(() =>
			expect(screen.getByRole("alert")).toHaveTextContent(
				"Couldn't open the file picker. The picker closed without an answer."
			)
		);
	});

	it("says why the clipboard couldn't be read, and still pastes what the webview has", async () => {
		const native = fakeNative();
		native.native.readClipboardFiles.mockRejectedValueOnce(
			new NativeFailure({ kind: "not_offered", path: "/Users/max/old-notes.txt" })
		);
		const { source } = renderComposer({ native });
		const textbox = await findTextbox();
		const file = new File(["hello"], "notes.txt", { type: "text/plain" });

		fireEvent.paste(textbox, { clipboardData: clipboard({ files: [file], types: ["Files"] }) });

		expect(await screen.findByRole("alert")).toHaveTextContent(
			"MaxHQ won't read old-notes.txt, since it wasn't dropped, pasted or picked."
		);
		await waitFor(() =>
			expect(source.stagePasted).toHaveBeenCalledWith(
				DEVICE,
				"notes.txt",
				"text/plain",
				expect.any(Uint8Array)
			)
		);
	});

	it("words a failed paste that isn't the native module's", () => {
		expect(intakeFailure(new Error("pasteboard is busy"), "paste")).toBe(
			"Couldn't read the clipboard. Pasteboard is busy."
		);
	});

	it("pastes files copied in Finder as paths", async () => {
		const native = fakeNative();
		native.native.readClipboardFiles.mockResolvedValueOnce(["/Users/max/report.pdf"]);
		const { source } = renderComposer({ native });
		const textbox = await findTextbox();

		fireEvent.paste(textbox, {
			clipboardData: clipboard({ text: "report.pdf", types: ["Files"] }),
		});

		await waitFor(() =>
			expect(source.stageAttachments).toHaveBeenCalledWith(DEVICE, ["/Users/max/report.pdf"])
		);
		expect(textIn(textbox)).toBe(`${MARK} `);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("report.pdf")]);
	});

	it("pastes an image from the clipboard as bytes", async () => {
		const native = fakeNative();
		native.native.readClipboardImage.mockResolvedValueOnce(new Uint8Array([137, 80, 78, 71]));
		const { source } = renderComposer({ native });
		const textbox = await findTextbox();

		fireEvent.paste(textbox, { clipboardData: clipboard({ types: ["Files"] }) });

		await waitFor(() =>
			expect(source.stagePasted).toHaveBeenCalledWith(
				DEVICE,
				"pasted-image.png",
				"image/png",
				new Uint8Array([137, 80, 78, 71])
			)
		);
		expect(
			await screen.findByRole("button", { name: /^Image 1, pasted-image\.png/ })
		).toBeInTheDocument();
	});

	it("falls back to the pasted files the webview has", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		const file = new File(["hello"], "notes.txt", { type: "text/plain" });

		fireEvent.paste(textbox, { clipboardData: clipboard({ files: [file], types: ["Files"] }) });

		await waitFor(() =>
			expect(source.stagePasted).toHaveBeenCalledWith(
				DEVICE,
				"notes.txt",
				"text/plain",
				new TextEncoder().encode("hello")
			)
		);
	});

	it("turns a very long paste into a text file", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		const long = "log line\n".repeat(Math.ceil(PASTED_TEXT_ATTACHMENT_THRESHOLD_BYTES / 9) + 1);

		fireEvent.paste(textbox, { clipboardData: clipboard({ text: long }) });

		await waitFor(() =>
			expect(source.stagePasted).toHaveBeenCalledWith(
				DEVICE,
				"pasted-text.txt",
				"text/plain",
				expect.any(Uint8Array)
			)
		);
		expect(textIn(textbox)).toBe(`${MARK} `);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("pasted-text.txt")]);
	});

	it("pastes ordinary text as text, line breaks and all", async () => {
		const { source } = renderComposer();
		const textbox = await findTextbox();

		fireEvent.paste(textbox, { clipboardData: clipboard({ text: "one\ntwo" }) });

		expect(textIn(textbox)).toBe("one\ntwo");
		expect(source.stagePasted).not.toHaveBeenCalled();
	});
});

/** A passage quoted from agent `from`'s timeline. */
const quoted = (from: string, text = "the part you want"): QuoteRef => ({
	type: "quote",
	agent: from,
	agent_title: from === AGENT ? "This agent" : "Fix login",
	item: "m1",
	text,
	position: 0,
	source: "reply",
	from: null,
});

/** A quote dragged out of a timeline, as the drop router hands it over. */
const quoteDrag = (quote: QuoteRef) => new Map([[QUOTE_MIME, JSON.stringify(quote)]]);

describe("quotes", () => {
	it("adds a quote another view sends, then each one after", async () => {
		quoteInbox.send(AGENT, quoted("a2", "one"), false);
		renderComposer();
		const textbox = await findTextbox();
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Quote from Fix login, /)])
		);

		act(() => quoteInbox.send(AGENT, quoted("a2", "two"), true));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(2));
		expect(quoteInbox.take(AGENT)).toEqual([]);
	});

	it("adds a quote dropped on it", async () => {
		const { drop } = renderComposer();
		const textbox = await findTextbox();
		const drag = quoteDrag(quoted("a2"));
		expect(drop().takesPageDrag?.(drag, { x: 0, y: 0 })).toBe(true);
		act(() => drop().onHover?.(true));
		expect(screen.getByText("Drop to add the quote")).toBeInTheDocument();
		act(() => drop().onHover?.(false));
		expect(screen.queryByText("Drop to add the quote")).not.toBeInTheDocument();

		act(() => drop().onPageDrop?.(drag, { x: 0, y: 0 }));
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Quote from Fix login, /)])
		);
	});

	it("adds a dropped quote at the caret the box kept, wherever the pointer lets go", async () => {
		const { drop } = renderComposer();
		const textbox = await findTextbox();
		await type(textbox, "See this");
		act(() => {
			editorOf(textbox).commands.setTextSelection(5);
			editorOf(textbox).commands.blur();
		});
		// Were the pointer to count, it would land at the start.
		editorOf(textbox).view.posAtCoords = () => ({ pos: 1, inside: -1 });
		const drag = quoteDrag(quoted("a2"));
		act(() => {
			drop().takesPageDrag?.(drag, { x: 10, y: 10 });
			drop().onHover?.(true);
		});
		expect(screen.getByText("Drop to add the quote at the cursor")).toBeInTheDocument();

		act(() => drop().onPageDrop?.(drag, { x: 10, y: 10 }));
		await waitFor(() => expect(textIn(textbox)).toBe(`See ${MARK} this`));
		expect(editorOf(textbox).state.selection.from).toBe(7);
	});

	it("takes its own agent's quote only over the box, and another's anywhere", async () => {
		const pane = document.createElement("section");
		const { drop } = renderComposer({ dropTarget: pane });
		const textbox = await findTextbox();
		vi.spyOn(boxAround(textbox), "getBoundingClientRect").mockReturnValue(
			new DOMRect(0, 400, 600, 120)
		);
		const own = quoteDrag(quoted(AGENT));
		expect(drop().takesPageDrag?.(own, { x: 300, y: 200 })).toBe(false);
		expect(drop().takesPageDrag?.(own, { x: 300, y: 450 })).toBe(true);
		expect(drop().takesPageDrag?.(quoteDrag(quoted("a2")), { x: 300, y: 200 })).toBe(true);
	});
});

describe("footer pickers", () => {
	it("changes the agent's model and permission mode", async () => {
		const claude = account({
			id: "acc",
			models: [
				{ id: "opus", label: "Opus 4.8", default: true },
				{ id: "sonnet", label: "Sonnet 4.6", default: false },
			],
		});
		const { source } = renderComposer({
			agent: heldAgent({ account: "acc" }),
			account: claude,
		});
		await findTextbox();

		fireEvent.click(screen.getByRole("button", { name: /^Model/ }));
		fireEvent.click(await screen.findByRole("menuitemradio", { name: /Sonnet 4\.6/ }));
		await waitFor(() =>
			expect(source.setModel).toHaveBeenCalledWith(AGENT, "sonnet", undefined)
		);

		fireEvent.click(screen.getByRole("button", { name: /^Permission mode/ }));
		fireEvent.click(await screen.findByRole("menuitemradio", { name: /Accept edits/ }));
		await waitFor(() =>
			expect(source.setPermission).toHaveBeenCalledWith(AGENT, "accept_edits")
		);
		expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
	});

	it("asks before Auto the first time, and not once you said yes", async () => {
		const { source } = renderComposer();
		await findTextbox();
		const pickAuto = async () => {
			// The menu stays open behind the question, so after Cancel another
			// click on the button would close it, not open it.
			const button = screen.getByRole("button", { name: /^Permission mode/ });
			if (button.getAttribute("aria-expanded") !== "true") fireEvent.click(button);
			const auto = await screen.findByRole("menuitemradio", { name: /Auto/ });
			expect(auto).toHaveTextContent("test, build and lint scripts without asking");
			fireEvent.click(auto);
		};

		await pickAuto();
		let dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent("Use Auto mode?");
		expect(dialog).toHaveTextContent("Use it on repos you trust.");
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
		expect(source.setPermission).not.toHaveBeenCalled();

		await pickAuto();
		dialog = await screen.findByRole("alertdialog");
		fireEvent.click(within(dialog).getByRole("button", { name: "Use Auto" }));
		await waitFor(() => expect(source.setPermission).toHaveBeenCalledWith(AGENT, "auto"));
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBe("confirmed");

		await pickAuto();
		await waitFor(() => expect(source.setPermission).toHaveBeenCalledTimes(2));
		expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
	});

	it("hides the permission picker until the agent reports a mode", async () => {
		renderComposer({ agent: heldAgent({ permission: null }) });
		await findTextbox();
		expect(screen.queryByRole("button", { name: /^Permission mode/ })).not.toBeInTheDocument();
	});
});

describe("sending in an Auto you didn't pick", () => {
	const inAuto = () =>
		renderComposer({
			agent: heldAgent({ permission: { requested: "auto", effective: null } }),
			more: entities({ workspaces: [workspace({ id: "w1", device: DEVICE })] }),
		});
	const sendAndAnswer = async (button: string) => {
		const textbox = await findTextbox();
		await type(textbox, "Run the tests");
		fireEvent.keyDown(textbox, { key: "Enter" });
		const dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent("This agent is set to Auto.");
		expect(dialog).toHaveTextContent("Use it on repos you trust.");
		fireEvent.click(within(dialog).getByRole("button", { name: button }));
	};

	it("sends in Auto once you say Use Auto, and doesn't ask again", async () => {
		const { source } = inAuto();
		await sendAndAnswer("Use Auto");
		await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(1));
		expect(source.setPermission).not.toHaveBeenCalled();
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBe("confirmed");
		await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());

		const textbox = await findTextbox();
		await type(textbox, "Again");
		fireEvent.keyDown(textbox, { key: "Enter" });
		await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(2));
		expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
	});

	it("switches the agent and the device's default with Ask first instead", async () => {
		const { source } = inAuto();
		await sendAndAnswer("Ask first instead");
		await waitFor(() => expect(source.sendInput).toHaveBeenCalledTimes(1));
		expect(source.setPermission).toHaveBeenCalledWith(AGENT, "ask");
		expect(source.setDeviceSettings).toHaveBeenCalledWith(
			expect.objectContaining({ device: DEVICE, default_permission: "ask" })
		);
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBeNull();
	});

	it("keeps the message when the agent can't leave Auto", async () => {
		const { source } = inAuto();
		source.setPermission.mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "It ended" },
		});
		await sendAndAnswer("Ask first instead");
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't change the permission mode. It ended."
		);
		expect(source.sendInput).not.toHaveBeenCalled();
		expect(textIn(await findTextbox())).toBe("Run the tests");
	});
});
