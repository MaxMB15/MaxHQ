// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { MOCK_MOVE } from "./mock-hosts";
import { mockSource } from "./mock-source";
import { DEVBOX, MAC, MHQ_WORKSPACE } from "./mock-view";

const source = () => mockSource({ now: () => NOW, tick: false });

const view = async (mock: ReturnType<typeof source>) => applySnapshot(await mock.snapshot());

const settle = async () => {
	await vi.runAllTimersAsync();
};

afterEach(() => vi.useRealTimers());

describe("mock host calls", () => {
	it("refuses a call the device's maxd doesn't offer, in words", async () => {
		const mock = source();
		expect(await mock.planMove(MAC, "ws-alpha")).toMatchObject({
			status: "error",
			error: { message: expect.stringMatching(/maxd is too old to move workspaces/) },
		});
	});

	it("applies the fixes maxd can do now", async () => {
		const mock = source();
		await mock.applyHost(DEVBOX, ["caps.parent"]);
		const items = (await view(mock)).tables.host_report.get(DEVBOX)!.items;
		expect(items.find((i) => i.id === "caps.parent")?.state).toEqual({ state: "ok" });
		expect(items.find((i) => i.id === "caps.zram")?.state.state).toBe("drift");
	});

	it("installs a tool through its phases", async () => {
		vi.useFakeTimers();
		const mock = source();
		await mock.installTool(MAC, { type: "host" }, { type: "helper", tool: "uv" }, "0.9.0");
		const mid = (await view(mock)).tables.device_tools.get(MAC)!.tools;
		expect(mid.find((t) => t.kind.type === "helper" && t.kind.tool === "uv")?.state.type).toBe(
			"running"
		);
		await settle();
		const done = (await view(mock)).tables.device_tools.get(MAC)!.tools;
		expect(done.find((t) => t.kind.type === "helper" && t.kind.tool === "uv")).toMatchObject({
			version: "0.9.0",
			state: { type: "idle" },
		});
	});

	it("installs once its licence is accepted as read", async () => {
		vi.useFakeTimers();
		const mock = source();
		const sdk = (await view(mock)).tables.device_tools
			.get(MAC)!
			.tools.find((t) => t.state.type === "licences")!;
		if (sdk.state.type !== "licences") throw new Error("no licence");
		const licence = sdk.state.pending[0]!;
		expect(await mock.acceptLicence(MAC, licence.id, "0".repeat(64))).toMatchObject({
			status: "ok",
			data: { status: "refused" },
		});
		await mock.acceptLicence(MAC, licence.id, licence.text_sha256);
		await settle();
		const after = (await view(mock)).tables.device_tools.get(MAC)!.tools;
		expect(after.some((t) => t.state.type === "licences")).toBe(false);
	});

	it("asks to confirm a binary with no checksum", async () => {
		vi.useFakeTimers();
		const mock = source();
		const kiro = (await view(mock)).tables.acp_registry
			.get(MAC)!
			.agents.find((a) => a.id === "kiro")!;
		const host = { type: "host" } as const;
		expect(await mock.installAcpAgent(MAC, "kiro", kiro.version, host, false)).toMatchObject({
			data: { status: "refused" },
		});
		await mock.installAcpAgent(MAC, "kiro", kiro.version, host, true);
		await settle();
		const after = (await view(mock)).tables.acp_registry.get(MAC)!.agents;
		expect(after.find((a) => a.id === "kiro")?.installed).toBe(kiro.version);
	});

	it("walks a move a step at a time, and back while it's kept", async () => {
		vi.useFakeTimers();
		const mock = source();
		expect(await mock.runMoveStep(MOCK_MOVE, "switch")).toMatchObject({
			data: { status: "refused" },
		});
		await mock.runMoveStep(MOCK_MOVE, "copy");
		await settle();
		expect((await view(mock)).tables.move.get(MOCK_MOVE)?.step).toBe("converge");
		await mock.runMoveStep(MOCK_MOVE, "converge");
		await settle();
		await mock.runMoveStep(MOCK_MOVE, "switch");
		await settle();
		const kept = (await view(mock)).tables.move.get(MOCK_MOVE)!;
		expect(kept.step).toBe("keep");
		expect(kept.kept_until).not.toBeNull();
		await mock.moveBack(MOCK_MOVE);
		expect((await view(mock)).tables.move.get(MOCK_MOVE)?.state).toEqual({
			state: "moved_back",
		});
	});

	it("copies again before the switch and clears the change", async () => {
		vi.useFakeTimers();
		const mock = source();
		await mock.runMoveStep("move-bravo", "copy");
		await settle();
		const move = (await view(mock)).tables.move.get("move-bravo")!;
		expect(move).toMatchObject({ step: "switch", source_changed: false });
	});

	it("lists a container's snapshots", async () => {
		const listed = await source().listSnapshots(MHQ_WORKSPACE);
		expect(listed).toMatchObject({ status: "ok", data: { status: "snapshots" } });
	});
});
