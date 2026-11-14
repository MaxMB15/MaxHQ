// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ToolInfo } from "../bindings";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	installProgress,
	installRows,
	missingHelpers,
	pendingLicences,
	sameKind,
	samePlace,
	toolName,
} from "./tools";

const load = async () =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

describe("tool installs", () => {
	it("shows each helper and ACP agent with how its install stands", async () => {
		const view = await load();
		const rows = installRows(view, MAC, new Map([["goose", "Goose"]]));
		const byName = new Map(rows.map((r) => [r.name, r]));
		expect(rows.every((r) => r.tool.kind.type !== "agent")).toBe(true);
		expect(byName.get("Java")?.status).toBe("Installing");
		expect(byName.get("Java")?.progress?.text).toMatch(/^Downloading .* of /);
		expect(byName.get("Java")?.progress?.fraction).toBeCloseTo(120 / 190);
		expect(byName.get("Android SDK")?.status).toBe("Waiting for you to accept its licences");
		expect(byName.get("Goose")).toBeDefined();
		const devbox = new Map(installRows(view, DEVBOX).map((r) => [r.name, r]));
		expect(devbox.get("uv")?.status).toMatch(/^Failed: /);
		expect(devbox.get("Node.js")?.progress?.text).toBe("Checking the download");
	});

	it("gathers the licences installs wait on", async () => {
		const asks = pendingLicences(await load());
		expect(asks.length).toBeGreaterThan(0);
		expect(asks.every((a) => a.device === MAC)).toBe(true);
	});

	it("tells tools and places apart", () => {
		expect(sameKind({ type: "acp", id: "a" }, { type: "acp", id: "a" })).toBe(true);
		expect(sameKind({ type: "acp", id: "a" }, { type: "helper", tool: "node" })).toBe(false);
		expect(sameKind({ type: "helper", tool: "node" }, { type: "helper", tool: "uv" })).toBe(
			false
		);
		expect(samePlace({ type: "host" }, { type: "workspace", workspace: "ws-alpha" })).toBe(
			false
		);
		expect(
			samePlace(
				{ type: "workspace", workspace: "ws-alpha" },
				{ type: "workspace", workspace: "ws-alpha" }
			)
		).toBe(true);
		expect(toolName({ type: "acp", id: "kiro" })).toBe("kiro");
	});

	it("says nothing of progress for a tool at rest, and sizes without a total", () => {
		const tool = { state: { type: "idle" } } as ToolInfo;
		expect(installProgress(tool)).toBeNull();
		const unsized = {
			state: { type: "running", message: "Fetching", phase: "downloading", bytes_done: 2048 },
		} as ToolInfo;
		expect(installProgress(unsized)?.fraction).toBeNull();
		const bare = { state: { type: "running", message: "Fetching" } } as ToolInfo;
		expect(installProgress(bare)).toEqual({ text: "Fetching", fraction: null });
	});

	it("offers the helpers a device doesn't list, and the Android SDK only on a Mac", async () => {
		const view = await load();
		expect(missingHelpers(view, MAC)).toEqual(["uv", "gh", "agent_device"]);
		expect(missingHelpers(view, DEVBOX)).toEqual(["node", "agent_device", "jdk"]);
	});
});
