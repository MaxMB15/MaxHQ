// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { PROTOCOL_VERSION, SIGNED_UPDATES_SINCE } from "../bindings";
import { device, link, MIN, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	applyProgress,
	closeFinishedRun,
	describeRunDevice,
	describeUpdateCapsule,
	dismissSettled,
	failedDevices,
	formatUpdateElapsed,
	checksSignatures,
	compareVersions,
	isBehind,
	isUnsigned,
	offersUpdate,
	planUpdate,
	restartDevices,
	startRun,
	versionLine,
} from "./update-view";
import type { Entity, ToolInfo } from "./wire";

const LINUX = "x86_64-unknown-linux-musl";
const BUNDLED = [{ target: LINUX, build: "new", signed: true }];
const APP = "0.1.0";

const tool = (over: Partial<ToolInfo>): ToolInfo => ({
	kind: { type: "agent", tool: "claude" },
	place: { type: "host" },
	path: "/usr/local/bin/claude",
	version: "2.1.40",
	latest: "2.1.42",
	owner: "user",
	method: "native",
	update: { type: "needs_tick" },
	state: { type: "idle" },
	...over,
});

const viewOf = (entities: Entity[]) =>
	applySnapshot({ seq: 1, entities } as Parameters<typeof applySnapshot>[0]);

describe("a CLI's version", () => {
	it("says how far behind it is and who installed it", () => {
		expect(versionLine(tool({}))).toBe(
			"Version 2.1.40, 2.1.42 is out. You installed it with native."
		);
		expect(versionLine(tool({ latest: "2.1.40", owner: "max_hq" }))).toBe(
			"Version 2.1.40, the newest. MaxHQ installed it with native."
		);
		expect(versionLine(tool({ latest: null }))).toBe(
			"Version 2.1.40. You installed it with native."
		);
		expect(versionLine(tool({ version: null }))).toBe(
			"Version unknown. You installed it with native."
		);
	});

	it("offers Update unless you or nobody updates it", () => {
		expect(offersUpdate(tool({}))).toBe(true);
		expect(offersUpdate(tool({ update: { type: "never" } }))).toBe(false);
	});
});

describe("compareVersions", () => {
	it("orders older, the same and newer", () => {
		expect(compareVersions("0.1.4", "0.1.5")).toBeLessThan(0);
		expect(compareVersions("0.1.5", "0.1.5")).toBe(0);
		expect(compareVersions("0.2.0", "0.1.5")).toBeGreaterThan(0);
		expect(compareVersions("0.1.10", "0.1.9")).toBeGreaterThan(0);
		expect(compareVersions("1.0.0", "0.99.99")).toBeGreaterThan(0);
	});

	it("puts a pre-release before its release, as semver does", () => {
		expect(compareVersions("0.1.5-beta.1", "0.1.5")).toBeLessThan(0);
		expect(compareVersions("0.1.5-beta.2", "0.1.5-beta.10")).toBeLessThan(0);
		expect(compareVersions("0.1.5-alpha", "0.1.5-beta")).toBeLessThan(0);
		expect(compareVersions("0.1.5+build.7", "0.1.5")).toBe(0);
	});

	it("can't compare a build that has no version", () => {
		expect(compareVersions("", "0.1.5")).toBeNull();
		expect(compareVersions("0.1.5", null)).toBeNull();
		expect(compareVersions("dev", "0.1.5")).toBeNull();
	});
});

describe("isBehind", () => {
	const d = { ...device({ id: "dev-a" }), build: "old", target: LINUX, maxd_version: "0.1.5" };

	it("compares the build with the one the app carries for the target", () => {
		expect(isBehind(d, BUNDLED, "0.1.5")).toBe(true);
		expect(isBehind({ ...d, build: "new" }, BUNDLED, "0.1.5")).toBe(false);
	});

	it("is never behind when either side names no build", () => {
		expect(isBehind(d, null, "0.1.5")).toBe(false);
		expect(isBehind({ ...d, build: null }, BUNDLED, "0.1.5")).toBe(false);
		expect(isBehind({ ...d, target: "aarch64-apple-darwin" }, BUNDLED, "0.1.5")).toBe(false);
	});

	it("leaves a maxd newer than the app alone, and updates an older one", () => {
		expect(isBehind({ ...d, maxd_version: "0.1.6" }, BUNDLED, "0.1.5")).toBe(false);
		expect(isBehind({ ...d, maxd_version: "0.1.4" }, BUNDLED, "0.1.5")).toBe(true);
	});

	it("goes by the build alone when a version doesn't read", () => {
		expect(isBehind({ ...d, maxd_version: "" }, BUNDLED, "0.1.5")).toBe(true);
		expect(isBehind({ ...d, maxd_version: "0.1.6" }, BUNDLED, null)).toBe(true);
	});
});

