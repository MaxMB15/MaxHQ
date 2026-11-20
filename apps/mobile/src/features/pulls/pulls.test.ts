// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { CredentialKind, Project } from "@maxhq/shared/bindings";
import { agent, device, entities, link, NOW, project, workspace } from "@maxhq/shared/test/build";
import { credential, detail, pullLink, snapshot } from "@maxhq/shared/test/pulls";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { agentDevice, chipActions, mergeQuestion, pullId, pullProjects } from "./pulls";

const APP: CredentialKind = {
	type: "github_app",
	app_id: 1,
	slug: null,
	key_sha256: "00",
	repos: ["owner/web", "owner/api"],
};

const withRepo = (id: string, name: string, repo: string | null): Project => ({
	...project(id, name),
	repo,
});

const view = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			devices: [device({ id: "dev-box", name: "devbox" })],
			links: [link({ id: "box", name: "devbox" })],
			projects: [
				withRepo("p-web", "Web", "github.com/owner/web"),
				withRepo("p-api", "API", "git@github.com:owner/api.git"),
				withRepo("p-other", "Other", "github.com/owner/other"),
				withRepo("p-none", "None", null),
			],
			workspaces: [
				workspace({ id: "w-web", project: "p-web", device: "dev-box" }),
				workspace({ id: "w-api", project: "p-api", device: "dev-box" }),
				workspace({ id: "w-other", project: "p-other", device: "dev-box" }),
			],
		}),
		credential("dev-box", APP),
	],
});

describe("pullProjects", () => {
	it("offers the projects the device's App reaches, by name", () => {
		expect(pullProjects(view, NOW).map((p) => p.id)).toEqual(["p-api", "p-web"]);
	});
});

describe("agentDevice", () => {
	it("is the device of the agent's workspace", () => {
		expect(agentDevice(view, { workspace: "w-web" })).toBe("dev-box");
		expect(agentDevice(view, { workspace: "gone" })).toBeUndefined();
	});
});

describe("pullId", () => {
	it("is the same for two spellings of one repository", () => {
		expect(pullId({ repo: "Owner/Repo", number: 7 })).toBe(
			pullId({ repo: "owner/repo", number: 7 })
		);
		expect(pullId({ repo: "owner/repo", number: 7 })).not.toBe(
			pullId({ repo: "owner/repo", number: 8 })
		);
	});
});

describe("chipActions", () => {
	const held = agent({ id: "a1", control: "held" });
	const failing = { ...pullLink(7), snapshot: snapshot({ checks: "failing" }) };

	it("offers GitHub, the messages, Unlink and Link another, in the desktop's order", () => {
		expect(chipActions(held, failing).map((a) => a.kind)).toEqual([
			"github",
			"prompt",
			"prompt",
			"unlink",
			"link",
		]);
	});

	it("offers no messages to an agent MaxHQ only watches", () => {
		expect(chipActions({ control: "observed" }, failing).map((a) => a.kind)).toEqual([
			"github",
			"unlink",
			"link",
		]);
	});
});

describe("mergeQuestion", () => {
	it("names the PR, where it goes and the method, which its button says too", () => {
		const pr = detail(1234, { title: "Fix the header", base: "main" });
		expect(mergeQuestion(pr, "squash")).toEqual({
			title: "Merge pull request #1234?",
			detail: "Squash and merge: Fix the header, into main in owner/repo. You can't undo a merge.",
			action: "Squash and merge",
		});
		expect(mergeQuestion(pr, "merge").action).toBe("Create a merge commit");
	});
});
