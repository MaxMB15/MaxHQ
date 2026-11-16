// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type AgentRef,
	codeSpans,
	type Draft,
	excerptIds,
	farAgents,
	farNotes,
	type FileFacts,
	fileKeys,
	fileNumbers,
	fromUserInput,
	type GitItemRef,
	gitKey,
	gitNumbers,
	gitTargetOf,
	hasNote,
	isBlank,
	keyedGitItems,
	labeledText,
	normalize,
	noteMarker,
	type NoteRef,
	OBJECT_MARK,
	plainText,
	type QuoteRef,
	type Segment,
	toUserInput,
	trimEnd,
	withExcerpts,
} from "./draft";

const M = OBJECT_MARK;

const text = (value: string): Segment => ({ type: "text", text: value });
const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });

const quote: QuoteRef = {
	type: "quote",
	agent: "agent-2",
	agent_title: "Fix login",
	item: "item-9",
	text: "the cache misses on the second poll",
	position: 0,
	source: "reply",
	from: null,
};

const agent: AgentRef = {
	type: "agent",
	agent: "agent-3",
	title: "Usage meter",
	vendor: "codex",
	device: "device-1",
	excerpt: null,
};

const pr = (number: number, title = "Cache usage between polls"): GitItemRef => ({
	type: "git_item",
	repo: "owner/repo",
	number,
	kind: "pull_request",
	title,
	state: "open",
	url: `https://github.com/owner/repo/pull/${number}`,
	head: "usage-cache",
	base: "main",
	body: "",
	checked_at: 1_793_361_600_000,
});

const ids: Record<string, string> = { "chip-1": "att-shot", "chip-2": "att-main" };
const idOf = (key: string) => ids[key] ?? null;

describe("toUserInput", () => {
	it("marks each placed pill and lists its ref in order", () => {
		const draft: Draft = {
			segments: [
				text("Compare "),
				file("chip-1"),
				text(" with "),
				{ type: "pill", pill: quote },
				text(", then ask "),
				{ type: "pill", pill: agent },
				text(" about #412."),
			],
			gitItems: [pr(412)],
		};
		expect(toUserInput(draft, idOf)).toEqual({
			text: `Compare ${M} with ${M}, then ask ${M} about #412.`,
			attachments: ["att-shot"],
			refs: [{ type: "attachment", id: "att-shot" }, quote, agent, pr(412)],
		});
	});

	it("sends plain text the way it always has", () => {
		expect(toUserInput({ segments: [text("Run the tests")], gitItems: [] }, idOf)).toEqual({
			text: "Run the tests",
			attachments: [],
		});
	});

	it("drops a file that hasn't staged", () => {
		const draft = { segments: [text("See "), file("chip-9")], gitItems: [] };
		expect(toUserInput(draft, idOf)).toEqual({ text: "See ", attachments: [] });
	});

	it("lists a file placed twice once in attachments", () => {
		const draft = { segments: [file("chip-1"), text(" and "), file("chip-1")], gitItems: [] };
		const input = toUserInput(draft, idOf);
		expect(input.attachments).toEqual(["att-shot"]);
		expect(input.refs).toHaveLength(2);
	});

	it("keeps a pull request only while the text still names it", () => {
		const draft = {
			segments: [text("See #409 and #409, not #398")],
			gitItems: [pr(398), pr(412), pr(409)],
		};
		expect(
			toUserInput(draft, idOf).refs?.map((ref) => ref.type === "git_item" && ref.number)
		).toEqual([409, 398]);
	});

	it("ignores a number inside code", () => {
		const draft = { segments: [text("Run `git show #412`")], gitItems: [pr(412)] };
		expect(toUserInput(draft, idOf).refs).toBeUndefined();
	});

	it("takes stray marks out of typed text", () => {
		const draft = { segments: [text(`a${M}b`), file("chip-2")], gitItems: [] };
		expect(toUserInput(draft, idOf).text).toBe(`ab${M}`);
	});
});