describe("signed updates", () => {
	it("counts a build from source, or an app that names no signing, as unsigned", () => {
		const d = { ...device({ id: "dev-a" }), build: "old", target: LINUX };
		expect(isUnsigned(d, BUNDLED)).toBe(false);
		expect(isUnsigned(d, [{ ...BUNDLED[0]!, signed: false }])).toBe(true);
		expect(isUnsigned(d, null)).toBe(true);
	});

	it("knows a maxd from before signed updates installs without a check", () => {
		expect(checksSignatures(SIGNED_UPDATES_SINCE)).toBe(true);
		expect(checksSignatures(PROTOCOL_VERSION)).toBe(true);
		expect(checksSignatures(SIGNED_UPDATES_SINCE - 1)).toBe(false);
		expect(checksSignatures(null)).toBe(false);
	});
});

describe("planUpdate", () => {
	const entities = (
		over: {
			ticked?: ("claude" | "codex")[];
			tools?: ToolInfo[];
			protocol?: number | null;
		} = {}
	): Entity[] => [
		{ entity: "device", ...device({ id: "dev-a" }), build: "old", target: LINUX },
		{
			entity: "device_link",
			...link({ id: "a" }),
			protocol: over.protocol === undefined ? PROTOCOL_VERSION : over.protocol,
		},
		{ entity: "device", ...device({ id: "dev-b" }), build: "new", target: LINUX },
		{ entity: "device_link", ...link({ id: "b" }) },
		{ entity: "device", ...device({ id: "dev-c" }), build: "old", target: LINUX },
		{ entity: "device_link", ...link({ id: "c", state: { state: "unknown" } }) },
		{
			entity: "workspace",
			...workspace({
				id: "ws-1",
				device: "dev-b",
				location: { type: "container", instance: "mhq-one", path: "/w" },
			}),
		},
		{
			entity: "device_tools",
			device: "dev-b",
			tools: over.tools ?? [
				tool({}),
				tool({
					kind: { type: "agent", tool: "codex" },
					place: { type: "workspace", workspace: "ws-1" },
					update: { type: "auto" },
					version: "0.51.0",
					latest: "0.52.0",
				}),
				tool({
					kind: { type: "agent", tool: "codex" },
					update: { type: "manual", command: "brew upgrade codex" },
				}),
				// A helper MaxHQ installed, which Update everything leaves out.
				tool({
					kind: { type: "helper", tool: "node" },
					place: { type: "workspace", workspace: "ws-1" },
					update: { type: "auto" },
				}),
				tool({
					kind: { type: "agent", tool: "cursor" },
					update: { type: "never" },
					version: "1.0.0",
					latest: "1.1.0",
				}),
			],
		},
		...(over.ticked
			? [
					{
						entity: "device_settings" as const,
						device: "dev-b",
						default_permission: "ask" as const,
						workspace_network: null,
						workspace_pool: null,
						tool_updates: over.ticked,
					},
				]
			: []),
	];

	it("lists connected devices that are behind or have a CLI MaxHQ may update", () => {
		const plan = planUpdate(viewOf(entities()), BUNDLED, APP);
		expect(plan.rows).toEqual([
			{
				device: "dev-a",
				label: "a",
				maxd: true,
				unsigned: false,
				unchecked: false,
				tools: [],
			},
			{
				device: "dev-b",
				label: "b",
				maxd: false,
				unsigned: false,
				unchecked: false,
				tools: ["Codex in mhq-one"],
			},
		]);
		expect(plan.summary).toBe("Update 1 device and 1 CLI");
		expect(plan.tools).toBe(true);
		expect(plan.unsigned).toBe(false);
	});

	it("asks first when a device would get a build from source", () => {
		const plan = planUpdate(viewOf(entities()), [{ ...BUNDLED[0]!, signed: false }], APP);
		expect(plan.rows[0]).toMatchObject({ device: "dev-a", maxd: true, unsigned: true });
		// A device only getting its CLIs gets no maxd, signed or not.
		expect(plan.rows[1]).toMatchObject({ device: "dev-b", unsigned: false });
		expect(plan.unsigned).toBe(true);
	});

	it("marks a device whose maxd is too old to check signatures", () => {
		for (const protocol of [SIGNED_UPDATES_SINCE - 1, null]) {
			const plan = planUpdate(viewOf(entities({ protocol })), BUNDLED, APP);
			expect(plan.rows[0]).toMatchObject({ device: "dev-a", unchecked: true });
		}
	});

	it("leaves out a device whose maxd is newer than the app", () => {
		const plan = planUpdate(viewOf(entities({ tools: [] })), BUNDLED, "0.0.9");
		expect(plan).toEqual({ summary: null, rows: [], tools: false, unsigned: false });
		// Its CLIs still update, without a maxd.
		const tools = planUpdate(viewOf(entities()), BUNDLED, "0.0.9");
		expect(tools.rows.map((r) => [r.device, r.maxd])).toEqual([["dev-b", false]]);
		expect(tools.summary).toBe("Update 1 CLI");
	});

	it("adds the host's CLIs once you tick them", () => {
		const plan = planUpdate(viewOf(entities({ ticked: ["claude"] })), BUNDLED, APP);
		expect(plan.rows[1]?.tools).toEqual(["Claude", "Codex in mhq-one"]);
		expect(plan.summary).toBe("Update 1 device and 2 CLIs");
	});

	it("has nothing to do when every build matches and no CLI is behind", () => {
		const plan = planUpdate(viewOf(entities({ tools: [] })), null, APP);
		expect(plan).toEqual({ summary: null, rows: [], tools: false, unsigned: false });
	});
});

