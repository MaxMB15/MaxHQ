// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { LINK_GAP_MS, linkGate, pageLinkQuestion, visualLinkQuestion } from "./links";

describe("a link from a visual", () => {
	it("asks about web and mail links, one question at a time", () => {
		const gate = linkGate();
		expect(gate.ask("https://example.com", 10_000)).toBe(true);
		// The first is still open.
		expect(gate.ask("https://example.com/b", 20_000)).toBe(false);
		gate.answered(20_000);
		expect(gate.ask("mailto:someone@example.com", 20_000 + LINK_GAP_MS)).toBe(true);
	});

	it("waits a second after an answer before it asks again", () => {
		const gate = linkGate();
		expect(gate.ask("https://example.com", 0)).toBe(true);
		gate.answered(5_000);
		expect(gate.ask("https://example.com", 5_500)).toBe(false);
		expect(gate.ask("https://example.com", 6_000)).toBe(true);
	});

	it("never asks about other kinds of link", () => {
		const gate = linkGate();
		expect(gate.ask("javascript:alert(1)", 0)).toBe(false);
		expect(gate.ask("file:///etc/hosts", 5_000)).toBe(false);
		expect(gate.ask("maxhq://pair", 10_000)).toBe(false);
		expect(gate.ask("about:blank", 15_000)).toBe(false);
		// None of them held the gate.
		expect(gate.ask("https://example.com", 15_000)).toBe(true);
	});

	it("names the agent that showed the visual, when there is one", () => {
		expect(visualLinkQuestion("Release checklist")).toEqual({
			title: "Open this link?",
			detail: "The visual from Release checklist wants to open this address in your browser. Anything in the address leaves MaxHQ, so open it only if you expect it.",
		});
		expect(visualLinkQuestion(null).detail).toMatch(/^This visual wants to open/);
		expect(pageLinkQuestion().detail).toMatch(/^Something in this window tried/);
	});
});
