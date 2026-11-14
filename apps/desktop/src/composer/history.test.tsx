// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";

import { clearKept } from "@maxhq/shared/agent-view/kept";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";

import {
	AGENT,
	attachment,
	editorOf,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	queuedMessage,
	renderComposer,
	sentMessage,
	textIn,
	type,
} from "./testing";

beforeAll(installBrowserFakes);

const sent = [sentMessage("m1", "Fix the login", 2), sentMessage("m2", "Now the tests", 4)];

/** Presses a key in the box. True when the composer left it to the editor. */
const press = (textbox: HTMLElement, key: string) => fireEvent.keyDown(textbox, { key });

/** Puts the caret `at` a position in the box. */
const caretAt = (textbox: HTMLElement, at: number) =>
	act(() => {
		editorOf(textbox).commands.setTextSelection(at);
	});

describe("earlier messages on Up and Down", () => {
	it("brings back the last message you sent on Up in an empty composer", async () => {
		renderComposer({ sent });
		const textbox = await findTextbox();

		expect(press(textbox, "ArrowUp")).toBe(false);
		expect(textIn(textbox)).toBe("Now the tests");
	});

	it("walks back to the oldest and forward to the empty box", async () => {
		renderComposer({ sent });
		const textbox = await findTextbox();

		press(textbox, "ArrowUp");
		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Fix the login");
		expect(press(textbox, "ArrowUp")).toBe(true);
		expect(textIn(textbox)).toBe("Fix the login");

		press(textbox, "ArrowDown");
		expect(textIn(textbox)).toBe("Now the tests");
		press(textbox, "ArrowDown");
		expect(textIn(textbox)).toBe("");
		expect(press(textbox, "ArrowDown")).toBe(true);
	});

	it("keeps what you typed and puts it back with Down", async () => {
		renderComposer({ sent });
		const textbox = await findTextbox();
		await type(textbox, "half a thought");
		caretAt(textbox, 1);

		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Now the tests");
		press(textbox, "ArrowDown");
		expect(textIn(textbox)).toBe("half a thought");
	});

	it("saves what you typed, not the message it shows, for after a restart", async () => {
		renderComposer({ sent });
		let textbox = await findTextbox();
		await type(textbox, "half a thought");
		caretAt(textbox, 1);
		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Now the tests");

		cleanup();
		clearKept();
		renderComposer({ sent });
		textbox = await findTextbox();
		expect(textIn(textbox)).toBe("half a thought");
	});

	it("moves the caret, not the history, with the caret inside the text", async () => {
		renderComposer({ sent });
		const textbox = await findTextbox();
		await type(textbox, "half a thought");

		expect(press(textbox, "ArrowUp")).toBe(true);
		caretAt(textbox, 5);
		expect(press(textbox, "ArrowUp")).toBe(true);
		expect(textIn(textbox)).toBe("half a thought");
	});

	it("moves through a message's lines before going on to an older one", async () => {
		renderComposer({
			sent: [
				sentMessage("m1", "Fix the login", 2),
				sentMessage("m2", "Now the tests\nand the docs", 4),
			],
		});
		const textbox = await findTextbox();

		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Now the tests\nand the docs");
		expect(press(textbox, "ArrowUp")).toBe(true);

		caretAt(textbox, 1);
		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Fix the login");
	});

	it("takes back a queued message before any you sent", async () => {
		const { source } = renderComposer({
			agent: heldAgent({ activity: { state: "busy" } }),
			queued: [queuedMessage("q1", "And bump the version", 6)],
			sent,
		});
		const textbox = await findTextbox();

		press(textbox, "ArrowUp");

		await waitFor(() => expect(source.cancelQueued).toHaveBeenCalledWith(AGENT, "q1"));
		await waitFor(() => expect(textIn(textbox)).toBe("And bump the version"));
	});

	it("brings back a file as a pill, staged again from the agent", async () => {
		const notes = attachment("notes.md");
		const { source } = renderComposer({
			sent: [
				sentMessage(
					"m1",
					`Read ${MARK} first`,
					2,
					[notes],
					[{ type: "attachment", id: notes.id }]
				),
			],
		});
		const textbox = await findTextbox();

		press(textbox, "ArrowUp");

		expect(source.stageFound).toHaveBeenCalledWith(AGENT, [notes.path]);
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringContaining("notes.md")])
		);
		expect(textIn(textbox)).toBe(`Read ${MARK} first`);
	});

	it("sends a message it brought back like any other, and starts over after", async () => {
		const { source } = renderComposer({ sent });
		const textbox = await findTextbox();

		press(textbox, "ArrowUp");
		press(textbox, "ArrowUp");
		press(textbox, "Enter");

		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				expect.objectContaining({ text: "Fix the login" }),
				expect.anything()
			)
		);
		await waitFor(() => expect(textIn(textbox)).toBe(""));
		press(textbox, "ArrowUp");
		expect(textIn(textbox)).toBe("Now the tests");
	});
});
