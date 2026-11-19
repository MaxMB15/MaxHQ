// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { account, agent } from "../test/build";

import { canResume, resumeWording, showsResume } from "./resume";

const GEMINI = account({
	id: "a-gemini",
	vendor: "acp",
	label: "Gemini",
	config_dir: "",
	acp: {
		images: true,
		load: false,
		plan: true,
		auth: [{ id: "login", name: "Log in with Google", description: "Opens a page" }],
	},
});

describe("the resume banner", () => {
	it("offers Resume for a paused agent MaxHQ holds, not one in a terminal or a sub-agent", () => {
		const paused = agent({
			id: "a1",
			control: "held",
			activity: { state: "paused", reason: "by_you" },
		});
		expect(canResume(paused)).toBe(true);
		expect(canResume({ ...paused, control: "observed" })).toBe(false);
		expect(canResume({ ...paused, parent: "a0" })).toBe(false);
	});

	it("offers Resume for a Claude CLI that exited, not a Codex one", () => {
		const exited = agent({
			id: "a1",
			control: "held",
			link: { state: "down", reason: "exited", since: null },
			activity: { state: "error", reason: "exited with status 1" },
		});
		expect(canResume(exited)).toBe(true);
		expect(canResume({ ...exited, vendor: "codex" })).toBe(false);
	});

	it("says why it stopped, and that a local agent loses its history", () => {
		const stopped = agent({
			id: "a1",
			control: "held",
			activity: { state: "paused", reason: "resumes_made_no_progress" },
		});
		expect(resumeWording(stopped, undefined, "studio").title).toBe("Paused after 3 resumes");
		const errored = {
			...stopped,
			activity: { state: "error", reason: "out of memory" },
		} as const;
		expect(resumeWording(errored, undefined, "studio").body).toBe(
			"Out of memory. The conversation stays, and Resume starts it again."
		);
		const local = resumeWording(
			{ ...stopped, vendor: "acp", activity: { state: "paused", reason: "by_you" } },
			GEMINI,
			"studio"
		);
		expect(local.signIn).toBe(false);
		expect(local.body).toContain("can't load an earlier session");
	});

	it("turns into the sign-in card with the agent's ways to sign in", () => {
		const signedOut = agent({
			id: "a1",
			vendor: "acp",
			control: "held",
			account: "a-gemini",
			activity: { state: "error", reason: "Gemini needs a sign-in." },
		});
		expect(showsResume(signedOut, GEMINI)).toBe(true);
		const card = resumeWording(signedOut, GEMINI, "studio");
		expect(card.signIn).toBe(true);
		expect(card.title).toBe("Gemini needs a sign-in");
		expect(card.body).toContain("on studio");
		expect(card.methods.map((m) => m.name)).toEqual(["Log in with Google"]);
	});
});
