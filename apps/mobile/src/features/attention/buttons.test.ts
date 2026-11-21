// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { answerButtons } from "./buttons";

const options = [
	{ id: "deny", label: "Deny" },
	{ id: "allow", label: "Allow once" },
	{ id: "session", label: "Allow for this session" },
];

describe("answerButtons", () => {
	it("puts an approval's first yes first and its no last", () => {
		expect(answerButtons(options, true).buttons.map((b) => [b.option.id, b.tone])).toEqual([
			["allow", "primary"],
			["session", "secondary"],
			["deny", "danger"],
		]);
	});

	it("lists a question's options in order", () => {
		expect(answerButtons(options, false).buttons.map((b) => b.option.id)).toEqual([
			"deny",
			"allow",
			"session",
		]);
	});

	it("moves an approval's no onto the reason field when it takes one", () => {
		const { buttons, deny } = answerButtons(options, true, true);
		expect(buttons.map((b) => b.option.id)).toEqual(["allow", "session"]);
		expect(deny?.id).toBe("deny");
	});

	it("puts the no first in the main button's look when the agent wants no as the default", () => {
		const shown = (acceptsText: boolean) => {
			const { buttons, deny } = answerButtons(options, true, acceptsText, true);
			return [buttons.map((b) => [b.option.id, b.tone]), deny];
		};
		const first = [
			["deny", "primary"],
			["allow", "secondary"],
			["session", "secondary"],
		];
		expect(shown(false)).toEqual([first, null]);
		expect(shown(true)).toEqual([first, null]);
	});

	it("keeps a question's options as buttons even when it takes text", () => {
		expect(answerButtons(options, false, true).deny).toBeNull();
	});
});
