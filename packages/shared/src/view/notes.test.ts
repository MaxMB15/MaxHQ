// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type DeviceLink,
	type Entity,
	NOTE_PILLS_SINCE,
	type NoteSummary,
	type NoteVersion,
	type NoteVersionInfo,
} from "../bindings";
import { formatWhen } from "../status/format";
import { agent, device, entities, link, MIN, NOW, project, workspace } from "../test/build";

import {
	firstLines,
	noteChoices,
	noteCopyName,
	notePillBlocked,
	type NotesReach,
	noteOn,
	notesEntry,
	notesOf,
	notesReach,
	noteSummary,
	projectsWithNotes,
	remoteNotesReach,
	savedBy,
	shownTitle,
	untitled,
	versionHeading,
	versionWhen,
} from "./notes";
import { applySnapshot } from "./replica";

const note = (over: Partial<NoteSummary> & { note: string }): NoteSummary => ({
	title: over.note,
	head: `head-${over.note}`,
	at: NOW - MIN,
	by: null,
	created_on: "dev-mac",
	deleted: false,
	size: 10,
	...over,
});

const notes = (dev: string, list: NoteSummary[], proj = "p-a"): Entity => ({
	entity: "project_notes",
	device: dev,
	project: proj,
	notes: list,
});

const MAC = link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } });
const BOX = link({ id: "box", name: "devbox", device: "dev-box" });

const viewOf = (lists: Entity[], links: DeviceLink[] = [MAC, BOX]) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				projects: [project("p-a", "Atlas"), project("p-b", "Billing")],
				devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
				links,
			}),
			...lists,
		],
	});

const titles = (lists: Entity[]) => notesOf(viewOf(lists), "p-a").map((row) => row.shown);

describe("notesOf", () => {
	it("joins every device's notes, keeping each one's newest report", () => {
		const view = viewOf([
			notes("dev-mac", [note({ note: "n1", title: "Old", at: NOW - 5 * MIN })]),
			notes("dev-box", [
				note({ note: "n1", title: "New", at: NOW - MIN }),
				note({ note: "n2", title: "Only here" }),
			]),
		]);
		const rows = notesOf(view, "p-a");
		expect(rows.map((r) => [r.note, r.shown, r.from])).toEqual([
			["n1", "New", "dev-box"],
			["n2", "Only here", "dev-box"],
		]);
	});

	it("breaks a tie on the time by device ID", () => {
		const view = viewOf([
			notes("dev-mac", [note({ note: "n1", title: "Mac's" })]),
			notes("dev-box", [note({ note: "n1", title: "Box's" })]),
		]);
		expect(notesOf(view, "p-a").map((r) => r.shown)).toEqual(["Box's"]);
	});

	it("drops a note whose newest report is a tombstone, but not an older one", () => {
		expect(
			titles([
				notes("dev-mac", [note({ note: "n1", deleted: true, at: NOW })]),
				notes("dev-box", [note({ note: "n1", at: NOW - 5 * MIN })]),
			])
		).toEqual([]);
		expect(
			titles([
				notes("dev-mac", [note({ note: "n1", deleted: true, at: NOW - 5 * MIN })]),
				notes("dev-box", [note({ note: "n1", title: "Back", at: NOW })]),
			])
		).toEqual(["Back"]);
	});

	it("sorts by title in natural order", () => {
		expect(
			titles([
				notes("dev-mac", [
					note({ note: "a", title: "Untitled 10" }),
					note({ note: "b", title: "untitled 2" }),
					note({ note: "c", title: "Design" }),
					note({ note: "d", title: "Untitled" }),
				]),
			])
		).toEqual(["Design", "Untitled", "untitled 2", "Untitled 10"]);
	});

	it("tells notes with the same title apart by device, or by ID on the same device", () => {
		expect(
			titles([
				notes("dev-mac", [
					note({ note: "n1", title: "Plan", created_on: "dev-mac" }),
					note({ note: "n2", title: "Plan", created_on: "dev-box" }),
				]),
			])
		).toEqual(["Plan (devbox)", "Plan (Mac)"]);
		expect(
			titles([
				notes("dev-mac", [
					note({ note: "0198aaaa-1111", title: "Plan" }),
					note({ note: "0198bbbb-2222", title: "Plan" }),
				]),
			])
		).toEqual(["Plan (0198aaaa)", "Plan (0198bbbb)"]);
	});

	it("keeps other projects' notes out", () => {
		expect(titles([notes("dev-mac", [note({ note: "n1" })], "p-b")])).toEqual([]);
	});
});

describe("noteSummary", () => {
	it("gives the newest report, a tombstone too", () => {
		const view = viewOf([notes("dev-mac", [note({ note: "n1", deleted: true })])]);
		expect(noteSummary(view, "p-a", "n1")?.deleted).toBe(true);
		expect(noteSummary(view, "p-a", "n2")).toBeUndefined();
	});
});

