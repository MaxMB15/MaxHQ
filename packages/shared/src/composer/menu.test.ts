// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentId, NoteId, ProjectId } from "../bindings";
import type { AgentRef, GitItemRef } from "../pills/draft";
import type { SkillFeed } from "../skills/feed";
import { agent } from "../test/build";
import type { NoteChoice } from "../view/notes";

import {
	FINDS_EVERYTHING,
	groupName,
	menuContents,
	type MenuInputs,
	menuGroups,
	menuLabel,
	menuLooks,
	type MenuOption,
	menuPick,
	menuStatus,
	menuWord,
	optionText,
} from "./menu";

const SKILLS: SkillFeed = { cli: "claude", skills: [], loading: false, notice: null };

const inputs = (over: Partial<MenuInputs> = {}): MenuInputs => ({
	agent: "a1" as AgentId,
	agents: [agent({ id: "a1", title: "Self" }), agent({ id: "a2", title: "Usage meter" })],
	trigger: { kind: "path", query: "" },
	keep: null,
	skills: SKILLS,
	...over,
});

const item: GitItemRef = {
	type: "git_item",
	repo: "owner/repo",
	number: 412,
	kind: "pull_request",
	state: "open",
	title: "Fix the cache",
	url: "https://example.com/pull/412",
} as GitItemRef;

const note: NoteChoice = {
	ref: {
		type: "note",
		project: "p1" as ProjectId,
		note: "n1" as NoteId,
		title: "Release steps",
		copy: null,
	} as NoteChoice["ref"],
	shown: "Release steps",
	projectName: "Other",
	own: false,
};

describe("menuLooks", () => {
	it("looks up files only for a field that takes them, and nothing without an agent", () => {
		expect(menuLooks(inputs())).toBe(true);
		expect(menuLooks(inputs({ files: false }))).toBe(false);
		expect(menuLooks(inputs({ agent: null }))).toBe(false);
		expect(menuLooks(inputs({ trigger: { kind: "git-item", query: "4" } }))).toBe(true);
	});
});

describe("menuContents", () => {
	it("offers found paths, other agents and notes for @", () => {
		const contents = menuContents(inputs({ notes: [note] }), {
			kind: "path",
			paths: [{ path: "src/main.rs", kind: "file" }] as never,
		});
		expect(contents.options.map((option) => option.kind)).toEqual(["path", "agent", "note"]);
		expect(contents.loading).toBe(false);
	});

	it("waits for the first answer", () => {
		expect(menuContents(inputs(), null).loading).toBe(true);
	});

	it("says when the folder couldn't be searched", () => {
		const contents = menuContents(inputs(), { kind: "failed", for: "path" });
		expect(contents.notice).toBe("Couldn't search this agent's folder");
	});

	it("offers the found pull requests and Keep as text for #", () => {
		const contents = menuContents(
			inputs({ trigger: { kind: "git-item", query: "412" }, keep: 412 }),
			{
				kind: "git-item",
				search: { repo: "owner/repo", items: [item], unavailable: null },
			}
		);
		expect(contents.options).toEqual([
			{ kind: "git", item },
			{ kind: "keep", number: 412 },
		]);
	});

	it("offers nothing with no menu open", () => {
		expect(menuContents(inputs({ trigger: null }), null).options).toEqual([]);
	});
});

describe("menuPick", () => {
	const pill: AgentRef = {
		type: "agent",
		agent: "a2" as AgentId,
		title: "Usage meter",
		vendor: "claude",
		device: "d1",
		excerpt: null,
	} as AgentRef;
	const ways = { onPath: () => [], agentPill: () => pill };

	it("puts an agent's pill in", () => {
		const option: MenuOption = { kind: "agent", agent: agent({ id: "a2" }) };
		expect(menuPick(option, ways).segments).toEqual([{ type: "pill", pill }]);
	});

	it("puts a number in as text, spaced, and seeds its pill", () => {
		const chosen = menuPick({ kind: "git", item }, ways);
		expect(chosen).toMatchObject({
			segments: [{ type: "text", text: "#412" }],
			spaced: true,
			seed: item,
		});
	});

	it("asks the box for a picked folder's segments, with its slash", () => {
		const asked: string[] = [];
		const onPath = (path: string) => {
			asked.push(path);
			return [{ type: "text" as const, text: path }];
		};
		menuPick({ kind: "path", path: "src", folder: true }, { ...ways, onPath });
		expect(asked).toEqual(["src/"]);
	});

	it("puts nothing in for a field that takes no files", () => {
		const chosen = menuPick(
			{ kind: "path", path: "a", folder: false },
			{ ...ways, onPath: null }
		);
		expect(chosen.segments).toBeNull();
	});

	it("keeps a number as text", () => {
		expect(menuPick({ kind: "keep", number: 7 }, ways)).toMatchObject({
			segments: null,
			keep: 7,
		});
	});
});

describe("menu words", () => {
	it("names the word at the caret so a closed menu stays shut", () => {
		expect(menuWord("see @src", 8)).toBe("4:@src");
		expect(menuWord("see src", 7)).toBeNull();
		expect(menuWord("see @src", null)).toBeNull();
	});

	it("labels options and groups them by kind", () => {
		const options: MenuOption[] = [
			{ kind: "path", path: "src", folder: true },
			{ kind: "path", path: "a.ts", folder: false },
			{ kind: "note", note },
		];
		expect(optionText(options[0]!)).toEqual({ label: "src/" });
		expect(optionText({ kind: "note", note })).toEqual({
			label: "Release steps",
			detail: "Other",
		});
		expect(menuGroups(options).map((group) => [group.kind, group.start])).toEqual([
			["path", 0],
			["note", 2],
		]);
		expect(groupName("git")).toBe("Pull requests and issues");
	});

	it("says what an empty menu found", () => {
		const empty = { options: [], loading: false, notice: null };
		expect(menuStatus("path", empty, { files: false, notes: false })).toBe("No agents match");
		expect(menuStatus("skill", { ...empty, loading: true })).toBe("Searching");
		expect(menuLabel("path", FINDS_EVERYTHING)).toBe("Files, folders, agents and notes");
	});
});
