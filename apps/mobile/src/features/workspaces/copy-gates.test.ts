// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { device, entities } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { closedGates, closedText, copyGates, gatesOpen } from "./copy-gates";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [
			device({ id: "dev-mac", name: "Home Mac", os: "macos" }),
			device({ id: "dev-box", name: "devbox" }),
		],
	}),
});

describe("the devices a copy from the phone needs", () => {
	it("needs the device it goes to, and the one it comes from once files are ticked", () => {
		expect(copyGates("dev-box", "dev-mac", 2)).toEqual({
			target: "dev-box",
			source: "dev-mac",
		});
		expect(copyGates("dev-box", "dev-mac", 0)).toEqual({ target: "dev-box", source: null });
		expect(copyGates(null, null, 3)).toEqual({ target: null, source: null });
	});

	it("opens only once every device it needs lets the phone", () => {
		const both = copyGates("dev-box", "dev-mac", 1);
		expect(gatesOpen(both, { target: true, source: true })).toBe(true);
		expect(gatesOpen(both, { target: true, source: false })).toBe(false);
		expect(gatesOpen(both, { target: false, source: true })).toBe(false);
		const repoOnly = copyGates("dev-box", "dev-mac", 0);
		expect(gatesOpen(repoOnly, { target: true, source: false })).toBe(true);
		expect(gatesOpen(copyGates(null, null, 0), { target: false, source: false })).toBe(true);
	});

	it("names the devices that don't let it, each once", () => {
		const both = copyGates("dev-box", "dev-mac", 1);
		expect(closedGates(view, both, { target: false, source: false })).toEqual([
			"devbox",
			"Home Mac",
		]);
		expect(closedGates(view, both, { target: true, source: false })).toEqual(["Home Mac"]);
		expect(closedGates(view, both, { target: true, source: true })).toEqual([]);
		const same = copyGates("dev-box", "dev-box", 1);
		expect(closedGates(view, same, { target: false, source: false })).toEqual(["devbox"]);
		expect(closedText(["devbox", "Home Mac"])).toBe("devbox and Home Mac");
	});
});
