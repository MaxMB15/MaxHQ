// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Consent, ConsentRow, Question } from "../bindings";
import { consentAnswer, consentGroups, HIDDEN_VALUE, shownValue } from "./consent";

const row = (group: string, name: string, value: string, secret = false): ConsentRow => ({
	group,
	name,
	value,
	secret,
	warning: null,
	strong: false,
});

const remember = {
	id: "workflow-env-remember",
	label: "Remember for this project",
	settings:
		"Allow keeps this for the project. Change it later in Settings › Projects › app › General.",
};

const question = (consent: Consent | undefined): Question => ({
	id: "workflow-env",
	header: "Workflow variables",
	text: "plain text",
	options: [
		{ label: "Allow", description: null },
		{ label: "Not now", description: null },
	],
	multi: false,
	other: false,
	secret: false,
	consent,
});

const consent: Consent = {
	title: "Give this agent the workflow's variables?",
	risk: "These go into this agent's environment.",
	changes: [],
	facts: [],
	rows: [row("example", "EXAMPLE_MODE", "on")],
	remember,
};

describe("consentGroups", () => {
	it("keeps each heading's rows together, in the order the headings come", () => {
		const groups = consentGroups([
			row("example", "EXAMPLE_A", "1"),
			row("git", "GIT_SSH_COMMAND", "ssh"),
			row("example", "EXAMPLE_B", "2"),
		]);
		expect(groups.map((group) => group.name)).toEqual(["example", "git"]);
		expect(groups[0]?.rows.map((r) => r.name)).toEqual(["EXAMPLE_A", "EXAMPLE_B"]);
		expect(groups[1]?.rows.map((r) => r.name)).toEqual(["GIT_SSH_COMMAND"]);
	});

	it("has nothing to group when there are no rows", () => {
		expect(consentGroups([])).toEqual([]);
	});
});

describe("shownValue", () => {
	it("hides a secret until you ask to see it", () => {
		const secret = row("example", "EXAMPLE_TOKEN", "abc123", true);
		expect(shownValue(secret, false)).toBe(HIDDEN_VALUE);
		expect(shownValue(secret, true)).toBe("abc123");
	});

	it("shows any other value as it is", () => {
		expect(shownValue(row("example", "EXAMPLE_MODE", "on"), false)).toBe("on");
	});
});

describe("consentAnswer", () => {
	it("sends the pick alone when the box isn't ticked", () => {
		expect(consentAnswer(question(consent), "Allow", false)).toEqual({
			type: "questions",
			answers: [{ id: "workflow-env", values: ["Allow"] }],
		});
	});

	it("adds the box as its own answer once it's ticked", () => {
		expect(consentAnswer(question(consent), "Allow", true)).toEqual({
			type: "questions",
			answers: [
				{ id: "workflow-env", values: ["Allow"] },
				{ id: "workflow-env-remember", values: ["Remember for this project"] },
			],
		});
	});

	it("sends no box a card doesn't offer", () => {
		const unkept = { ...consent, remember: null };
		expect(consentAnswer(question(unkept), "Not now", true)).toEqual({
			type: "questions",
			answers: [{ id: "workflow-env", values: ["Not now"] }],
		});
		expect(consentAnswer(question(undefined), "Not now", true).type).toBe("questions");
	});
});
