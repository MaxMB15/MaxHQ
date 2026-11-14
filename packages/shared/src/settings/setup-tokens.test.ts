// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import { applySnapshot } from "../view/replica";

import { knowsSetupTokens, setupTokenQuestion, takesSetupTokens } from "./setup-tokens";

const settings = (device: string, on?: boolean): Entity =>
	({
		entity: "device_settings",
		device,
		default_permission: "auto",
		workspace_network: null,
		workspace_pool: null,
		...(on === undefined ? {} : { claude_setup_token: on }),
	}) as Entity;

const view = applySnapshot({
	seq: 1,
	entities: [settings("dev-on", true), settings("dev-off", false), settings("dev-old")],
});

describe("setup-tokens", () => {
	it("takes them only where the setting is on, or where maxd predates it", () => {
		expect(takesSetupTokens(view, "dev-on")).toBe(true);
		expect(takesSetupTokens(view, "dev-off")).toBe(false);
		expect(takesSetupTokens(view, "dev-old")).toBe(true);
		expect(knowsSetupTokens(view, "dev-off")).toBe(true);
		expect(knowsSetupTokens(view, "dev-old")).toBe(false);
	});

	it("asks with what turning them on means and the alternative", () => {
		const question = setupTokenQuestion("devbox");
		expect(question.title).toBe("Turn on setup-tokens for devbox?");
		expect(question.detail).toContain("personal use");
		expect(question.detail).toContain("stores your Claude setup-token on devbox");
		expect(question.detail).toContain("sign in with claude on each device");
	});
});
