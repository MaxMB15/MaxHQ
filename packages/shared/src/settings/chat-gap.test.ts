// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import { applySnapshot } from "../view/replica";

import { chatGap, chatGapOptions } from "./chat-gap";

const settings = (device: string, minutes?: number) =>
	({
		entity: "device_settings",
		device,
		default_permission: "auto",
		workspace_network: null,
		workspace_pool: null,
		...(minutes === undefined ? {} : { chat_gap_minutes: minutes }),
	}) as Entity;

describe("chat gap", () => {
	it("offers 15 minutes to 4 hours, then Never", () => {
		expect(chatGapOptions(30).map((o) => o.label)).toEqual([
			"15 minutes",
			"30 minutes",
			"1 hour",
			"2 hours",
			"4 hours",
			"Never",
		]);
		expect(chatGapOptions(30).map((o) => o.value)).toEqual([
			"15",
			"30",
			"60",
			"120",
			"240",
			"0",
		]);
	});

	it("reads the device's pause, or null from a maxd too old for it", () => {
		const view = applySnapshot({
			seq: 1,
			entities: [settings("dev-new", 0), settings("dev-old")],
		});
		expect(chatGap(view, "dev-new")).toBe(0);
		expect(chatGap(view, "dev-old")).toBeNull();
		expect(chatGap(view, "dev-gone")).toBeNull();
	});
});
