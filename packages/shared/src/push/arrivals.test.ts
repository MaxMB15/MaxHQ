// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent, DeviceLink } from "../bindings";
import { SYNC_GRACE_MS } from "../status/reach";
import { agent, attention, entities, link, MIN, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import { arrivalText, bannerArrival, newPrompts, stillWaiting, waitingPrompts } from "./arrivals";
import { heardLive } from "./push";

const waiting = (id: string, prompt: string, over: Partial<Agent> = {}) =>
	agent({ id, activity: { state: "needs_input", attention: prompt }, ...over });

const view = (agents: Agent[], since = NOW - 30 * MIN, links?: DeviceLink[]) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			agents,
			workspaces: [workspace({ id: "w1" })],
			links: links ?? [
				link({ id: "mac", device: "dev-mac", state: { state: "connected", since } }),
			],
			attention: [
				attention({ id: "e1", agent: "a1", created_at: NOW - 2 * MIN }),
				attention({
					id: "e2",
					agent: "a2",
					kind: "question",
					summary: "",
					created_at: NOW - MIN,
				}),
			],
		}),
	});

describe("newPrompts", () => {
	it("is quiet for what was waiting at the start, and finds what comes after", () => {
		const start = view([waiting("a1", "e1"), agent({ id: "a2" })]);
		const known = waitingPrompts(start);
		expect(newPrompts(known, start, NOW).arrived).toEqual([]);

		const later = view([waiting("a1", "e1"), waiting("a2", "e2")]);
		const { arrived, known: after } = newPrompts(known, later, NOW);
		expect(arrived.map((each) => each.attention)).toEqual(["e2"]);
		expect(arrived[0]?.event?.kind).toBe("question");
		expect(newPrompts(after, later, NOW).arrived).toEqual([]);
	});

	it("is quiet for what a device sends as its link comes up", () => {
		const reconnect = view([waiting("a1", "e1"), waiting("a2", "e2")], NOW - 1_000);
		const { arrived, known } = newPrompts(new Set(), reconnect, NOW);
		expect(arrived).toEqual([]);
		expect([...known].sort()).toEqual(["e1", "e2"]);

		const dialing = link({
			id: "mac",
			device: "dev-mac",
			state: { state: "connecting", step: "dialing", attempt: 1 },
		});
		expect(
			newPrompts(new Set(), view([waiting("a1", "e1")], NOW, [dialing]), NOW).arrived
		).toEqual([]);
	});

	it("puts several in the order they were asked", () => {
		const both = view([waiting("a2", "e2"), waiting("a1", "e1")], NOW - SYNC_GRACE_MS);
		const { arrived } = newPrompts(new Set(), both, NOW);
		expect(arrived.map((each) => each.attention)).toEqual(["e1", "e2"]);
	});

	it("forgets a prompt once it's answered", () => {
		const asked = view([waiting("a1", "e1")]);
		const answered = view([agent({ id: "a1" })]);
		expect(newPrompts(waitingPrompts(asked), answered, NOW).known.size).toBe(0);
	});
});

describe("the banner", () => {
	const both = view([waiting("a1", "e1"), waiting("a2", "e2")]);
	const { arrived } = newPrompts(new Set(), both, NOW);
	const [first, second] = arrived;

	it("shows the newest prompt for an agent not on screen", () => {
		expect(bannerArrival(arrived, null)?.attention).toBe("e2");
		expect(bannerArrival(arrived, "a2")?.attention).toBe("e1");
		expect(bannerArrival(arrived.slice(1), "a2")).toBeNull();
	});

	it("says who asks and what", () => {
		expect(first && arrivalText(first)).toEqual({ title: "a1", body: "Run cargo test" });
		expect(second && arrivalText(second)).toEqual({ title: "a2", body: "Asks you a question" });
	});

	it("goes once the prompt is answered", () => {
		expect(first && stillWaiting(both, first)).toBe(true);
		expect(first && stillWaiting(view([agent({ id: "a1" })]), first)).toBe(false);
	});
});

describe("heardLive", () => {
	const live = view([waiting("a1", "e1")]);

	it("covers a push about a prompt the view holds", () => {
		expect(heardLive({ agent: "a1", attention: "e1", options: [] }, live)).toBe(true);
		expect(heardLive({ agent: "a1", attention: "e9", options: [] }, live)).toBe(false);
		expect(heardLive({ agent: "a1", attention: null, options: [] }, live)).toBe(false);
		expect(heardLive(null, live)).toBe(false);
	});
});
