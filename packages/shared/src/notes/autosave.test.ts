// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteVersion } from "../bindings";
import type { NoteSave, SaveNoteResult } from "../data/source";

import { createAutosave, IDLE_MS, incomingText, MAX_WAIT_MS, saveText } from "./autosave";

const version = (hash: string, body: string, over: Partial<NoteVersion> = {}): NoteVersion => ({
	hash,
	project: "p-a",
	note: "n1",
	parents: [],
	title: "Plan",
	body,
	deleted: false,
	at: 0,
	by: null,
	...over,
});

/** A device that keeps what it's sent, or answers with `answer` when given. */
const device = () => {
	const sent: NoteSave[] = [];
	let count = 0;
	let answer: ((save: NoteSave) => SaveNoteResult) | null = null;
	let hold: Promise<void> | null = null;
	const save = vi.fn(async (save: NoteSave): Promise<SaveNoteResult> => {
		sent.push(save);
		if (hold) await hold;
		if (answer) return answer(save);
		count += 1;
		return {
			status: "ok",
			data: {
				version: version(`v${count}`, save.body, {
					title: save.title,
					deleted: save.deleted,
				}),
				merged: false,
			},
		};
	});
	return {
		sent,
		save,
		answerWith: (next: typeof answer) => {
			answer = next;
		},
		holdUntil: (gate: Promise<void>) => {
			hold = gate;
		},
	};
};

const start = (initial = version("v0", "one\n"), read = async () => null as NoteVersion | null) => {
	const box = device();
	const incoming = vi.fn();
	const saving = createAutosave({ initial, save: box.save, read });
	saving.follow(incoming);
	return { box, incoming, saving };
};

beforeEach(() => {
	vi.useFakeTimers();
});
afterEach(() => {
	vi.useRealTimers();
});

