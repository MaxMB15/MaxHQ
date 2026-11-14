// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Agent } from "../bindings";
import type { ActionResult } from "../data/source";
import { agent, entities, MIN, NOW, project, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import { agentAfterDelete, deletableAgents, deleteAgents, deleteQuestion } from "./delete-agents";

const held = (over: Partial<Agent> & { id: string }) => agent({ control: "held", ...over });

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-a", "Alpha"), project("p-b", "Beta")],
		workspaces: [
			workspace({ id: "w-a", project: "p-a" }),
			workspace({ id: "w-a2", project: "p-a" }),
			workspace({ id: "w-b", project: "p-b" }),
		],
		agents: [
			held({ id: "a1", workspace: "w-a", last_activity_at: NOW - 3 * MIN }),
			held({ id: "a2", workspace: "w-a2", last_activity_at: NOW - MIN }),
			held({ id: "a3", workspace: "w-a", last_activity_at: NOW - 2 * MIN }),
			held({ id: "a-sub", workspace: "w-a", parent: "a1" }),
			agent({ id: "a-terminal", workspace: "w-a", last_activity_at: NOW - 9 * MIN }),
			held({ id: "b1", workspace: "w-b" }),
		],
	}),
});

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

describe("deleting agents", () => {
	it("takes only the leads MaxHQ holds", () => {
		const ids = ["a1", "a-sub", "a-terminal", "gone", "b1"];
		expect(deletableAgents(view, ids).map((a) => a.id)).toEqual(["a1", "b1"]);
	});

	it("names the agent when there's one, and counts them when there are more", () => {
		const [a1, b1] = deletableAgents(view, ["a1", "b1"]);
		expect(deleteQuestion([a1!]).title).toBe('Delete agent "a1"?');
		expect(deleteQuestion([a1!, b1!]).title).toBe("Delete 2 agents?");
	});

	it("deletes one after another, past a failure, and says what went wrong", async () => {
		const asked: string[] = [];
		const deleteAgent = vi.fn(async (id: string): Promise<ActionResult> => {
			asked.push(id);
			if (id === "a3") throw new Error("lost the connection");
			return id === "a2"
				? { status: "ok", data: { status: "refused", reason: "the device said no" } }
				: DONE;
		});
		const agents = deletableAgents(view, ["a1", "a2", "a3", "b1"]);
		const outcome = await deleteAgents({ deleteAgent }, agents);
		expect(asked).toEqual(["a1", "a2", "a3", "b1"]);
		expect(outcome.deleted).toEqual(new Set(["a1", "b1"]));
		expect(outcome.failures).toEqual([
			"Couldn't delete a2. The device said no.",
			"Couldn't delete a3. Lost the connection.",
		]);
	});

	it("opens the first lead left in the deleted agent's project", () => {
		// a2 was active last, then a3, then the one from a terminal.
		expect(agentAfterDelete(view, "a1", new Set(["a1"]))).toBe("a2");
		expect(agentAfterDelete(view, "a1", new Set(["a1", "a2"]))).toBe("a3");
		expect(agentAfterDelete(view, "b1", new Set(["b1"]))).toBeNull();
		expect(agentAfterDelete(view, "gone", new Set())).toBeNull();
	});
});
