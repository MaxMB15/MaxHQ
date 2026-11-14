// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { DeviceLink } from "@maxhq/shared/bindings";
import { SYNC_GRACE_MS } from "@maxhq/shared/status/reach";
import { entities, link, MIN, NOW } from "@maxhq/shared/test/build";
import type { FeedRow } from "@maxhq/shared/timeline/output";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { feedScroll, missingAgent } from "./opening";

const viewOf = (links: DeviceLink[]) => applySnapshot({ seq: 1, entities: entities({ links }) });

const ready = { state: "ready" } as const;

describe("missingAgent", () => {
	it("says Loading on a cold start, before the view or the device has answered", () => {
		expect(missingAgent({ state: "loading" }, viewOf([]), NOW)).toBe("loading");
		const dialing = link({
			id: "mac",
			state: { state: "connecting", step: "dialing", attempt: 1 },
		});
		expect(missingAgent(ready, viewOf([dialing]), NOW)).toBe("loading");
		const fresh = link({ id: "mac", state: { state: "connected", since: NOW } });
		expect(missingAgent(ready, viewOf([fresh]), NOW + 1_000)).toBe("loading");
		expect(missingAgent(ready, viewOf([fresh]), NOW + SYNC_GRACE_MS)).toBe("gone");
	});

	it("says gone once every device has answered or is out of reach", () => {
		const down = link({
			id: "nl",
			state: { state: "unreachable", reason: "timed out", last_seen: NOW - MIN },
		});
		expect(missingAgent(ready, viewOf([link({ id: "mac" }), down]), NOW)).toBe("gone");
	});
});

describe("feedScroll", () => {
	const card = (id: string, attention: string): FeedRow => ({
		id,
		kind: "question",
		gap: 10,
		attention,
	});
	const line: FeedRow = { id: "u", kind: "user_message", gap: 0, attention: null };

	it("goes to the prompt's card", () => {
		expect(feedScroll([line, card("p", "e1")], "e1", false)).toBe(1);
	});

	it("goes to the end once the loaded feed has no card for it", () => {
		expect(feedScroll([line], "e1", false)).toBeNull();
		expect(feedScroll([line], "e1", true)).toBe("end");
	});

	it("stays put when opened for no prompt", () => {
		expect(feedScroll([line, card("p", "e1")], undefined, true)).toBeNull();
	});
});
