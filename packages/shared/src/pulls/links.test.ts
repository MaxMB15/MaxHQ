// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { agent, entities, workspace } from "../test/build";
import { pullLink, REPO } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import { agentsWithPull, devicesWithPull, samePull, sameRepo, shownPulls } from "./links";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		workspaces: [
			workspace({ id: "w-mac", device: "dev-mac" }),
			workspace({ id: "w-box", device: "dev-box" }),
		],
		agents: [
			agent({ id: "a1", workspace: "w-mac", pulls: [pullLink(7)] }),
			agent({ id: "a2", workspace: "w-mac", pulls: [pullLink(7, "you"), pullLink(8)] }),
			agent({
				id: "a3",
				workspace: "w-box",
				pulls: [{ ...pullLink(7), key: { repo: "Owner/Repo", number: 7 } }],
			}),
			agent({ id: "a4", workspace: "w-box", pulls: [pullLink(9, "dismissed")] }),
			agent({ id: "a5", workspace: "w-box" }),
		],
	}),
});

describe("sameRepo and samePull", () => {
	it("compare repositories whatever their case", () => {
		expect(sameRepo("Owner/Repo", REPO)).toBe(true);
		expect(sameRepo("owner/other", REPO)).toBe(false);
		expect(samePull({ repo: "OWNER/repo", number: 7 }, { repo: REPO, number: 7 })).toBe(true);
		expect(samePull({ repo: REPO, number: 8 }, { repo: REPO, number: 7 })).toBe(false);
	});
});

describe("shownPulls", () => {
	it("leaves out the links you dismissed", () => {
		const links = [pullLink(1), pullLink(2, "dismissed"), pullLink(3, "branch")];
		expect(shownPulls(agent({ id: "a", pulls: links })).map((l) => l.key.number)).toEqual([
			1, 3,
		]);
	});

	it("is empty for an agent its device gave no links", () => {
		expect(shownPulls(agent({ id: "a" }))).toEqual([]);
	});
});

describe("agentsWithPull and devicesWithPull", () => {
	it("find the agents showing a PR, and their devices once each", () => {
		const key = { repo: REPO, number: 7 };
		expect(
			agentsWithPull(view, key)
				.map((a) => a.id)
				.sort()
		).toEqual(["a1", "a2", "a3"]);
		expect(devicesWithPull(view, key).sort()).toEqual(["dev-box", "dev-mac"]);
	});

	it("skip a dismissed link", () => {
		expect(devicesWithPull(view, { repo: REPO, number: 9 })).toEqual([]);
	});
});
