// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC, MHQ_WORKSPACE } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { bootLine, deleteRefusal, deviceWorkspaces, showsSnapshots } from "./device";
import { isMade } from "./state";

const load = async (scenario = "connected") =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

describe("device workspaces", () => {
	it("lists what MaxHQ made on a device with Incus", async () => {
		const view = await load();
		const section = deviceWorkspaces(view, DEVBOX, NOW)!;
		expect(section.made.map((w) => w.id)).toContain(MHQ_WORKSPACE);
		expect(section.makes).toBe(true);
		expect(section.live).toBe(true);
		expect(section.hint).toBe("Containers MaxHQ makes and runs");
	});

	it("shows nothing for a device without Incus and nothing made", async () => {
		expect(deviceWorkspaces(await load(), MAC, NOW)).toBeNull();
	});

	it("names the containers it leaves alone, and how to make one while away", async () => {
		const view = await load("peer_offline");
		const section = deviceWorkspaces(view, DEVBOX, NOW)!;
		expect(section.live).toBe(false);
		expect(section.footer).toBe(
			"MaxHQ didn't make the other 6 containers here, so it leaves them alone."
		);
		const workspace = new Map(
			[...view.tables.workspace].filter(([, w]) => w.device !== DEVBOX || isMade(w))
		);
		const alone = { ...view, tables: { ...view.tables, workspace } };
		expect(deviceWorkspaces(alone, DEVBOX, NOW)!.footer).toBe("Connect to devbox to make one.");
	});

	it("offers snapshots of a container it can start or stop", async () => {
		const view = await load();
		const made = view.tables.workspace.get(MHQ_WORKSPACE)!;
		if (!isMade(made)) throw new Error("the sample workspace is made by MaxHQ");
		expect(showsSnapshots(view, made, ["stop", "delete"])).toBe(true);
		expect(showsSnapshots(view, made, ["delete"])).toBe(false);
		expect(showsSnapshots(view, made, [])).toBe(false);
	});

	it("reads a refused delete and the boot line", async () => {
		expect(
			deleteRefusal({ status: "ok", data: { status: "refused", reason: "2 commits" } })
		).toBe("2 commits");
		expect(deleteRefusal({ status: "ok", data: { status: "done" } })).toBeNull();
		const made = (await load()).tables.workspace.get(MHQ_WORKSPACE)!;
		if (!isMade(made)) throw new Error("the sample workspace is made by MaxHQ");
		expect(
			bootLine({ ...made, managed: { ...made.managed, start_at_boot: true } }, "devbox")
		).toBe("Starts when devbox boots");
		expect(
			bootLine({ ...made, managed: { ...made.managed, start_at_boot: false } }, "devbox")
		).toBe("Starts only when you start it");
	});
});