describe("farAgents", () => {
	const near: AgentRef = { ...agent, agent: "agent-4", device: "device-2" };
	const pill = (ref: AgentRef): Segment => ({ type: "pill", pill: ref });

	it("lists each agent on another device once, in text order", () => {
		const segments = [pill(agent), text(" and "), pill(near), text(" then "), pill(agent)];
		expect(farAgents(segments, "device-2")).toEqual([agent]);
		expect(farAgents(segments, "device-9")).toEqual([agent, near]);
	});

	it("lists none when every agent is on the same device", () => {
		expect(farAgents([pill(agent), file("chip-1")], "device-1")).toEqual([]);
	});
});

describe("withExcerpts", () => {
	const live: AgentRef = { ...agent, agent: "agent-4", title: "Sidebar sort" };

	it("gives each agent its excerpt and lists the excerpt with the attachments once", () => {
		const input = {
			text: `${M} ${M} ${M} and ${M}`,
			attachments: ["att-shot"],
			refs: [{ type: "attachment" as const, id: "att-shot" }, agent, live, agent],
		};
		expect(withExcerpts(input, new Map([["agent-3", "att-excerpt"]]))).toEqual({
			text: input.text,
			attachments: ["att-shot", "att-excerpt"],
			refs: [
				{ type: "attachment", id: "att-shot" },
				{ ...agent, excerpt: "att-excerpt" },
				live,
				{ ...agent, excerpt: "att-excerpt" },
			],
		});
	});

	it("sends an agent without an excerpt live", () => {
		const input = {
			text: M,
			attachments: [],
			refs: [{ ...agent, excerpt: "att-old" }],
		};
		expect(withExcerpts(input, new Map())).toEqual({ text: M, attachments: [], refs: [agent] });
	});

	it("leaves a message with no refs as it is", () => {
		const input = { text: "Run the tests", attachments: [] };
		expect(withExcerpts(input, new Map([["agent-3", "att-excerpt"]]))).toBe(input);
	});
});

describe("excerptIds", () => {
	it("names the attachments that are agents' excerpts", () => {
		const refs = [agent, { ...agent, excerpt: "att-excerpt" }, pr(412)];
		expect(excerptIds(refs)).toEqual(new Set(["att-excerpt"]));
		expect(excerptIds(undefined)).toEqual(new Set());
	});
});

describe("trimEnd", () => {
	it("drops the spaces after the last pill and blank text at the end", () => {
		expect(trimEnd([text("See "), file("chip-1"), text(" \n ")])).toEqual([
			text("See "),
			file("chip-1"),
		]);
		expect(trimEnd([text("Run it  \n")])).toEqual([text("Run it")]);
		expect(trimEnd([])).toEqual([]);
	});
});

describe("fromUserInput", () => {
	it("turns a message back into the draft it came from", () => {
		const draft: Draft = {
			segments: [
				text("Compare "),
				file("att-shot"),
				text(" with "),
				{ type: "pill", pill: quote },
				text(" in #412"),
			],
			gitItems: [pr(412)],
		};
		const keyed = (key: string) => key;
		expect(fromUserInput(toUserInput(draft, keyed))).toEqual(draft);
	});

	it("gives each file the key the caller picks", () => {
		const input = {
			text: `Look ${M}`,
			attachments: ["att-shot"],
			refs: [{ type: "attachment" as const, id: "att-shot" }],
		};
		expect(fromUserInput(input, (id) => `key-${id}`).segments).toEqual([
			text("Look "),
			file("key-att-shot"),
		]);
	});

	it("puts files an older client listed after the text", () => {
		expect(fromUserInput({ text: "Look at these", attachments: ["a", "b"] }).segments).toEqual([
			text("Look at these "),
			file("a"),
			text(" "),
			file("b"),
		]);
	});

	it("brings an agent back without its excerpt, and leaves the excerpt out", () => {
		const input = {
			text: `Ask ${M}`,
			attachments: ["att-excerpt", "att-shot"],
			refs: [{ ...agent, excerpt: "att-excerpt" }],
		};
		expect(fromUserInput(input).segments).toEqual([
			text("Ask "),
			{ type: "pill", pill: agent },
			text(" "),
			file("att-shot"),
		]);
	});

	it("brings a visual back as its pill, once", () => {
		const visual = {
			type: "visual" as const,
			attachment: "att-page",
			title: "Weekly usage",
			from: null,
		};
		const input = { text: `Thinner bars on ${M}`, attachments: ["att-page"], refs: [visual] };
		expect(fromUserInput(input).segments).toEqual([
			text("Thinner bars on "),
			{
				type: "pill",
				pill: { type: "visual", key: "att-page", title: "Weekly usage", from: null },
			},
		]);
		expect(labeledText(input, () => "visual-weekly-usage.html")).toBe(
			"Thinner bars on Weekly usage"
		);
	});

	it("drops a mark with no ref and appends a ref with no mark", () => {
		expect(
			fromUserInput({ text: `a${M}b${M}c`, attachments: [], refs: [agent] }).segments
		).toEqual([text("a"), { type: "pill", pill: agent }, text("bc")]);
		expect(fromUserInput({ text: "hello", attachments: [], refs: [agent] }).segments).toEqual([
			text("hello "),
			{ type: "pill", pill: agent },
		]);
	});
});

