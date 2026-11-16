// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it } from "vitest";

import { type Entity, FINISHED_SUB_AGENTS_SINCE, type WorkflowRun } from "../bindings";
import { agent, entities, link, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	finishedBanner,
	finishedLead,
	finishedNote,
	finishedTitle,
	keepCardSubAgent,
	keepSubAgent,
	nameSubAgent,
	subAgentLead,
} from "./finished";
import { clearKept } from "./kept";

const lead = agent({ id: "lead", workspace: "w-mac", control: "held" });
const mac = link({ id: "mac", device: "dev-mac", protocol: FINISHED_SUB_AGENTS_SINCE });

/** The sub-agents run of `lead`, with `agents` bound to its nodes. */
const run = (agents: string[], workflow = "maxhq.agents"): Entity => {
	const found: WorkflowRun = {
		id: `wfr-${workflow}`,
		key: { device: "dev-mac", project: "p1", workflow, run: "lead" },
		label: "Sub-agents",
		progress: { done: 1, total: 2 },
		current: [],
		needs_you: false,
		orchestrators: ["lead"],
		last_activity: NOW,
		done: false,
		agents,
	};
	return { entity: "workflow_run", ...found };
};

const viewOf = (
	over: { leads?: (typeof lead)[]; extra?: Entity[]; protocol?: number | null } = {}
) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				agents: over.leads ?? [lead],
				links: [
					{
						...mac,
						protocol: over.protocol === undefined ? mac.protocol : over.protocol,
					},
				],
				workspaces: [workspace({ id: "w-mac" })],
			}),
			...(over.extra ?? []),
		],
	});

beforeEach(clearKept);

describe("finished sub-agents", () => {
	it("opens one the window saw under its lead, with the title it had", () => {
		const view = viewOf();
		expect(finishedLead(view, "sub")).toBeNull();
		expect(finishedNote(view, "sub")).toBeNull();

		keepSubAgent("sub", "lead", "Map the session files");
		expect(finishedLead(view, "sub")).toBe(view.tables.agent.get("lead"));
		expect(finishedNote(view, "sub")).toBeNull();
		expect(finishedTitle("sub")).toBe("Map the session files");

		// A later keep without a title keeps the one seen.
		keepSubAgent("sub", "lead", null);
		expect(finishedTitle("sub")).toBe("Map the session files");
	});

	it("opens one a window never saw through the sub-agents run it's bound to", () => {
		const view = viewOf({ extra: [run(["sub"]), run(["other"], "pipeline")] });
		expect(finishedLead(view, "sub")?.id).toBe("lead");
		// Other workflows bind agents of their own, which aren't sub-agents.
		expect(finishedLead(view, "other")).toBeNull();
		expect(finishedNote(view, "other")).toBeNull();

		expect(finishedTitle("sub")).toBeNull();
		nameSubAgent(view, "sub", "Flags in the docs");
		nameSubAgent(view, "other", "Not a sub-agent");
		expect(finishedTitle("sub")).toBe("Flags in the docs");
		expect(finishedTitle("other")).toBeNull();
	});

	it("opens one a card in a feed under the lead opened", () => {
		const mid = agent({ id: "mid", workspace: "w-mac", parent: "lead", control: "held" });
		const view = viewOf({ leads: [lead, mid] });
		keepCardSubAgent(view, "mid", "sub", "Read the docs");
		expect(finishedLead(view, "sub")).toBe(view.tables.agent.get("lead"));
		expect(finishedTitle("sub")).toBe("Read the docs");

		// A card in a feed the view no longer lists keeps nothing.
		keepCardSubAgent(view, "gone", "other", "Other");
		expect(finishedLead(view, "other")).toBeNull();
	});

	it("leaves a listed agent to its own view", () => {
		const sub = agent({ id: "sub", workspace: "w-mac", parent: "lead", control: "held" });
		const view = viewOf({ leads: [lead, sub], extra: [run(["sub"])] });
		keepSubAgent("sub", "lead", "sub");
		expect(finishedLead(view, "sub")).toBeNull();
		expect(finishedNote(view, "sub")).toBeNull();
		expect(subAgentLead(view.tables.agent, sub)).toBe("lead");
		expect(subAgentLead(view.tables.agent, lead)).toBeNull();
	});

	it("names the lead at the top of a nested sub-agent's tree", () => {
		const mid = agent({ id: "mid", workspace: "w-mac", parent: "lead" });
		const nested = agent({ id: "nested", workspace: "w-mac", parent: "mid" });
		const agents = new Map([lead, mid, nested].map((a) => [a.id, a]));
		expect(subAgentLead(agents, nested)).toBe("lead");
	});

	it("says plainly that it finished once its lead is gone too", () => {
		keepSubAgent("sub", "lead", "sub");
		const view = viewOf({ leads: [] });
		expect(finishedLead(view, "sub")).toBeNull();
		expect(finishedNote(view, "sub")).toBe(
			"This sub-agent finished, and its lead is no longer here."
		);
	});

	it("shows the history only where maxd can read it", () => {
		keepSubAgent("sub", "lead", "sub");
		const codex = viewOf({ leads: [{ ...lead, vendor: "codex" }] });
		expect(finishedLead(codex, "sub")).toBeNull();
		expect(finishedNote(codex, "sub")).toBe(
			"This sub-agent finished. MaxHQ shows the history of finished sub-agents only for Claude."
		);

		const old = viewOf({ protocol: FINISHED_SUB_AGENTS_SINCE - 1 });
		expect(finishedLead(old, "sub")).toBeNull();
		expect(finishedNote(old, "sub")).toBe(
			"This sub-agent finished. Showing its history needs a newer maxd on mac."
		);
		// A link that hasn't said its version yet gets to try.
		expect(finishedLead(viewOf({ protocol: null }), "sub")?.id).toBe("lead");

		const contained = applySnapshot({
			seq: 1,
			entities: entities({
				agents: [lead],
				links: [mac],
				workspaces: [
					workspace({
						id: "w-mac",
						location: { type: "container", instance: "box", path: "/work" },
					}),
				],
			}),
		});
		expect(finishedNote(contained, "sub")).toBe(
			"This sub-agent finished. MaxHQ can't show the history of finished sub-agents in a container yet."
		);
	});

	it("says in place of the composer that it takes no messages", () => {
		expect(finishedBanner().title).toBe("This sub-agent finished.");
	});
});
