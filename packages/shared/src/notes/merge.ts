// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a note version is hashed and how two heads merge, for the mock data
// source. The real app never merges versions here: every device's `maxd`
// does, with `crates/core/src/notes`. This file keeps the same rules so the
// dev preview acts the same, and a change to the rules there comes here
// too. The phone's text box also uses `mergeText` to keep what you typed
// around text from elsewhere, in `./autosave`.
//
// The hash matches the Rust one byte for byte. The line diff is jsdiff's,
// the one `./diff` uses, and `similar` in Rust may line up an ambiguous
// change, such as a repeated line, differently. That changes which merged
// text comes out in rare cases, which the preview can live with.

import { diffArrays } from "diff";

import type { NoteId, NoteVersion, VersionHash } from "../bindings";

/** Compares two strings by code unit, which for hex hashes is Rust's byte order. */
const byCode = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

/**
 * The hash of a version with these fields: lowercase hex SHA-256 over
 * compact JSON of the note, its sorted parents, title, body and whether
 * it's deleted, with the keys in that sorted order. `JSON.stringify`
 * escapes strings the way `serde_json` does.
 */
export const hashOf = (
	note: NoteId,
	parents: readonly VersionHash[],
	title: string,
	body: string,
	deleted: boolean
): VersionHash => {
	const json = JSON.stringify({ body, deleted, note, parents: [...parents].sort(byCode), title });
	return sha256(utf8(json));
};

/** The version with its parents sorted and its hash set from its fields. */
export const sealed = (version: NoteVersion): NoteVersion => {
	const parents = [...new Set(version.parents)].sort(byCode);
	return {
		...version,
		parents,
		hash: hashOf(version.note, parents, version.title, version.body, version.deleted),
	};
};

/** The versions no other version names as a parent, sorted by hash. Give it one note's versions. */
export const heads = (versions: readonly NoteVersion[]): NoteVersion[] => {
	const named = new Set(versions.flatMap((v) => v.parents));
	const seen = new Set<VersionHash>();
	return versions
		.filter((v) => !named.has(v.hash))
		.sort((a, b) => byCode(a.hash, b.hash))
		.filter((v) => !seen.has(v.hash) && seen.add(v.hash));
};

/** Every version `from` follows, itself included, among `versions`. */
const ancestors = (from: VersionHash, byHash: Map<VersionHash, NoteVersion>): Set<VersionHash> => {
	const seen = new Set<VersionHash>();
	const next = [from];
	for (let hash = next.pop(); hash !== undefined; hash = next.pop()) {
		if (seen.has(hash)) continue;
		seen.add(hash);
		next.push(...(byHash.get(hash)?.parents ?? []));
	}
	return seen;
};

/**
 * The version `a` and `b` both follow that's nearest to them. Of several,
 * any that another follows is dropped and the smallest hash of the rest
 * wins. Null when they share no history, and the merge starts from an
 * empty note.
 */
export const base = (
	a: NoteVersion,
	b: NoteVersion,
	versions: readonly NoteVersion[]
): NoteVersion | null => {
	const byHash = new Map(versions.map((v) => [v.hash, v]));
	if (!byHash.has(a.hash) || !byHash.has(b.hash)) return null;
	const fromB = ancestors(b.hash, byHash);
	const shared = [...ancestors(a.hash, byHash)]
		.filter((hash) => fromB.has(hash) && byHash.has(hash))
		.sort(byCode);
	// A candidate another candidate follows is further back.
	const further = new Set<VersionHash>();
	for (const hash of shared) {
		for (const above of ancestors(hash, byHash)) if (above !== hash) further.add(above);
	}
	const nearest = shared.find((hash) => !further.has(hash));
	return nearest === undefined ? null : (byHash.get(nearest) ?? null);
};

/**
 * Merges two heads of a note from `base`, or from an empty note when they
 * share none. The head with the smaller hash is ours. The rules are the
 * Rust ones: lines one side changed take that side, lines both changed
 * keep ours then theirs, a title both changed keeps ours, and an edit
 * beats a delete.
 */
