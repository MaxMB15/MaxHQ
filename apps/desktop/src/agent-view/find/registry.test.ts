// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { finderFor, registerFinder } from "./registry";

describe("the find shortcut's target", () => {
	const undo: (() => void)[] = [];
	afterEach(() => {
		for (const call of undo.splice(0)) call();
		document.body.replaceChildren();
	});

	// An agent view in a pane, with a terminal inside it.
	const setup = () => {
		const pane = document.createElement("div");
		const agent = document.createElement("section");
		const terminal = document.createElement("div");
		const input = document.createElement("textarea");
		const composer = document.createElement("textarea");
		terminal.append(input);
		agent.append(terminal, composer);
		pane.append(agent);
		document.body.append(pane);
		const inAgent = vi.fn();
		const inTerminal = vi.fn();
		undo.push(registerFinder(agent, inAgent), registerFinder(terminal, inTerminal));
		return { pane, input, composer, inAgent, inTerminal };
	};

	it("finds in the innermost view that holds focus", () => {
		const { input, composer, inAgent, inTerminal } = setup();
		expect(finderFor(input, null)).toBe(inTerminal);
		expect(finderFor(composer, null)).toBe(inAgent);
	});

	it("finds in the outermost view of the active pane, or the only one, when focus is elsewhere", () => {
		const { pane, inAgent } = setup();
		expect(finderFor(document.body, pane)).toBe(inAgent);
		expect(finderFor(null, null)).toBe(inAgent);
	});
});
