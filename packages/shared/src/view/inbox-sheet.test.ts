// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, Inbox } from "../bindings";
import { agent, entities, MIN, NOW, workspace } from "../test/build";

import { DEFAULT_INBOX } from "./inbox";
import { inboxMenu, inboxTargets } from "./inbox-actions";
import { inboxSheet } from "./inbox-sheet";
import { applySnapshot } from "./replica";

const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });

const view = applySnapshot({
	seq: 1,
	entities: entities({
		agents: [
			agent({ id: "a" }),
			agent({
				id: "b",
				inbox: inbox({
					pin: { at: NOW, key: "m" },
					settled: { at: NOW, by: "you" },
					snoozed_until: NOW + 60 * MIN,
					auto_settle_off: true,
				}),
			}),
		],
	}),
});

const menuOf = (id: string) => inboxMenu(view, inboxTargets(view, [id]), NOW);
const labels = (id: string, deletable: boolean) =>
	inboxSheet(menuOf(id), deletable).map((entry) => entry.label);

describe("inboxSheet", () => {
	it("lists pin, snooze, settle, auto-settle, rename and delete in order", () => {
		expect(labels("a", true)).toEqual([
			"Pin",
			"Snooze",
			"Settle",
			"Turn off auto-settle",
			"Rename",
			"Delete agent",
		]);
	});

	it("offers the way back for each one that's on", () => {
		expect(labels("b", false)).toEqual([
			"Unpin",
			"Wake",
			"Unsettle",
			"Turn on auto-settle",
			"Rename",
		]);
	});

	it("offers continuing after the limit for a held Claude Code or Codex agent", () => {
		const held = applySnapshot({
			seq: 1,
			entities: [
				...entities({
					agents: [agent({ id: "h", control: "held" })],
					workspaces: [workspace({ id: "w1" })],
				}),
				{
					entity: "device_settings",
					device: "dev-mac",
					default_permission: "auto",
					workspace_network: null,
					workspace_pool: null,
					continue_claude_at_limit: true,
					continue_codex_at_limit: false,
				} as Entity,
			],
		});
		const sheet = inboxSheet(inboxMenu(held, inboxTargets(held, ["h"]), NOW), false);
		expect(sheet.map((entry) => entry.label)).toContain("Don't continue after the limit");
		expect(sheet.map((entry) => entry.label)).not.toContain("Continue after the limit");
	});

	it("carries the menu's calls and the agents a snooze is for", () => {
		const menu = menuOf("a");
		const [pin, snooze, settle] = inboxSheet(menu, false);
		expect(pin).toEqual({ kind: "change", label: "Pin", choice: menu.pin });
		expect(snooze).toEqual({
			kind: "snooze",
			label: "Snooze",
			agents: ["a"],
			untilLimit: null,
		});
		expect(settle).toMatchObject({ choice: menu.settle });
	});

	it("keeps Rename and Delete where the device keeps no inbox", () => {
		expect(inboxSheet(null, true).map((entry) => entry.kind)).toEqual(["rename", "delete"]);
	});
});
