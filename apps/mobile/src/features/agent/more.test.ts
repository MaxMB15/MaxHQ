// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { type Agent, type AgentId, EFFORT_SINCE } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { keptConsent } from "@maxhq/shared/settings/auto-mode";
import { pullLink } from "@maxhq/shared/test/pulls";
import { account, agent, entities, link, workspace } from "@maxhq/shared/test/build";
import { createRenames, shownTitle } from "@maxhq/shared/ui/rename-agent";
import { applySnapshot } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import {
	copyOutSheet,
	createRenameDialog,
	effortPick,
	MORE_LABEL,
	modelSheet,
	moreChoices,
	permissionSheet,
	pickPermission,
	runControl,
	type Sheet,
	stopQuestion,
} from "./more";

// A Claude agent MaxHQ holds, with a Codex login on the same Mac to move to.
const held = agent({
	id: "h",
	title: "Fix the flaky test",
	control: "held",
	account: "claude",
	session: "s1",
	model: "opus",
	permission: { requested: "ask", effective: null },
});
const controls = (over: Partial<Agent> = {}) => {
	const target = { ...held, ...over };
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			agents: [target],
			workspaces: [workspace({ id: "w1" })],
			accounts: [
				account({
					id: "claude",
					models: [
						{ id: "sonnet", label: "Sonnet", default: true },
						{ id: "opus", label: "Opus", default: false },
					],
				}),
				account({ id: "codex", vendor: "codex", label: "Codex", plan: "plus" }),
			],
		}),
	});
	return { view, agent: target };
};
const labels = (sheet: Sheet) =>
	sheet.choices.map((c) => (c.disabled ? `${c.label}, greyed` : c.label));

const setup = (answer: ActionResult = { status: "ok", data: { status: "done" } }) => {
	const calls: [AgentId, string][] = [];
	const source = {
		renameAgent: async (id: AgentId, title: string) => {
			calls.push([id, title]);
			return answer;
		},
	};
	const view = createViewStore();
	view.setState({
		view: applySnapshot({
			seq: 1,
			entities: entities({ agents: [agent({ id: "a", title: "Fix the flaky test" })] }),
		}),
	});
	const renames = createRenames(source, view);
	const current = () => view.getState().view.tables.agent.get("a")!;
	const dialog = createRenameDialog(renames);
	return { calls, renames, current, dialog };
};

// A Codex agent at High, on a Mac whose maxd speaks `protocol`.
const atEffort = (protocol: number) => {
	const target = agent({
		id: "x",
		vendor: "codex",
		control: "held",
		account: "codex",
		model: "gpt-sol",
		effort: "high",
	});
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			agents: [target],
			workspaces: [workspace({ id: "w1" })],
			links: [link({ id: "local", device: "dev-mac", protocol })],
			accounts: [
				account({
					id: "codex",
					vendor: "codex",
					models: [
						{
							id: "gpt-sol",
							label: "GPT Sol",
							default: true,
							efforts: ["low", "medium", "high"],
							default_effort: "medium",
						},
					],
				}),
			],
		}),
	});
	return { view, agent: target };
};