describe("normalize", () => {
	it("joins text, drops empty text and stray marks", () => {
		expect(normalize([text("a"), text(""), text(`b${M}`), file("k"), text(M)])).toEqual([
			text("ab"),
			file("k"),
		]);
	});
});

describe("isBlank", () => {
	it("counts a pill as something to send", () => {
		expect(isBlank({ segments: [text("  \n")], gitItems: [] })).toBe(true);
		expect(isBlank({ segments: [file("k")], gitItems: [] })).toBe(false);
	});
});

describe("fileKeys", () => {
	it("lists each placed file once, in order", () => {
		expect(fileKeys([file("b"), text(" "), file("a"), file("b")])).toEqual(["b", "a"]);
	});
});

describe("codeSpans", () => {
	it("finds fences, unclosed fences and backtick spans", () => {
		const fenced = "before\n```\ncode\n```\nafter";
		expect(codeSpans(fenced)).toEqual([{ from: 7, to: 19 }]);
		expect(codeSpans("a\n~~~\nopen")).toEqual([{ from: 2, to: 10 }]);
		expect(codeSpans("run `ls` and ``a`b``")).toEqual([
			{ from: 4, to: 8 },
			{ from: 13, to: 20 },
		]);
		expect(codeSpans("one ` tick")).toEqual([]);
	});

	it("reads three backticks closed on their line as a span, not a fence", () => {
		expect(codeSpans("```a``` b\n#12")).toEqual([{ from: 0, to: 7 }]);
	});
});

describe("gitNumbers", () => {
	it("finds numbers that start a word, outside code", () => {
		expect(gitNumbers("Fixes #12, see (#7) and #0").map((found) => found.number)).toEqual([
			12, 7,
		]);
		expect(gitNumbers("a#12 #12b &#12 ##12 `#12`")).toEqual([]);
		expect(gitNumbers("#3")).toEqual([{ from: 0, to: 2, repo: null, number: 3 }]);
	});

	it("finds a number in another repo", () => {
		expect(gitNumbers("See some-one/else.js#7 and #8")).toEqual([
			{ from: 4, to: 22, repo: "some-one/else.js", number: 7 },
			{ from: 27, to: 29, repo: null, number: 8 },
		]);
		expect(gitNumbers("path/to/x#7 a/b/c#7 x/y#7b `a/b#7`")).toEqual([]);
	});
});

describe("git keys", () => {
	const other = { ...pr(7), repo: "Some-One/Else" };

	it("tell another repo's number from the agent's own, whatever the case", () => {
		expect(gitKey({ repo: null, number: 7 })).toBe("#7");
		expect(gitKey({ repo: "Some-One/Else", number: 7 })).toBe("some-one/else#7");
	});

	it("name an item the way the text does", () => {
		const text = "See some-one/else#7 and #12";
		expect(gitTargetOf(text, other)).toEqual({ repo: "Some-One/Else", number: 7 });
		expect(gitTargetOf(text, pr(12))).toEqual({ repo: null, number: 12 });
		expect([...keyedGitItems(text, [other, pr(7), pr(12), pr(12, "Again")])]).toEqual([
			["some-one/else#7", other],
			["#7", pr(7)],
			["#12", pr(12)],
		]);
	});

	it("send the same number in two repos as two refs", () => {
		const draft = { segments: [text("some-one/else#7 and #7")], gitItems: [pr(7), other] };
		expect(toUserInput(draft, idOf).refs).toEqual([other, pr(7)]);
	});
});

