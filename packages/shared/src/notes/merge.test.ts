// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The same cases as the Rust tests in crates/core/src/notes, so the mock
// keeps the rules `maxd` merges by.

import { describe, expect, it } from "vitest";

import type { NoteVersion } from "../bindings";
import { base, hashOf, heads, merge, sealed, settle, sha256, utf8 } from "./merge";

const NOTE = "0190f5a2-7c3e-7b1a-9d4e-2f6a8b0c1d2e";

/** A sealed version of the test note after `parents`. */
const version = (
	parents: readonly NoteVersion[],
	title: string,
	body: string,
	at: number
): NoteVersion =>
	sealed({
		hash: "",
		project: "proj-atlas-maps",
		note: NOTE,
		parents: parents.map((p) => p.hash),
		title,
		body,
		deleted: false,
		at,
		by: null,
	});

/** Merges two heads the way a device holding `versions` does. */
const merged = (a: NoteVersion, b: NoteVersion, versions: readonly NoteVersion[]): NoteVersion =>
	merge(a, b, base(a, b, versions));

describe("note hashes", () => {
	it("hashes the way SHA-256 does", () => {
		const bytes = (text: string) => utf8(text);
		expect(sha256(bytes(""))).toBe(
			"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
		);
		expect(sha256(bytes("abc"))).toBe(
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
		);
		expect(sha256(bytes("a".repeat(1000)))).toBe(
			"41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
		);
	});

	it("encodes UTF-8 the way Rust does", () => {
		expect([...utf8("é€🗺\u{d800}")]).toEqual([
			0xc3, 0xa9, 0xe2, 0x82, 0xac, 0xf0, 0x9f, 0x97, 0xba, 0xef, 0xbf, 0xbd,
		]);
	});

	it("matches the Rust hash", () => {
		// The same vector is in crates/core/src/notes/mod.rs.
		const hash = hashOf(
			NOTE,
			["b".repeat(64), "a".repeat(64)],
			'Plan "v2"',
			"- [ ] ship\n\tcafé \u0001\n",
			false
		);
		expect(hash).toBe("7692edc0276a95e2dd67cf9d836c4e34d3ccf261887f3cb0d6d83172aeffc73a");
	});

	it("leaves when and by whom out", () => {
		const a = version([], "Notes", "x\n", 1);
		const b = sealed({
			...version([], "Notes", "x\n", 99),
			by: { device: "dev-devbox", agent: null },
		});
		expect(b.hash).toBe(a.hash);
	});
});

