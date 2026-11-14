// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { ANY_PLACE, NO_FILES, NOTHING_TYPED, type Typed } from "@maxhq/shared/pills/answer";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { agent, entities, workspace } from "@maxhq/shared/test/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { AnswerField, answerMaxHeight } from "./answer-field";
import { Composer } from "./composer";
import { ComposerNativeProvider } from "./native";
import { createPaneFiles, PaneFilesContext } from "./pane-files";
import {
	AGENT,
	attachment,
	clipboard,
	DEVICE,
	editorOf,
	fakeNative,
	fakeSource,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	SHOT_DRAG,
	staged,
	textIn,
	type,
} from "./testing";

beforeAll(installBrowserFakes);

/** Sets how tall the field's text runs against what it shows, until the returned call. */
const tall = (scrollHeight: number, clientHeight: number) => {
	const spies = [
		vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(scrollHeight),
		vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(clientHeight),
	];
	return () => spies.forEach((spy) => spy.mockRestore());
};

/**
 * The field for an answer to "Fix login", keeping what you type as a card
 * does, alone or in a pane with the agent's composer.
 */
const renderField = ({
	native = fakeNative(),
	shut = false,
	composer = false,
}: { native?: ReturnType<typeof fakeNative>; shut?: boolean; composer?: boolean } = {}) => {
	const { source } = fakeSource({
		agent: heldAgent(),
		more: entities({
			agents: [agent({ id: "a2", title: "Sidebar sort" })],
			workspaces: [workspace({ id: "w1" })],
		}),
	});
	const onChange = vi.fn<(typed: Typed) => void>();
	const onEnter = vi.fn<() => void>();
	const Card = ({ shut }: { shut: boolean }) => {
		const [value, setValue] = useState<Typed>(NOTHING_TYPED);
		return (
			<>
				<AnswerField
					value={value}
					onChange={(typed) => {
						onChange(typed);
						setValue(typed);
					}}
					onEnter={onEnter}
					label="Your answer"
					placeholder="Type your answer"
					agent={AGENT}
					place={ANY_PLACE}
					device={DEVICE}
					look="card"
					shut={shut}
				/>
				<button type="button" onClick={() => setValue(NOTHING_TYPED)}>
					Sent
				</button>
			</>
		);
	};
	const pane = createPaneFiles();
	const timeline = createTimelineStore();
	const tree = (shut: boolean) => (
		<ServicesProvider source={source}>
			<ComposerNativeProvider value={native.native}>
				{composer ? (
					<PaneFilesContext value={pane}>
						<Card shut={shut} />
						<Composer agent={AGENT} device={DEVICE} queued={[]} timeline={timeline} />
					</PaneFilesContext>
				) : (
					<Card shut={shut} />
				)}
			</ComposerNativeProvider>
		</ServicesProvider>
	);
	const view = render(tree(shut));
	const box = screen.getByRole("textbox", { name: "Your answer" });
	const frame = () => box.closest(".answer-field");
	return {
		box,
		source,
		onChange,
		onEnter,
		drop: native.drop,
		/** The handlers the field gave its drop target, beside the composer's. */
		dropOnField: () => {
			const handlers = native.native.registerDropTarget.mock.calls.findLast(
				([element]) => element === frame()
			)?.[1];
			if (!handlers) throw new Error("The field has no drop target");
			return handlers;
		},
		frame,
		shut: (next: boolean) => view.rerender(tree(next)),
	};
};

