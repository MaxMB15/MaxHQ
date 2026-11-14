// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { ServicesProvider, useView } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { ProposalDiff, SkillProposal } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { DEVFLOW, GIT_FLOW_PROPOSAL, THIRD_PARTY } from "@maxhq/shared/data/mock-skills";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { clearSkillScans } from "@maxhq/shared/skills/scan";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { ProposalViewer } from "./proposal-viewer";

afterEach(() => clearSkillScans());

const renderApp = (scenario = "connected", agent?: string) => {
	const source = mockSource({ now: () => NOW, tick: false, scenario });
	const ui = createUiStore();
	if (agent) ui.getState().selectAgent(agent, false);
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return source;
};

const openSettings = async (group: string, page: string) => {
	fireEvent.click(await screen.findByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(
		await within(within(nav).getByRole("group", { name: group })).findByRole("button", {
			name: page,
		})
	);
	return within(await screen.findByRole("dialog", { name: "Settings" }));
};

const openSkillsTab = async (project: string) => {
	const settings = await openSettings("Projects", project);
	fireEvent.click(settings.getByRole("tab", { name: "Skills" }));
	return settings;
};

describe("the skills library page", () => {
	it("lists each source with its trust and its skills' badges", async () => {
		renderApp();
		const page = await openSettings("App", "Skills");
		const mine = page.getByRole("group", { name: "example/agent-skills" });
		expect(within(mine).getByText("Yours")).toBeTruthy();
		expect(within(mine).getByText("native-rendering-discipline")).toBeTruthy();
		expect(within(mine).getByText("Allowed tools")).toBeTruthy();
		const theirs = page.getByRole("group", { name: "example/third-party-skills" });
		expect(within(theirs).getAllByText("Third party").length).toBeGreaterThan(1);
		expect(within(theirs).getByText(/at v1\.2\.0/)).toBeTruthy();
		expect(page.getByText("Held by Mac")).toBeTruthy();
		const always = page.getByRole("list", { name: "Skills every agent gets" });
		expect(within(always).getByText("unslop")).toBeTruthy();
	});

	it("adds a source from owner/repo and refuses a name the library has", async () => {
		const source = renderApp();
		const add = vi.spyOn(source, "addSkillSource");
		const page = await openSettings("App", "Skills");
		const form = within(page.getByRole("form", { name: "Add a source" }));
		const where = form.getByPlaceholderText(/owner\/repo/);
		fireEvent.change(where, { target: { value: "example/devflow" } });
		expect(
			form.getByText("The library already has a source called example/devflow.")
		).toBeTruthy();
		expect(form.getByRole("button", { name: "Add source" })).toHaveProperty("disabled", true);
		fireEvent.change(where, { target: { value: "example/more-skills" } });
		fireEvent.click(form.getByRole("button", { name: "Add source" }));
		await waitFor(() =>
			expect(add).toHaveBeenCalledWith(
				MAC,
				"example/more-skills",
				{ kind: "git", url: "https://github.com/example/more-skills" },
				null,
				"own"
			)
		);
		expect(await page.findByRole("group", { name: "example/more-skills" })).toBeTruthy();
	});

	it("asks for a tick before tidying a device's global skills, then sends the plan it showed", async () => {
		const source = renderApp("skills_conflict");
		const apply = vi.spyOn(source, "applyGlobalSkills");
		const page = await openSettings("App", "Skills");
		const globals = within(page.getByRole("region", { name: "Global skills" }));
		const macRow = globals.getByText("Mac").closest("div.flex.items-center") as HTMLElement;
		fireEvent.click(within(macRow).getByRole("button", { name: "Tidy global skills" }));
		const dialog = within(
			await screen.findByRole("dialog", { name: "Tidy global skills on Mac" })
		);
		const moves = await dialog.findByRole("region", { name: "Move into the library" });
		expect(within(moves).getByText("/Users/me/.claude/skills/git-flow")).toBeTruthy();
		expect(
			within(dialog.getByRole("region", { name: "Copies that go to the backup" })).getByText(
				"/Users/me/.codex/skills/unslop"
			)
		).toBeTruthy();
		expect(dialog.getByText(/never touches settings\.json/)).toBeTruthy();
		const go = dialog.getByRole("button", { name: "Apply" });
		expect(go).toHaveProperty("disabled", true);
		fireEvent.click(dialog.getByRole("checkbox", { name: "Tidy Mac" }));
		fireEvent.click(go);
		await waitFor(() => expect(apply).toHaveBeenCalledWith(MAC, "plan-mac-1", []));
		expect(await page.findByText("Tidied Mac. Undo puts it back.")).toBeTruthy();
		expect(screen.queryByRole("dialog", { name: "Tidy global skills on Mac" })).toBeNull();
	});

	it("shows the repos that join and sends the copy you pick", async () => {
		const source = renderApp("skills_conflict");
		const apply = vi.spyOn(source, "applyGlobalSkills");
		const page = await openSettings("App", "Skills");
		const globals = within(page.getByRole("region", { name: "Global skills" }));
		const macRow = globals.getByText("Mac").closest("div.flex.items-center") as HTMLElement;
		fireEvent.click(within(macRow).getByRole("button", { name: "Tidy global skills" }));
		const dialog = within(
			await screen.findByRole("dialog", { name: "Tidy global skills on Mac" })
		);
		const joins = within(await dialog.findByRole("region", { name: "Join as sources" }));
		expect(joins.getByText("owner/skills")).toBeTruthy();
		expect(joins.getByText("/Users/me/.agents/skills/grill-me")).toBeTruthy();
		const pick = within(dialog.getByRole("region", { name: "Pick one copy" }));
		fireEvent.click(pick.getByRole("radio", { name: "/Users/me/.codex/skills/review" }));
		fireEvent.click(dialog.getByRole("checkbox", { name: "Tidy Mac" }));
		fireEvent.click(dialog.getByRole("button", { name: "Apply" }));
		await waitFor(() =>
			expect(apply).toHaveBeenCalledWith(MAC, "plan-mac-1", [
				"/Users/me/.codex/skills/review",
			])
		);
	});

	it("offers Undo only once a device has a tidy to undo", async () => {
		renderApp("skills_conflict");
		const page = await openSettings("App", "Skills");
		const globals = within(page.getByRole("region", { name: "Global skills" }));
		const macRow = globals.getByText("Mac").closest("div.flex.items-center") as HTMLElement;
		const row = within(macRow);
		await waitFor(() =>
			expect(row.getByRole("button", { name: "Tidy global skills" })).toBeTruthy()
		);
		expect(row.queryByRole("button", { name: "Undo" })).toBeNull();
		fireEvent.click(row.getByRole("button", { name: "Tidy global skills" }));
		const dialog = within(
			await screen.findByRole("dialog", { name: "Tidy global skills on Mac" })
		);
		fireEvent.click(await dialog.findByRole("checkbox", { name: "Tidy Mac" }));
		fireEvent.click(dialog.getByRole("button", { name: "Apply" }));
		fireEvent.click(await row.findByRole("button", { name: "Undo" }));
		await waitFor(() => expect(row.queryByRole("button", { name: "Undo" })).toBeNull());
	});

	it("shows a device's refusal in place of a plan", async () => {
		renderApp("skills_conflict");
		const page = await openSettings("App", "Skills");
		const globals = within(page.getByRole("region", { name: "Global skills" }));
		const row = globals.getByText("devbox").closest("div.flex.items-center") as HTMLElement;
		fireEvent.click(within(row).getByRole("button", { name: "Tidy global skills" }));
		const dialog = within(await screen.findByRole("dialog", { name: /on devbox/ }));
		expect(await dialog.findByRole("alert")).toHaveProperty(
			"textContent",
			"Container config folders are MaxHQ's, so there's nothing to tidy."
		);
		expect(dialog.queryByRole("button", { name: "Apply" })).toBeNull();
	});
});

describe("a project's Skills tab", () => {
	it("shows what each CLI sees, with origins and the scan's marks", async () => {
		const source = renderApp("skills_conflict");
		const scan = vi.spyOn(source, "scanSkills");
		const tab = await openSkillsTab("MaxHQ");
		const claude = within(await tab.findByRole("group", { name: "Claude" }));
		expect(claude.getByText("A global skill of the same name runs instead")).toBeTruthy();
		expect(claude.getByText("Synced")).toBeTruthy();
		expect(claude.getByRole("img", { name: /Skill listing/ })).toBeTruthy();
		const codex = within(tab.getByRole("group", { name: "Codex" }));
		expect(
			codex.getAllByText("Also in another folder, a different copy").length
		).toBeGreaterThan(0);
		expect(codex.getByText(/config\.toml/)).toBeTruthy();
		const cursor = within(tab.getByRole("group", { name: "Cursor" }));
		expect(cursor.getByText(/~\/\.cursor on this Mac aren't read/)).toBeTruthy();
		expect(scan).toHaveBeenCalledWith(MAC, { type: "workspace", workspace: "ws-mac-maxhq" });
	});

	it("adds from the library, refuses the always set, and applies", async () => {
		const source = renderApp();
		const set = vi.spyOn(source, "setProjectSkills");
		const apply = vi.spyOn(source, "applySkills");
		const tab = await openSkillsTab("MaxHQ");
		const library = within(tab.getByRole("list", { name: "Library skills" }));
		expect(library.getByRole("button", { name: "Add unslop" })).toHaveProperty(
			"disabled",
			true
		);
		expect(library.getByText("Every agent already gets unslop")).toBeTruthy();
		fireEvent.click(library.getByRole("button", { name: "Add grill-me" }));
		await waitFor(() =>
			expect(set).toHaveBeenCalledWith(
				MAC,
				"proj-maxhq",
				expect.objectContaining({
					skills: expect.arrayContaining([expect.objectContaining({ name: "grill-me" })]),
				})
			)
		);
		const mine = within(await tab.findByRole("list", { name: "Project skills" }));
		expect(await mine.findByText("grill-me")).toBeTruthy();
		expect(await tab.findByText("Not in the checkouts yet")).toBeTruthy();
		fireEvent.click(tab.getByRole("button", { name: "Apply" }));
		await waitFor(() => expect(apply).toHaveBeenCalledWith(MAC, "proj-maxhq"));
		await waitFor(() => expect(tab.queryByText("Not in the checkouts yet")).toBeNull());
	});

	it("pins, removes and copies another project's picks", async () => {
		const source = renderApp();
		const set = vi.spyOn(source, "setProjectSkills");
		const tab = await openSkillsTab("MaxHQ");
		const mine = within(tab.getByRole("list", { name: "Project skills" }));
		fireEvent.click(mine.getByRole("button", { name: "Pin git-flow" }));
		await waitFor(() =>
			expect(set).toHaveBeenLastCalledWith(
				MAC,
				"proj-maxhq",
				expect.objectContaining({
					skills: expect.arrayContaining([
						expect.objectContaining({ name: "git-flow", pinned: true }),
					]),
				})
			)
		);
		fireEvent.click(await mine.findByRole("button", { name: "Remove screen-reader-check" }));
		await waitFor(() => expect(mine.queryByText("screen-reader-check")).toBeNull());
		fireEvent.change(tab.getByRole("combobox", { name: "Add from project" }), {
			target: { value: "proj-comet" },
		});
		expect(await mine.findByText("grill-me")).toBeTruthy();
	});

	it("opens a proposal and approves it", async () => {
		const source = renderApp("skills_proposal");
		const answer = vi.spyOn(source, "answerSkillProposal");
		const tab = await openSkillsTab("MaxHQ");
		const proposals = within(tab.getByRole("region", { name: "Proposals" }));
		fireEvent.click(proposals.getByRole("button", { name: "Review" }));
		const viewer = within(await screen.findByRole("dialog", { name: /^Skill change from / }));
		const files = within(await viewer.findByRole("list", { name: "Files" }));
		expect(files.getByText("references/release.md")).toBeTruthy();
		expect(files.getByText(/Rebase it on dev/)).toBeTruthy();
		fireEvent.click(viewer.getByRole("button", { name: "Approve" }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				GIT_FLOW_PROPOSAL,
				"approve",
				false,
				undefined,
				expect.objectContaining({ tree: expect.any(String) })
			)
		);
		await waitFor(() => expect(tab.queryByRole("region", { name: "Proposals" })).toBeNull());
	});

	it("shows drift with a Restore", async () => {
		const source = renderApp("skills_drift");
		const apply = vi.spyOn(source, "applySkills");
		const tab = await openSkillsTab("Comet");
		const banner = within(tab.getByRole("region", { name: "Changed skills" }));
		expect(banner.getByText(/grill-me changed in .*Comet on Mac/)).toBeTruthy();
		fireEvent.click(banner.getByRole("button", { name: "Restore" }));
		await waitFor(() => expect(apply).toHaveBeenCalledWith(MAC, "proj-comet"));
		await waitFor(() =>
			expect(tab.queryByRole("region", { name: "Changed skills" })).toBeNull()
		);
	});
});

describe("a link that leads out of a checkout", () => {
	it("says which link stopped the skills", async () => {
		renderApp("skills_drift");
		const tab = await openSkillsTab("Comet");
		const banner = within(tab.getByRole("region", { name: "Skills that can't land" }));
		expect(
			banner.getByText(
				/Skills can't land through \.claude in .*Comet on Mac: a link there leads out/
			)
		).toBeTruthy();
	});

	it("shows nothing when no link is in the way", async () => {
		renderApp();
		const tab = await openSkillsTab("Comet");
		expect(tab.queryByRole("region", { name: "Skills that can't land" })).toBeNull();
	});
});

describe("a project's devflow pack", () => {
	it("shows each checkout's install and takes the pack out", async () => {
		const source = renderApp();
		const set = vi.spyOn(source, "setProjectSkills");
		const tab = await openSkillsTab("MaxHQ");
		const picker = tab.getByRole("combobox", { name: "Devflow pack" });
		expect((picker as HTMLSelectElement).value).toBe(DEVFLOW);
		const checkouts = within(tab.getByRole("list", { name: "Pack in the checkouts" }));
		expect(checkouts.getByText("Installed at c41e9b0")).toBeTruthy();
		fireEvent.change(picker, { target: { value: "" } });
		const dialog = within(await screen.findByRole("dialog"));
		const left = within(dialog.getByRole("list", { name: "Left for you to delete" }));
		for (const path of ["CLAUDE.md", "AGENTS.md", ".devflow/", ".mcp.json"]) {
			expect(left.getByText(path)).toBeTruthy();
		}
		expect(set).not.toHaveBeenCalled();
		fireEvent.click(dialog.getByRole("button", { name: "Remove" }));
		await waitFor(() =>
			expect(set).toHaveBeenCalledWith(
				MAC,
				"proj-maxhq",
				expect.not.objectContaining({ pack: expect.anything() })
			)
		);
	});

	it("shows a failed install's last lines with a retry that applies", async () => {
		const source = renderApp("skills_drift");
		const apply = vi.spyOn(source, "applySkills");
		const tab = await openSkillsTab("MaxHQ");
		fireEvent.click(tab.getByRole("button", { name: "devbox" }));
		const checkouts = within(await tab.findByRole("list", { name: "Pack in the checkouts" }));
		expect(checkouts.getByText("Install of c41e9b0 failed")).toBeTruthy();
		expect(checkouts.getByText(/uv isn't on the PATH/)).toBeTruthy();
		fireEvent.click(checkouts.getByRole("button", { name: /^Retry the pack in / }));
		await waitFor(() => expect(apply).toHaveBeenCalledWith(DEVBOX, "proj-maxhq"));
		expect(await checkouts.findByText("Installed at c41e9b0")).toBeTruthy();
	});

	it("marks a pack someone else vendored", async () => {
		renderApp();
		const tab = await openSkillsTab("Comet");
		const checkouts = within(tab.getByRole("list", { name: "Pack in the checkouts" }));
		expect(checkouts.getByText(/^Vendored by someone else/)).toBeTruthy();
		expect(
			(tab.getByRole("combobox", { name: "Devflow pack" }) as HTMLSelectElement).value
		).toBe("");
	});
});

describe("the agent strip's skills chip", () => {
	it("marks a Cursor agent waiting for changed skills, and opens its project's Skills tab", async () => {
		renderApp("skills_drift", "agent-cursor-seek");
		const chip = await screen.findByRole("button", {
			name: "Skills changed, restarts on next message",
		});
		fireEvent.click(chip);
		const settings = within(await screen.findByRole("dialog", { name: "Settings" }));
		expect(settings.getByRole("tab", { name: "Skills" }).getAttribute("aria-selected")).toBe(
			"true"
		);
		expect(settings.getByRole("region", { name: "Changed skills" })).toBeTruthy();
	});

	it("stays off without drift", async () => {
		renderApp("connected", "agent-cursor-seek");
		await screen.findByRole("heading", { name: "Seek bar thumbnails" });
		expect(screen.queryByRole("button", { name: /Skills changed/ })).toBeNull();
	});
});

describe("the proposal viewer", () => {
	const Harness = ({ thirdParty = false }: { thirdParty?: boolean }) => {
		const proposal = useView((s) => s.view.tables.skill_proposal.get(GIT_FLOW_PROPOSAL));
		return proposal ? (
			<ProposalViewer
				proposal={thirdParty ? { ...proposal, source: THIRD_PARTY } : proposal}
				diff={{
					tree: "tree-read",
					target: {
						name: "git-flow",
						source: thirdParty ? THIRD_PARTY : proposal.source,
						base: "base-read",
					},
					files: [
						{
							path: "SKILL.md",
							status: "modified",
							additions: 1,
							deletions: 1,
							patch: "@@ -1 +1 @@\n-old line\n+new line\n",
						},
					],
					flags: { scripts: false, hooks: false, allowed_tools: false },
				}}
				onOpenChange={() => {}}
				onDone={() => {}}
			/>
		) : null;
	};

	const renderViewer = (thirdParty = false) => {
		const source = mockSource({ now: () => NOW, tick: false, scenario: "skills_proposal" });
		render(
			<FixedNow value={NOW}>
				<ServicesProvider source={source}>
					<Harness thirdParty={thirdParty} />
				</ServicesProvider>
			</FixedNow>
		);
		return source;
	};

	it("shows the diff it's given", async () => {
		const source = renderViewer();
		const read = vi.spyOn(source, "skillProposalDiff");
		const viewer = within(await screen.findByRole("dialog"));
		expect(await viewer.findByText(/new line/)).toBeTruthy();
		expect(read).not.toHaveBeenCalled();
	});

	it("asks for a fork's name before approving a third party's skill", async () => {
		const source = renderViewer(true);
		const answer = vi.spyOn(source, "answerSkillProposal");
		const viewer = within(await screen.findByRole("dialog"));
		const approve = viewer.getByRole("button", { name: "Approve" }) as HTMLButtonElement;
		expect(approve.disabled).toBe(true);
		expect(viewer.getByText(/Name a fork/)).toBeTruthy();
		fireEvent.change(viewer.getByRole("textbox", { name: /Fork name/ }), {
			target: { value: "my-git-flow" },
		});
		expect(approve.disabled).toBe(false);
		fireEvent.click(approve);
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				GIT_FLOW_PROPOSAL,
				"approve",
				false,
				"my-git-flow",
				{ tree: "tree-read", base: "base-read" }
			)
		);
	});

	/** A viewer that reads its diff from `source`, with `change` made to its proposal. */
	const Reading = ({ change }: { change?: Partial<SkillProposal> }) => {
		const proposal = useView((s) => s.view.tables.skill_proposal.get(GIT_FLOW_PROPOSAL));
		return proposal ? (
			<ProposalViewer
				proposal={{ ...proposal, ...change }}
				onOpenChange={() => {}}
				onDone={() => {}}
			/>
		) : null;
	};

	const reading = (source: ReturnType<typeof mockSource>, change?: Partial<SkillProposal>) => (
		<FixedNow value={NOW}>
			<ServicesProvider source={source}>
				<Reading change={change} />
			</ServicesProvider>
		</FixedNow>
	);

	const newSource = () =>
		mockSource({ now: () => NOW, tick: false, scenario: "skills_proposal" });

	const shown = (diff: Partial<ProposalDiff>): ActionResult => ({
		status: "ok",
		data: {
			status: "skill_proposal_diff",
			diff: {
				files: [],
				flags: { scripts: false, hooks: false, allowed_tools: false },
				...diff,
			},
		},
	});

	const REPLACING = {
		tree: "tree-read",
		target: { name: "git-flow", source: null, base: "base-read" },
	};

	it("keeps Approve off until the diff is in, and sends what it showed", async () => {
		const source = newSource();
		let arrive: (result: ActionResult) => void = () => {};
		vi.spyOn(source, "skillProposalDiff").mockReturnValue(
			new Promise((resolve) => {
				arrive = resolve;
			})
		);
		const answer = vi.spyOn(source, "answerSkillProposal");
		render(reading(source));
		const viewer = within(await screen.findByRole("dialog"));
		const approve = viewer.getByRole("button", { name: "Approve" }) as HTMLButtonElement;
		const pushes = viewer.getByRole("button", {
			name: "Approve and push",
		}) as HTMLButtonElement;
		await viewer.findByText("Reading the change");
		expect(approve.disabled).toBe(true);
		expect(pushes.disabled).toBe(true);
		arrive(shown(REPLACING));
		await waitFor(() => expect(approve.disabled).toBe(false));
		fireEvent.click(approve);
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(GIT_FLOW_PROPOSAL, "approve", false, undefined, {
				tree: "tree-read",
				base: "base-read",
			})
		);
	});

	it("keeps Approve off when the diff can't be read, and Reject on", async () => {
		const source = newSource();
		vi.spyOn(source, "skillProposalDiff").mockRejectedValue(new Error("It's gone."));
		render(reading(source));
		const viewer = within(await screen.findByRole("dialog"));
		expect((await viewer.findByRole("alert")).textContent).toMatch(/It's gone/);
		const button = (name: string) => viewer.getByRole("button", { name }) as HTMLButtonElement;
		expect(button("Approve").disabled).toBe(true);
		expect(button("Reject").disabled).toBe(false);
	});

	it("says when a proposal that claims no base replaces a skill", async () => {
		const source = newSource();
		vi.spyOn(source, "skillProposalDiff").mockResolvedValue(shown(REPLACING));
		render(reading(source, { base: null }));
		const viewer = within(await screen.findByRole("dialog"));
		expect(await viewer.findByText(/^Replaces git-flow/)).toBeTruthy();
		expect(viewer.queryByText("New skill")).toBeNull();
	});

	it("reads the flags from the diff, not from what the proposal claims", async () => {
		const source = newSource();
		const flags = { scripts: true, hooks: false, allowed_tools: false };
		vi.spyOn(source, "skillProposalDiff").mockResolvedValue(shown({ flags }));
		render(reading(source, { flags: { ...flags, scripts: false } }));
		const viewer = within(await screen.findByRole("dialog"));
		expect(await viewer.findByText("Scripts")).toBeTruthy();
	});

	it("says so when the proposal changes while it's open, and reads it again", async () => {
		const source = newSource();
		const read = vi
			.spyOn(source, "skillProposalDiff")
			.mockResolvedValue(shown({ tree: "tree-read" }));
		const { rerender } = render(reading(source));
		const viewer = within(await screen.findByRole("dialog"));
		await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
		expect(viewer.queryByText(/changed since you opened it/)).toBeNull();
		rerender(reading(source, { tree: "tree-swapped" }));
		expect(await viewer.findByText(/changed since you opened it/)).toBeTruthy();
		await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
	});
});

describe("the sidebar's skill changes", () => {
	it("lists a proposal by its agent and opens it, and a reject takes it away", async () => {
		const source = renderApp("skills_proposal");
		const answer = vi.spyOn(source, "answerSkillProposal");
		const changes = within(await screen.findByRole("region", { name: "1 skill change" }));
		fireEvent.click(changes.getByRole("button", { name: /^Skill change from / }));
		const viewer = within(await screen.findByRole("dialog", { name: /^Skill change from / }));
		expect(await viewer.findByText("references/release.md")).toBeTruthy();
		fireEvent.click(viewer.getByRole("button", { name: "Reject" }));
		await waitFor(() =>
			expect(answer).toHaveBeenCalledWith(
				GIT_FLOW_PROPOSAL,
				"reject",
				false,
				undefined,
				undefined
			)
		);
		await waitFor(() =>
			expect(screen.queryByRole("region", { name: "1 skill change" })).toBeNull()
		);
	});
});