describe("labeledText", () => {
	it("puts each pill's words where its mark sits", () => {
		const input = {
			text: `See ${M} and ${M}, ask ${M}${M}`,
			attachments: ["att-shot"],
			refs: [{ type: "attachment" as const, id: "att-shot" }, pr(412), quote, agent],
		};
		const names = (id: string) => (id === "att-shot" ? "shot.png" : undefined);
		expect(labeledText(input, names)).toBe(
			"See shot.png and “the cache misses on the…”, ask Usage meter"
		);
	});
});

describe("plainText", () => {
	const facts: Record<string, FileFacts> = {
		a: { name: "shot.png", kind: "image" },
		b: { name: "spec.pdf", kind: "pdf" },
		c: { name: "pasted-image.png", kind: "image" },
		d: { name: "src/[old]", kind: "folder" },
		e: { name: "src/main.rs", kind: "file" },
	};
	const factsOf = (key: string) => facts[key];

	it("numbers images and PDFs per kind in text order", () => {
		expect([...fileNumbers([file("b"), file("a"), file("e"), file("c")], factsOf)]).toEqual([
			["b", 1],
			["a", 1],
			["c", 2],
		]);
	});

	it("writes each pill the way another app can read it", () => {
		const segments = [
			file("a"),
			text(" "),
			file("b"),
			text(" "),
			file("c"),
			text(" "),
			file("d"),
			text(" "),
			file("e"),
			text(" "),
			{ type: "pill", pill: quote } as const,
			text(" "),
			{ type: "pill", pill: agent } as const,
			text(" #412"),
		];
		expect(plainText(segments, factsOf)).toBe(
			'[Image 1: shot.png] [PDF 1: spec.pdf] [Image 2: pasted-image.png] [Folder: src/old/] [File: src/main.rs] "the cache misses on the second poll" [Agent: Usage meter] #412'
		);
	});
});

describe("note pills", () => {
	const note: NoteRef = {
		type: "note",
		project: "proj-1",
		note: "note-1",
		title: "GTFS release steps",
		copy: null,
	};
	const other: NoteRef = { ...note, project: "proj-2", note: "note-2", title: "" };
	const pill = (ref: NoteRef): Segment => ({ type: "pill", pill: ref });

	it("finds a note's pill in the text", () => {
		expect(hasNote([text("see "), pill(note)])).toBe(true);
		expect(hasNote([text("see "), file("k")])).toBe(false);
	});

	it("lists each note from another project once, in text order", () => {
		const segments = [pill(other), text(" "), pill(note), text(" "), pill(other)];
		expect(farNotes(segments, "proj-1")).toEqual([other]);
		expect(farNotes(segments, "")).toEqual([other, note]);
	});

	it("gives a note from another project its copy and lists the copy once", () => {
		const input = {
			text: `${M} and ${M} ${M}`,
			attachments: [],
			refs: [note, other, other],
		};
		const sent = withExcerpts(input, new Map(), new Map([["note-2", "att-copy"]]));
		expect(sent).toEqual({
			text: input.text,
			attachments: ["att-copy"],
			refs: [note, { ...other, copy: "att-copy" }, { ...other, copy: "att-copy" }],
		});
		expect(excerptIds(sent.refs)).toEqual(new Set(["att-copy"]));
	});

	it("comes back from a sent message without its copy", () => {
		const draft = fromUserInput({
			text: `Read ${M}`,
			attachments: ["att-copy"],
			refs: [{ ...note, copy: "att-copy" }],
		});
		expect(draft.segments).toEqual([text("Read "), pill(note)]);
	});

	it("reads as its title, or Untitled", () => {
		const input = { text: `${M} ${M}`, attachments: [], refs: [note, other] };
		expect(labeledText(input, () => undefined)).toBe("GTFS release steps Untitled");
		expect(plainText([pill(note), text(" "), pill(other)], () => undefined)).toBe(
			"[Note: GTFS release steps] [Note: Untitled]"
		);
		expect(noteMarker("Steps [draft]")).toBe("[Note: Steps draft]");
	});
});
