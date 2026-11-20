// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type DeviceLink, type Entity, TERMINALS_SINCE, type Terminal } from "../bindings";
import { entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { inDrawer, supportsTerminals, terminalById, terminalsOf } from "./select";

const terminal = (over: Partial<Terminal> & { id: string }): Entity => ({
	entity: "terminal",
	agent: "agent-1",
	device: "dev-mac",
	kind: { type: "shell" },
	moved: false,
	title: "zsh",
	cwd: "~/code/example-app",
	status: { state: "running" },
	password_prompt: false,
	started_at: NOW - 10 * MIN,
	cols: 120,
	rows: 32,
	...over,
});

const run = (id: string, over: Partial<Terminal> = {}): Entity =>
	terminal({
		id,
		kind: { type: "run", command: "pnpm test", private: false },
		title: "pnpm test",
		...over,
	});

const CAN = { transfers: true, tunnels: true, workspaces: false, terminals: true };

const MAC = link({
	id: "mac",
	name: "Mac",
	device: "dev-mac",
	route: { type: "local" },
	capabilities: CAN,
	protocol: TERMINALS_SINCE,
});

const viewOf = (list: Entity[], links: DeviceLink[] = [MAC]) =>
	applySnapshot({ seq: 1, entities: [...entities({ links }), ...list] });

const ids = (list: readonly { id: string }[]) => list.map((t) => t.id);

describe("terminalsOf", () => {
	it("gives an agent's shells and moved commands in the order they started", () => {
		const view = viewOf([
			terminal({ id: "t-late", started_at: NOW - MIN }),
			terminal({ id: "t-early", started_at: NOW - 20 * MIN }),
			run("t-moved", { moved: true, started_at: NOW - 5 * MIN }),
			run("t-card", { started_at: NOW - 15 * MIN }),
			terminal({ id: "t-other", agent: "agent-2" }),
		]);
		expect(ids(terminalsOf(view, "agent-1"))).toEqual(["t-early", "t-moved", "t-late"]);
		expect(ids(terminalsOf(view, "agent-2"))).toEqual(["t-other"]);
		expect(terminalsOf(view, "agent-3")).toEqual([]);
	});

	it("puts the session first, whenever it started", () => {
		const view = viewOf([
			terminal({ id: "t-early", started_at: NOW - 20 * MIN }),
			terminal({ id: "t-session", kind: { type: "session" }, started_at: NOW - MIN }),
		]);
		expect(ids(terminalsOf(view, "agent-1"))).toEqual(["t-session", "t-early"]);
	});

	it("keeps tabs that started at once in a steady order", () => {
		const view = viewOf([terminal({ id: "t-b" }), terminal({ id: "t-a" })]);
		expect(ids(terminalsOf(view, "agent-1"))).toEqual(["t-a", "t-b"]);
	});
});

describe("inDrawer", () => {
	it("puts a shell and a moved command in the drawer, and leaves a run in the timeline", () => {
		const view = viewOf([
			terminal({ id: "s" }),
			run("r"),
			run("m", { moved: true }),
			terminal({ id: "x", kind: { type: "session" } }),
		]);
		const at = (id: string) => {
			const found = terminalById(view, id);
			if (!found) throw new Error(`no terminal ${id}`);
			return inDrawer(found);
		};
		expect([at("s"), at("r"), at("m"), at("x")]).toEqual([true, false, true, true]);
	});
});

describe("terminalById", () => {
	it("finds a terminal, run or shell, and nothing for an id it doesn't have", () => {
		const view = viewOf([run("t-card")]);
		expect(terminalById(view, "t-card")?.title).toBe("pnpm test");
		expect(terminalById(view, "t-gone")).toBeUndefined();
	});
});

describe("supportsTerminals", () => {
	it("says yes for a maxd that has terminals and speaks their protocol", () => {
		expect(supportsTerminals(viewOf([]), "dev-mac")).toBe(true);
	});

	it("says no for an older maxd, one that doesn't say, or one not known yet", () => {
		const older = { ...MAC, protocol: TERMINALS_SINCE - 1 };
		const without = { ...MAC, capabilities: { ...CAN, terminals: false } };
		const unknown = { ...MAC, protocol: null };
		expect(supportsTerminals(viewOf([], [older]), "dev-mac")).toBe(false);
		expect(supportsTerminals(viewOf([], [without]), "dev-mac")).toBe(false);
		expect(supportsTerminals(viewOf([], [unknown]), "dev-mac")).toBe(false);
		expect(supportsTerminals(viewOf([]), "dev-box")).toBe(false);
	});
});
