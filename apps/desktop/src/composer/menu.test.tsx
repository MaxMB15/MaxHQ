// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import {
	type EffectiveSkill,
	NOTE_PILLS_SINCE,
	type NoteSummary,
	type NoteVersion,
} from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { type GitItemRef, type NoteRef, OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { clearSkillScans } from "@maxhq/shared/skills/scan";
import { agent, entities, link, NOW, project, workspace } from "@maxhq/shared/test/build";

import { agentDrops } from "./agent-drop";
import {
	AGENT,
	DEVICE,
	editorOf,
	findTextbox,
	installBrowserFakes,
	pillsIn,
	renderComposer,
	textIn,
	type,
} from "./testing";

import { setLanguageChoice } from "@maxhq/shared/i18n";
import { ComposerMenu } from "./menu";

beforeAll(installBrowserFakes);

const pr = (number: number, title: string): GitItemRef => ({
	type: "git_item",
	repo: "owner/repo",
	number,
	kind: "pull_request",
	title,
	state: "open",
	url: `https://github.com/owner/repo/pull/${number}`,
	head: "cache-key",
	base: "main",
	body: "",
	checked_at: NOW,
});

const other = agent({ id: "a2", title: "Sidebar sort", vendor: "codex" });

/** The composer for "Fix login", with its folder and one more agent. */
const setup = async () => {
	const composer = renderComposer();
	composer.source.findPaths.mockResolvedValue([
		{ path: "src/main.rs", kind: "file" },
		{ path: "src", kind: "folder" },
	]);
	const textbox = await findTextbox();
	composer.upsert(entities({ agents: [other], workspaces: [workspace({ id: "w1" })] }));
	return { ...composer, textbox };
};

const key = (textbox: HTMLElement, name: string) => fireEvent.keyDown(textbox, { key: name });

const menu = (name: RegExp) => screen.findByRole("listbox", { name });

const options = (list: HTMLElement) =>
	within(list)
		.getAllByRole("option")
		.map((option) => option.textContent);

describe("the @ menu", () => {
	it("uses the interface locale for its noun list", () => {
		setLanguageChoice("de");
		try {
			render(
				<ComposerMenu
					id="localized-menu"
					kind="path"
					contents={{ options: [], loading: false, notice: null }}
					active={0}
					onActive={() => {}}
					onPick={() => {}}
				/>
			);
			const label = screen.getByRole("listbox").getAttribute("aria-label");
			expect(label).toContain("Dateien");
			expect(label).toContain("Notizen");
		} finally {
			setLanguageChoice("en");
		}
	});

	it("lists files and folders, then the other agents in their own group", async () => {
		const { source, textbox } = await setup();
		await type(textbox, "Look at @");

		// This agent's maxd is too old for notes, so the menu doesn't offer them.
		const list = await menu(/^Files, folders and agents$/);
		await waitFor(() => expect(options(list)).toEqual(["src/main.rs", "src/", "Sidebar sort"]));
		expect(within(list).getByRole("group", { name: "Files and folders" })).toBeInTheDocument();
		expect(within(list).getByRole("group", { name: "Agents" })).toBeInTheDocument();
		expect(source.findPaths).toHaveBeenCalledWith(AGENT, "");
		expect(textbox).toHaveAttribute("aria-controls", list.id);
	});

	it("moves with the arrows, and Enter attaches the file from the agent's folder", async () => {
		const { source, textbox } = await setup();
		await type(textbox, "Look at @");
		const list = await menu(/^Files/);
		await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(3));

		const selected = () => within(list).getByRole("option", { selected: true });
		expect(selected()).toHaveTextContent("src/main.rs");
		key(textbox, "ArrowDown");
		expect(selected()).toHaveTextContent("src/");
		expect(textbox).toHaveAttribute("aria-activedescendant", selected().id);
		key(textbox, "ArrowUp");
		key(textbox, "Enter");

		// It stages by the path the search gave, from the folder it searched.
		await waitFor(() => expect(source.stageFound).toHaveBeenCalledWith(AGENT, ["src/main.rs"]));
		expect(source.stageAttachments).not.toHaveBeenCalled();
		expect(textIn(textbox)).toBe(`Look at ${MARK} `);
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("main.rs")]);
		expect(screen.queryByRole("listbox")).toBeNull();
	});

	it("attaches a folder from the agent's folder the same way", async () => {
		const { source, textbox } = await setup();
		await type(textbox, "Look at @");
		const list = await menu(/^Files/);
		fireEvent.click(await within(list).findByRole("option", { name: "src/" }));

		await waitFor(() => expect(source.stageFound).toHaveBeenCalledWith(AGENT, ["src"]));
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("src")]);
	});

	it("names another agent with a pill, and sends it", async () => {
		const { source, textbox } = await setup();
		await type(textbox, "Ask @side");
		const list = await menu(/^Files/);
		fireEvent.click(await within(list).findByRole("option", { name: "Sidebar sort" }));

		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Agent Sidebar sort, Codex/)])
		);
		await type(textbox, "first");
		key(textbox, "Enter");
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `Ask ${MARK} first`,
					attachments: [],
					refs: [
						{
							type: "agent",
							agent: "a2",
							title: "Sidebar sort",
							vendor: "codex",
							device: DEVICE,
							excerpt: null,
						},
					],
				},
				"queue"
			)
		);
	});

	it("closes on Escape and leaves the text", async () => {
		const { textbox } = await setup();
		await type(textbox, "Look at @src");
		await menu(/^Files/);
		key(textbox, "Escape");
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(textIn(textbox)).toBe("Look at @src");
		key(textbox, "Enter");
		await waitFor(() => expect(textIn(textbox)).toBe(""));
	});

	it("opens only at the start of a word, and never in code", async () => {
		const { source, textbox } = await setup();
		await type(textbox, "mail me@home");
		await type(textbox, " and `echo @src` too");
		act(() => {
			editorOf(textbox).commands.setTextSelection(28);
		});
		expect(textIn(textbox).slice(0, 27)).toBe("mail me@home and `echo @src");
		await new Promise((done) => setTimeout(done, 250));
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(source.findPaths).not.toHaveBeenCalled();
	});

	it("leaves / as text", async () => {
		const { textbox } = await setup();
		await type(textbox, "/review");
		expect(screen.queryByRole("listbox")).toBeNull();
	});
});

