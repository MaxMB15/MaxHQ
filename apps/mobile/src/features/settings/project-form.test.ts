// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { projectRows } from "./project-form";

const ALL = { settle: true, pulls: true, workflows: true, memory: true };
const NONE = { settle: false, pulls: false, workflows: false, memory: false };

describe("a device's card on a project's settings", () => {
	it("takes changes while the device is there", () => {
		const rows = projectRows({
			name: "devbox",
			live: true,
			capabilities: ALL,
			deviceDays: 5,
			allowed: null,
		});
		expect(rows.state).toBe("Kept on the device");
		expect(rows.settle).toEqual({
			note: "Days with no activity before an agent in this project settles. Leave it empty to use devbox's, which is 5 days. 0 never settles them.",
			editable: true,
		});
		expect([rows.pulls, rows.workflows, rows.memory].every((r) => r.editable)).toBe(true);
	});

	it("takes none while the device is away", () => {
		const rows = projectRows({
			name: "devbox",
			live: false,
			capabilities: ALL,
			deviceDays: 0,
			allowed: null,
		});
		expect(rows.state).toBe("Connect to devbox to change it");
		expect(rows.settle.note).toContain("which is never.");
		expect([rows.settle, rows.pulls, rows.workflows, rows.memory].some((r) => r.editable)).toBe(
			false
		);
	});

	it("says when the device's maxd is too old for a setting", () => {
		const rows = projectRows({
			name: "devbox",
			live: true,
			capabilities: NONE,
			deviceDays: 5,
			allowed: null,
		});
		for (const row of [rows.settle, rows.pulls, rows.workflows, rows.memory]) {
			expect(row).toEqual({
				note: "devbox's maxd is too old for this. Update it first.",
				editable: false,
			});
		}
	});

	it("names the workflow variables you allowed", () => {
		const rows = projectRows({
			name: "devbox",
			live: true,
			capabilities: ALL,
			deviceDays: 5,
			allowed: "You allowed EXAMPLE_MODE.",
		});
		expect(rows.workflows.note).toMatch(/own name\. You allowed EXAMPLE_MODE\.$/);
	});
});
