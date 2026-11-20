// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { cloneWording } from "./state";

describe("the clone banner", () => {
	it("names the stage and how far it got while git clones", () => {
		const cloning = cloneWording({
			state: "cloning",
			stage: "receiving",
			percent: 42,
			stalled: false,
		});
		expect(cloning.title).toBe("Cloning: receiving objects, 42%");
		expect(cloning.body).toBe("Agents can start in this folder once git is done.");
	});

	it("says a clone stalled, with how far it got when git said", () => {
		const at = (percent: number | null) =>
			cloneWording({ state: "cloning", stage: "receiving", percent, stalled: true }).title;
		expect(at(17)).toBe("Clone stalled at 17%");
		expect(at(null)).toBe("Clone stalled");
	});

	it("says why a clone failed, as a sentence, with what Retry and Remove do", () => {
		expect(cloneWording({ state: "failed", reason: "repository not found." }).body).toBe(
			"Repository not found. Retry runs it again, and Remove forgets the folder."
		);
		expect(cloneWording({ state: "cancelled" }).title).toBe("Clone cancelled");
	});
});
