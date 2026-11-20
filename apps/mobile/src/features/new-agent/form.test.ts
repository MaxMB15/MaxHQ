// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type AgentSpec, EFFORT_SINCE } from "@maxhq/shared/bindings";
import { emptyChoices, resolve } from "@maxhq/shared/new-agent/spec";
import { account, device, entities, link, project, workspace } from "@maxhq/shared/test/build";
import { applySnapshot, type ViewState } from "@maxhq/shared/view/replica";

import {
	DEFAULT_EFFORT,
	effortOptions,
	effortToSend,
	initialChoices,
	SHOWN_BRANCHES,
	shownBranches,
	withEffort,
} from "./form";

const MODELS = [
	{
		id: "opus",
		label: "Opus",
		default: true,
		efforts: ["low", "medium", "high"],
		default_effort: "high",
	},
	{ id: "haiku", label: "Haiku", default: false, efforts: [] },
];

const viewOf = ({
	protocol = EFFORT_SINCE,
	projects = true,
}: { protocol?: number; projects?: boolean } = {}): ViewState =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "dev-mac", name: "studio", os: "macos" })],
			links: [
				link({
					id: "mac",
					name: "Mac",
					device: "dev-mac",
					route: { type: "local" },
					protocol,
				}),
			],
			accounts: [
				account({ id: "a-claude", models: MODELS }),
				account({ id: "a-cursor", vendor: "cursor", label: "Cursor", models: MODELS }),
			],
			projects: projects ? [project("p1", "MaxHQ")] : [],
			workspaces: projects ? [workspace({ id: "ws-mac", project: "p1" })] : [],
		}),
	});

const resolved = (view: ViewState, account: string, model: string | null = null) =>
	resolve(view, { ...emptyChoices("p1"), workspace: "ws-mac", account, model });

describe("the choices it opens with", () => {
	it("start in the project it opened from", () => {
		expect(initialChoices(viewOf(), "p1")).toEqual(emptyChoices("p1"));
	});

	it("start in a folder when there's no project", () => {
		expect(initialChoices(viewOf({ projects: false }), null).place).toBe("folder");
	});
});

describe("the effort picker", () => {
	it("offers Default, then the model's levels", () => {
		const view = viewOf();
		expect(effortOptions(view, resolved(view, "a-claude")).map((o) => o.value)).toEqual([
			DEFAULT_EFFORT,
			"low",
			"medium",
			"high",
		]);
	});

	it("hides for a vendor that takes no level, an older maxd, or a model with none", () => {
		const view = viewOf();
		expect(effortOptions(view, resolved(view, "a-cursor"))).toEqual([]);
		const old = viewOf({ protocol: EFFORT_SINCE - 1 });
		expect(effortOptions(old, resolved(old, "a-claude"))).toEqual([]);
		expect(effortOptions(view, resolved(view, "a-claude", "haiku"))).toEqual([]);
	});

	it("sends the level you picked only while it's offered", () => {
		const view = viewOf();
		const options = effortOptions(view, resolved(view, "a-claude"));
		expect(effortToSend(options, "medium")).toBe("medium");
		expect(effortToSend(options, DEFAULT_EFFORT)).toBeNull();
		expect(effortToSend(options, null)).toBeNull();
		expect(effortToSend(options, "max")).toBeNull();
		expect(effortToSend([], "medium")).toBeNull();
	});
});

describe("the spec", () => {
	const spec: AgentSpec = {
		vendor: "claude",
		account: "a-claude",
		place: { type: "workspace", workspace: "ws-mac" },
		model: null,
		permission: "ask",
		title: null,
		first: { text: "Fix the login", attachments: [] },
		branch: null,
	};

	it("carries a level you picked, and leaves it out otherwise", () => {
		expect(withEffort(spec, "low")).toEqual({ ...spec, effort: "low" });
		expect(withEffort(spec, null)).toBe(spec);
		expect("effort" in withEffort(spec, null)).toBe(false);
	});
});

describe("the branches it suggests", () => {
	const suggestions = ["fix-login", "fix-logout", "docs", "a", "b", "c", "d", "e"].map(
		(name) => ({ name, pull: null, current: false })
	);

	it("match what's typed, without the one typed whole", () => {
		expect(shownBranches(suggestions, "fix-login").map((s) => s.name)).toEqual([]);
		expect(shownBranches(suggestions, "fix").map((s) => s.name)).toEqual([
			"fix-login",
			"fix-logout",
		]);
	});

	it("show a few at once", () => {
		expect(shownBranches(suggestions, "")).toHaveLength(SHOWN_BRANCHES);
	});
});
