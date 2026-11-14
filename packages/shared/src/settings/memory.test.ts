// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { historyOptions, historyText, memoryItemsOptions, NO_LIMIT_WARNING } from "./memory";

describe("memory settings", () => {
	it("offers 64 to 4096 items", () => {
		expect(memoryItemsOptions(256).map((o) => o.label)).toEqual([
			"64 items",
			"128 items",
			"256 items",
			"512 items",
			"1,024 items",
			"2,048 items",
			"4,096 items",
		]);
	});

	it("offers 32 MB to 4 GB, then No limit with what it risks", () => {
		const options = historyOptions(256);
		expect(options.map((o) => o.label)).toEqual([
			"32 MB",
			"64 MB",
			"128 MB",
			"256 MB",
			"512 MB",
			"1 GB",
			"2 GB",
			"4 GB",
			"No limit",
		]);
		expect(options.at(-1)).toEqual({
			value: "0",
			label: "No limit",
			detail: NO_LIMIT_WARNING,
			warn: true,
		});
		expect(options.filter((o) => o.detail)).toHaveLength(1);
	});

	it("keeps a value set elsewhere among the picks", () => {
		const items = memoryItemsOptions(300);
		expect(items.findIndex((o) => o.value === "300")).toBe(3);
		const history = historyOptions(1536);
		expect(history.findIndex((o) => o.label === "1,536 MB")).toBe(6);
		expect(history.at(-1)?.value).toBe("0");
	});

	it("words sizes in MB or whole GB", () => {
		expect(historyText(0)).toBe("No limit");
		expect(historyText(32)).toBe("32 MB");
		expect(historyText(1024)).toBe("1 GB");
		expect(historyText(1536)).toBe("1,536 MB");
	});
});
