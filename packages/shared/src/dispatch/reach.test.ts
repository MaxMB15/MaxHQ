// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Account, DeviceLink, Entity } from "../bindings";
import { account, device, entities, link, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { dispatcherAccountLabel, dispatcherTitle } from "./plan";
import { dispatcherDevice, dispatcherReach } from "./reach";

const dispatcher = (on: string): Entity => ({
	entity: "dispatcher",
	device: on,
	workspace: `ws-dispatcher-${on}`,
	agent: null,
	plan: null,
});

const viewOf = (parts: { links: DeviceLink[]; dispatchers?: string[]; accounts?: Account[] }) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "dev-mac", name: "studio", os: "macos" }),
					device({ id: "dev-air", name: "air", os: "macos" }),
					device({ id: "dev-box" }),
				],
				links: parts.links,
				accounts: parts.accounts ?? [
					account({ id: "codex", device: "dev-mac", vendor: "codex", label: "Codex" }),
					account({ id: "claude", device: "dev-mac", label: "Personal" }),
				],
			}),
			...(parts.dispatchers ?? ["dev-mac"]).map(dispatcher),
		],
	});

const box = link({ id: "box", device: "dev-box" });
const studio = link({ id: "studio", name: "Studio", device: "dev-mac" });
const away: DeviceLink["state"] = { state: "unreachable", reason: "timed out", last_seen: NOW };

describe("the dispatcher's Mac", () => {
	it("is the Mac the client runs on", () => {
		const here = link({ id: "here", device: "dev-mac", route: { type: "local" } });
		const view = viewOf({ links: [box, here] });
		expect(dispatcherDevice(view)).toBe("dev-mac");
		expect(dispatcherReach(view, null)).toMatchObject({ device: "dev-mac", off: null });
	});

	it("is, on a phone, the paired Mac with a dispatcher, a connected one first", () => {
		const air = link({ id: "air", device: "dev-air", state: away });
		expect(
			dispatcherDevice(
				viewOf({ links: [box, air, studio], dispatchers: ["dev-air", "dev-mac"] })
			)
		).toBe("dev-mac");
		expect(dispatcherDevice(viewOf({ links: [box, air], dispatchers: ["dev-air"] }))).toBe(
			"dev-air"
		);
	});

	it("falls back to a paired Mac whose MaxHQ has no dispatcher yet", () => {
		const view = viewOf({ links: [box, studio], dispatchers: [] });
		expect(dispatcherReach(view, null)).toMatchObject({
			device: "dev-mac",
			dispatcher: undefined,
			off: "Update MaxHQ on Studio to use the dispatcher",
		});
	});

	it("is none without a Mac", () => {
		const view = viewOf({ links: [box], dispatchers: [] });
		expect(dispatcherReach(view, null)).toMatchObject({
			device: null,
			off: "Pair a Mac to use the dispatcher",
		});
	});
});

describe("whether the dispatcher takes a request", () => {
	it("does while its Mac is connected and has an account", () => {
		const reach = dispatcherReach(viewOf({ links: [studio] }), null);
		expect(reach.off).toBeNull();
		expect(reach.dispatcher?.device).toBe("dev-mac");
	});

	it("doesn't while its Mac is out of reach, though the view keeps its dispatcher", () => {
		const view = viewOf({ links: [{ ...studio, state: away }] });
		expect(dispatcherReach(view, null)).toMatchObject({
			dispatcher: { device: "dev-mac" },
			off: "Can't reach Studio",
		});
	});

	it("doesn't without a Claude or Codex account on the Mac", () => {
		const cursor = account({ id: "cursor", device: "dev-mac", vendor: "cursor" });
		expect(dispatcherReach(viewOf({ links: [studio], accounts: [cursor] }), null).off).toBe(
			"Sign in to Claude or Codex on Studio"
		);
		const here = link({ id: "here", device: "dev-mac", route: { type: "local" } });
		expect(dispatcherReach(viewOf({ links: [here], accounts: [cursor] }), null).off).toBe(
			"Sign in to Claude or Codex on this Mac"
		);
	});
});

describe("the dispatcher's account", () => {
	it("is the first Claude account, else the first Codex one, unless you picked", () => {
		const view = viewOf({ links: [studio] });
		expect(dispatcherReach(view, null).account?.id).toBe("claude");
		expect(dispatcherReach(view, "codex").account?.id).toBe("codex");
		const codexOnly = viewOf({
			links: [studio],
			accounts: [account({ id: "codex", device: "dev-mac", vendor: "codex" })],
		});
		expect(dispatcherReach(codexOnly, "gone").account?.id).toBe("codex");
	});

	it("names an account as Codex, never ChatGPT", () => {
		const codex = account({
			id: "codex",
			vendor: "codex",
			plan: "plus",
			email: "octocat@example.com",
		});
		expect(dispatcherAccountLabel(codex)).toBe("Codex Plus, octocat@example.com");
		expect(dispatcherTitle(codex)).toBe("Dispatcher, as Codex Plus");
		expect(dispatcherTitle(undefined)).toBe("Dispatcher");
	});
});