describe("noteOn", () => {
	it("gives one device's report of a note", () => {
		const view = viewOf([
			notes("dev-mac", [note({ note: "n1", head: "old" })]),
			notes("dev-box", [note({ note: "n1", head: "new", at: NOW })]),
		]);
		expect(noteOn(view, "dev-mac", "p-a", "n1")?.head).toBe("old");
		expect(noteOn(view, "dev-box", "p-b", "n1")).toBeUndefined();
	});
});

describe("projectsWithNotes", () => {
	it("names the projects with a live note on some device", () => {
		const view = viewOf([
			notes("dev-mac", [note({ note: "n1" })]),
			notes("dev-mac", [note({ note: "n2", deleted: true })], "p-b"),
		]);
		expect([...projectsWithNotes(view)]).toEqual(["p-a"]);
	});
});

describe("untitled", () => {
	it("takes the first free Untitled title", () => {
		expect(untitled(viewOf([]), "p-a")).toBe("Untitled");
		const view = viewOf([
			notes("dev-mac", [
				note({ note: "a", title: "Untitled" }),
				note({ note: "b", title: "Untitled 3" }),
			]),
		]);
		expect(untitled(view, "p-a")).toBe("Untitled 2");
	});
});

describe("notesReach", () => {
	const lists = [notes("dev-box", [note({ note: "n1" })])];
	const down = { state: "unreachable" as const, reason: "auth failed", last_seen: null };

	it("goes through this Mac while it's connected", () => {
		expect(notesReach(viewOf(lists), "p-a")).toEqual({
			device: "dev-mac",
			readOnly: false,
			why: null,
		});
	});

	it("falls back to a connected device that holds the notes, read-only", () => {
		const reach = notesReach(viewOf(lists, [{ ...MAC, state: down }, BOX]), "p-a");
		expect(reach.device).toBe("dev-box");
		expect(reach.readOnly).toBe(true);
		expect(reach.why).toContain("devbox's copy");
	});

	it("reaches nothing when no holder is connected", () => {
		const reach = notesReach(
			viewOf(lists, [
				{ ...MAC, state: down },
				{ ...BOX, state: down },
			]),
			"p-a"
		);
		expect(reach.device).toBeNull();
		expect(reach.why).toContain("no device that has these notes");
	});
});

describe("remoteNotesReach", () => {
	const down = { state: "unreachable" as const, reason: "auth failed", last_seen: null };
	const withHome = (links: DeviceLink[], lists: Entity[] = []) =>
		applySnapshot({
			seq: 1,
			entities: [
				...entities({
					projects: [project("p-a", "Atlas")],
					devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
					links,
					workspaces: [workspace({ id: "w1", project: "p-a", device: "dev-box" })],
				}),
				...lists,
			],
		});

	it("writes through a connected device that holds the notes", () => {
		const view = withHome([MAC, BOX], [notes("dev-mac", [note({ note: "n1" })])]);
		expect(remoteNotesReach(view, "p-a")).toEqual({
			device: "dev-mac",
			readOnly: false,
			why: null,
		});
	});

	it("falls back to a connected device with a workspace in the project", () => {
		expect(remoteNotesReach(withHome([MAC, BOX]), "p-a").device).toBe("dev-box");
		const lists = [notes("dev-mac", [note({ note: "n1" })])];
		expect(remoteNotesReach(withHome([{ ...MAC, state: down }, BOX], lists), "p-a")).toEqual({
			device: "dev-box",
			readOnly: false,
			why: null,
		});
	});

	it("reaches nothing when no device with the project is connected", () => {
		expect(remoteNotesReach(withHome([MAC, { ...BOX, state: down }]), "p-a")).toEqual({
			device: null,
			readOnly: true,
			why: "No device that has this project is connected.",
		});
	});
});

describe("notesEntry", () => {
	const writable: NotesReach = { device: "dev-box", readOnly: false, why: null };
	const readOnly: NotesReach = { device: null, readOnly: true, why: "No device." };

	it("shows with its live notes, and a plus while a note can be made", () => {
		const view = viewOf([notes("dev-box", [note({ note: "n1" }), note({ note: "n2" })])]);
		expect(notesEntry(view, "p-a", writable)).toEqual({ count: 2, writable: true });
		expect(notesEntry(view, "p-a", readOnly)).toEqual({ count: 2, writable: false });
	});

	it("shows an empty entry only where a note can be made", () => {
		const view = viewOf([]);
		expect(notesEntry(view, "p-a", writable)).toEqual({ count: 0, writable: true });
		expect(notesEntry(view, "p-a", readOnly)).toBeNull();
	});
});

