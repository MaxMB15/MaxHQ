// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import { keepsInboxes, parseSettleDays, settleDaysText } from "./inbox";

const caps = { transfers: true, tunnels: true, workspaces: true };

describe("auto-settle days", () => {
	it("reads 0 to 90 days", () => {
		expect(parseSettleDays(" 5 ")).toEqual({ ok: true, days: 5 });
		expect(parseSettleDays("0")).toEqual({ ok: true, days: 0 });
		expect(parseSettleDays("90")).toEqual({ ok: true, days: 90 });
	});

	it("says why other text isn't days", () => {
		expect(parseSettleDays("91")).toEqual({
			ok: false,
			reason: "91 isn't a number of days. Use 0 to 90.",
		});
		expect(parseSettleDays("2.5")).toEqual({
			ok: false,
			reason: "2.5 isn't a number of days. Use 0 to 90.",
		});
		expect(parseSettleDays("")).toEqual({
			ok: false,
			reason: "Nothing isn't a number of days. Use 0 to 90.",
		});
	});

	it("words 0 as never", () => {
		expect(settleDaysText(0)).toBe("Never");
		expect(settleDaysText(1)).toBe("1 day");
		expect(settleDaysText(5)).toBe("5 days");
	});
});

describe("a device that keeps inboxes", () => {
	it("is one whose maxd said so", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				links: [
					link({ id: "new", device: "dev-new", capabilities: { ...caps, inbox: true } }),
					link({ id: "old", device: "dev-old", capabilities: caps }),
					link({ id: "unheard", device: "dev-unheard", capabilities: null }),
				],
			}),
		});
		expect(keepsInboxes(view, "dev-new")).toBe(true);
		expect(keepsInboxes(view, "dev-old")).toBe(false);
		expect(keepsInboxes(view, "dev-unheard")).toBe(false);
		expect(keepsInboxes(view, "dev-gone")).toBe(false);
	});
});
