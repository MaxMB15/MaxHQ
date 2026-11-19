// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AttachmentRef } from "../bindings";
import type { ClipboardFragment } from "../pills/clipboard";
import { agent, entities, NOW } from "../test/build";
import { applyDelta, applySnapshot, EMPTY_VIEW } from "../view/replica";
import { createViewStore } from "../view/store";

import {
	createSavedDrafts,
	type DraftStorage,
	draftText,
	dropGoneDrafts,
	goneAgents,
	MAX_SAVED_CHARS,
	SAVED_FOR_MS,
	STAGED_FOR_MS,
	textDraft,
} from "./saved-drafts";

let clock = NOW;

beforeEach(() => {
	vi.useFakeTimers();
	clock = NOW;
});

afterEach(() => {
	vi.useRealTimers();
});

/** Storage in memory that counts its reads and writes. */
const memory = (text: string | null = null) => {
	const held = { text, reads: 0, writes: 0 };
	const storage: DraftStorage = {
		read: () => {
			held.reads += 1;
			return held.text;
		},
		write: (next) => {
			held.text = next;
			held.writes += 1;
		},
	};
	return { held, storage };
};

const drafts = (storage: DraftStorage) => createSavedDrafts(storage, { now: () => clock });

/** A staged file's ID as maxd makes one at `at`, a UUIDv7. */
const stagedId = (at: number) => {
	const hex = at.toString(16).padStart(12, "0");
	return `${hex.slice(0, 8)}-${hex.slice(8)}-7abc-8def-0123456789ab`;
};

const ref = (id: string): AttachmentRef => ({
	id,
	name: "shot.png",
	kind: "image",
	bytes: 10,
	mime: "image/png",
	path: "/staged/shot.png",
	thumbnail: "data:image/png;base64,AAAA",
});

/** A draft with words around a file, staged with the ID `id`. */
const withFile = (words = "Look at ", id = stagedId(NOW)): ClipboardFragment => ({
	version: 1,
	origin: "composer-a",
	segments: [
		{ type: "text", text: words },
		{ type: "pill", pill: { type: "attachment", key: "file-1" } },
		{ type: "text", text: " please" },
	],
	files: [
		{
			key: "file-1",
			name: "shot.png",
			kind: "image",
			source: "/Users/me/shot.png",
			ref: ref(id),
			device: "device-mac",
		},
	],
	gitItems: [],
});

