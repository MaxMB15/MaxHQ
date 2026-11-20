// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ActionOutcome, Entity } from "../bindings";
import type { ActionResult } from "../data/source";
import { account, entities } from "../test/build";
import { credential } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import {
	apnsLine,
	appLines,
	codexStatus,
	forgetNote,
	outcomesOf,
	sessionLine,
	tokenLine,
} from "./login-parts";
import { type Held, heldOf, type Target } from "./logins";

const ok = (data: object): ActionResult => ({ status: "ok", data: data as ActionOutcome });

describe("outcomesOf", () => {
	it("gives each device its own result, in order", () => {
		expect(
			outcomesOf(
				["dev-a", "dev-b"],
				[ok({ status: "done" }), ok({ status: "refused", reason: "Setup-tokens are off" })],
				"sk-1"
			)
		).toEqual([
			{ device: "dev-a", failure: null },
			{ device: "dev-b", failure: "Setup-tokens are off" },
		]);
	});

	it("hides the secret in a reply that echoes it", () => {
		const [outcome] = outcomesOf(
			["dev-a"],
			[ok({ status: "refused", reason: "bad token sk-1" })],
			"sk-1"
		);
		expect(outcome?.failure).toBe("bad token <secret>");
	});

	it("says a device that gave no result didn't answer", () => {
		expect(outcomesOf(["dev-a"], [], "sk-1")).toEqual([
			{ device: "dev-a", failure: "The device didn't answer" },
		]);
	});
});

describe("forgetNote", () => {
	it("says nothing once the device dropped it", () => {
		expect(forgetNote(ok({ status: "done" }))).toBeNull();
		expect(forgetNote(ok({ status: "signed_out", revoked: true }))).toBeNull();
	});

	it("says why the device didn't", () => {
		expect(forgetNote(ok({ status: "refused", reason: "it's in use" }))).toEqual({
			text: "It's in use",
			error: true,
		});
	});

	it("warns when OpenAI didn't confirm the sign-out", () => {
		expect(forgetNote(ok({ status: "signed_out", revoked: false }))?.text).toMatch(
			/^Signed out on the device, but OpenAI didn't confirm/
		);
	});
});

describe("the line each held login shows", () => {
	const view = (more: Entity[]) =>
		applySnapshot({
			seq: 1,
			entities: [
				...entities({ accounts: [account({ id: "work", label: "Work" })] }),
				...more,
			],
		});

	it("names a token's Mac login, or says it's gone", () => {
		const held = (origin: string) =>
			heldOf(
				view([
					credential("dev-a", {
						type: "claude_token",
						origin,
						account: "a1",
						label: "Box",
					}),
				]),
				"claude_token"
			)[0]!;
		expect(tokenLine(view([]), held("work"))).toBe("Box, for Work on the Mac");
		expect(tokenLine(view([]), held("old"))).toBe("Box, for a Mac login that's gone");
	});

	it("says who a Codex session is signed in as", () => {
		const session = (email: string | null) =>
			heldOf(
				view([credential("dev-a", { type: "chat_gpt", account: "c1", email })]),
				"chat_gpt"
			)[0]!;
		expect(sessionLine(session("me@example.com"))).toBe("Codex, me@example.com");
		expect(sessionLine(session(null))).toBe("Codex");
		const target: Target = { device: "dev-a", name: "alpha", live: true };
		expect(codexStatus(target, session("me@example.com"))).toBe("Signed in as me@example.com");
		expect(codexStatus(target, undefined)).toBe("Not signed in");
		expect(codexStatus({ ...target, live: false }, undefined)).toBe(
			"Connect to alpha to sign in."
		);
	});

	it("shows an App's ID, slug, key and reach", () => {
		const app = {
			kind: {
				type: "github_app",
				app_id: 7,
				slug: "maxhq-me",
				key_sha256: "0123456789abcdef",
				repos: [],
			},
		} as unknown as Held<"github_app">;
		expect(appLines(app)).toEqual([
			"App 7, maxhq-me. Key SHA-256 0123456789ab…",
			"Reaches no repo yet. Install the App on the repos your projects use.",
		]);
	});

	it("shows an APNs key's ID and team", () => {
		const key = { kind: { type: "apns", key_id: "K1", team_id: "T1" } } as Held<"apns">;
		expect(apnsLine(key)).toBe("Key K1, team T1.");
	});
});