describe("savedBy", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "dev-box", name: "devbox" })],
			agents: [agent({ id: "a1", title: "Fixer" })],
		}),
	});

	it("names the device, the agent on it, or a merge", () => {
		expect(savedBy(view, { device: "dev-box", agent: null })).toBe("devbox");
		expect(savedBy(view, { device: "dev-box", agent: "a1" })).toBe("Fixer on devbox");
		expect(savedBy(view, { device: "dev-box", agent: "gone" })).toBe("An agent on devbox");
		expect(savedBy(view, null)).toBe("Merged");
	});
});

describe("shownTitle", () => {
	it("calls a blank title Untitled", () => {
		expect(shownTitle("Plan")).toBe("Plan");
		expect(shownTitle("  ")).toBe("Untitled");
	});
});

describe("versionWhen", () => {
	const info = (over: Partial<NoteVersionInfo> = {}): NoteVersionInfo => ({
		hash: "h1",
		parents: [],
		title: "Plan",
		deleted: false,
		at: NOW - 5 * MIN,
		by: null,
		size: 10,
		...over,
	});
	const when = formatWhen(NOW - 5 * MIN, NOW);

	it("marks the current version", () => {
		expect(versionWhen(info(), true, NOW)).toBe(`${when} · Current`);
	});

	it("marks a deleted one, unless it's current", () => {
		expect(versionWhen(info({ deleted: true }), false, NOW)).toBe(`${when} · Deleted`);
		expect(versionWhen(info({ deleted: true }), true, NOW)).toBe(`${when} · Current`);
	});

	it("gives only the time for any other", () => {
		expect(versionWhen(info(), false, NOW)).toBe(when);
	});
});

describe("versionHeading", () => {
	it("names the version's title and when it was saved", () => {
		const version: NoteVersion = {
			hash: "h1",
			project: "p-a",
			note: "n1",
			parents: [],
			title: "Plan",
			body: "Body",
			deleted: false,
			at: NOW - 5 * MIN,
			by: null,
		};
		expect(versionHeading(version, NOW)).toBe(`"Plan" from ${formatWhen(NOW - 5 * MIN, NOW)}`);
	});
});

describe("notePillBlocked", () => {
	it("lets a maxd that knows note pills take them", () => {
		const view = viewOf([], [{ ...MAC, protocol: NOTE_PILLS_SINCE }, BOX]);
		expect(notePillBlocked(view, "dev-mac")).toBeNull();
	});

	it("says why an older maxd, or one whose version isn't known yet, can't", () => {
		const view = viewOf([], [MAC, { ...BOX, protocol: NOTE_PILLS_SINCE - 1 }]);
		expect(notePillBlocked(view, "dev-box")).toBe(
			"maxd on devbox is older than this app; update it to send notes"
		);
		expect(notePillBlocked(view, "dev-mac")).not.toBeNull();
	});
});

describe("firstLines", () => {
	it("takes the first lines with text, trimmed", () => {
		expect(firstLines("\n# Plan \n\n- one\n  - two\n- three\n- four", 3)).toEqual([
			"# Plan",
			"- one",
			"- two",
		]);
		expect(firstLines("  \n")).toEqual([]);
	});
});

describe("noteCopyName", () => {
	it("makes a Markdown file name out of the title", () => {
		expect(noteCopyName("GTFS release steps")).toBe("GTFS release steps.md");
		expect(noteCopyName("a/b: c?")).toBe("a b c.md");
		expect(noteCopyName("..hidden")).toBe("hidden.md");
		expect(noteCopyName("")).toBe("Untitled.md");
		expect(noteCopyName("///")).toBe("Note.md");
		expect(noteCopyName("x".repeat(100))).toBe(`${"x".repeat(60)}.md`);
	});
});

describe("noteChoices", () => {
	const view = viewOf([
		notes("dev-mac", [note({ note: "n1", title: "Release steps" })], "p-a"),
		notes(
			"dev-mac",
			[note({ note: "n2", title: "Release notes" }), note({ note: "n3", title: "Plan" })],
			"p-b"
		),
	]);

	it("puts the agent's own project's notes first and matches the title", () => {
		const choices = noteChoices(view, "p-b", "release");
		expect(choices.map((choice) => [choice.shown, choice.projectName, choice.own])).toEqual([
			["Release notes", "Billing", true],
			["Release steps", "Atlas", false],
		]);
		expect(choices[1]!.ref).toEqual({
			type: "note",
			project: "p-a",
			note: "n1",
			title: "Release steps",
			copy: null,
		});
	});

	it("lists every project's notes by project name for an agent without one", () => {
		expect(noteChoices(view, null, "").map((choice) => choice.shown)).toEqual([
			"Release steps",
			"Plan",
			"Release notes",
		]);
	});
});