describe("saved drafts", () => {
	it("come back in the next run as they were typed, pills and files too", () => {
		const { storage } = memory();
		drafts(storage).save("agent-a", withFile());
		vi.advanceTimersByTime(300);
		expect(drafts(storage).get("agent-a")).toEqual(withFile());
		expect(drafts(storage).get("agent-b")).toBeNull();
	});

	it("write once a wait while you type, and answer from what waits", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		for (const words of ["L", "Lo", "Loo", "Look "]) saved.save("agent-a", withFile(words));
		expect(held.writes).toBe(0);
		expect(saved.get("agent-a")).toEqual(withFile("Look "));
		vi.advanceTimersByTime(300);
		expect(held.writes).toBe(1);
		expect(drafts(storage).get("agent-a")).toEqual(withFile("Look "));
	});

	it("go at once when sent or emptied, even before they're written", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		saved.save("agent-a", withFile());
		saved.save("agent-b", withFile());
		vi.advanceTimersByTime(300);

		saved.save("agent-a", textDraft("  \n"));
		expect(held.writes).toBe(2);
		expect(drafts(storage).get("agent-a")).toBeNull();

		saved.save("agent-c", withFile());
		saved.drop(["agent-b", "agent-c"]);
		expect(saved.get("agent-c")).toBeNull();
		vi.advanceTimersByTime(300);
		expect(drafts(storage).get("agent-b")).toBeNull();
		expect(drafts(storage).get("agent-c")).toBeNull();
	});

	it("drop a draft from an earlier run, and write nothing for one they never held", () => {
		const { held, storage } = memory();
		drafts(storage).save("agent-a", withFile());
		vi.advanceTimersByTime(300);

		const next = drafts(storage);
		next.drop(["agent-a"]);
		expect(drafts(storage).get("agent-a")).toBeNull();
		const { reads, writes } = held;
		next.save("agent-b", null);
		next.drop(["agent-c"]);
		expect(held).toMatchObject({ reads, writes });
	});

	it("keep the drafts another window wrote", () => {
		const { storage } = memory();
		const one = drafts(storage);
		const two = drafts(storage);
		one.save("agent-a", withFile("One "));
		two.save("agent-b", withFile("Two "));
		vi.advanceTimersByTime(300);
		expect(drafts(storage).get("agent-a")).toEqual(withFile("One "));
		expect(drafts(storage).get("agent-b")).toEqual(withFile("Two "));
	});

	it("forget a draft left alone for 30 days", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		saved.save("agent-a", withFile());
		saved.flush();
		clock += SAVED_FOR_MS + 1;
		expect(saved.get("agent-a")).toBeNull();
		saved.save("agent-b", withFile());
		saved.flush();
		expect(held.text).not.toContain("agent-a");
		expect(held.text).toContain("agent-b");
	});

	it("keep the newest drafts that fit the cap", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		const big = textDraft("x".repeat(MAX_SAVED_CHARS * 0.4));
		for (const agent of ["agent-a", "agent-b", "agent-c"]) {
			saved.save(agent, big);
			clock += 1;
		}
		saved.flush();
		expect(held.text!.length).toBeLessThanOrEqual(MAX_SAVED_CHARS + 100);
		const next = drafts(storage);
		expect(next.get("agent-a")).toBeNull();
		expect(next.get("agent-b")).toEqual(big);
		expect(next.get("agent-c")).toEqual(big);
	});

	it("bring a file back as its marker once maxd may have deleted its staged copy", () => {
		const { storage } = memory();
		const unstaged = withFile();
		const saved = drafts(storage);
		saved.save("agent-a", withFile());
		saved.save("agent-b", withFile("Look at ", "not-a-uuid"));
		saved.save("agent-c", { ...unstaged, files: [{ ...unstaged.files[0]!, ref: null }] });
		vi.advanceTimersByTime(300);
		const file = (agent: string) => drafts(storage).get(agent)?.files[0];
		const marker = { ...withFile().files[0], ref: null, device: null, source: null };

		clock = NOW + STAGED_FOR_MS - 1;
		expect(file("agent-a")).toEqual(withFile().files[0]);
		clock = NOW + STAGED_FOR_MS;
		expect(file("agent-a")).toEqual(marker);
		// An ID that carries no time is never used again, and neither is a
		// path from an earlier run.
		clock = NOW;
		expect(file("agent-b")).toEqual(marker);
		expect(file("agent-c")).toEqual(marker);
	});

	it("read a garbled or unknown store as none, and keep the drafts that check out", () => {
		for (const text of ["not json", "[]", '{"version":2,"drafts":{}}', '{"version":1}']) {
			expect(drafts(memory(text).storage).get("agent-a")).toBeNull();
		}
		const mixed = JSON.stringify({
			version: 1,
			drafts: {
				"agent-a": { at: NOW, fragment: withFile() },
				"agent-b": { at: "now", fragment: withFile() },
				"agent-c": { at: NOW, fragment: { version: 1, segments: "oops" } },
			},
		});
		const saved = drafts(memory(mixed).storage);
		expect(saved.get("agent-a")).toEqual(withFile());
		expect(saved.get("agent-b")).toBeNull();
		expect(saved.get("agent-c")).toBeNull();
	});

	it("keep going when storage throws", () => {
		const broken: DraftStorage = {
			read: () => {
				throw new Error("no storage");
			},
			write: () => {
				throw new Error("full");
			},
		};
		const saved = drafts(broken);
		saved.save("agent-a", withFile());
		expect(saved.get("agent-a")).toEqual(withFile());
		expect(() => vi.advanceTimersByTime(300)).not.toThrow();
		expect(saved.get("agent-a")).toBeNull();
		expect(() => saved.drop(["agent-a"])).not.toThrow();
	});
});

