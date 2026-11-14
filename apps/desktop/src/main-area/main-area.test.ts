// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "@/data/mock-source";
import { MAC, DEVBOX_LINK } from "@maxhq/shared/data/mock-view";
import { NOW } from "@maxhq/shared/test/build";
import { applySnapshot, type ViewState } from "@maxhq/shared/view/replica";
import { type DeviceNode, deviceGroups } from "@maxhq/shared/view/select";

import { deviceChips, splitDetail } from "./device-detail";

const load = async (scenario?: string): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

const nodes = (view: ViewState): DeviceNode[] => deviceGroups(view, NOW).flatMap((g) => g.devices);

describe("device detail", () => {
	it("lists how each device is reached and what it runs", async () => {
		const live = nodes(await load());
		const mac = live.find((n) => n.link.device === MAC)!;
		const nl = live.find((n) => n.link.id === DEVBOX_LINK)!;
		expect(deviceChips(mac)).toEqual(["This Mac", "macOS", "maxd 0.1.0"]);
		expect(deviceChips(nl)).toEqual([
			"Needs Tailscale",
			"SSH as dev@devbox",
			"Linux",
			"maxd 0.1.0",
		]);

		const offline = nodes(await load("peer_offline")).find((n) => n.link.id === DEVBOX_LINK)!;
		expect(deviceChips(offline).at(-1)).toBe("maxd 0.1.0 when last seen");
	});

	it("splits a failed check's finding from its reason", () => {
		expect(splitDetail("No answer. devbox is offline on the tailnet")).toEqual([
			"No answer",
			"devbox is offline on the tailnet",
		]);
		expect(splitDetail("No answer")).toEqual(["No answer", undefined]);
	});
});