describe("notes in the @ menu", () => {
	const summary = (note: string, title: string): NoteSummary => ({
		note,
		title,
		head: `head-${note}`,
		at: NOW - 60_000,
		by: null,
		created_on: DEVICE,
		deleted: false,
		size: 10,
	});
	const steps: NoteRef = {
		type: "note",
		project: "p-a",
		note: "n1",
		title: "Release steps",
		copy: null,
	};
	const COPY = "att-Release steps.md";

	/** The composer for "Fix login", in Billing, with notes in Billing and Atlas. */
	const withNotes = async (protocol: number | null) => {
		const here = link({
			id: "mac",
			name: "Mac",
			device: DEVICE,
			route: { type: "local" },
			...(protocol === null ? {} : { protocol }),
		});
		const composer = renderComposer({ links: [here] });
		const textbox = await findTextbox();
		composer.upsert([
			...entities({
				projects: [project("p-a", "Atlas"), project("p-b", "Billing")],
				workspaces: [workspace({ id: "w1", project: "p-b" })],
			}),
			{
				entity: "project_notes",
				device: DEVICE,
				project: "p-a",
				notes: [summary("n1", "Release steps")],
			},
			{
				entity: "project_notes",
				device: DEVICE,
				project: "p-b",
				notes: [summary("n2", "Release notes")],
			},
		]);
		return { ...composer, textbox };
	};

	it("lists the agent's own project's notes first, and sends another's with a copy", async () => {
		const { source, textbox } = await withNotes(NOTE_PILLS_SINCE);
		const readNote = vi.fn<DataSource["readNote"]>(async () => ({
			status: "ok",
			data: {
				hash: "head-n1",
				project: "p-a",
				note: "n1",
				parents: [],
				title: "Release steps",
				body: "- Tag it\n",
				deleted: false,
				at: NOW,
				by: null,
			} as NoteVersion,
		}));
		Object.assign(source, { readNote });
		await type(textbox, "Follow @rel");

		const list = await menu(/^Files/);
		const notes = await within(list).findByRole("group", { name: "Notes" });
		expect(
			within(notes)
				.getAllByRole("option")
				.map((option) => option.textContent)
		).toEqual(["Release notes", "Release stepsAtlas"]);
		fireEvent.click(within(notes).getByRole("option", { name: /Release steps/ }));
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringMatching(/^Note Release steps, Atlas/)])
		);

		key(textbox, "Enter");
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{ text: `Follow ${MARK}`, attachments: [COPY], refs: [{ ...steps, copy: COPY }] },
				"queue"
			)
		);
		expect(readNote).toHaveBeenCalledWith(DEVICE, "p-a", "n1", null);
		expect(source.stagePasted).toHaveBeenCalledWith(
			DEVICE,
			"Release steps.md",
			"text/markdown",
			new TextEncoder().encode("- Tag it\n")
		);
	});

	it("says why a note can't be copied, and keeps the message", async () => {
		const { source, textbox } = await withNotes(NOTE_PILLS_SINCE);
		await type(textbox, "Follow @steps");
		const list = await menu(/^Files/);
		fireEvent.click(await within(list).findByRole("option", { name: /Release steps/ }));
		key(textbox, "Enter");

		expect(
			await screen.findByText(/^Couldn't copy the note Release steps\. No notes\./)
		).toBeInTheDocument();
		expect(source.sendInput).not.toHaveBeenCalled();
		expect(pillsIn(textbox)).toHaveLength(1);
	});

	it("leaves notes out for an older maxd, and turns a dropped one away", async () => {
		const { textbox } = await withNotes(NOTE_PILLS_SINCE - 1);
		await type(textbox, "Follow @rel");
		const list = await menu(/^Files/);
		await waitFor(() =>
			expect(within(list).queryByRole("group", { name: "Notes" })).toBeNull()
		);

		const elementFromPoint = document.elementFromPoint;
		document.elementFromPoint = () => textbox;
		try {
			act(() => {
				agentDrops.drop(steps, { x: 10, y: 10 });
			});
		} finally {
			document.elementFromPoint = elementFromPoint;
		}
		expect(
			await screen.findByText("maxd on Mac is older than this app; update it to send notes")
		).toBeInTheDocument();
		expect(pillsIn(textbox)).toEqual([]);
	});
});

