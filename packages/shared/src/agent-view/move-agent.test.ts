// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type Agent, type Entity, MOVE_AGENT_SINCE } from "../bindings";
import { mockTasks } from "../data/mock-tasks";
import {
	CLAUDE,
	DEVBOX,
	DEVBOX_CLAUDE,
	DEVBOX_CODEX,
	MAC,
	MHQ_WORKSPACE,
	mockEntities,
} from "../data/mock-view";
import { account, agent, device, entities, link, NOW, project, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	canMove,
	MOVE_FOLDER,
	moveAccountOptions,
	moveBlocked,
	moveDeviceOptions,
	movedBanner,
	moveForm,
	movePlaceOptions,
	NO_MOVE_CHOICES,
	sameRepoPlace,
} from "./move-agent";

// The sample view, with each device's maxd speaking `protocol`.
const sample = (protocol: (device: string | null) => number = () => MOVE_AGENT_SINCE) =>
	applySnapshot({
		seq: 1,
		entities: [...mockEntities(NOW), ...mockTasks({ now: () => NOW }).entities()].map(
			(e): Entity => (e.entity === "device_link" ? { ...e, protocol: protocol(e.device) } : e)
		),
	});

const view = sample();
const agentIn = (v: ViewState, id: string): Agent => {
	const found = v.tables.agent.get(id);
	if (!found) throw new Error(`no ${id}`);
	return found;
};
// Waits for your answer, on Claude in MaxHQ, with two tasks on the Mac.
const release = agentIn(view, "agent-release");
// Idle and asleep, on Cursor in Comet.
const seek = agentIn(view, "agent-cursor-seek");

describe("which agents move", () => {
	it("moves a lead MaxHQ runs, and nothing it only watches", () => {
		expect(canMove(view, release)).toBe(true);
		expect(canMove(view, { ...release, control: "observed" })).toBe(false);
		expect(canMove(view, { ...release, parent: "agent-lead" })).toBe(false);
		expect(canMove(view, { ...release, moved: { to: { device: DEVBOX, agent: "a2" } } })).toBe(
			false
		);
	});

	it("needs a newer maxd on the Mac and on the agent's device", () => {
		expect(moveBlocked(view, release)).toBeNull();
		const old = sample((d) => (d === MAC ? MOVE_AGENT_SINCE - 1 : MOVE_AGENT_SINCE));
		expect(moveBlocked(old, release)).toBe("Moving needs a newer maxd on Mac.");
		const gtfs = agentIn(view, "agent-gtfs");
		const oldBox = sample((d) => (d === DEVBOX ? MOVE_AGENT_SINCE - 1 : MOVE_AGENT_SINCE));
		expect(moveBlocked(oldBox, gtfs)).toBe("Moving needs a newer maxd on devbox.");
	});

	it("turns off a device whose maxd is too old to take it", () => {
		const options = moveDeviceOptions(view, release);
		expect(options.map((o) => [o.value, o.disabled ?? false])).toEqual([[DEVBOX, false]]);
		const oldBox = sample((d) => (d === DEVBOX ? MOVE_AGENT_SINCE - 1 : MOVE_AGENT_SINCE));
		expect(moveDeviceOptions(oldBox, release)).toEqual([
			expect.objectContaining({
				value: DEVBOX,
				detail: "Needs a newer maxd",
				disabled: true,
			}),
		]);
		expect(moveForm(oldBox, release, NO_MOVE_CHOICES, NOW)).toMatchObject({
			device: null,
			problem: "No other device can take it now.",
			to: null,
		});
	});
});

describe("the accounts there", () => {
	it("puts the agent's own vendor first", () => {
		const options = moveAccountOptions(view, release, DEVBOX, NOW);
		expect(options.filter((o) => !o.disabled).map((o) => o.value)).toEqual([
			DEVBOX_CLAUDE,
			DEVBOX_CODEX,
		]);
		const codexAgent = { ...release, vendor: "codex" as const };
		expect(moveAccountOptions(view, codexAgent, DEVBOX, NOW)[0]?.value).toBe(DEVBOX_CODEX);
	});

	it("shows a vendor not signed in there, turned off, with a way to sign in", () => {
		const options = moveAccountOptions(view, seek, DEVBOX, NOW);
		expect(options.at(-1)).toEqual({
			value: "sign-in:cursor",
			label: "Cursor",
			detail: "Not signed in on devbox",
			disabled: true,
		});
		const form = moveForm(view, seek, NO_MOVE_CHOICES, NOW);
		expect(form.missing).toEqual(["cursor"]);
		// It falls back to the first account there and says the vendor changes.
		expect(form.account?.id).toBe(DEVBOX_CLAUDE);
		expect(form.lines).toContain("It goes on with Claude on devbox.");
	});
});

