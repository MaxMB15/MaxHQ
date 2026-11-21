// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { InlineRef, Question } from "../bindings";

import {
	answerRefs,
	approvalChoices,
	failureOf,
	isDeny,
	isLate,
	mainAnswer,
	plainChoices,
	questionsAnswer,
} from "./answer";

const refused = (reason: string) =>
	({ status: "ok", data: { status: "refused", reason } }) as const;

describe("isLate", () => {
	it("knows an answer another one beat", () => {
		expect(isLate(refused("Someone else answered first"))).toBe(true);
		expect(isLate(refused("The device no longer has this request"))).toBe(true);
	});

	it("leaves other refusals and successes as they are", () => {
		const expired = refused("This request expired before your answer arrived");
		expect(isLate(expired)).toBe(false);
		expect(failureOf(expired)).toBe("This request expired before your answer arrived.");
		expect(isLate({ status: "ok", data: { status: "done" } })).toBe(false);
		expect(isLate({ status: "error", error: { kind: "failed", message: "x" } })).toBe(false);
	});
});

describe("approvalChoices", () => {
	const option = (id: string, label: string) => ({ id, label });
	const ids = (options: Parameters<typeof approvalChoices>[0]) => {
		const { primary, others, deny } = approvalChoices(options);
		return {
			primary: primary?.id ?? null,
			others: others.map((other) => other.id),
			deny: deny?.id ?? null,
		};
	};

	it("keeps each real agent's buttons as they were", () => {
		expect(
			ids([
				option("allow", "Allow once"),
				option("allow_session", "Allow for this session"),
				option("deny", "Deny"),
			])
		).toEqual({ primary: "allow", others: ["allow_session"], deny: "deny" });
		expect(
			ids([
				option("accept_edits", "Yes, and accept edits"),
				option("accept", "Yes, and ask before each edit"),
				option("deny", "No, keep planning"),
			])
		).toEqual({ primary: "accept", others: ["accept_edits"], deny: "deny" });
		expect(
			ids([
				option("accept", "Approve"),
				option("accept_for_session", "Approve for this session"),
				option("always_allow", "Always allow touch"),
				option("network_0", "Always block example.com"),
				option("decline", "Decline"),
				option("cancel", "Decline and stop"),
			])
		).toEqual({
			primary: "accept",
			others: ["accept_for_session", "always_allow", "network_0", "cancel"],
			deny: "decline",
		});
		expect(
			ids([
				option("grant_turn", "Allow for this turn"),
				option("grant_session", "Allow for this session"),
				option("decline", "Decline"),
			])
		).toEqual({ primary: null, others: ["grant_turn", "grant_session"], deny: "decline" });
		expect(
			ids([
				option("allow_once", "Allow once"),
				option("allow_always", "Allow always"),
				option("reject_once", "Reject"),
			])
		).toEqual({ primary: "allow_once", others: ["allow_always"], deny: "reject_once" });
	});

	it("puts the yes that lasts behind the one that doesn't, wherever it's listed", () => {
		expect(
			ids([
				option("allow_always", "Allow always"),
				option("allow", "Allow"),
				option("deny", "Deny"),
			])
		).toEqual({ primary: "allow", others: ["allow_always"], deny: "deny" });
		expect(
			ids([
				option("a", "Allow for this session"),
				option("b", "Yes, don't ask again"),
				option("c", "Allow"),
			])
		).toEqual({ primary: "c", others: ["a", "b"], deny: null });
	});

	it("never puts a yes that lasts on the main button", () => {
		const kinded = (id: string, label: string, kind: string) => ({ id, label, kind });
		expect(
			ids([kinded("a1", "Allow", "allow_always"), kinded("a2", "Reject", "reject_once")])
		).toEqual({ primary: null, others: ["a1"], deny: "a2" });
		for (const label of ["Allow all edits", "Trust this folder", "Yes"]) {
			expect(
				ids([kinded("a1", label, "allow_always"), kinded("a2", "Once", "allow_once")])
			).toEqual({ primary: "a2", others: ["a1"], deny: null });
		}
		expect(ids([option("always_allow", "Run it"), option("decline", "Decline")])).toEqual({
			primary: null,
			others: ["always_allow"],
			deny: "decline",
		});
		expect(ids([option("accept_for_session", "Yes"), option("decline", "Decline")])).toEqual({
			primary: null,
			others: ["accept_for_session"],
			deny: "decline",
		});
	});

	it("goes by the agent's kind before any label", () => {
		const kinded = (id: string, label: string, kind: string) => ({ id, label, kind });
		expect(isDeny(kinded("a", "Allow", "reject_once"))).toBe(true);
		expect(isDeny(kinded("deny", "No-ask mode", "allow_once"))).toBe(false);
		expect(
			ids([kinded("a", "Always", "allow_once"), kinded("b", "Allow", "reject_once")])
		).toEqual({ primary: "a", others: [], deny: "b" });
		// A word this doesn't know says nothing, so the label decides.
		expect(
			ids([kinded("a", "Deny", "allow_soon"), kinded("b", "Allow", "constructor")])
		).toEqual({ primary: "b", others: [], deny: "a" });
	});

	it("reads a no from its label, and a yes that only starts like one as a yes", () => {
		for (const label of [
			"Not now",
			"Block",
			"Never",
			"Never allow",
			"Always deny",
			"Skip",
			"Stop",
			"Don't run",
			"Do not run",
			"No",
			"No, keep planning",
		]) {
			expect(isDeny(option("x", label))).toBe(true);
		}
		for (const label of ["No-ask mode", "no-prompt", "Never ask again", "Stop asking"]) {
			expect(isDeny(option("x", label))).toBe(false);
		}
		expect(ids([option("x", "No-ask mode"), option("y", "Allow")])).toEqual({
			primary: "y",
			others: ["x"],
			deny: null,
		});
	});

	it("counts Don't allow as a no, and never as the main button", () => {
		for (const label of ["Don't allow", "Don\u2019t allow", "Do not allow", "Always block x"]) {
			expect(isDeny(option("x", label))).toBe(true);
		}
		expect(isDeny(option("x", "Don't ask again"))).toBe(false);
		expect(ids([option("x", "Don't allow"), option("y", "Allow")])).toEqual({
			primary: "y",
			others: [],
			deny: "x",
		});
	});

	it("has no main button when every option is a no", () => {
		expect(ids([option("decline", "Decline"), option("cancel", "Cancel")])).toEqual({
			primary: null,
			others: ["cancel"],
			deny: "decline",
		});
	});

	it("puts the no on the main button when turning it down comes first", () => {
		const options = [
			option("allow", "Allow"),
			option("allow_always", "Allow always"),
			option("deny", "Deny"),
		];
		const choices = approvalChoices(options, true);
		expect([choices.primary?.id, choices.others.map((o) => o.id), choices.deny?.id]).toEqual([
			"deny",
			["allow", "allow_always"],
			"deny",
		]);
		expect(mainAnswer(choices, "")).toEqual({ type: "option", id: "deny" });
		expect(mainAnswer(choices, " Not this one ")).toEqual({
			type: "text",
			text: "Not this one",
		});
		expect(mainAnswer(approvalChoices(options), "ignored")).toEqual({
			type: "option",
			id: "allow",
		});
		// With no no to put first, the card stays as it was.
		expect(approvalChoices([option("allow", "Allow")], true).primary?.id).toBe("allow");
		expect(mainAnswer(approvalChoices([option("always", "Allow always")]), "")).toBeNull();
	});

	it("takes the no for this one request first", () => {
		expect(
			ids([
				option("reject_always", "Reject always"),
				option("reject_once", "Reject"),
				option("allow", "Allow"),
			])
		).toEqual({ primary: "allow", others: ["reject_always"], deny: "reject_once" });
	});

	it("marks what a label hides, and keeps its id", () => {
		const { primary } = approvalChoices([option("allow", "Allow\u202e once")]);
		expect(primary).toEqual({ id: "allow", label: "Allow[U+202E] once" });
	});
});

