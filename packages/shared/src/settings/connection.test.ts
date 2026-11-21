// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { device, entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import { connectionFacts } from "./connection";

const view = () =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [
				device({
					id: "dev-devbox",
					name: "devbox",
					maxd_version: "0.1.5",
					build: "8d6fac90a1b2c3d4",
					target: "x86_64-unknown-linux-musl",
				}),
				device({ id: "dev-mac", name: "Mac", os: "macos" }),
			],
			links: [
				link({
					id: "devbox",
					route: { type: "ssh", host: "owner@devbox" },
					via: ["jump.example.ts.net"],
					protocol: 54,
				}),
				link({ id: "mac", device: "dev-mac", route: { type: "local" } }),
			],
		}),
	});

describe("a device's connection facts", () => {
	it("says how it's reached, through what, and what it runs", () => {
		expect(connectionFacts(view(), "dev-devbox")).toEqual([
			{ label: "Reached by", value: "ssh owner@devbox" },
			{ label: "Through", value: "jump.example.ts.net" },
			{ label: "maxd", value: "0.1.5" },
			{ label: "Protocol", value: "54" },
			{ label: "Build ID", value: "8d6fac90a1b2c3d4" },
			{ label: "System", value: "Linux" },
			{ label: "Built for", value: "x86_64-unknown-linux-musl" },
			{ label: "Device ID", value: "dev-devbox" },
		]);
	});

	it("leaves out what the device hasn't said", () => {
		expect(connectionFacts(view(), "dev-mac")).toEqual([
			{ label: "Reached by", value: "This Mac" },
			{ label: "maxd", value: "0.1.0" },
			{ label: "System", value: "macOS" },
			{ label: "Device ID", value: "dev-mac" },
		]);
	});
});
