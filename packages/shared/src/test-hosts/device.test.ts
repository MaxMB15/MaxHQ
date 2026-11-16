// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { TestLease } from "../bindings";
import { BUNDLE } from "../data/mock-started";
import { mockTestHosts } from "../data/mock-test-hosts";
import { MAC } from "../data/mock-view";
import { agent, device, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	commandOutcome,
	leaseHeld,
	leaseSub,
	leaseTitle,
	localNote,
	minuteChoices,
	testHostsChange,
} from "./device";

const sample = mockTestHosts({ now: () => NOW }).entities();
const [lease] = sample.filter((e) => e.entity === "test_lease") as unknown as TestLease[];

const view = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			agents: [agent({ id: BUNDLE, title: "Sign the bundle" })],
			devices: [device({ id: MAC, name: "Mac", os: "macos" })],
		}),
		...sample,
	],
});

describe("a device's test host limits", () => {
	it("offers each choice in order, with the one kept even when it isn't one", () => {
		expect(minuteChoices([15, 30, 60], 30).map((c) => c.value)).toEqual([15, 30, 60]);
		expect(minuteChoices([15, 30, 60], 45).map((c) => c.value)).toEqual([15, 30, 45, 60]);
		expect(minuteChoices([15], 15)[0]!.label).toBe("15 min");
	});

	it("sends the change with the rest of the settings as the device has them", () => {
		const sent = testHostsChange(view, MAC, (s) => ({ ...s, idle_minutes: 20 }));
		expect(sent.device).toBe(MAC);
		expect(sent.test_hosts?.idle_minutes).toBe(20);
		expect(sent.test_hosts?.max_vms).toBe(1);
		expect(sent.keep_awake).toBe("charger");
	});
});

describe("a lease's lines", () => {
	it("names it by its kind and holder, with where it stands and its host", () => {
		expect(leaseTitle(view, lease!)).toBe("iOS Simulator for Sign the bundle");
		expect(leaseSub(view, lease!, NOW)).toBe("Ready, 8 min left, Mac VM");
		expect(leaseHeld(view, lease!, NOW)).toBe(
			"Held by Sign the bundle on Mac, Mac VM. Ready, 8 min left."
		);
	});

	it("says how a command ended", () => {
		expect(commandOutcome({ at: NOW, command: ["true"], exit_code: 0 })).toBe("Exit 0");
		expect(commandOutcome({ at: NOW, command: ["sleep"], exit_code: null })).toBe(
			"Didn't finish"
		);
	});

	it("says only agents on the device lease its machines", () => {
		expect(localNote("devbox")).toMatch(/^Only agents on devbox lease/);
	});
});