describe("the More sheet", () => {
	it("offers Effort where the model lists levels and the maxd sets them", () => {
		const now = atEffort(EFFORT_SINCE);
		expect(moreChoices(now.view, now.agent)).toContain("effort");
		const old = atEffort(EFFORT_SINCE - 1);
		expect(moreChoices(old.view, old.agent)).not.toContain("effort");
	});

	it("lists the levels side by side with the agent's own and the model's default", () => {
		const { view, agent: target } = atEffort(EFFORT_SINCE);
		const pick = effortPick(view, target);
		expect(pick.message).toBe("Runs at High.");
		expect(pick.current).toBe("high");
		expect(pick.levels.map((level) => [level.label, level.isDefault])).toEqual([
			["Low", false],
			["Medium", true],
			["High", false],
		]);
	});

	it("shows the model's default for an agent that names no level", () => {
		const { view, agent: target } = atEffort(EFFORT_SINCE);
		const pick = effortPick(view, { ...target, effort: null });
		expect(pick.message).toBe("Runs at Medium.");
		expect(pick.current).toBe("medium");
	});

	it("offers the controls of an agent MaxHQ holds, and what reads for one it watches", () => {
		const { view, agent: target } = controls();
		expect(moreChoices(view, target).map((c) => MORE_LABEL[c])).toEqual([
			"Find",
			"Your prompts",
			"Rename",
			"Permission",
			"Model",
			"Link a pull request",
			"Details",
			"Ports and processes",
			"Copy and export…",
			"Move to…",
			"Stop agent",
			"Delete agent",
		]);
		const watched = controls({ control: "observed", permission: null, session: null });
		expect(moreChoices(watched.view, watched.agent)).toEqual([
			"find",
			"prompts",
			"rename",
			"link",
			"details",
			"running",
			"copyOut",
		]);
	});

	it("offers Allowed to run agents only while it holds a permission", () => {
		const { view, agent: target } = controls();
		expect(moreChoices(view, target)).not.toContain("allowed");
		expect(moreChoices(view, target, true)).toContain("allowed");
	});

	it("offers Sub-agents for a lead that has some, and no Move for one that moved", () => {
		const { view, agent: target } = controls();
		const lead = applySnapshot({
			seq: 1,
			entities: entities({
				agents: [target, agent({ id: "sub", parent: target.id, control: "held" })],
				workspaces: [workspace({ id: "w1" })],
			}),
		});
		expect(moreChoices(lead, target)).toContain("subAgents");
		const moved = {
			...target,
			moved: { to: { device: "devbox", agent: "h2" }, from: null },
		} as Agent;
		expect(moreChoices(view, moved)).not.toContain("move");
	});

	it("copies the session ID when the CLI gave one, then exports or copies the conversation", () => {
		const picked: string[] = [];
		const sheet = copyOutSheet({ ...held, vendor_session_id: "abc-123" }, (what) =>
			picked.push(what)
		);
		expect(sheet.message).toBe("abc-123");
		expect(sheet.choices.map((c) => c.label)).toEqual([
			"Copy session ID",
			"Export as Markdown…",
			"Copy as Markdown",
		]);
		for (const choice of sheet.choices) choice.pick();
		expect(picked).toEqual(["session", "file", "copy"]);
		const none = copyOutSheet({ ...held, vendor_session_id: null }, () => {});
		expect(none.message).toBeUndefined();
		expect(none.choices.map((c) => c.label)).toEqual([
			"Export as Markdown…",
			"Copy as Markdown",
		]);
	});

	it("offers Link a pull request only while no PR shows on the agent", () => {
		const linked = controls({ pulls: [pullLink(7)] });
		expect(moreChoices(linked.view, linked.agent)).not.toContain("link");
		const dismissed = controls({ pulls: [pullLink(7, "dismissed")] });
		expect(moreChoices(dismissed.view, dismissed.agent)).toContain("link");
	});

	it("leaves Stop agent out once its device is out of reach", () => {
		const { view, agent: target } = controls({
			link: { state: "down", reason: "unreachable", since: null },
		});
		expect(moreChoices(view, target)).toEqual([
			"find",
			"prompts",
			"rename",
			"permission",
			"model",
			"link",
			"details",
			"running",
			"copyOut",
			"move",
			"delete",
		]);
	});

	it("lists the permission modes the desktop lists, the running one greyed", () => {
		const { view, agent: target } = controls();
		const picked: string[] = [];
		const sheet = permissionSheet(view, target, target.permission!, (mode) =>
			picked.push(mode)
		);
		expect(sheet.title).toBe("Permission mode");
		expect(sheet.message).toBe(
			"Runs as Ask first. Auto runs the repo's own test, build and lint scripts without asking. Use it on repos you trust."
		);
		expect(labels(sheet)).toEqual([
			"Ask first, greyed",
			"Accept edits",
			"Auto",
			"Plan only",
			"Full access",
		]);
		sheet.choices[2]!.pick();
		expect(picked).toEqual(["auto"]);
	});

	it("greys Auto and Full access, and says where to turn them on, while the switch is off", () => {
		const { view, agent: target } = controls();
		const sheet = permissionSheet(view, target, target.permission!, () => {}, false);
		expect(sheet.message).toBe(
			"Runs as Ask first. To pick Auto or Full access from the phone, turn on Let this phone do everything for this device in the Mac's Settings, under Phones."
		);
		expect(labels(sheet)).toEqual([
			"Ask first, greyed",
			"Accept edits",
			"Auto (Needs the switch), greyed",
			"Plan only",
			"Full access (Needs the switch), greyed",
		]);
	});

	it("greys the modes Codex can't run in and says when the CLI runs another", () => {
		const { view, agent: target } = controls({
			vendor: "codex",
			account: "codex",
			permission: { requested: "plan", effective: "ask" },
		});
		const sheet = permissionSheet(view, target, target.permission!, () => {});
		expect(sheet.message).toMatch(/^Runs as Ask first\. You asked for Plan only\./);
		expect(labels(sheet)).toContain("Plan only (Not in Codex), greyed");
	});

	it("asks before Auto the first time, and not once you said yes", () => {
		const { view, agent: target } = controls();
		let kept: string | null = null;
		const consent = keptConsent({
			read: () => kept,
			write: (value) => {
				kept = value;
			},
		});
		const set: string[] = [];
		let answer = false;
		const ask = vi.fn((yes: () => void) => {
			if (answer) yes();
		});
		const sheet = permissionSheet(
			view,
			target,
			target.permission!,
			pickPermission(consent, ask, (mode) => set.push(mode))
		);
		const auto = sheet.choices[2]!;
		auto.pick();
		expect(ask).toHaveBeenCalledTimes(1);
		expect(set).toEqual([]);
		answer = true;
		auto.pick();
		auto.pick();
		sheet.choices[1]!.pick();
		expect(ask).toHaveBeenCalledTimes(2);
		expect(set).toEqual(["auto", "auto", "accept_edits"]);
	});

	it("lists the account's models and the accounts to move to", () => {
		const { view, agent: target } = controls();
		const picked: [string, string | undefined][] = [];
		const opened: Sheet[] = [];
		const sheet = modelSheet(
			view,
			target,
			(model, to) => picked.push([model, to]),
			(next) => opened.push(next)
		);
		expect(sheet.message).toBe("Runs Opus on Claude Max.");
		expect(labels(sheet)).toEqual(["Sonnet (default)", "Opus, greyed", "Move to Codex Plus"]);

		sheet.choices[0]!.pick();
		sheet.choices[2]!.pick();
		expect(opened.map((s) => [s.title, labels(s)])).toEqual([
			["Move to Codex Plus", ["Default model"]],
		]);
		opened[0]!.choices[0]!.pick();
		expect(picked).toEqual([
			["sonnet", undefined],
			["", "codex"],
		]);
	});

	it("asks before Stop agent in the desktop's words", () => {
		expect(stopQuestion("Fix the flaky test")).toEqual({
			title: 'Stop agent "Fix the flaky test"?',
			detail: "Ends its process. The conversation stays.",
		});
	});

	it("says why a control failed as the desktop's header does", async () => {
		const done: ActionResult = { status: "ok", data: { status: "done" } };
		expect(await runControl("stop the agent", async () => done)).toBeNull();
		expect(
			await runControl("stop the agent", async () => ({
				status: "ok",
				data: { status: "refused", reason: "the agent isn't running" },
			}))
		).toBe("Couldn't stop the agent. The agent isn't running.");
		expect(
			await runControl("change the model", () => Promise.reject(new Error("Timed out")))
		).toBe("Couldn't change the model. Timed out");
	});
});

