// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { link, workspace } from "../test/build";
import {
	CHECKING_SSH,
	editorOptions,
	editorReach,
	LAST_USED,
	preferredEditor,
	readDefaultEditor,
	writeDefaultEditor,
} from "./open";

const CURSOR = { id: "cursor", label: "Cursor" };
const VSCODE = { id: "vscode", label: "VS Code" };

describe("editorReach", () => {
	const folder = workspace({ id: "ws-site", location: { type: "folder", path: "/src/site" } });

	it("opens a folder here as it is, and one elsewhere over its SSH host", () => {
		expect(editorReach(folder, link({ id: "mac", route: { type: "local" } }))).toEqual({
			kind: "local",
		});
		expect(
			editorReach(folder, link({ id: "devbox", route: { type: "ssh", host: "dev@devbox" } }))
		).toEqual({ kind: "remote", host: "dev@devbox" });
	});

	it("opens a WSL distro on this PC through the editor's WSL support", () => {
		const wsl = (via: string | null) =>
			editorReach(
				folder,
				link({ id: "ubuntu", route: { type: "wsl", distro: "Ubuntu", via } })
			);
		expect(wsl(null)).toEqual({ kind: "wsl", distro: "Ubuntu" });
		expect(wsl("dev@pc")).toEqual({
			kind: "unavailable",
			reason: "A WSL distro on another PC can't open in an editor from here yet",
		});
	});

	it("says why a device with no link can't open", () => {
		expect(editorReach(folder, undefined)).toEqual({
			kind: "unavailable",
			reason: "No route to this device",
		});
	});

	describe("a container", () => {
		const container = workspace({
			id: "ws-box",
			location: { type: "container", instance: "mhq-site", path: "/home/agent/work/site" },
		});
		const devbox = link({ id: "devbox" });
		const alias = "mhq-site.1a2b3c4d.maxhq";

		it("waits on its SSH setup before it opens", () => {
			expect(editorReach(container, devbox)).toEqual({
				kind: "unavailable",
				reason: CHECKING_SSH,
			});
		});

		it("goes over SSH to its alias once your config includes MaxHQ's", () => {
			expect(editorReach(container, devbox, { type: "included", alias })).toEqual({
				kind: "remote",
				host: alias,
			});
		});

		it("asks with the line to add, and after a no shows the entry to copy", () => {
			const line = 'Include "/Users/someone/.maxhq/ssh/config"';
			expect(editorReach(container, devbox, { type: "ask", alias, line })).toEqual({
				kind: "ask",
				host: alias,
				line,
			});
			const block = `Host ${alias}\n  User agent\n`;
			expect(editorReach(container, devbox, { type: "declined", alias, block })).toEqual({
				kind: "declined",
				host: alias,
				block,
			});
		});

		it("says why when no editor reaches it, link or not", () => {
			const unavailable = {
				type: "unavailable",
				reason: "The container isn't running",
			} as const;
			expect(editorReach(container, devbox, unavailable)).toEqual({
				kind: "unavailable",
				reason: "The container isn't running",
			});
			expect(editorReach(container, undefined, unavailable)).toEqual({
				kind: "unavailable",
				reason: "The container isn't running",
			});
		});
	});
});

describe("preferredEditor", () => {
	it("starts with the last editor used while this Mac has it, else the first", () => {
		expect(preferredEditor([CURSOR, VSCODE], "vscode")).toBe(VSCODE);
		expect(preferredEditor([CURSOR, VSCODE], "zed")).toBe(CURSOR);
		expect(preferredEditor([CURSOR, VSCODE], null)).toBe(CURSOR);
		expect(preferredEditor([], "cursor")).toBeNull();
	});

	it("uses a set editor over the last one, and falls back when this Mac lacks it", () => {
		const cursor = { kind: "editor", id: "cursor" } as const;
		expect(preferredEditor([CURSOR, VSCODE], "vscode", cursor)).toBe(CURSOR);
		expect(preferredEditor([VSCODE], "vscode", cursor)).toBe(VSCODE);
		expect(preferredEditor([VSCODE, CURSOR], null, { kind: "editor", id: "zed" })).toBe(VSCODE);
		expect(preferredEditor([CURSOR, VSCODE], "vscode", LAST_USED)).toBe(VSCODE);
	});

	it("uses none with the default None", () => {
		expect(preferredEditor([CURSOR, VSCODE], "vscode", { kind: "none" })).toBeNull();
	});
});

describe("editorOptions", () => {
	const known = [CURSOR, VSCODE];

	it("lists every editor the app knows and marks the ones this Mac lacks", () => {
		expect(editorOptions({ available: [VSCODE], known })).toEqual([
			{ ...CURSOR, found: false },
			{ ...VSCODE, found: true },
		]);
	});

	it("lists none until the app says", () => {
		expect(editorOptions(undefined)).toEqual([]);
	});
});

describe("the saved default editor", () => {
	it("reads back what it wrote", () => {
		for (const choice of [
			LAST_USED,
			{ kind: "none" },
			{ kind: "editor", id: "vscode" },
		] as const)
			expect(readDefaultEditor(writeDefaultEditor(choice))).toEqual(choice);
	});

	it("reads as Last used when nothing or nothing valid is saved", () => {
		for (const raw of [
			null,
			"not json",
			"null",
			"[]",
			'"none"',
			JSON.stringify({ version: 2, kind: "none" }),
			JSON.stringify({ version: 1, kind: "editor" }),
			JSON.stringify({ version: 1, kind: "editor", id: "" }),
			JSON.stringify({ version: 1, kind: "sometimes" }),
		])
			expect(readDefaultEditor(raw)).toEqual(LAST_USED);
	});
});
