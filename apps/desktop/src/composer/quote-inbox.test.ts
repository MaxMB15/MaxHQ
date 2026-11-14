// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { describe, expect, it, vi } from "vitest";

import { createQuoteInbox } from "./quote-inbox";

const quote = (text: string): QuoteRef => ({
	type: "quote",
	agent: "a1",
	agent_title: "Fix the build",
	item: "i1",
	text,
	position: 0,
	source: "reply",
	from: null,
});

describe("createQuoteInbox", () => {
	it("keeps quotes for an agent until its composer takes them", () => {
		const inbox = createQuoteInbox();
		inbox.send("a2", quote("one"), false);
		inbox.send("a2", quote("two"), true);
		inbox.send("a3", quote("other"), false);
		expect(inbox.take("a2")).toEqual([
			{ quote: quote("one"), focus: false },
			{ quote: quote("two"), focus: true },
		]);
		expect(inbox.take("a2")).toEqual([]);
		expect(inbox.take("a3")).toHaveLength(1);
	});

	it("tells a mounted composer as each quote arrives", () => {
		const inbox = createQuoteInbox();
		const heard = vi.fn();
		const undo = inbox.watch("a2", heard);
		inbox.send("a3", quote("other"), false);
		expect(heard).not.toHaveBeenCalled();
		inbox.send("a2", quote("one"), false);
		expect(heard).toHaveBeenCalledTimes(1);
		undo();
		inbox.send("a2", quote("two"), false);
		expect(heard).toHaveBeenCalledTimes(1);
	});
});