export const merge = (a: NoteVersion, b: NoteVersion, from: NoteVersion | null): NoteVersion => {
	const [ours, theirs] = byCode(a.hash, b.hash) <= 0 ? [a, b] : [b, a];
	const baseTitle = from?.title ?? "";
	const baseBody = from?.body ?? "";
	const baseDeleted = from?.deleted ?? false;
	const unchanged = (side: NoteVersion): boolean =>
		side.title === baseTitle && side.body === baseBody && side.deleted === baseDeleted;
	const deleted = ours.deleted
		? theirs.deleted || unchanged(theirs)
		: theirs.deleted && unchanged(ours);
	return sealed({
		hash: "",
		project: ours.project,
		note: ours.note,
		parents: [ours.hash, theirs.hash],
		title: ours.title === baseTitle ? theirs.title : ours.title,
		body: mergeText(baseBody, ours.body, theirs.body),
		deleted,
		at: Math.max(ours.at, theirs.at),
		by: null,
	});
};

/**
 * Merges a note's heads into one: the two with the smallest hashes, then
 * that merge with the next, in hash order. Gives the merges it made, the
 * last of which is the head, and none when there's one head already.
 */
export const settle = (versions: readonly NoteVersion[]): NoteVersion[] => {
	const all = [...versions];
	const [first, ...rest] = heads(versions);
	const made: NoteVersion[] = [];
	if (!first) return made;
	let head = first;
	for (const next of rest) {
		const merged = merge(head, next, base(head, next, all));
		all.push(merged);
		made.push(merged);
		head = merged;
	}
	return made;
};

/** Where one side's change sits: the base lines it replaces and the lines it puts there. */
type Hunk = { side: number; base: [number, number]; lines: [number, number] };

/** A text's lines, each with its newline, the last maybe without. */
const lines = (text: string): string[] => text.match(/[^\n]*\n|[^\n]+/g) ?? [];

/** The hunks that turn `from` into `to`, from jsdiff's line diff. */
const hunks = (side: number, from: string[], to: string[]): Hunk[] => {
	const out: Hunk[] = [];
	let a = 0;
	let b = 0;
	for (const change of diffArrays(from, to)) {
		const count = change.value.length;
		if (!change.added && !change.removed) {
			a += count;
			b += count;
			continue;
		}
		// Back-to-back changes are one hunk.
		let hunk = out[out.length - 1];
		if (!hunk || hunk.base[1] !== a || hunk.lines[1] !== b) {
			hunk = { side, base: [a, a], lines: [b, b] };
			out.push(hunk);
		}
		if (change.removed) a += count;
		else b += count;
		hunk.base[1] = a;
		hunk.lines[1] = b;
	}
	return out;
};

/** A three-way merge of `ours` and `theirs` from `from`, line by line, with no conflict markers. */
export const mergeText = (from: string, ours: string, theirs: string): string => {
	const base = lines(from);
	const sides = [lines(ours), lines(theirs)] as const;
	// Inserts sort before a change at the same line, so an insert just
	// above a changed line stays apart from it.
	const all = [...hunks(0, base, sides[0]), ...hunks(1, base, sides[1])].sort(
		(p, q) => p.base[0] - q.base[0] || p.base[1] - q.base[1] || p.side - q.side
	);

	let out = "";
	let at = 0;
	let i = 0;
	for (let first = all[0]; first; first = all[i]) {
		const start = first.base[0];
		let end = first.base[1];
		const region = [first];
		i++;
		for (let next = all[i]; next; next = all[i]) {
			const overlaps = next.base[0] < end;
			const sameInsert = start === end && next.base[0] === start && next.base[1] === end;
			if (!overlaps && !sameInsert) break;
			end = Math.max(end, next.base[1]);
			region.push(next);
			i++;
		}
		out += base.slice(at, start).join("");
		at = end;

		// Each side's lines for the region, widened by the base lines its
		// own hunks don't cover.
		const text = (side: number): string => {
			const mine = region.filter((h) => h.side === side);
			const first = mine[0];
			const last = mine[mine.length - 1];
			if (!first || !last) return base.slice(start, end).join("");
			const from = first.lines[0] - (first.base[0] - start);
			const to = last.lines[1] + (end - last.base[1]);
			return (sides[side] ?? []).slice(from, to).join("");
		};
		const o = base.slice(start, end).join("");
		const a = text(0);
		const b = text(1);
		if (a === b || b === o) out += a;
		else if (a === o) out += b;
		else out += a + (a !== "" && !a.endsWith("\n") ? "\n" : "") + b;
	}
	return out + base.slice(at).join("");
};

