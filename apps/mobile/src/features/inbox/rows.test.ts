// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent } from "@maxhq/shared/bindings";
import { agent, entities, link, MIN, NOW, workspace } from "@maxhq/shared/test/build";
import { DEFAULT_INBOX } from "@maxhq/shared/view/inbox";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { loudRow, phoneRowMeta, rowSubtitle } from "./rows";

describe("rowSubtitle", () => {
	it("names the agent and its device, and a state its dot alone shows", () => {
		expect(rowSubtitle("claude", "devbox", "busy")).toBe("Claude on devbox");
		expect(rowSubtitle("codex", "devbox", "idle_cache_warm")).toBe(
			"Codex on devbox, cache warm"
		);
	});
});

describe("loudRow", () => {
	it("brightens rows with news", () => {
		expect(loudRow("needs_input")).toBe(true);
		expect(loudRow("busy")).toBe(false);
	});
});

describe("phoneRowMeta", () => {
	const metaOf = (over: Partial<Agent>) => {
		const shown = agent({ id: "a", workspace: "w", last_activity_at: NOW - 4 * MIN, ...over });
		const devbox = link({ id: "devbox" });
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				agents: [shown],
				links: [devbox],
				workspaces: [workspace({ id: "w", device: "dev-devbox" })],
			}),
		});
		return phoneRowMeta(view, shown, NOW);
	};

	it("marks a quiet agent asleep once its idle CLI stopped", () => {
		expect(metaOf({ asleep: true })).toEqual({ kind: "asleep", text: "Asleep" });
		expect(metaOf({})).toEqual({ kind: "text", text: "4m" });
	});

	it("says when a snoozed agent wakes before anything else", () => {
		const inbox = { ...DEFAULT_INBOX, snoozed_until: NOW + 60 * MIN };
		expect(metaOf({ asleep: true, inbox })).toEqual({ kind: "text", text: "Wakes 1h" });
	});
});
