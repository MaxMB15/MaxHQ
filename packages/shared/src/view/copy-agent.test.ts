// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agent, entities, workspace } from "../test/build";

import { agentCopies, sessionIdLabel } from "./copy-agent";
import { applySnapshot } from "./replica";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		workspaces: [workspace({ id: "w1" })],
		agents: [
			agent({ id: "full", vendor: "codex", branch: "fix/login", vendor_session_id: "th-1" }),
			agent({ id: "bare", workspace: "gone" }),
		],
	}),
});

const copiesOf = (id: string) => {
	const found = view.tables.agent.get(id);
	if (!found) throw new Error(`no agent ${id}`);
	return agentCopies(view, found);
};

describe("agentCopies", () => {
	it("offers the folder path, the branch and the session ID, in order", () => {
		expect(copiesOf("full")).toEqual([
			{ label: "Folder path", text: "/work/w1" },
			{ label: "Branch", text: "fix/login" },
			{ label: "Thread ID", text: "th-1" },
		]);
	});

	it("leaves out what the agent doesn't have", () => {
		expect(copiesOf("bare")).toEqual([]);
	});

	it("names the ID the way each CLI does", () => {
		expect(["claude", "codex", "cursor", "acp"].map((v) => sessionIdLabel(v as never))).toEqual(
			["Session ID", "Thread ID", "Chat ID", "Session ID"]
		);
	});
});