/** The SHA-256 round constants. */
const K = new Uint32Array([
	0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
	0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
	0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
	0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
	0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
	0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
	0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
	0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

/**
 * A string's UTF-8 bytes, as `TextEncoder` gives them, which React Native
 * may not have. A lone surrogate becomes U+FFFD.
 */
export const utf8 = (text: string): Uint8Array => {
	const out: number[] = [];
	for (const char of text) {
		let code = char.codePointAt(0) ?? 0xfffd;
		if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
		if (code < 0x80) out.push(code);
		else if (code < 0x800) out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
		else if (code < 0x10000)
			out.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
		else
			out.push(
				0xf0 | (code >> 18),
				0x80 | ((code >> 12) & 0x3f),
				0x80 | ((code >> 6) & 0x3f),
				0x80 | (code & 0x3f)
			);
	}
	return Uint8Array.from(out);
};

/**
 * Lowercase hex SHA-256 of `data`. Web Crypto's digest is async, and the
 * mock answers from a plain function, so it's written out here.
 */
export const sha256 = (data: Uint8Array): string => {
	const bits = data.length * 8;
	const padded = new Uint8Array((((data.length + 8) >> 6) + 1) << 6);
	padded.set(data);
	padded[data.length] = 0x80;
	const view = new DataView(padded.buffer);
	view.setUint32(padded.length - 8, Math.floor(bits / 0x1_0000_0000));
	view.setUint32(padded.length - 4, bits >>> 0);

	const h = [
		0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
		0x5be0cd19,
	];
	const w = new Uint32Array(64);
	const word = (t: number): number => w[t] ?? 0;
	const rotr = (x: number, n: number): number => (x >>> n) | (x << (32 - n));
	for (let block = 0; block < padded.length; block += 64) {
		for (let t = 0; t < 16; t++) w[t] = view.getUint32(block + t * 4);
		for (let t = 16; t < 64; t++) {
			const s0 = rotr(word(t - 15), 7) ^ rotr(word(t - 15), 18) ^ (word(t - 15) >>> 3);
			const s1 = rotr(word(t - 2), 17) ^ rotr(word(t - 2), 19) ^ (word(t - 2) >>> 10);
			w[t] = word(t - 16) + s0 + word(t - 7) + s1;
		}
		const r = [...h];
		for (let t = 0; t < 64; t++) {
			const [a = 0, b = 0, c = 0, d = 0, e = 0, f = 0, g = 0, hh = 0] = r;
			const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
			const ch = (e & f) ^ (~e & g);
			const t1 = (hh + s1 + ch + (K[t] ?? 0) + word(t)) >>> 0;
			const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
			const maj = (a & b) ^ (a & c) ^ (b & c);
			r.splice(0, 8, (t1 + s0 + maj) >>> 0, a, b, c, (d + t1) >>> 0, e, f, g);
		}
		for (let t = 0; t < 8; t++) h[t] = ((h[t] ?? 0) + (r[t] ?? 0)) >>> 0;
	}
	return h.map((part) => part.toString(16).padStart(8, "0")).join("");
};