describe("the rename dialog", () => {
	it("opens on the agent's name and saves what you typed", async () => {
		const { calls, renames, current, dialog } = setup();
		dialog.open(current());
		expect(dialog.store.getState()).toMatchObject({
			agent: current(),
			draft: "Fix the flaky test",
		});

		dialog.edit("  Flaky test ");
		const saved = dialog.save();
		expect(dialog.store.getState().agent).toBeNull();
		expect(shownTitle(renames.store.getState(), current())).toBe("Flaky test");
		expect(await saved).toBeNull();
		expect(calls).toEqual([["a", "Flaky test"]]);
	});

	it("closes on Cancel with nothing sent", async () => {
		const { calls, current, dialog } = setup();
		dialog.open(current());
		dialog.edit("Something else");
		dialog.cancel();
		expect(dialog.store.getState().agent).toBeNull();
		expect(await dialog.save()).toBeNull();
		expect(calls).toEqual([]);
	});

	it("says why maxd refused, with the old name back", async () => {
		const { renames, current, dialog } = setup({
			status: "ok",
			data: { status: "refused", reason: "couldn't store the name" },
		});
		dialog.open(current());
		dialog.edit("Flaky test");
		expect(await dialog.save()).toBe(
			"Couldn't rename Fix the flaky test. Couldn't store the name."
		);
		expect(shownTitle(renames.store.getState(), current())).toBe("Fix the flaky test");
	});
});