describe("the session cards each composer placed", () => {
	it("come back in the next run, and stay when the draft empties", () => {
		const { storage } = memory();
		const saved = drafts(storage);
		saved.save("agent-a", withFile());
		saved.setPlaced("agent-a", new Set(["card-1"]));
		expect(saved.placed("agent-a")).toEqual(new Set(["card-1"]));
		vi.advanceTimersByTime(300);

		saved.save("agent-a", null);
		const next = drafts(storage);
		expect(next.get("agent-a")).toBeNull();
		expect(next.placed("agent-a")).toEqual(new Set(["card-1"]));
		expect(next.placed("agent-b")).toEqual(new Set());
	});

	it("go with the agent, and after 30 days", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		saved.setPlaced("agent-a", new Set(["card-1"]));
		saved.setPlaced("agent-b", new Set(["card-2"]));
		saved.flush();

		drafts(storage).drop(["agent-a"]);
		expect(drafts(storage).placed("agent-a")).toEqual(new Set());
		expect(drafts(storage).placed("agent-b")).toEqual(new Set(["card-2"]));

		clock += SAVED_FOR_MS + 1;
		expect(drafts(storage).placed("agent-b")).toEqual(new Set());
		const later = drafts(storage);
		later.setPlaced("agent-c", new Set(["card-3"]));
		later.flush();
		expect(held.text).not.toContain("card-2");
	});

	it("keep only the last few, and none once emptied", () => {
		const { held, storage } = memory();
		const saved = drafts(storage);
		const cards = Array.from({ length: 20 }, (_, index) => `card-${index}`);
		saved.setPlaced("agent-a", new Set(cards));
		saved.flush();
		expect([...drafts(storage).placed("agent-a")]).toEqual(cards.slice(-16));

		saved.setPlaced("agent-a", new Set());
		saved.flush();
		expect(held.text).not.toContain("agent-a");
	});

	it("keep the ones another window placed, and read garbled ones as none", () => {
		const { storage } = memory();
		const one = drafts(storage);
		const two = drafts(storage);
		one.setPlaced("agent-a", new Set(["card-1"]));
		two.save("agent-b", withFile());
		vi.advanceTimersByTime(300);
		expect(drafts(storage).placed("agent-a")).toEqual(new Set(["card-1"]));
		expect(drafts(storage).get("agent-b")).toEqual(withFile());

		const mixed = JSON.stringify({
			version: 1,
			placed: {
				"agent-a": { at: NOW, items: ["card-1"] },
				"agent-b": { at: NOW, items: [7] },
				"agent-c": { at: "now", items: ["card-3"] },
			},
		});
		const saved = drafts(memory(mixed).storage);
		expect(saved.placed("agent-a")).toEqual(new Set(["card-1"]));
		expect(saved.placed("agent-b")).toEqual(new Set());
		expect(saved.placed("agent-c")).toEqual(new Set());
		expect(saved.get("agent-a")).toBeNull();
	});
});

describe("the drafts of deleted agents", () => {
	const loaded = (seq: number, ...ids: string[]) =>
		applySnapshot({ seq, entities: entities({ agents: ids.map((id) => agent({ id })) }) });

	it("are the agents the view loses once it has loaded", () => {
		const before = loaded(4, "agent-a", "agent-b");
		const removed = applyDelta(before, {
			seq: 5,
			changes: [{ op: "remove", key: { entity: "agent", id: "agent-b" } }],
		});
		if (removed.kind !== "applied") throw new Error("the delta didn't apply");
		expect(goneAgents(before, removed.view)).toEqual(["agent-b"]);
		expect(goneAgents(before, loaded(9, "agent-a"))).toEqual(["agent-b"]);
		expect(goneAgents(EMPTY_VIEW, before)).toEqual([]);
		expect(goneAgents(before, EMPTY_VIEW)).toEqual([]);
	});

	it("go when the view loses the agent", () => {
		const { storage } = memory();
		const saved = drafts(storage);
		const view = createViewStore();
		const stop = dropGoneDrafts(view, saved);
		saved.save("agent-a", withFile());
		saved.save("agent-b", withFile());
		saved.flush();

		view.setState({ view: loaded(4, "agent-a", "agent-b") });
		expect(saved.get("agent-a")).not.toBeNull();
		view.setState({ view: loaded(5, "agent-a") });
		expect(drafts(storage).get("agent-a")).not.toBeNull();
		expect(drafts(storage).get("agent-b")).toBeNull();

		stop();
		view.setState({ view: loaded(6) });
		expect(drafts(storage).get("agent-a")).not.toBeNull();
	});
});

describe("the phone's drafts", () => {
	it("are plain text, with each pill as its marker", () => {
		expect(draftText(textDraft("Hello"))).toBe("Hello");
		expect(draftText(withFile())).toBe("Look at [Image 1: shot.png] please");
		expect(draftText(null)).toBe("");
	});
});