describe("pills in answers to questions", () => {
	const question = (id: string, multi = false): Question => ({
		id,
		header: null,
		text: "Which one?",
		options: [{ label: "Fast", description: null }],
		multi,
		other: true,
		secret: false,
	});
	const agent = (id: string, title: string): InlineRef => ({
		type: "agent",
		agent: id,
		title,
		vendor: "claude",
		device: "dev-mac",
		excerpt: null,
	});
	const pr = (repo: string): InlineRef => ({
		type: "git_item",
		repo,
		number: 7,
		kind: "pull_request",
		title: "Fix",
		state: "open",
		url: "https://github.com/owner/repo/pull/7",
		head: null,
		base: null,
		body: "",
		checked_at: 0,
	});
	const questions = [question("q1"), question("q2", true), question("q3")];
	const choices = {
		q1: {
			values: [],
			other: "Like \uFFFC #7",
			refs: [agent("a1", "Release"), pr("owner/repo")],
		},
		q2: {
			values: ["Fast"],
			other: "And \uFFFC #7",
			refs: [agent("a2", "Docs"), pr("Owner/Repo")],
		},
		// A pill left in a box you then emptied goes nowhere.
		q3: { values: ["Fast"], other: "  ", refs: [agent("a3", "Gone")] },
	};

	it("go question by question, each pull request once", () => {
		expect(answerRefs(questions, choices)).toEqual([
			agent("a1", "Release"),
			agent("a2", "Docs"),
			pr("Owner/Repo"),
		]);
		expect(questionsAnswer(questions, choices)).toEqual({
			type: "questions",
			answers: [
				{ id: "q1", values: ["Like \uFFFC #7"] },
				{ id: "q2", values: ["Fast", "And \uFFFC #7"] },
				{ id: "q3", values: ["Fast"] },
			],
		});
	});

	it("read as their words for a maxd too old for them", () => {
		const plain = plainChoices(choices);
		expect(plain.q1).toEqual({ values: [], other: "Like [Agent: Release] #7" });
		expect(plain.q2).toEqual({ values: ["Fast"], other: "And [Agent: Docs] #7" });
		expect(answerRefs(questions, plain)).toEqual([]);
	});
});
