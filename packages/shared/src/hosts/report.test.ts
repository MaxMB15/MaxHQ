// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { ROOT_FIX } from "../data/mock-hosts";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	fixableNow,
	hostGroups,
	hostProblems,
	hostReportOf,
	offersHostSpec,
	rootParts,
	sourceText,
	terminalLine,
} from "./report";

const load = async () =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

describe("host report", () => {
	it("groups the items, problems first, each with its fix", async () => {
		const view = await load();
		const report = hostReportOf(view, DEVBOX)!;
		const groups = hostGroups(view, report);
		expect(groups.map((g) => [g.label, g.problems])).toEqual([
			["Incus", 0],
			["Limits", 2],
			["Tools", 1],
		]);
		const limits = groups[1]!.items;
		expect(limits.map((i) => [i.id, i.status, i.fix.kind])).toEqual([
			["caps.parent", "Missing", "now"],
			["caps.zram", "off, should be on", "terminal"],
		]);
		expect(groups[2]!.items[0]!.status).toBe("Couldn't run node --version.");
		expect(hostProblems(report)).toBe(3);
		expect(fixableNow(report)).toEqual(["caps.parent", "tools.user.node"]);
		expect(sourceText(report.source)).toBe("From /etc/maxhq/maxhq-host.toml");
		expect(sourceText(hostReportOf(view, MAC)!.source)).toBe("No spec file, so the defaults");
	});

	it("runs a root fix over ssh along the device's route, or alone on the Mac", async () => {
		const view = await load();
		expect(terminalLine(view, DEVBOX, ROOT_FIX)).toBe(`ssh -t dev@devbox '${ROOT_FIX}'`);
		expect(terminalLine(view, MAC, ROOT_FIX)).toBe(ROOT_FIX);
		expect(terminalLine(view, "dev-gone", ROOT_FIX)).toBeNull();
		const zram = hostGroups(view, hostReportOf(view, DEVBOX)!)[1]!.items[1]!;
		expect(zram.fix).toEqual({ kind: "terminal", line: `ssh -t dev@devbox '${ROOT_FIX}'` });
	});

	it("shows each root command once, with the items it fixes", async () => {
		const view = await load();
		const parts = rootParts(hostGroups(view, hostReportOf(view, DEVBOX)!));
		expect(parts).toEqual([
			{
				line: `ssh -t dev@devbox '${ROOT_FIX}'`,
				item: "caps.zram",
				labels: ["Compressed swap"],
			},
		]);
		expect(rootParts(hostGroups(view, hostReportOf(view, MAC)!))).toEqual([]);
	});

	it("knows which devices check their host", async () => {
		const view = await load();
		expect(offersHostSpec(view, DEVBOX)).toBe(true);
		expect(offersHostSpec(view, MAC)).toBe(true);
		expect(offersHostSpec(view, "dev-gone")).toBe(false);
	});
});
