// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Agent } from "../bindings";
import type { DataSource } from "../data/source";
import { agent, entities, NOW } from "../test/build";

import { DEFAULT_INBOX } from "./inbox";
import { applySnapshot } from "./replica";
import { lookAt } from "./seen";
import { createViewStore } from "./store";

const setup = (agents: Agent[], dismissed: "ok" | "fails" = "ok") => {
	const markSeen = vi.fn<DataSource["markSeen"]>(async () => ({ status: "ok", data: null }));
	const setInbox = vi.fn<DataSource["setInbox"]>(async () => [
		dismissed === "ok"
			? { status: "ok", data: { status: "done" } }
			: { status: "error", error: { kind: "failed", message: "no" } },
	]);
	const source = { markSeen, setInbox } as unknown as DataSource;
	const view = createViewStore();
	view.setState({ view: applySnapshot({ seq: 1, entities: entities({ agents }) }) });
	return { looking: { source, view }, markSeen, setInbox };
};

const woke = (id: string, over: Partial<Agent> = {}): Agent =>
	agent({ id, inbox: { ...DEFAULT_INBOX, woke_at: NOW }, ...over });

describe("lookAt", () => {
	it("marks a done, unread agent seen, once while the call is out", () => {
		const { looking, markSeen } = setup([
			agent({ id: "a", activity: { state: "done_unread" } }),
			agent({ id: "b" }),
		]);
		lookAt(looking, "a", false);
		lookAt(looking, "a", true);
		lookAt(looking, "b", false);
		expect(markSeen.mock.calls).toEqual([["a"]]);
	});

	it("clears the Woke marker on the lead a sub-agent shows beside", () => {
		const { looking, setInbox } = setup([woke("lead"), agent({ id: "sub", parent: "lead" })]);
		lookAt(looking, "sub", false);
		expect(setInbox.mock.calls).toEqual([[["lead"], { change: "dismiss_woke" }]]);
	});

	it("tries a failed clear again only when the agent is opened", async () => {
		const { looking, setInbox } = setup([woke("a")], "fails");
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		lookAt(looking, "a", false);
		await vi.waitFor(() => expect(warn).toHaveBeenCalled());
		lookAt(looking, "a", false);
		expect(setInbox).toHaveBeenCalledTimes(1);
		lookAt(looking, "a", true);
		expect(setInbox).toHaveBeenCalledTimes(2);
		warn.mockRestore();
	});

	it("does nothing for an agent the view doesn't have", () => {
		const { looking, markSeen, setInbox } = setup([]);
		lookAt(looking, "gone", true);
		expect(markSeen).not.toHaveBeenCalled();
		expect(setInbox).not.toHaveBeenCalled();
	});
});
