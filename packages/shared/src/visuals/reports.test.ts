// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { MOST_HOSTS } from "./frame";
import { addReport, loadableHosts, NO_REPORTS, type VisualReport, withHosts } from "./reports";

const all = (...reports: VisualReport[]) => reports.reduce(addReport, NO_REPORTS);

describe("a visual's reports", () => {
	it("counts every error and keeps each message once", () => {
		const reports = all(
			{ type: "error", error: { message: "x is not defined", line: 3 } },
			{ type: "error", error: { message: "x is not defined", line: 3 } },
			{ type: "error", error: { message: "boom", line: null } }
		);
		expect(reports.errorCount).toBe(3);
		expect(reports.errors).toEqual([
			{ message: "x is not defined", line: 3 },
			{ message: "boom", line: null },
		]);
	});

	it("keeps only the first twenty errors", () => {
		const reports = all(
			...Array.from({ length: 25 }, (_, i): VisualReport => ({
				type: "error",
				error: { message: `e${i}`, line: null },
			}))
		);
		expect(reports.errors).toHaveLength(20);
		expect(reports.errorCount).toBe(25);
	});

	it("keeps each blocked host once, loadable when any load from it would be", () => {
		const reports = all(
			{ type: "blocked", url: "https://cdn.example.com/a.js", what: "connect-src" },
			{ type: "blocked", url: "https://cdn.example.com/b.js", what: "script-src-elem" },
			{ type: "blocked", url: "https://cdn.example.com/c", what: "connect-src" },
			{ type: "blocked", url: "inline", what: "script-src" }
		);
		expect([...reports.blocked.values()]).toEqual([
			{ host: "cdn.example.com", loadable: true },
		]);
	});

	it("keeps each missing file once", () => {
		const reports = all(
			{ type: "missing", path: "data.json" },
			{ type: "missing", path: "data.json" }
		);
		expect(reports.missing).toEqual(["data.json"]);
	});
});

describe("Load this time", () => {
	const blocked = all(
		{ type: "blocked", url: "https://a.example.com/x.js", what: "script-src-elem" },
		{ type: "blocked", url: "http://b.example.com/x.js", what: "script-src-elem" },
		{ type: "blocked", url: "https://c.example.com/x.css", what: "style-src-elem" },
		{ type: "blocked", url: "https://d.example.com/x", what: "connect-src" }
	).blocked;

	it("offers only the https hosts it can help, and not those through already", () => {
		expect(loadableHosts(blocked, [])).toEqual(["a.example.com", "c.example.com"]);
		expect(loadableHosts(blocked, ["a.example.com"])).toEqual(["c.example.com"]);
	});

	it("offers none past the most a visual takes", () => {
		const full = ["w.example.com", "x.example.com", "y.example.com", "z.example.com"];
		expect(full).toHaveLength(MOST_HOSTS);
		expect(loadableHosts(blocked, full)).toEqual([]);
		expect(loadableHosts(blocked, full.slice(1))).toEqual(["a.example.com"]);
	});

	it("adds hosts once, up to the most", () => {
		expect(withHosts(["a.example.com"], ["a.example.com", "b.example.com"])).toEqual([
			"a.example.com",
			"b.example.com",
		]);
		expect(
			withHosts(
				["a.example.com", "b.example.com", "c.example.com"],
				["d.example.com", "e.example.com"]
			)
		).toHaveLength(MOST_HOSTS);
	});
});