describe("merging two heads", () => {
	it("lands edits to different lines in either order", () => {
		const first = version([], "Plan", "one\ntwo\nthree\nfour\n", 1);
		const a = version([first], "Plan", "ONE\ntwo\nthree\nfour\n", 2);
		const b = version([first], "Plan", "one\ntwo\nthree\nFOUR\n", 3);
		const all = [first, a, b];
		const ab = merged(a, b, all);
		expect(ab.body).toBe("ONE\ntwo\nthree\nFOUR\n");
		expect(merged(b, a, all)).toEqual(ab);
		expect(ab.at).toBe(3);
		expect(ab.by).toBeNull();
		expect(ab.parents).toEqual([a.hash, b.hash].sort());
		expect(ab.hash).toBe(hashOf(ab.note, ab.parents, ab.title, ab.body, ab.deleted));
	});

	it("lands edits to neighboring lines", () => {
		const first = version([], "Plan", "one\ntwo\nthree\n", 1);
		const a = version([first], "Plan", "ONE\ntwo\nthree\n", 2);
		const b = version([first], "Plan", "one\nTWO\nthree\n", 3);
		expect(merged(a, b, [first, a, b]).body).toBe("ONE\nTWO\nthree\n");
	});

	it("keeps the same edit on both sides once", () => {
		const first = version([], "Plan", "one\ntwo\n", 1);
		const a = version([first], "Plan", "one\ntwo\nthree\n", 2);
		const b = version([first], "Plan", "one\ntwo\nthree\n", 3);
		expect(a.hash).toBe(b.hash);
		const c = version([first], "Plan 2", "one\ntwo\nthree\n", 3);
		const ac = merged(a, c, [first, a, c]);
		expect(ac.body).toBe("one\ntwo\nthree\n");
		expect(ac.title).toBe("Plan 2");
	});

	it("keeps different edits to one line, ours first", () => {
		const first = version([], "Plan", "one\ntwo\nthree\n", 1);
		const a = version([first], "Mine", "one\nA\nthree\n", 2);
		const b = version([first], "Yours", "one\nB\nthree\n", 3);
		const [ours, theirs] = a.hash < b.hash ? [a, b] : [b, a];
		const ab = merged(a, b, [first, a, b]);
		const line = (v: NoteVersion) => v.body.split("\n")[1];
		expect(ab.body).toBe(`one\n${line(ours)}\n${line(theirs)}\nthree\n`);
		expect(ab.title).toBe(ours.title);
	});

	it("keeps a last line without a newline apart", () => {
		const first = version([], "Plan", "one\ntwo", 1);
		const a = version([first], "Plan", "one\nA", 2);
		const b = version([first], "Plan", "one\nB", 3);
		expect(["one\nA\nB", "one\nB\nA"]).toContain(merged(a, b, [first, a, b]).body);
	});

	it("lets an edit beat a delete", () => {
		const first = version([], "Plan", "one\n", 1);
		const edited = version([first], "Plan", "one\ntwo\n", 2);
		const deleted = sealed({ ...first, parents: [first.hash], deleted: true, at: 3 });
		const kept = merged(edited, deleted, [first, edited, deleted]);
		expect(kept.deleted).toBe(false);
		expect(kept.body).toBe("one\ntwo\n");

		const untouched = version([first], "Plan", "one\n", 5);
		expect(merged(untouched, deleted, [first, untouched, deleted]).deleted).toBe(true);
	});

	it("merges notes made on two devices from an empty one", () => {
		const a = version([], "Ideas", "alpha\n", 1);
		const b = version([], "Ideas", "beta\n", 2);
		expect(base(a, b, [a, b])).toBeNull();
		expect(["alpha\nbeta\n", "beta\nalpha\n"]).toContain(merged(a, b, [a, b]).body);
	});

	it("gives the same hash for the same pair twice", () => {
		const first = version([], "Plan", "one\n", 1);
		const a = version([first], "Plan", "one\nA\n", 2);
		const b = version([first], "Plan", "B\none\n", 3);
		const once = merged(a, b, [first, a, b]);
		expect(merged(a, b, [first, a, b]).hash).toBe(once.hash);
		expect(once.body).toBe("B\none\nA\n");
	});

	it("converges on a criss-cross history", () => {
		const first = version([], "Plan", "one\ntwo\nthree\n", 1);
		const a = version([first], "Plan", "ONE\ntwo\nthree\n", 2);
		const b = version([first], "Plan", "one\ntwo\nTHREE\n", 3);
		const a2 = sealed({ ...merge(a, b, first), body: "ONE\ntwo\nTHREE\nfour\n" });
		const b2 = version([a, b], "Plan", "ONE\nTWO\nTHREE\n", 5);
		const all = [first, a, b, a2, b2];
		expect(base(a2, b2, all)?.hash).toBe([a.hash, b.hash].sort()[0]);
		const one = merged(a2, b2, all);
		expect(merged(b2, a2, all).hash).toBe(one.hash);
		expect(heads(all)).toHaveLength(2);
		expect(settle(all).at(-1)?.hash).toBe(one.hash);
	});

	it("settles three heads into one whatever order they arrive in", () => {
		const first = version([], "Plan", "one\ntwo\nthree\n", 1);
		const a = version([first], "Plan", "ONE\ntwo\nthree\n", 2);
		const b = version([first], "Plan", "one\nTWO\nthree\n", 3);
		const c = version([first], "Plan", "one\ntwo\nTHREE\n", 4);
		const one = settle([first, a, b, c]);
		expect(one).toHaveLength(2);
		expect(settle([c, b, a, first])).toEqual(one);
		const head = one[1]!;
		expect(head.body).toBe("ONE\nTWO\nTHREE\n");
		const all = [first, a, b, c, ...one];
		expect(heads(all)).toEqual([head]);
		expect(settle(all)).toEqual([]);
	});
});
