// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agent, entities, project, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import { toVisualFence } from "../visuals/fence";
import { VISUAL_MIME, writeVisualTransfer } from "../visuals/transfer";
import { MAX_VISUAL_BYTES } from "../visuals/visual";

import {
	appendVisual,
	MAX_NOTE_BODY,
	noteTooLarge,
	placeBlock,
	placedAt,
	placeVisual,
	projectOfAgent,
	takeOut,
	transferredVisual,
	visualTooLarge,
} from "./visuals";

const visual = { html: "<p>hi</p>", title: "Hi" };
const fence = toVisualFence(visual);

const bodyOf = (placing: ReturnType<typeof placeBlock>): string =>
	"body" in placing ? placing.body : `refused: ${placing.refused}`;

describe("placeBlock", () => {
	it("gives the block lines of its own with blank lines around it", () => {
		expect(bodyOf(placeBlock("", 0, "B"))).toBe("B\n");
		expect(bodyOf(placeBlock("one", 3, "B"))).toBe("one\n\nB\n");
		expect(bodyOf(placeBlock("one\n", 4, "B"))).toBe("one\n\nB\n");
		expect(bodyOf(placeBlock("one\n\n", 5, "B"))).toBe("one\n\nB\n");
		expect(bodyOf(placeBlock("one\ntwo", 4, "B"))).toBe("one\n\nB\n\ntwo");
		expect(bodyOf(placeBlock("one\n\ntwo", 4, "B"))).toBe("one\n\nB\n\ntwo");
		expect(bodyOf(placeBlock("two", 0, "B"))).toBe("B\n\ntwo");
	});

	it("says what it put where", () => {
		const placing = placeBlock("one\ntwo", 4, "B");
		expect("placed" in placing && placing.placed).toEqual({ at: 4, text: "\nB\n\n" });
	});

	it("keeps the place inside the text", () => {
		expect(bodyOf(placeBlock("one", 99, "B"))).toBe("one\n\nB\n");
		expect(bodyOf(placeBlock("one", -4, "B"))).toBe("B\n\none");
	});

	it("refuses a note past its limit", () => {
		const big = "a".repeat(MAX_NOTE_BODY - 2);
		expect(placeBlock(big, big.length, "BBBB")).toEqual({
			refused: "This would make the note 1.0 MB, and a note holds at most 1 MB.",
		});
	});
});

describe("placeVisual and appendVisual", () => {
	it("put the visual's fence in", () => {
		expect(bodyOf(placeVisual("one\ntwo", 4, visual))).toBe(`one\n\n${fence}\n\ntwo`);
		expect(bodyOf(appendVisual("one\n", visual))).toBe(`one\n\n${fence}\n`);
	});

	it("refuse a visual past its limit", () => {
		const html = "a".repeat(MAX_VISUAL_BYTES + 1);
		expect(placeVisual("", 0, { html, title: "" })).toEqual({
			refused: "This visual is 513 KB, and a visual holds at most 512 KB.",
		});
	});
});

describe("the size checks", () => {
	it("pass what fits", () => {
		expect(visualTooLarge("a".repeat(MAX_VISUAL_BYTES))).toBeNull();
		expect(noteTooLarge("a".repeat(MAX_NOTE_BODY))).toBeNull();
	});
});

describe("takeOut", () => {
	it("takes out what was placed, where it was put", () => {
		const placing = placeVisual("one\ntwo", 4, visual);
		if (!("placed" in placing)) throw new Error("refused");
		expect(takeOut(placing.body, placing.placed)).toBe("one\ntwo");
	});

	it("finds the only copy after the text around it moved", () => {
		const placing = appendVisual("one\n", visual);
		if (!("placed" in placing)) throw new Error("refused");
		expect(takeOut(`zero\n${placing.body}`, placing.placed)).toBe("zero\none\n");
	});

	it("gives null when it was changed or has a twin", () => {
		const placing = appendVisual("one\n", visual);
		if (!("placed" in placing)) throw new Error("refused");
		expect(takeOut("one\n", placing.placed)).toBeNull();
		const text = placing.placed.text;
		expect(takeOut(`a${text}b${text}`, placing.placed)).toBeNull();
	});
});

describe("placedAt", () => {
	it("finds what was placed, or says -1", () => {
		const placed = { at: 2, text: "\nB\n\n" };
		expect(placedAt("on\nB\n\nx", placed)).toBe(2);
		expect(placedAt("zzzon\nB\n\nx", placed)).toBe(5);
		expect(placedAt("on", placed)).toBe(-1);
	});
});

describe("projectOfAgent", () => {
	it("goes through the agent's workspace", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				projects: [project("p-a", "Atlas")],
				workspaces: [workspace({ id: "w1", project: "p-a" })],
				agents: [agent({ id: "a1", workspace: "w1" })],
			}),
		});
		expect(projectOfAgent(view, "a1")).toBe("p-a");
		expect(projectOfAgent(view, "nobody")).toBeNull();
	});
});

describe("transferredVisual", () => {
	const data = (types: Record<string, string>) => ({
		getData: (type: string) => types[type] ?? "",
	});

	it("reads a visual in the MaxHQ type", () => {
		const raw = writeVisualTransfer({ ...visual, from: "Atlas on devbox" });
		expect(transferredVisual(data({ [VISUAL_MIME]: raw }))).toEqual({ visual });
	});

	it("says why a visual is too large", () => {
		const raw = writeVisualTransfer({
			html: "a".repeat(MAX_VISUAL_BYTES + 1),
			title: "",
			from: null,
		});
		expect(transferredVisual(data({ [VISUAL_MIME]: raw }))).toEqual({
			refused: "This visual is 513 KB, and a visual holds at most 512 KB.",
		});
	});

	it("gives null for anything else", () => {
		expect(transferredVisual(data({ "text/plain": "hi" }))).toBeNull();
		expect(transferredVisual(data({ [VISUAL_MIME]: "not json" }))).toBeNull();
		expect(transferredVisual(data({ [VISUAL_MIME]: '{"html":1}' }))).toBeNull();
	});
});
