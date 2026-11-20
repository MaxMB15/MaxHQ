// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { browserNative, type ComposerNative, ComposerNativeProvider } from "@/composer/native";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { fakeSource } from "@/composer/testing";
import type { Agent } from "@maxhq/shared/bindings";
import { QUOTE_MIME } from "@maxhq/shared/pills/quote";
import { agent as agentOf } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { TimelineQuotes } from "./quotes";

const REPLY = "The second one, with the part you want.";

const reply = timelineItem(
	{ kind: "assistant_message", text: REPLY, streaming: false },
	{ id: "m1", ordinal: 1 }
);

/** A conversation with one reply, quoted by `agent`, and the menu it opens. */
const setup = (agent: Agent = agentOf({ id: "a1", title: "Fix the build", control: "held" })) => {
	document.body.innerHTML = `<main><div data-quote-item="m1"><p>${REPLY}</p></div></main>`;
	const viewport = document.querySelector("main")!;
	const store = createTimelineStore();
	store.setState({ items: new Map([[reply.id, reply]]), order: [reply.id], rows: [reply.id] });
	const contextMenu = vi.fn<NonNullable<ComposerNative["contextMenu"]>>(() => true);
	const { source } = fakeSource({ agent });
	render(
		<ServicesProvider source={source}>
			<ComposerNativeProvider value={{ ...browserNative, contextMenu }}>
				<TimelineQuotes agent={agent} viewport={viewport} store={store} />
			</ComposerNativeProvider>
		</ServicesProvider>
	);
	const text = viewport.querySelector("p")!.firstChild as Text;
	const from = REPLY.indexOf("the part");
	document.getSelection()!.setBaseAndExtent(text, from, text, from + "the part".length);
	return { viewport, text, contextMenu };
};

const picked = expect.objectContaining({
	type: "quote",
	agent: "a1",
	agent_title: "Fix the build",
	item: "m1",
	text: "the part",
	source: "reply",
});

beforeEach(() => {
	// The picked text sits at the top left of the page.
	Range.prototype.getClientRects = () => [new DOMRect(0, 0, 100, 20)] as unknown as DOMRectList;
	Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 100, 20);
});

afterEach(() => {
	document.getSelection()?.removeAllRanges();
	document.body.innerHTML = "";
	quoteInbox.take("a1");
});

describe("TimelineQuotes", () => {
	it("offers Add to chat, the other agents and Copy on a right click on the selection", () => {
		const { text, contextMenu } = setup();
		const event = fireEvent.contextMenu(text.parentElement!, { clientX: 40, clientY: 10 });
		expect(event).toBe(false);
		const [entries, at] = contextMenu.mock.calls[0]!;
		expect(at).toEqual({ x: 40, y: 10 });
		expect(entries.map((entry) => ("text" in entry ? entry.text : entry.kind))).toEqual([
			"Add to chat",
			"Add to chat in",
			"separator",
			"the part",
		]);

		const add = entries[0]!;
		if (add.kind !== "item") throw new Error("Add to chat isn't an item");
		add.action();
		expect(quoteInbox.take("a1")).toEqual([{ quote: picked, focus: true }]);
		expect(document.getSelection()?.isCollapsed ?? true).toBe(true);
	});

	it("leaves a right click off the selection to the webview", () => {
		const { text, contextMenu } = setup();
		fireEvent.contextMenu(text.parentElement!, { clientX: 400, clientY: 300 });
		expect(contextMenu).not.toHaveBeenCalled();
	});

	it("offers no Add to chat for an agent you only watch", () => {
		const { text, contextMenu } = setup(
			agentOf({ id: "a1", title: "Fix the build", control: "observed" })
		);
		fireEvent.contextMenu(text.parentElement!, { clientX: 40, clientY: 10 });
		const [entries] = contextMenu.mock.calls[0]!;
		expect(entries.map((entry) => ("text" in entry ? entry.text : entry.kind))).toEqual([
			"Add to chat in",
			"separator",
			"the part",
		]);
	});

	it("puts the quote on a drag of the selection and lets it reach the document", () => {
		const { text } = setup();
		const data = new Map<string, string>();
		const heard = vi.fn();
		document.addEventListener("dragstart", heard);
		const event = new Event("dragstart", { bubbles: true, cancelable: true });
		Object.defineProperty(event, "dataTransfer", {
			value: {
				effectAllowed: "all",
				setData: (type: string, value: string) => data.set(type, value),
			},
		});
		text.dispatchEvent(event);
		document.removeEventListener("dragstart", heard);

		expect(JSON.parse(data.get(QUOTE_MIME)!)).toEqual(picked);
		expect(data.get("text/plain")).toBe("the part");
		expect(heard).toHaveBeenCalled();
	});
});