describe("the # menu", () => {
	it("looks up only what you stopped typing at", async () => {
		const { source, textbox } = await setup();
		source.findGitItems.mockResolvedValue({
			repo: "owner/repo",
			items: [pr(412, "Cache the session key")],
			unavailable: null,
		});
		await type(textbox, "#4");
		await type(textbox, "1");
		await type(textbox, "2");

		const list = await menu(/^Pull requests and issues$/);
		await within(list).findByRole("option", { name: /Cache the session key/ });
		expect(source.findGitItems.mock.calls.map(([, query]) => query)).toEqual(
			expect.arrayContaining(["412"])
		);
		expect(source.findGitItems.mock.calls.every(([, query]) => query === "412")).toBe(true);
	});

	it("puts the number in on Tab, drawn as its pill at once", async () => {
		const { source, textbox } = await setup();
		source.findGitItems.mockResolvedValue({
			repo: "owner/repo",
			items: [pr(412, "Cache the session key"), pr(398, "Sort the sidebar")],
			unavailable: null,
		});
		await type(textbox, "See #");
		const list = await menu(/^Pull requests/);
		await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(2));
		fireEvent.keyDown(textbox, { key: "Tab" });

		expect(textIn(textbox)).toBe("See #412 ");
		const drawn = textbox.querySelector("[data-git-number]");
		expect(drawn).toHaveAttribute("data-git-number", "412");
		expect(drawn).toHaveTextContent("#412");
		expect(screen.queryByRole("listbox")).toBeNull();
	});

	it("says why it can't look anything up", async () => {
		const { source, textbox } = await setup();
		source.findGitItems.mockResolvedValue({
			repo: null,
			items: [],
			unavailable: "This agent's folder has no GitHub repository",
		});
		await type(textbox, "#");
		const list = await menu(/^Pull requests/);
		await waitFor(() =>
			expect(within(list).getByRole("status")).toHaveTextContent(
				"This agent's folder has no GitHub repository"
			)
		);
	});

	it.each([412, 1234])("keeps drawn number %s as text from inside it", async (number) => {
		const { source, textbox } = await setup();
		source.findGitItems.mockResolvedValue({
			repo: "owner/repo",
			items: [pr(number, "Cache the session key")],
			unavailable: null,
		});
		await type(textbox, `See #${number} now`);
		await waitFor(() => expect(textbox.querySelector("[data-git-number]")).not.toBeNull());

		// The caret goes into the number, where the digits can change.
		act(() => {
			editorOf(textbox).commands.setTextSelection(8);
		});
		const list = await menu(/^Pull requests/);
		fireEvent.click(
			await within(list).findByRole("option", { name: `Keep #${number} as text` })
		);

		expect(textbox.querySelector("[data-git-number]")).toBeNull();
		expect(textIn(textbox)).toBe(`See #${number} now`);
		expect(screen.queryByRole("listbox")).toBeNull();
	});
});

