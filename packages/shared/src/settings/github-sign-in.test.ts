// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ActionOutcome, Entity } from "../bindings";
import type { ActionResult } from "../data/source";
import type { SignInDevice } from "../pulls/devices";
import { device, entities, MIN, NOW } from "../test/build";
import { credential, githubUser } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import {
	appSettingsUrl,
	appSlugOn,
	defaultHolder,
	type Started,
	signInStateOf,
	startedFrom,
} from "./github-sign-in";

const view = (more: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
			}),
			...more,
		],
	});

const ok = (data: object): ActionResult => ({ status: "ok", data: data as ActionOutcome });

const code = (verification_uri = "https://github.com/login/device"): ActionResult =>
	ok({
		status: "device_code",
		sign_in: "sign-1",
		user_code: "ABCD-1234",
		verification_uri,
		expires_at: NOW + 15 * MIN,
	});

const app = (device: string, slug: string | null) =>
	credential(device, { type: "github_app", app_id: 7, slug, key_sha256: "00", repos: [] });

describe("startedFrom", () => {
	it("waits on the code the device gave", () => {
		const started = startedFrom(code(), "dev-box", null);
		expect(started).toEqual({
			state: "waiting",
			code: {
				device: "dev-box",
				signIn: "sign-1",
				userCode: "ABCD-1234",
				verificationUri: "https://github.com/login/device",
				expiresAt: NOW + 15 * MIN,
				before: null,
			},
		});
	});

	it("turns down a page that isn't GitHub's sign-in page", () => {
		for (const page of [
			"https://example.com/login",
			"https://github.com/login/oauth/authorize?client_id=Iv1.example",
			"https://github.com/apps/example-app/installations/new",
			"https://github.com/login/device/../oauth/authorize",
		]) {
			expect(startedFrom(code(page), "dev-box", null)).toEqual({
				state: "failed",
				device: "dev-box",
				reason: "The device named a page that isn't GitHub's sign-in page.",
			});
		}
	});

	it("says why the device refused", () => {
		const started = startedFrom(
			ok({ status: "refused", reason: "No GitHub App here" }),
			"dev-box",
			null
		);
		expect(started).toEqual({
			state: "failed",
			device: "dev-box",
			reason: "No GitHub App here",
		});
	});

	it("says the device gave no code when it answered something else", () => {
		expect(startedFrom(ok({ status: "done" }), "dev-box", null)).toMatchObject({
			state: "failed",
			reason: "The device didn't give a code.",
		});
	});

	it("says when the device isn't connected", () => {
		const result: ActionResult = {
			status: "error",
			error: { kind: "not_connected", device: "dev-box" },
		};
		expect(startedFrom(result, "dev-box", null)).toEqual({
			state: "failed",
			device: "dev-box",
			reason: "dev-box isn't connected.",
		});
	});
});

describe("signInStateOf", () => {
	const waiting = startedFrom(code(), "dev-box", null) as Started;

	it("waits while the code still works", () => {
		expect(signInStateOf(waiting, view(), NOW)).toMatchObject({
			state: "waiting",
			userCode: "ABCD-1234",
		});
	});

	it("is done once the device holds a new login", () => {
		expect(signInStateOf(waiting, view([githubUser("dev-box", "me")]), NOW)).toEqual({
			state: "done",
			device: "dev-box",
			login: "me",
		});
	});

	it("keeps waiting while the device holds only the login it had", () => {
		const held = githubUser("dev-box", "me") as Entity & { stored_at: number };
		const before = startedFrom(code(), "dev-box", held.stored_at);
		expect(signInStateOf(before, view([held]), NOW).state).toBe("waiting");
	});

	it("runs out with the code", () => {
		expect(signInStateOf(waiting, view(), NOW + 15 * MIN)).toEqual({
			state: "expired",
			device: "dev-box",
		});
	});

	it("passes any other state through", () => {
		const idle: Started = { state: "idle" };
		expect(signInStateOf(idle, view(), NOW)).toBe(idle);
	});
});

describe("the App on a device", () => {
	const devices: SignInDevice[] = [
		{ device: "dev-mac", name: "Mac" } as SignInDevice,
		{ device: "dev-box", name: "box" } as SignInDevice,
	];

	it("starts the picker on a device holding an App", () => {
		expect(defaultHolder(view([app("dev-box", null)]), devices)).toBe("dev-box");
		expect(defaultHolder(view(), devices)).toBe("dev-mac");
		expect(defaultHolder(view(), [])).toBeUndefined();
	});

	it("finds the App's slug and its settings page", () => {
		expect(appSlugOn(view([app("dev-box", "maxhq-me")]), "dev-box")).toBe("maxhq-me");
		expect(appSlugOn(view([app("dev-box", null)]), "dev-box")).toBeNull();
		expect(appSettingsUrl("maxhq-me")).toBe("https://github.com/settings/apps/maxhq-me");
	});
});
