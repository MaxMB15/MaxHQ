// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent, AgentId } from "../bindings";
import type { ActionResult } from "../data/source";
import { agent, entities } from "../test/build";
import { applySnapshot } from "../view/replica";
import { createViewStore } from "../view/store";

import { createRenames, givenName, renameAgent, shownTitle } from "./rename-agent";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

// A source whose answers wait until the test gives them.
const setup = () => {
	const calls: [AgentId, string][] = [];
	const answers: ((result: ActionResult | Error) => void)[] = [];
	const source = {
		renameAgent: (id: AgentId, title: string) => {
			calls.push([id, title]);
			return new Promise<ActionResult>((resolve, reject) =>
				answers.push((result) =>
					result instanceof Error ? reject(result) : resolve(result)
				)
			);
		},
	};
	const view = createViewStore();
	const show = (title: string) =>
		view.setState({
			view: applySnapshot({
				seq: 1,
				entities: entities({ agents: [agent({ id: "a", title })] }),
			}),
		});
	show("Fix the flaky test");
	const renames = createRenames(source, view);
	const current = (): Agent => view.getState().view.tables.agent.get("a")!;
	const title = () => shownTitle(renames.store.getState(), current());
	const answer = (result: ActionResult | Error) => answers.shift()!(result);
	return { calls, renames, show, current, title, answer };
};

describe("renaming an agent", () => {
	it("trims a draft and cuts it as maxd does", () => {
		expect(givenName("  Flaky test \n")).toBe("Flaky test");
		expect(givenName("é".repeat(90))).toBe("é".repeat(80));
		expect(givenName(" \t ")).toBe("");
	});

	it("shows the name at once and keeps it until the view has it, in either order", async () => {
		const { calls, renames, show, current, title, answer } = setup();
		const done = renameAgent(renames, current(), "  Flaky test ");
		expect(calls).toEqual([["a", "Flaky test"]]);
		expect(title()).toBe("Flaky test");

		answer(DONE);
		expect(await done).toBeNull();
		expect(title()).toBe("Flaky test");
		show("Flaky test");
		expect(renames.store.getState().pending.size).toBe(0);

		// The view's change first, then the answer.
		const again = renameAgent(renames, current(), "Second");
		show("Second");
		answer(DONE);
		expect(await again).toBeNull();
		expect(renames.store.getState().pending.size).toBe(0);
		expect(title()).toBe("Second");
	});

	it("puts the old title back and says why when maxd refuses or the call fails", async () => {
		const { renames, current, title, answer } = setup();
		const refused = renameAgent(renames, current(), "Flaky test");
		answer({ status: "ok", data: { status: "refused", reason: "couldn't store the name" } });
		expect(await refused).toBe("Couldn't rename Fix the flaky test. Couldn't store the name.");
		expect(title()).toBe("Fix the flaky test");

		const failed = renameAgent(renames, current(), "Flaky test");
		answer(new Error("devbox isn't connected"));
		expect(await failed).toBe("Couldn't rename Fix the flaky test. Devbox isn't connected.");
		expect(title()).toBe("Fix the flaky test");
	});

	it("sends a blank draft to clear the name, and nothing when the title stays", async () => {
		const { calls, renames, current, title, answer } = setup();
		expect(await renameAgent(renames, current(), " Fix the flaky test ")).toBeNull();
		expect(calls).toEqual([]);

		const cleared = renameAgent(renames, current(), "  ");
		expect(calls).toEqual([["a", ""]]);
		expect(title()).toBe("Fix the flaky test");
		answer(DONE);
		expect(await cleared).toBeNull();
	});

	it("keeps a later rename's name when an earlier one fails", async () => {
		const { renames, current, title, answer } = setup();
		const first = renameAgent(renames, current(), "First");
		const second = renameAgent(renames, current(), "Second");
		answer({ status: "ok", data: { status: "unknown_agent" } });
		await first;
		expect(title()).toBe("Second");
		answer(DONE);
		await second;
	});
});