const skill = (over: Partial<EffectiveSkill> & { name: string }): EffectiveSkill => ({
	description: null,
	path: `/work/w1/.claude/skills/${over.name}/SKILL.md`,
	scope: null,
	enabled: true,
	display_name: null,
	short_description: null,
	user_invocation_only: null,
	user_invocable: null,
	place: "project",
	cli: "claude",
	mark: null,
	...over,
});

const scanned = (skills: EffectiveSkill[]): ActionResult => ({
	status: "ok",
	data: { status: "skills", skills },
});

/** The composer for "Fix login" on a Mac whose maxd scans skills. */
const setupSkills = async (skills = true) => {
	const composer = renderComposer({
		links: [
			link({
				id: "mac",
				device: DEVICE,
				capabilities: { transfers: true, tunnels: true, workspaces: true, skills },
			}),
		],
	});
	const scan = vi
		.spyOn(composer.source, "scanSkills")
		.mockResolvedValue(
			scanned([
				skill({ name: "git-flow", description: "Branch strategy" }),
				skill({ name: "unslop", place: "always", description: "Cut AI tells" }),
				skill({ name: "agent-only", user_invocable: false }),
				skill({ name: "codex-only", cli: "codex" }),
			])
		);
	const textbox = await findTextbox();
	composer.upsert(entities({ workspaces: [workspace({ id: "w1" })] }));
	return { ...composer, scan, textbox };
};

describe("the $ menu", () => {
	afterEach(clearSkillScans);

	it("scans the agent's workspace once it opens and lists what its CLI offers", async () => {
		const { scan, textbox } = await setupSkills();
		expect(scan).not.toHaveBeenCalled();
		await type(textbox, "Use $");

		const list = await menu(/^Skills$/);
		await waitFor(() =>
			expect(options(list)).toEqual(["Git FlowBranch strategy", "UnslopCut AI tells"])
		);
		expect(scan).toHaveBeenCalledWith(DEVICE, { type: "workspace", workspace: "w1" });
	});

	it("narrows as you type and puts the skill in as $name", async () => {
		const { textbox } = await setupSkills();
		await type(textbox, "Use $uns");
		const list = await menu(/^Skills$/);
		await waitFor(() => expect(options(list)).toEqual(["UnslopCut AI tells"]));
		key(textbox, "Enter");
		await waitFor(() => expect(textIn(textbox)).toBe("Use $unslop "));
		expect(screen.queryByRole("listbox")).toBeNull();
	});

	it("leaves money alone", async () => {
		const { scan, textbox } = await setupSkills();
		await type(textbox, "It costs $5");
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(scan).not.toHaveBeenCalled();
	});

	it("says so when the agent's device doesn't list skills", async () => {
		const { scan, textbox } = await setupSkills(false);
		await type(textbox, "$");
		const list = await menu(/^Skills$/);
		expect(within(list).getByRole("status")).toHaveTextContent(
			"This agent's device doesn't list skills"
		);
		expect(scan).not.toHaveBeenCalled();
	});
});