describe("createAutosave", () => {
	it("saves a second after the last change, from the version it was edited from", async () => {
		const { box, saving } = start();
		saving.edit({ body: "one\ntwo\n" });
		expect(saving.snapshot()).toMatchObject({ unsaved: true, save: { kind: "saving" } });
		await vi.advanceTimersByTimeAsync(IDLE_MS - 1);
		saving.edit({ body: "one\ntwo\nthree\n" });
		await vi.advanceTimersByTimeAsync(IDLE_MS - 1);
		expect(box.sent).toEqual([]);
		await vi.advanceTimersByTimeAsync(1);
		expect(box.sent).toEqual([
			{ base: "v0", title: "Plan", body: "one\ntwo\nthree\n", deleted: false },
		]);
		expect(saving.snapshot()).toMatchObject({
			unsaved: false,
			save: { kind: "saved" },
			base: "v1",
		});
	});

	it("saves at most ten seconds apart while typing goes on", async () => {
		const { box, saving } = start();
		for (let at = 0; at < MAX_WAIT_MS; at += IDLE_MS / 2) {
			saving.edit({ body: `typed ${at}\n` });
			await vi.advanceTimersByTimeAsync(IDLE_MS / 2);
		}
		expect(box.sent).toHaveLength(1);
		expect(box.sent[0]!.base).toBe("v0");
	});

	it("saves right away on a flush, and not at all with nothing changed", async () => {
		const { box, saving } = start();
		await saving.flush();
		expect(box.sent).toEqual([]);
		saving.edit({ title: "Plan B" });
		await saving.flush();
		expect(box.sent.map((s) => s.title)).toEqual(["Plan B"]);
		expect(saving.snapshot().title).toBe("Plan B");
	});

	it("names the version it saved last as the next save's base", async () => {
		const { box, saving } = start();
		saving.edit({ body: "a\n" });
		await saving.flush();
		saving.edit({ body: "b\n" });
		await saving.flush();
		expect(box.sent.map((s) => s.base)).toEqual(["v0", "v1"]);
	});

	it("puts a merged answer into the editor against the text it sent", async () => {
		const { box, incoming, saving } = start();
		box.answerWith((save) => ({
			status: "ok",
			data: { version: version("m1", `${save.body}from elsewhere\n`), merged: true },
		}));
		saving.edit({ body: "one\nmine\n" });
		await saving.flush();
		expect(incoming).toHaveBeenCalledWith("one\nmine\nfrom elsewhere\n", "one\nmine\n");
		expect(saving.snapshot()).toMatchObject({ unsaved: false, base: "m1" });
	});

	it("keeps what was typed during a save and saves it from the merged version", async () => {
		const { box, incoming, saving } = start();
		let open!: () => void;
		box.holdUntil(new Promise((resolve) => (open = resolve)));
		box.answerWith((save) => ({
			status: "ok",
			data: { version: version("m1", `${save.body}merged\n`), merged: true },
		}));
		saving.edit({ body: "sent\n" });
		const first = saving.flush();
		saving.edit({ body: "sent\nlater\n" });
		open();
		await first;
		expect(incoming).toHaveBeenCalledWith("sent\nmerged\n", "sent\n");
		// The editor hands back its text with both in it.
		saving.edit({ body: "sent\nlater\nmerged\n" });
		box.answerWith(null);
		box.holdUntil(Promise.resolve());
		await vi.advanceTimersByTimeAsync(IDLE_MS);
		expect(box.sent.at(-1)).toMatchObject({ base: "m1", body: "sent\nlater\nmerged\n" });
	});

	it("keeps the text and says why when the device refuses", async () => {
		const { box, saving } = start();
		box.answerWith(() => ({
			status: "error",
			error: { kind: "failed", message: "A note can be at most 1 MiB." },
		}));
		saving.edit({ body: "too long\n" });
		await saving.flush();
		expect(saving.snapshot()).toMatchObject({
			unsaved: true,
			save: { kind: "failed", reason: "A note can be at most 1 MiB." },
		});
		// No retry on its own; the next change tries again.
		await vi.advanceTimersByTimeAsync(MAX_WAIT_MS * 2);
		expect(box.sent).toHaveLength(1);
		box.answerWith(null);
		saving.edit({ body: "shorter\n" });
		await vi.advanceTimersByTimeAsync(IDLE_MS);
		expect(box.sent.at(-1)).toMatchObject({ base: "v0", body: "shorter\n" });
		expect(saving.snapshot().save).toEqual({ kind: "saved" });
	});

	it("takes a head that moved elsewhere while nothing waits", async () => {
		const newer = version("v9", "one\nfrom the agent\n", { title: "Plan v2" });
		const { incoming, saving } = start(version("v0", "one\n"), async () => newer);
		saving.headMoved("v9");
		await vi.runAllTimersAsync();
		expect(incoming).toHaveBeenCalledWith("one\nfrom the agent\n", "one\n");
		expect(saving.snapshot()).toMatchObject({ title: "Plan v2", base: "v9", unsaved: false });
	});

	it("leaves a moved head to the next save while a change waits", async () => {
		const read = vi.fn(async () => version("v9", "x\n"));
		const { incoming, saving } = start(version("v0", "one\n"), read);
		saving.edit({ body: "typing\n" });
		saving.headMoved("v9");
		await Promise.resolve();
		expect(read).not.toHaveBeenCalled();
		expect(incoming).not.toHaveBeenCalled();
	});

	it("restores an old version as a new one, after what waits", async () => {
		const { box, incoming, saving } = start();
		saving.edit({ body: "now\n" });
		const failure = await saving.replace({ title: "Plan", body: "then\n", deleted: false });
		expect(failure).toBeNull();
		expect(box.sent.map((s) => [s.base, s.body])).toEqual([
			["v0", "now\n"],
			["v1", "then\n"],
		]);
		expect(incoming).toHaveBeenCalledWith("then\n", "now\n");
		expect(saving.snapshot().base).toBe("v2");
	});

	it("deletes the note as a new version", async () => {
		const { box, saving } = start();
		await saving.replace({ deleted: true });
		expect(box.sent.at(-1)).toEqual({
			base: "v0",
			title: "Plan",
			body: "one\n",
			deleted: true,
		});
		expect(saving.snapshot().deleted).toBe(true);
		await saving.replace({});
		expect(box.sent.at(-1)).toMatchObject({ base: "v1", deleted: false });
		expect(saving.snapshot().deleted).toBe(false);
	});
});

describe("saveText", () => {
	it("says where saving stands", () => {
		expect(saveText({ kind: "saved" }, false)).toBe("Saved");
		expect(saveText({ kind: "saving" }, false)).toBe("Saving");
		expect(saveText({ kind: "failed", reason: "Too long." }, false)).toBe(
			"Not saved: Too long."
		);
		expect(saveText({ kind: "saved" }, true)).toBe("Read-only");
	});
});

describe("incomingText", () => {
	it("takes the incoming text when nothing was typed since", () => {
		expect(incomingText("one\n", "one\ntwo\n", "one\n")).toBe("one\ntwo\n");
	});

	it("keeps what was typed when the incoming text is what it was against", () => {
		expect(incomingText("one\nmine\n", "one\n", "one\n")).toBe("one\nmine\n");
	});

	it("puts the incoming change around what was typed when they don't touch", () => {
		expect(incomingText("one mine\nthree\n", "one\nthree theirs\n", "one\nthree\n")).toBe(
			"one mine\nthree theirs\n"
		);
	});

	it("merges by line when both changed the same place", () => {
		const merged = incomingText("one\nmine\n", "one\ntheirs\n", "one\n");
		expect(merged).toContain("mine");
		expect(merged).toContain("theirs");
	});
});
