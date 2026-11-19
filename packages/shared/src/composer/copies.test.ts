// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type {
	AgentId,
	AttachmentId,
	AttachmentRef,
	DeviceId,
	NoteId,
	ProjectId,
} from "../bindings";
import type { ActionResult, NoteResult } from "../data/source";
import type { Segment } from "../pills/draft";
import { device, entities, link, project } from "../test/build";
import { applySnapshot } from "../view/replica";

import { attachFailure, copyFailure, EXCERPT_BUDGET, excerptFailure, takeCopies } from "./copies";

const staged = (id: string, name: string): ActionResult => ({
	status: "ok",
	data: {
		status: "staged",
		attachment: {
			id: id as AttachmentId,
			name,
			kind: "file",
			bytes: 1,
			mime: "text/markdown",
			path: `/tmp/${name}`,
			thumbnail: null,
		} as AttachmentRef,
	},
});

const agentPill = (id: string, on: string): Segment => ({
	type: "pill",
	pill: {
		type: "agent",
		agent: id as AgentId,
		title: `Agent ${id}`,
		vendor: "claude",
		device: on as DeviceId,
		excerpt: null,
	},
});

const notePill = (id: string, inProject: string): Segment => ({
	type: "pill",
	pill: {
		type: "note",
		project: inProject as ProjectId,
		note: id as NoteId,
		title: `Note ${id}`,
		copy: null,
	},
});

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-a", "Atlas"), project("p-b", "Billing")],
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
		links: [link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } })],
	}),
});

type Fake = {
	staged: { device: DeviceId; name: string; text: string }[];
	asked: [AgentId, number][];
	read: NoteResult;
	excerpt: (agent: AgentId) => Promise<{ name: string; text: string }>;
};

const fakeSource = (fake: Fake) => ({
	excerptAgent: async (agent: AgentId, budget: number) => {
		fake.asked.push([agent, budget]);
		const { name, text } = await fake.excerpt(agent);
		return { name, text, messages: 1, omitted: 0 };
	},
	readNote: async () => fake.read,
	stagePasted: async (on: DeviceId, name: string, _mime: string, bytes: Uint8Array) => {
		fake.staged.push({ device: on, name, text: new TextDecoder().decode(bytes) });
		return staged(`att-${fake.staged.length}`, name);
	},
});

const newFake = (over: Partial<Fake> = {}): Fake => ({
	staged: [],
	asked: [],
	read: {
		status: "ok",
		data: { title: "Release steps", body: "Step one", deleted: false },
	} as NoteResult,
	excerpt: async (agent) => ({ name: `${agent}.md`, text: `About ${agent}` }),
	...over,
});

describe("takeCopies", () => {
	it("readies an excerpt of an agent on another device on the agent's own", async () => {
		const fake = newFake();
		const copies = await takeCopies(
			fakeSource(fake) as never,
			() => view,
			"dev-box" as DeviceId,
			[agentPill("a1", "dev-mac"), agentPill("a2", "dev-box")],
			"p-a" as ProjectId
		);
		expect(fake.asked).toEqual([["a1", EXCERPT_BUDGET]]);
		expect(fake.staged).toEqual([{ device: "dev-box", name: "a1.md", text: "About a1" }]);
		expect([...copies.excerpts]).toEqual([["a1", "att-1"]]);
		expect(copies.failure).toBeNull();
	});

	it("copies a note from another project, and leaves one from its own live", async () => {
		const fake = newFake();
		const copies = await takeCopies(
			fakeSource(fake) as never,
			() => view,
			"dev-box" as DeviceId,
			[notePill("n1", "p-b"), notePill("n2", "p-a")],
			"p-a" as ProjectId
		);
		expect(fake.staged.map((each) => each.text)).toEqual(["Step one"]);
		expect([...copies.notes]).toEqual([["n1", "att-1"]]);
	});

	it("says why an excerpt couldn't be taken", async () => {
		const fake = newFake({
			excerpt: async () => {
				throw new Error("the device isn't connected");
			},
		});
		const copies = await takeCopies(
			fakeSource(fake) as never,
			() => view,
			"dev-box" as DeviceId,
			[agentPill("a1", "dev-mac")],
			null
		);
		expect(copies.excerpts.size).toBe(0);
		expect(copies.failure).toBe(
			"Couldn't get an excerpt of Agent a1. The device isn't connected. Send again, or remove the agent."
		);
	});

	it("says a deleted note can't be copied", async () => {
		const fake = newFake({
			read: {
				status: "ok",
				data: { title: "Gone", body: "", deleted: true },
			} as NoteResult,
		});
		const copies = await takeCopies(
			fakeSource(fake) as never,
			() => view,
			"dev-box" as DeviceId,
			[notePill("n1", "p-b")],
			"p-a" as ProjectId
		);
		expect(copies.failure).toBe(
			"Couldn't copy the note Note n1. It was deleted. Send again, or remove the note."
		);
	});
});

describe("failure texts", () => {
	it("names one file and its reason", () => {
		expect(attachFailure([{ name: "a.png", error: "Too big" }])).toBe(
			"Couldn't attach a.png. Too big. Remove it, then send again."
		);
	});

	it("leaves out a reason that only says it failed", () => {
		expect(
			attachFailure([
				{ name: "a.png", error: "Couldn't get it ready" },
				{ name: "b.png", error: null },
			])
		).toBe("Couldn't attach 2 files. Remove them, then send again.");
	});

	it("leaves out reasons that differ", () => {
		expect(
			excerptFailure([
				{ title: "One", error: "x" },
				{ title: "Two", error: "y" },
			])
		).toBe("Couldn't get excerpts of 2 agents. Send again, or remove them.");
		expect(copyFailure([{ title: "Plan", error: null }])).toBe(
			"Couldn't copy the note Plan. Send again, or remove the note."
		);
	});
});
