// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import { agent, device, entities, link, NOW, workspace } from "../test/build";
import { githubApp, githubUser, summary } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import {
	githubLoginOn,
	githubSignIn,
	pullDevice,
	pullsDevice,
	signInDevices,
	signInReason,
} from "./devices";

const DOES = { transfers: true, tunnels: true, workspaces: true, pulls: true, github_user: true };
const OFFLINE = { state: "unreachable", reason: "auth failed", last_seen: null } as const;

/** Three devices: mac and box live, old offline. Each has the project "p". */
const viewWith = (extra: Entity[] = [], over: { boxDoes?: object } = {}) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "dev-mac", name: "Mac" }),
					device({ id: "dev-box", name: "box" }),
					device({ id: "dev-old", name: "old" }),
				],
				links: [
					link({ id: "mac", name: "Mac", capabilities: DOES }),
					link({ id: "box", name: "box", capabilities: { ...DOES, ...over.boxDoes } }),
					link({ id: "old", name: "old", capabilities: DOES, state: OFFLINE }),
				],
				workspaces: [
					workspace({ id: "w-mac", project: "p", device: "dev-mac" }),
					workspace({ id: "w-box", project: "p", device: "dev-box" }),
					workspace({ id: "w-old", project: "p", device: "dev-old" }),
				],
				agents: [
					agent({ id: "a-old", workspace: "w-old" }),
					agent({ id: "a-box", workspace: "w-box" }),
				],
			}),
			...extra,
		],
	});

describe("githubSignIn", () => {
	it("is none when no device holds a GitHub login", () => {
		expect(githubSignIn(viewWith([githubApp("dev-box")]), NOW)).toEqual({ state: "none" });
	});

	it("is ready on a live device holding one", () => {
		expect(githubSignIn(viewWith([githubUser("dev-box", "octo")]), NOW)).toEqual({
			state: "ready",
			device: "dev-box",
			name: "box",
			login: "octo",
		});
	});

	it("is offline when only an offline device holds one", () => {
		const signIn = githubSignIn(viewWith([githubUser("dev-old")]), NOW);
		expect(signIn).toMatchObject({ state: "offline", device: "dev-old", name: "old" });
	});

	it("takes a live holder over an offline one", () => {
		const view = viewWith([githubUser("dev-old"), githubUser("dev-mac")]);
		expect(githubSignIn(view, NOW)).toMatchObject({ state: "ready", device: "dev-mac" });
	});
});

describe("signInReason", () => {
	it("asks you to sign in, or names the offline holder", () => {
		expect(signInReason({ state: "none" }, "approve")).toBe(
			"Sign in to GitHub in Settings to approve here."
		);
		const offline = { state: "offline", device: "dev-old", name: "old", login: "you" } as const;
		expect(signInReason(offline, "approve")).toBe("Approving needs old, which is offline.");
		expect(signInReason({ ...offline, state: "ready" }, "merge")).toBeNull();
	});
});

describe("githubLoginOn", () => {
	it("gives a device's GitHub login and when it was stored", () => {
		const view = viewWith([githubUser("dev-box", "octo")]);
		expect(githubLoginOn(view, "dev-box")).toEqual({
			login: "octo",
			storedAt: NOW - 3_600_000,
		});
		expect(githubLoginOn(view, "dev-mac")).toBeUndefined();
	});
});

describe("signInDevices", () => {
	it("lists the live devices that can hold a sign-in, by name", () => {
		expect(signInDevices(viewWith(), NOW)).toEqual([
			{ device: "dev-box", name: "box" },
			{ device: "dev-mac", name: "Mac" },
		]);
	});

	it("leaves out a maxd too old to hold one", () => {
		const view = viewWith([], { boxDoes: { github_user: false } });
		expect(signInDevices(view, NOW).map((d) => d.device)).toEqual(["dev-mac"]);
	});
});

describe("pullsDevice", () => {
	it("takes a live device with a GitHub App first", () => {
		expect(pullsDevice(viewWith([githubApp("dev-box")]), "p", NOW)).toBe("dev-box");
		expect(pullsDevice(viewWith([githubApp("dev-old")]), "p", NOW)).toBe("dev-box");
	});

	it("takes one whose maxd reads PRs over one that doesn't", () => {
		const view = viewWith([], { boxDoes: { pulls: false } });
		expect(pullsDevice(view, "p", NOW)).toBe("dev-mac");
	});

	it("falls back to the first live device by ID", () => {
		expect(pullsDevice(viewWith(), "p", NOW)).toBe("dev-box");
	});

	it("is undefined for a project no device has", () => {
		expect(pullsDevice(viewWith(), "other", NOW)).toBeUndefined();
	});
});

describe("pullDevice", () => {
	it("reads on a live linked agent's device", () => {
		const pull = { ...summary(1), agents: ["a-old", "a-box"] };
		expect(pullDevice(viewWith([githubApp("dev-mac")]), pull, "p", NOW)).toBe("dev-box");
	});

	it("falls back to the project's device", () => {
		const pull = { ...summary(1), agents: ["a-old"] };
		expect(pullDevice(viewWith([githubApp("dev-mac")]), pull, "p", NOW)).toBe("dev-mac");
	});
});