describe("the folder there", () => {
	it("picks the same project's checkout", () => {
		expect(sameRepoPlace(view, release, DEVBOX)).toBe(MHQ_WORKSPACE);
		expect(sameRepoPlace(view, seek, DEVBOX)).toBe("ws-bravo");
		expect(moveForm(view, release, NO_MOVE_CHOICES, NOW).place).toBe(MHQ_WORKSPACE);
	});

	it("lists projects, then folders, then one you type, and never a dispatcher's home", () => {
		const places = movePlaceOptions(view, MAC);
		expect(places.at(-1)?.value).toBe(MOVE_FOLDER);
		expect(places.some((o) => o.value === "ws-dispatcher-mac")).toBe(false);
		const groups = places.map((o) => o.group);
		expect(groups.indexOf("Folders")).toBeGreaterThan(groups.lastIndexOf("Projects"));
	});

	// A Mac and a box with two clones of one repository under different
	// projects, and another repository on the box.
	const clones = (here: string | null) =>
		applySnapshot({
			seq: 1,
			entities: entities({
				devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
				links: [
					link({ id: "mac", device: "dev-mac", route: { type: "local" }, protocol: 44 }),
					link({ id: "box", device: "dev-box", protocol: 44 }),
				],
				projects: [
					{ ...project("p-here", "Comet"), repo: here },
					{ ...project("p-there", "Comet"), repo: "git@github.com:owner/repo" },
					{ ...project("p-other", "Other"), repo: "github.com/owner/other" },
				],
				workspaces: [
					workspace({ id: "w-here", project: "p-here" }),
					workspace({ id: "w-other", project: "p-other", device: "dev-box" }),
					workspace({ id: "w-there", project: "p-there", device: "dev-box" }),
				],
				accounts: [account({ id: "box-claude", device: "dev-box" })],
				agents: [agent({ id: "a", workspace: "w-here", control: "held" })],
			}),
		});

	it("matches the same remote across clone URL forms", () => {
		const built = clones("https://github.com/Owner/Repo.git");
		const a = agentIn(built, "a");
		expect(sameRepoPlace(built, a, "dev-box")).toBe("w-there");
		expect(moveForm(built, a, NO_MOVE_CHOICES, NOW).to).toEqual({
			device: "dev-box",
			account: "box-claude",
			model: null,
			place: { type: "workspace", workspace: "w-there" },
			mode: "auto",
		});
	});

	it("asks for a folder when no checkout matches", () => {
		const built = clones("github.com/owner/elsewhere");
		const form = moveForm(built, agentIn(built, "a"), NO_MOVE_CHOICES, NOW);
		expect(form).toMatchObject({ place: null, problem: "Pick a folder on box.", to: null });
	});

	it("asks for a full path when you type one", () => {
		const choices = { ...NO_MOVE_CHOICES, place: MOVE_FOLDER, path: "work/comet" };
		expect(moveForm(view, release, choices, NOW)).toMatchObject({
			problem: "Type the folder's full path, starting with /",
			to: null,
		});
		const typed = moveForm(view, release, { ...choices, path: " /srv/comet " }, NOW);
		expect(typed.to?.place).toEqual({ type: "folder", path: "/srv/comet" });
	});
});

describe("what the move says and sends", () => {
	it("stops a turn first, and says so", () => {
		const form = moveForm(view, release, NO_MOVE_CHOICES, NOW);
		expect(form.stop).toBe(true);
		expect(form.action).toBe("Stop and move");
		expect(form.lines).toEqual([
			"Only a summary of the conversation carries over.",
			"Files and uncommitted changes in MaxAIManager stay on Mac.",
			"This agent stops and settles on Mac.",
			"Its current turn stops.",
			"2 automations will follow it.",
		]);
		expect(form.to).toEqual({
			device: DEVBOX,
			account: DEVBOX_CLAUDE,
			model: null,
			place: { type: "workspace", workspace: MHQ_WORKSPACE },
			mode: "auto",
		});
	});

	it("moves an idle agent with a plain Move", () => {
		const form = moveForm(view, seek, NO_MOVE_CHOICES, NOW);
		expect(form.stop).toBe(false);
		expect(form.action).toBe("Move");
		expect(form.lines).not.toContain("Its current turn stops.");
	});

	it("keeps the agent's model on its own vendor when the account lists it", () => {
		const opus = view.tables.account.get(DEVBOX_CLAUDE)?.models[0]?.id;
		const onOpus = { ...release, account: CLAUDE, model: opus ?? null };
		expect(moveForm(view, onOpus, NO_MOVE_CHOICES, NOW).to?.model).toBe(opus ?? null);
		const picked = { ...NO_MOVE_CHOICES, account: DEVBOX_CODEX };
		expect(moveForm(view, onOpus, picked, NOW).to?.model).toBeNull();
	});

	it("doesn't take an account from a vendor not signed in there", () => {
		const picked = { ...NO_MOVE_CHOICES, account: "sign-in:cursor" };
		expect(moveForm(view, seek, picked, NOW).account?.id).toBe(DEVBOX_CLAUDE);
	});
});

