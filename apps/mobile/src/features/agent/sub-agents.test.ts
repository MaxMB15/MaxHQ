// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent } from "@maxhq/shared/bindings";
import { agent, entities, NOW } from "@maxhq/shared/test/build";
import { applySnapshot, type ViewState } from "@maxhq/shared/view/replica";

import { hasSubAgents, subAgentCount, subAgentRows } from "./sub-agents";

const viewOf = (agents: Agent[]): ViewState =>
	applySnapshot({ seq: 1, entities: entities({ agents }) });

const lead = agent({
	id: "lead",
	title: "Port the parser",
	activity: { state: "busy_in_subagents" },
});

describe("sub-agents", () => {
	it("counts an agent's sub-agents for the More sheet", () => {
		const view = viewOf([
			lead,
			agent({ id: "s1", parent: lead.id }),
			agent({ id: "s2", parent: lead.id }),
			agent({ id: "s3", parent: "s1" }),
		]);
		expect(subAgentCount(view, lead.id)).toBe(2);
		expect(hasSubAgents(view, lead.id)).toBe(true);
		expect(subAgentCount(view, "s3")).toBe(0);
		expect(hasSubAgents(view, "s3")).toBe(false);
	});

	it("lists the lead first, then its sub-agents oldest first", () => {
		const view = viewOf([
			lead,
			agent({
				id: "s2",
				title: "Write the tests",
				parent: lead.id,
				started_at: NOW - 1000,
				activity: { state: "starting" },
			}),
			agent({
				id: "s1",
				title: "Read the grammar",
				parent: lead.id,
				started_at: NOW - 5000,
				activity: { state: "busy" },
			}),
			agent({ id: "s9", parent: "someone-else" }),
		]);
		expect(subAgentRows(view, lead, NOW)).toEqual([
			{
				id: "lead",
				title: "Port the parser",
				look: "busy_in_subagents",
				status: "Waiting",
				lead: true,
				quiet: false,
			},
			{
				id: "s1",
				title: "Read the grammar",
				look: "busy",
				status: "Working",
				lead: false,
				quiet: false,
			},
			{
				id: "s2",
				title: "Write the tests",
				look: "starting",
				status: "Starting",
				lead: false,
				quiet: true,
			},
		]);
	});
});
