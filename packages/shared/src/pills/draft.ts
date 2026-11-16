// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A message being written, as text and pills in order. Each editor is a
// view of it: Tiptap on the desktop, the phone's own editor later. A file's
// pill names the file by the draft's own key, since the file only gets its
// attachment ID once it stages. Pull requests and issues aren't segments:
// the text keeps `#123` as typed, or `owner/repo#123` for another repo,
// and the draft remembers which resolved, so the editor draws each as a
// pill.

import type {
	AgentId,
	AttachmentId,
	AttachmentKind,
	DeviceId,
	NoteId,
	ProjectId,
} from "../bindings";

import { toVisualFence } from "../visuals/fence";

import { noteLabel, quoteLabel, visualLabel } from "./label";
import type { InlineRef, UserInput } from "./wire";

/** The mark in a message's text where a placed pill sits. */
export const OBJECT_MARK = "￼";

export type QuoteRef = Extract<InlineRef, { type: "quote" }>;
export type AgentRef = Extract<InlineRef, { type: "agent" }>;
export type NoteRef = Extract<InlineRef, { type: "note" }>;
export type OutputRef = Extract<InlineRef, { type: "output" }>;
export type GitItemRef = Extract<InlineRef, { type: "git_item" }>;

/**
 * A visual: an agent's web page, named by the draft's key for the staged
 * file that carries it, the way a large paste goes as a file.
 */
export type VisualPill = {
	readonly type: "visual";
	readonly key: string;
	readonly title: string;
	/** Where it came from, such as "Token audit on Mac mini". */
	readonly from: string | null;
};

/** A pill that takes one place in the text. */
export type PlacedPill =
	/** A file, named by the draft's key for it. */
	| { readonly type: "attachment"; readonly key: string }
	| VisualPill
	| QuoteRef
	| AgentRef
	| NoteRef
	| OutputRef;

/** The key of the staged file a pill stands for, or null when it stands for none. */
export const fileKeyOf = (pill: PlacedPill): string | null =>
	pill.type === "attachment" || pill.type === "visual" ? pill.key : null;

/** One run of text, or one pill. Text breaks lines with "\n". */
export type Segment =
	| { readonly type: "text"; readonly text: string }
	| { readonly type: "pill"; readonly pill: PlacedPill };

/** A message being written. */
export type Draft = {
	readonly segments: readonly Segment[];
	/** The pull requests and issues that resolved. Each `#N` in the text with one shows as its pill. */
	readonly gitItems: readonly GitItemRef[];
};

/** A draft with nothing in it. */
export const EMPTY_DRAFT: Draft = { segments: [], gitItems: [] };

const MARKS = new RegExp(OBJECT_MARK, "g");

/** Text with the object mark taken out, since only a pill may put one in. */
export const withoutMarks = (text: string): string => text.replace(MARKS, "");

/** Joins neighboring text, drops empty text and strips stray marks. */
export const normalize = (segments: readonly Segment[]): Segment[] => {
	const out: Segment[] = [];
	for (const segment of segments) {
		if (segment.type === "pill") {
			out.push(segment);
			continue;
		}
		const text = withoutMarks(segment.text);
		if (text === "") continue;
		const last = out.at(-1);
		if (last?.type === "text") out[out.length - 1] = { type: "text", text: last.text + text };
		else out.push({ type: "text", text });
	}
	return out;
};

/** The draft's text with each pill as one object mark. */
export const markedText = (segments: readonly Segment[]): string =>
	segments.map((segment) => (segment.type === "text" ? segment.text : OBJECT_MARK)).join("");

/** Whether a draft has nothing to send: no pill, and only blank text. */
export const isBlank = (draft: Draft): boolean =>
	draft.segments.every((segment) => segment.type === "text" && segment.text.trim() === "");

/** The keys of the files placed in a draft, each visual's page too, in order, once each. */
export const fileKeys = (segments: readonly Segment[]): string[] => [
	...new Set(
		segments.flatMap((segment) => {
			const key = segment.type === "pill" ? fileKeyOf(segment.pill) : null;
			return key === null ? [] : [key];
		})
	),
];

