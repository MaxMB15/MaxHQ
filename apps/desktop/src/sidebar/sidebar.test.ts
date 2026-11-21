// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "@/data/mock-source";
import { DEVBOX_LINK } from "@maxhq/shared/data/mock-view";
import type { LinkState } from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { applySnapshot, type ViewState } from "@maxhq/shared/view/replica";
import { type DeviceNode, deviceGroups } from "@maxhq/shared/view/select";

import { deviceRowStatus, shownDevice } from "./devices";

const load = async (scenario?: string): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

const nodes = (view: ViewState): DeviceNode[] => deviceGroups(view, NOW).flatMap((g) => g.devices);

const devbox = async (scenario?: string): Promise<DeviceNode> =>
	nodes(await load(scenario)).find((n) => n.link.id === DEVBOX_LINK)!;

const withState = (node: DeviceNode, state: LinkState): DeviceNode => ({
	...node,
	link: { ...node.link, state },
});

describe("device rows", () => {
	it("count agents when connected and say what's wrong otherwise", async () => {
		expect(deviceRowStatus(await devbox())).toEqual({ text: "11 agents", tone: "plain" });
		expect(deviceRowStatus(await devbox("rebooting"))).toEqual({
			text: "Restarting",
			tone: "connecting",
		});
		expect(deviceRowStatus(await devbox("approval"))).toEqual({
			text: "Waiting for approval",
			tone: "connecting",
		});
		expect(deviceRowStatus(await devbox("peer_offline"))).toEqual({
			text: "Offline",
			tone: "error",
		});
		expect(deviceRowStatus(await devbox("tailscale_off"))).toEqual({
			text: "Not reachable",
			tone: "error",
		});
		expect(deviceRowStatus(await devbox("unchecked"))).toEqual({
			text: "Not checked yet",
			tone: "plain",
		});
	});

	it("name the step a connection attempt is on", async () => {
		const node = await devbox();
		const connecting = (step: "dialing" | "installing_maxd" | "handshake") =>
			deviceRowStatus(withState(node, { state: "connecting", step, attempt: 1 })).text;
		expect(connecting("installing_maxd")).toBe("Installing maxd");
		expect(connecting("dialing")).toBe("Connecting");
		expect(connecting("handshake")).toBe("Connecting");
	});

	it("say when a link waits for your answer about its device, or stays cut off", async () => {
		const node = await devbox();
		const held = (state: "asking" | "different") =>
			deviceRowStatus({
				...node,
				link: {
					...node.link,
					device: null,
					claim: { device: "dev-devbox", known_by: "link-mac", state, since: NOW },
				},
			});
		expect(held("asking")).toEqual({ text: "Needs your answer", tone: "connecting" });
		expect(held("different")).toEqual({ text: "Cut off", tone: "error" });
	});

	it("show the picked device, else the first with a problem, else the first", async () => {
		const live = nodes(await load());
		expect(shownDevice(live, null)?.link.name).toBe("Mac");
		expect(shownDevice(live, DEVBOX_LINK)?.link.name).toBe("devbox");

		const offline = nodes(await load("peer_offline"));
		expect(shownDevice(offline, null)?.link.name).toBe("devbox");
		expect(shownDevice([], null)).toBeUndefined();
	});
});
