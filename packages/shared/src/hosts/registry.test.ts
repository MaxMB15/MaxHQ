// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { registryNames, registryOf, registryRows, uncheckedWarning } from "./registry";

const load = async () =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

describe("registry", () => {
	it("lists installed agents first, each with how it installs", async () => {
		const registry = registryOf(await load(), MAC)!;
		const rows = registryRows(registry);
		const byId = new Map(rows.map((r) => [r.agent.id, r]));
		expect(rows.slice(0, 2).every((r) => r.agent.installed !== null)).toBe(true);
		expect(byId.get("goose")).toMatchObject({ update: true, installable: true });
		expect(byId.get("goose")!.status).toMatch(/^Installed .*, .* is out$/);
		expect(byId.get("kiro")).toMatchObject({ needsConfirm: true, installable: true });
		expect(byId.get("mac-only")).toMatchObject({
			installable: false,
			status: "Not available for this device",
		});
		expect(uncheckedWarning(byId.get("kiro")!.agent)).toMatch(/no checksum for /);
	});

	it("searches by name, id and description", async () => {
		const registry = registryOf(await load(), MAC)!;
		expect(registryRows(registry, "  GEMINI ").map((r) => r.agent.id)).toEqual(["gemini"]);
		expect(registryRows(registry, "nothing like it")).toEqual([]);
		expect(registryNames(registry).get("fast-agent")).toBeDefined();
		expect(registryNames(undefined).size).toBe(0);
	});

	it("keeps the problem of a fetch that failed", async () => {
		expect(registryOf(await load(), DEVBOX)?.problem).not.toBeNull();
	});
});