describe("a run", () => {
	const rows = [
		{ device: "dev-a", label: "a" },
		{ device: "dev-b", label: "b" },
	];

	it("counts the devices that are done", () => {
		let run = startRun(rows, false, false, NOW);
		run = applyProgress(run, { device: "dev-a", status: { status: "uploading" } }, NOW);
		expect(describeUpdateCapsule(run, NOW)).toEqual({
			tone: "default",
			label: "0 of 2 updated",
			action: null,
		});
		run = applyProgress(run, { device: "dev-a", status: { status: "done" } }, NOW);
		expect(describeUpdateCapsule(run, NOW).label).toBe("1 of 2 updated");
	});

	it("names the slowest device after three minutes", () => {
		let run = startRun(rows, false, false, NOW);
		run = applyProgress(run, { device: "dev-b", status: { status: "restarting" } }, NOW);
		const later = NOW + 4 * MIN;
		expect(describeUpdateCapsule(run, later)).toEqual({
			tone: "slow",
			label: "b",
			slow: { status: "restarting", elapsed: "4m" },
			action: null,
		});
		expect(describeRunDevice(run.devices[1]!, later)).toBe("restarting 4m");
		expect(describeRunDevice(run.devices[0]!, later)).toBe("waiting");
	});

	it("offers a retry for failures and a dismiss for skips", () => {
		let run = startRun(rows, false, false, NOW);
		run = applyProgress(
			run,
			{ device: "dev-a", status: { status: "failed", reason: "no" } },
			NOW
		);
		run = applyProgress(
			run,
			{ device: "dev-b", status: { status: "skipped", reason: "offline" } },
			NOW
		);
		expect(describeUpdateCapsule(run, NOW)).toEqual({
			tone: "failed",
			label: "a failed",
			action: "retry",
		});
		expect(failedDevices(run)).toEqual(["dev-a"]);
		expect(describeRunDevice(run.devices[1]!, NOW)).toBe("skipped: offline");

		const retried = restartDevices(run, ["dev-a"], NOW + MIN);
		expect(retried.devices[0]?.status).toEqual({ status: "waiting" });
		const skipped = applyProgress(
			retried,
			{ device: "dev-a", status: { status: "done" } },
			NOW + MIN
		);
		expect(describeUpdateCapsule(skipped, NOW).action).toBe("dismiss");
		expect(dismissSettled(skipped)).toBeNull();
	});

	it("closes once every device is done", () => {
		let run = startRun(rows, false, false, NOW);
		for (const { device: d } of rows) {
			run = applyProgress(run, { device: d, status: { status: "done" } }, NOW);
		}
		expect(closeFinishedRun(run)).toBeNull();
	});
});

describe("formatUpdateElapsed", () => {
	it("rounds down to the minute and the hour", () => {
		expect(formatUpdateElapsed(30_000)).toBe("<1m");
		expect(formatUpdateElapsed(12 * MIN)).toBe("12m");
		expect(formatUpdateElapsed(75 * MIN)).toBe("1h 15m");
	});
});
