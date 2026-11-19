// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { FOLDERS_SINCE } from "../bindings";
import { device, entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import { browseWay, supportsFolders } from "./select";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		links: [
			link({ id: "mac", route: { type: "local" }, protocol: FOLDERS_SINCE }),
			link({ id: "old-mac", route: { type: "local" }, protocol: FOLDERS_SINCE - 1 }),
			link({ id: "box", name: "devbox", protocol: FOLDERS_SINCE }),
			link({ id: "old", name: "oldbox", protocol: FOLDERS_SINCE - 1 }),
			link({ id: "unknown", protocol: null }),
		],
		devices: [device({ id: "dev-gone", name: "gone" })],
	}),
});

describe("supportsFolders", () => {
	it("needs a link to a maxd that lists folders", () => {
		expect(supportsFolders(view, "dev-box")).toBe(true);
		expect(supportsFolders(view, "dev-old")).toBe(false);
		expect(supportsFolders(view, "dev-unknown")).toBe(false);
		expect(supportsFolders(view, "dev-gone")).toBe(false);
	});
});

describe("browseWay", () => {
	it("uses the system picker for this Mac in the app, even with an older maxd", () => {
		expect(browseWay(view, "dev-mac", true)).toEqual({ way: "native" });
		expect(browseWay(view, "dev-old-mac", true)).toEqual({ way: "native" });
	});

	it("uses MaxHQ's browser elsewhere, and for this Mac outside the app", () => {
		expect(browseWay(view, "dev-box", true)).toEqual({ way: "browser" });
		expect(browseWay(view, "dev-mac", false)).toEqual({ way: "browser" });
	});

	it("says to update an older maxd", () => {
		expect(browseWay(view, "dev-old", true)).toEqual({
			way: "none",
			why: "Update maxd on oldbox to browse its folders",
		});
		expect(browseWay(view, "dev-gone", true)).toEqual({
			way: "none",
			why: "Update maxd on gone to browse its folders",
		});
	});
});