describe("the answer field", () => {
	it("leaves spelling to the webview without native spell check", () => {
		const { box } = renderField();
		expect(box).toHaveAttribute("spellcheck", "true");
	});

	it("turns the webview's spell check off for the system's own", () => {
		const { box } = renderField({ native: fakeNative({ spelling: true }) });
		expect(box).toHaveAttribute("spellcheck", "false");
	});

	it("names an agent with a pill from the @ menu, and reports it", async () => {
		const { box, onChange } = renderField();
		await type(box, "Ask @side");
		fireEvent.click(await screen.findByRole("option", { name: "Sidebar sort" }));
		await waitFor(() =>
			expect(pillsIn(box)).toEqual([expect.stringMatching(/^Agent Sidebar sort/)])
		);
		expect(onChange).toHaveBeenLastCalledWith({
			text: `Ask ${MARK} `,
			refs: [
				{
					type: "agent",
					agent: "a2",
					title: "Sidebar sort",
					vendor: "claude",
					device: DEVICE,
					excerpt: null,
				},
			],
		});
	});

	it("says a dropped file can't go in, and keeps it out", () => {
		const { box, drop } = renderField();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		expect(screen.getByRole("alert")).toHaveTextContent(NO_FILES);
		expect(pillsIn(box)).toEqual([]);
	});

	it("hands a dropped file to the pane's composer, and keeps the focus", async () => {
		const { box, dropOnField } = renderField({ composer: true });
		const message = await findTextbox();
		act(() => {
			editorOf(box).commands.focus();
		});
		await waitFor(() => expect(box).toHaveFocus());
		act(() => dropOnField().onDrop?.(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await waitFor(() => expect(pillsIn(message)).toEqual([expect.stringContaining("main.rs")]));
		expect(pillsIn(box)).toEqual([]);
		expect(screen.queryByRole("alert")).toBeNull();
		// An editor takes the focus a frame after it's asked, so this waits one out.
		await act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));
		expect(box).toHaveFocus();
	});

	it("hands a file from a timeline or the clipboard to the composer the same way", async () => {
		const { box, source, dropOnField } = renderField({ composer: true });
		source.attachFromAgent.mockResolvedValue(
			staged(attachment("shot.png", { kind: "image", mime: "image/png" }))
		);
		const message = await findTextbox();
		act(() => dropOnField().onPageDrop?.(SHOT_DRAG, { x: 0, y: 0 }));
		await waitFor(() =>
			expect(pillsIn(message)).toEqual([expect.stringMatching(/^Image 1, shot\.png/)])
		);

		const file = new File(["hello"], "notes.txt", { type: "text/plain" });
		fireEvent.paste(box, { clipboardData: clipboard({ files: [file], types: ["Files"] }) });
		await waitFor(() =>
			expect(source.stagePasted).toHaveBeenCalledWith(
				DEVICE,
				"notes.txt",
				"text/plain",
				expect.any(Uint8Array)
			)
		);
		await waitFor(() => expect(pillsIn(message)).toHaveLength(2));
		expect(pillsIn(box)).toEqual([]);
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("sends with Enter, and breaks the line with Shift+Enter", async () => {
		const { box, onEnter } = renderField();
		await type(box, "Use the dev branch");
		fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
		expect(onEnter).not.toHaveBeenCalled();
		fireEvent.keyDown(box, { key: "Enter" });
		expect(onEnter).toHaveBeenCalledTimes(1);
	});

	it("empties once the card clears what you typed", async () => {
		const { box } = renderField();
		await type(box, "Use the dev branch");
		fireEvent.click(screen.getByRole("button", { name: "Sent" }));
		expect(textIn(box)).toBe("");
	});

	it("grows to eight lines, then scrolls", () => {
		const { frame } = renderField();
		expect(frame()).toHaveStyle({ "--answer-max": `${answerMaxHeight("card")}px` });
		expect(answerMaxHeight("card")).toBe(8 * 18 + 2 * 4);
		expect(screen.queryByRole("button", { name: "Show all the text" })).toBeNull();
	});

	it("opens to all the text once it scrolls, and back", async () => {
		const restore = tall(400, 152);
		try {
			const { box, frame } = renderField();
			const open = screen.getByRole("button", { name: "Show all the text" });
			expect(open).toHaveAttribute("aria-pressed", "false");
			expect(frame()).toHaveAttribute("data-size", "fit");
			fireEvent.click(open);
			expect(open).toHaveAttribute("aria-pressed", "true");
			expect(frame()).toHaveAttribute("data-size", "tall");
			// The text box keeps the focus, so the caret stays where you were typing.
			await waitFor(() => expect(box).toHaveFocus());
			fireEvent.click(open);
			expect(frame()).toHaveAttribute("data-size", "fit");
		} finally {
			restore();
		}
	});

	it("keeps to one line while shut, with nothing to open", () => {
		const restore = tall(400, 152);
		try {
			const { frame, shut } = renderField({ shut: true });
			expect(frame()).toHaveAttribute("data-state", "collapsed");
			expect(screen.queryByRole("button", { name: "Show all the text" })).toBeNull();
			shut(false);
			expect(frame()).toHaveAttribute("data-state", "expanded");
			expect(screen.getByRole("button", { name: "Show all the text" })).toBeInTheDocument();
		} finally {
			restore();
		}
	});
});