/** A span of text, from its first character up to but not including `to`. */
export type Span = { readonly from: number; readonly to: number };

/**
 * A line that opens a fenced code block. A backtick fence can't have another
 * backtick after it, so a line like ```` ```x``` ```` is a code span instead.
 */
export const FENCE_OPENER = /^ {0,3}(`{3,}(?!.*`)|~{3,})/;

/**
 * The code in Markdown text: fenced blocks, whose fence line starts with
 * three backticks or tildes, and backtick spans inside a line. An unclosed
 * fence runs to the end, and an unclosed backtick doesn't start a span.
 */
export const codeSpans = (text: string): Span[] => {
	const spans: Span[] = [];
	let fence: { char: string; size: number; from: number } | null = null;
	let at = 0;
	for (const line of text.split("\n")) {
		const end = at + line.length;
		const opener = FENCE_OPENER.exec(line);
		if (fence) {
			const closer = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(line);
			if (closer?.[1]?.[0] === fence.char && closer[1].length >= fence.size) {
				spans.push({ from: fence.from, to: end });
				fence = null;
			}
		} else if (opener?.[1]) {
			fence = { char: opener[1][0] ?? "`", size: opener[1].length, from: at };
		} else {
			const ticks = /(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g;
			for (let match = ticks.exec(line); match; match = ticks.exec(line))
				spans.push({ from: at + match.index, to: at + match.index + match[0].length });
		}
		at = end + 1;
	}
	if (fence) spans.push({ from: fence.from, to: text.length });
	return spans;
};

/** Whether an offset sits inside code. */
export const inCode = (spans: readonly Span[], offset: number): boolean =>
	spans.some((span) => offset > span.from && offset < span.to);

/**
 * A pull request or issue as the text names it: `#123` in the agent's
 * repo, where `repo` is null, or `owner/repo#123` in another.
 */
export type GitTarget = { readonly repo: string | null; readonly number: number };

/** A `#123` or `owner/repo#123` in the text. */
export type GitNumber = Span & GitTarget;

/**
 * `#` and digits at the start of a word, with an `owner/repo` before the
 * `#` for another repo, ending at a space or punctuation.
 */
const GIT_NUMBER = /(?<![\p{L}\p{N}_&/#￼-])(?:([\w.-]+\/[\w.-]+))?#(\d{1,9})(?![\p{L}\p{N}_-])/gu;

/** Every `#123` and `owner/repo#123` in the text outside code, in order. */
export const gitNumbers = (text: string): GitNumber[] => {
	const code = codeSpans(text);
	const found: GitNumber[] = [];
	for (const match of text.matchAll(GIT_NUMBER)) {
		const from = match.index;
		const number = Number(match[2]);
		if (number === 0 || inCode(code, from + 1)) continue;
		found.push({ from, to: from + match[0].length, repo: match[1] ?? null, number });
	}
	return found;
};

/** One key for each way the text names an item, whatever the case of its repo. */
export const gitKey = ({ repo, number }: GitTarget): string =>
	repo === null ? `#${number}` : `${repo.toLowerCase()}#${number}`;

/** The keys of the `owner/repo#123` the text has. */
const namedRepos = (text: string): Set<string> =>
	new Set(gitNumbers(text).flatMap((span) => (span.repo === null ? [] : [gitKey(span)])));

const targetIn = (named: ReadonlySet<string>, item: GitItemRef): GitTarget => {
	const target = { repo: item.repo, number: item.number };
	return named.has(gitKey(target)) ? target : { repo: null, number: item.number };
};

/**
 * How the text names an item: as `owner/repo#123` where it names the
 * item's repo that way, and as `#123` otherwise.
 */
export const gitTargetOf = (text: string, item: GitItemRef): GitTarget =>
	targetIn(namedRepos(text), item);

/** The items by the key of how the text names each. The first one for a key wins. */
export const keyedGitItems = (
	text: string,
	items: readonly GitItemRef[]
): Map<string, GitItemRef> => {
	const named = namedRepos(text);
	const keyed = new Map<string, GitItemRef>();
	for (const item of items) {
		const key = gitKey(targetIn(named, item));
		if (!keyed.has(key)) keyed.set(key, item);
	}
	return keyed;
};

/** What a draft needs to know about each file to send it. */
export type FileIds = (key: string) => AttachmentId | null;

/**
 * The message to send. Text gets an object mark where each placed pill
 * sits, and refs list those pills in the same order. A file that hasn't
 * staged has no ID, so its pill drops out. Then come the pull requests and
 * issues whose `#N` the text still has, in the order the text first names
 * them. A message with no refs leaves them out, the way it always went. A
 * visual goes as a visual when the agent's maxd knows them, `visuals`, and
 * as its page's file otherwise.
 */
export const toUserInput = (
	draft: Draft,
	idOf: FileIds,
	{ visuals = true }: { readonly visuals?: boolean } = {}
): UserInput => {
	let text = "";
	const refs: InlineRef[] = [];
	const attachments: AttachmentId[] = [];
	for (const segment of normalize(draft.segments)) {
		if (segment.type === "text") {
			text += segment.text;
			continue;
		}
		const { pill } = segment;
		if (pill.type === "attachment" || pill.type === "visual") {
			const id = idOf(pill.key);
			if (id === null) continue;
			refs.push(
				pill.type === "visual" && visuals
					? { type: "visual", attachment: id, title: pill.title, from: pill.from }
					: { type: "attachment", id }
			);
			if (!attachments.includes(id)) attachments.push(id);
		} else {
			refs.push(pill);
		}
		text += OBJECT_MARK;
	}
	const keyed = keyedGitItems(text, draft.gitItems);
	const named = new Set<string>();
	for (const span of gitNumbers(text)) {
		const key = gitKey(span);
		const item = keyed.get(key);
		if (!item || named.has(key)) continue;
		named.add(key);
		refs.push(item);
	}
	return refs.length === 0 ? { text, attachments } : { text, attachments, refs };
};

/**
 * The agents a draft names that run on a device other than `device`, once
 * each, in text order. MaxHQ's read tool can't reach them from there, so
 * each goes out with an excerpt.
 */
export const farAgents = (segments: readonly Segment[], device: DeviceId): AgentRef[] => {
	const seen = new Set<AgentId>();
	return segments.flatMap((segment) => {
		if (segment.type !== "pill" || segment.pill.type !== "agent") return [];
		const { pill } = segment;
		if (pill.device === device || seen.has(pill.agent)) return [];
		seen.add(pill.agent);
		return [pill];
	});
};

/**
 * The notes a draft names from a project other than `project`, once each,
 * in text order. The note tools only reach the agent's own project, so
 * each goes out as a read-only copy.
 */
export const farNotes = (segments: readonly Segment[], project: ProjectId): NoteRef[] => {
	const seen = new Set<NoteId>();
	return segments.flatMap((segment) => {
		if (segment.type !== "pill" || segment.pill.type !== "note") return [];
		const { pill } = segment;
		if (pill.project === project || seen.has(pill.note)) return [];
		seen.add(pill.note);
		return [pill];
	});
};

/**
 * The message with each agent's excerpt from `excerpts` and each note's
 * copy from `copies`, which also go with the attachments, since the
 * agent's device reads them from there. An agent or a note with none goes
 * live, read through MaxHQ's tools.
 */
export const withExcerpts = (
	input: UserInput,
	excerpts: ReadonlyMap<AgentId, AttachmentId>,
	copies: ReadonlyMap<NoteId, AttachmentId> = new Map()
): UserInput => {
	if (!input.refs) return input;
	const attachments = [...input.attachments];
	const take = (id: AttachmentId | null) => {
		if (id !== null && !attachments.includes(id)) attachments.push(id);
		return id;
	};
	const refs = input.refs.map((ref): InlineRef => {
		if (ref.type === "agent") return { ...ref, excerpt: take(excerpts.get(ref.agent) ?? null) };
		if (ref.type === "note") return { ...ref, copy: take(copies.get(ref.note) ?? null) };
		return ref;
	});
	return { ...input, attachments, refs };
};

/**
 * The attachments that are an agent's excerpt or a note's copy rather than
 * a file you added.
 */
export const excerptIds = (refs: readonly InlineRef[] | undefined): Set<AttachmentId> =>
	new Set(
		(refs ?? []).flatMap((ref) => {
			if (ref.type === "agent" && ref.excerpt !== null) return [ref.excerpt];
			if (ref.type === "note" && ref.copy !== null) return [ref.copy];
			return [];
		})
	);

/** The draft without the spaces and blank lines at its end. */
export const trimEnd = (segments: readonly Segment[]): Segment[] => {
	const out = normalize(segments);
	const last = out.at(-1);
	if (last?.type !== "text") return out;
	const text = last.text.trimEnd();
	return text === "" ? out.slice(0, -1) : [...out.slice(0, -1), { type: "text", text }];
};

/** A sent or queued message, which may come from a client that sends no refs. */
export type ReceivedInput = {
	readonly text: string;
	readonly attachments: readonly AttachmentId[];
	readonly refs?: readonly InlineRef[];
};

/**
 * A sent or queued message as a draft again, such as when you edit a queued
 * one. `keyOf` gives each file its key in the new draft. A mark with no ref
 * drops out, a ref with no mark goes at the end, and so does each
 * attachment with no ref of its own, the way older clients list files
 * after the text. An agent's excerpt or a note's copy was taken for the
 * send it went with, so it doesn't come back, and the next send takes a
 * new one.
 */
export const fromUserInput = (
	input: ReceivedInput,
	keyOf: (id: AttachmentId) => string = (id) => id
): Draft => {
	const refs = input.refs ?? [];
	const excerpts = excerptIds(refs);
	const placed = refs.flatMap((ref): PlacedPill[] => {
		if (ref.type === "git_item") return [];
		if (ref.type === "attachment") return [{ type: "attachment", key: keyOf(ref.id) }];
		if (ref.type === "agent") return [{ ...ref, excerpt: null }];
		if (ref.type === "note") return [{ ...ref, copy: null }];
		if (ref.type === "visual") {
			const { title, from = null } = ref;
			return [{ type: "visual", key: keyOf(ref.attachment), title, from }];
		}
		return [ref];
	});
	const segments: Segment[] = [];
	const pieces = input.text.split(OBJECT_MARK);
	pieces.forEach((piece, index) => {
		segments.push({ type: "text", text: piece });
		const pill = index < pieces.length - 1 ? placed[index] : undefined;
		if (pill) segments.push({ type: "pill", pill });
	});
	const placedIds = new Set(
		refs.flatMap((ref) => {
			if (ref.type === "attachment") return [ref.id];
			if (ref.type === "visual") return [ref.attachment];
			return [];
		})
	);
	const trailing = [
		...placed.slice(Math.max(0, pieces.length - 1)),
		...input.attachments
			.filter((id) => !placedIds.has(id) && !excerpts.has(id))
			.map((id): PlacedPill => ({ type: "attachment", key: keyOf(id) })),
	];
	for (const pill of trailing) {
		const before = markedText(segments);
		if (before !== "" && !/\s$/.test(before)) segments.push({ type: "text", text: " " });
		segments.push({ type: "pill", pill });
	}
	return {
		segments: normalize(segments),
		gitItems: refs.filter((ref): ref is GitItemRef => ref.type === "git_item"),
	};
};

/**
 * A ref that takes a place in the text: a file, a quote, an agent, a note
 * or a visual.
 */
export type PlacedRef = Exclude<InlineRef, GitItemRef>;

/** Whether a ref takes a place in the text, so a mark stands for it. */
export const isPlaced = (ref: InlineRef): ref is PlacedRef => ref.type !== "git_item";

/**
 * A sent or queued message's text with each mark as its pill's words: a
 * file's name, a quote's first words, an agent's title or a note's title.
 */
export const labeledText = (
	input: ReceivedInput,
	nameOf: (id: AttachmentId) => string | undefined
): string => {
	const placed = (input.refs ?? []).filter(isPlaced);
	let index = 0;
	return input.text.replace(MARKS, () => {
		const ref = placed[index++];
		switch (ref?.type) {
			case "attachment":
				return nameOf(ref.id) ?? "file";
			case "quote":
				return quoteLabel(ref.text);
			case "agent":
				return ref.title;
			case "note":
				return noteLabel(ref.title);
			case "visual":
				return visualLabel(ref.title);
			default:
				return "";
		}
	});
};

/** What a file's pill needs to read as plain text. */
export type FileFacts = {
	readonly name: string;
	readonly kind: AttachmentKind | "missing";
};

/**
 * Each image's and each PDF's number in the draft, counted per kind in
 * text order, the way the agent's markers count them.
 */
export const fileNumbers = (
	segments: readonly Segment[],
	factsOf: (key: string) => FileFacts | undefined
): Map<string, number> => {
	const counts = { image: 0, pdf: 0 };
	const numbers = new Map<string, number>();
	for (const key of fileKeys(segments)) {
		const kind = factsOf(key)?.kind;
		if (kind !== "image" && kind !== "pdf") continue;
		counts[kind] += 1;
		numbers.set(key, counts[kind]);
	}
	return numbers;
};

/** A label as it goes inside a marker, which can't hold brackets. */
const bare = (label: string) => label.replace(/[[\]]/g, "");

/** An agent as plain text, such as `[Agent: Usage meter]`. */
export const agentMarker = (title: string): string => `[Agent: ${bare(title)}]`;

/** A note as plain text, such as `[Note: Release steps]`. */
export const noteMarker = (title: string): string => `[Note: ${bare(noteLabel(title))}]`;

/**
 * A command's output as plain text, such as `[Output: pnpm test, exit 1]`,
 * the way the agent's marker names it without its number.
 */
export const outputMarker = (ref: OutputRef): string => {
	const end = ref.running ? ", running" : ref.exit_code === null ? "" : `, exit ${ref.exit_code}`;
	return `[Output: ${bare(ref.command)}${end}]`;
};

/** A visual as plain text, such as `[Visual: Weekly usage]`. */
export const visualMarker = (title: string): string => `[Visual: ${bare(visualLabel(title))}]`;

/** A file as the agent's marker names it, such as `[Image 1: shot.png]`. */
export const fileMarker = (facts: FileFacts, number: number | undefined): string => {
	const name = bare(facts.name);
	switch (facts.kind) {
		case "image":
			return `[Image ${number ?? 1}: ${name}]`;
		case "pdf":
			return `[PDF ${number ?? 1}: ${name}]`;
		case "folder":
			return `[Folder: ${name.endsWith("/") ? name : `${name}/`}]`;
		default:
			return `[File: ${name}]`;
	}
};

/**
 * The draft as plain text, for another app's paste: each file as its
 * marker, each quote in quote marks, each agent as `[Agent: title]`, each
 * note as `[Note: title]`, and each visual as its fenced block on lines of
 * its own when `pageOf` knows its page, or `[Visual: title]` when not. A
 * `#123` is already text.
 */
export const plainText = (
	segments: readonly Segment[],
	factsOf: (key: string) => FileFacts | undefined,
	pageOf: (key: string) => string | undefined = () => undefined
): string => {
	const numbers = fileNumbers(segments, factsOf);
	let out = "";
	segments.forEach((segment, index) => {
		if (segment.type === "text") {
			out += segment.text;
			return;
		}
		const { pill } = segment;
		switch (pill.type) {
			case "attachment": {
				const facts = factsOf(pill.key) ?? { name: "file", kind: "file" };
				out += fileMarker(facts, numbers.get(pill.key));
				return;
			}
			case "quote":
				out += `"${pill.text}"`;
				return;
			case "agent":
				out += agentMarker(pill.title);
				return;
			case "note":
				out += noteMarker(pill.title);
				return;
			case "visual": {
				const html = pageOf(pill.key);
				if (html === undefined) {
					out += visualMarker(pill.title);
					return;
				}
				// A fence reads back only on lines of its own.
				const next = segments[index + 1];
				const before = out === "" || out.endsWith("\n") ? "" : "\n";
				const after =
					next === undefined || (next.type === "text" && next.text.startsWith("\n"))
						? ""
						: "\n";
				out += `${before}${toVisualFence({ html, title: pill.title })}${after}`;
				return;
			}
		}
	});
	return out;
};