describe("the mode it runs in there", () => {
	// Runs on devbox, which starts agents in Full access.
	const gtfs = { ...agentIn(view, "agent-gtfs"), control: "held" as const };

	it("takes the lower of its mode now and the default there, and offers the higher", () => {
		// The Mac starts agents in Auto, below devbox's Full access.
		const form = moveForm(view, release, NO_MOVE_CHOICES, NOW);
		expect(form.mode).toBe("auto");
		expect(form.modes).toEqual([
			{ value: "auto", label: "Auto", detail: "Its mode now" },
			{ value: "full_access", label: "Full access", detail: "devbox's default" },
		]);
		const higher = moveForm(view, release, { ...NO_MOVE_CHOICES, mode: "full_access" }, NOW);
		expect(higher.to?.mode).toBe("full_access");
	});

	it("takes the default there when that's the lower one", () => {
		const form = moveForm(view, gtfs, NO_MOVE_CHOICES, NOW);
		expect(form.device).toBe(MAC);
		expect(form.mode).toBe("auto");
		expect(form.modes.map((o) => [o.value, o.detail])).toEqual([
			["auto", "Mac's default"],
			["full_access", "Its mode now"],
		]);
	});

	it("offers one mode when both are the same, and never one it can't run", () => {
		const full = {
			...release,
			permission: { requested: "full_access" as const, effective: null },
		};
		expect(moveForm(view, full, NO_MOVE_CHOICES, NOW).modes).toEqual([
			{
				value: "full_access",
				label: "Full access",
				detail: "Its mode now and devbox's default",
			},
		]);
		const planning = {
			...release,
			permission: { requested: "plan" as const, effective: null },
		};
		const codex = { ...NO_MOVE_CHOICES, account: DEVBOX_CODEX };
		// Codex has no Plan mode, so it asks first.
		expect(moveForm(view, planning, codex, NOW).modes.map((o) => [o.value, o.detail])).toEqual([
			["ask", "Its mode now"],
			["full_access", "devbox's default"],
		]);
	});

	it("falls back to the lower mode when the pick isn't offered", () => {
		const form = moveForm(view, gtfs, { ...NO_MOVE_CHOICES, mode: "plan" }, NOW);
		expect(form.mode).toBe("auto");
		// devbox set to Accept edits: Full access is above both, so it's never offered.
		const careful = applySnapshot({
			seq: 1,
			entities: [
				...mockEntities(NOW),
				{
					entity: "device_settings",
					device: DEVBOX,
					default_permission: "accept_edits",
					workspace_network: null,
					workspace_pool: null,
				},
			],
		});
		const asked = moveForm(careful, release, { ...NO_MOVE_CHOICES, mode: "full_access" }, NOW);
		expect(asked.modes.map((o) => o.value)).toEqual(["accept_edits", "auto"]);
		expect(asked.to?.mode).toBe("accept_edits");
	});
});

describe("the banners", () => {
	it("points a moved agent at the new one", () => {
		const to = { device: DEVBOX, agent: "agent-gtfs" };
		const banner = movedBanner(view, { ...release, moved: { to } });
		expect(banner).toMatchObject({ way: "to", title: "Moved to devbox", other: "agent-gtfs" });
		expect(
			movedBanner(view, { ...release, moved: { to: { ...to, agent: "gone" } } })?.other
		).toBe(null);
	});

	it("points the new agent back at the old one", () => {
		const from = { device: MAC, agent: "agent-release" };
		expect(movedBanner(view, { ...seek, moved: { from } })).toMatchObject({
			way: "from",
			title: "Moved from Mac",
			other: "agent-release",
		});
		expect(movedBanner(view, release)).toBeNull();
	});
});
