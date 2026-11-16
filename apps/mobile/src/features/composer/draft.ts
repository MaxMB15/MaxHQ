// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The phone's draft: the text with its pills, and the files you added with
// the paperclip, which show in the strip above the field rather than in
// the text. They go at the end of the message, each after a space, the way
// a file placed last goes from the desktop, so the agent gets the same
// message either way. A draft that comes back, saved or taken from the
// queue, gives the files at its end back to the strip.

import type { AttachmentId, AttachmentRef, DeviceId, UserInput } from "@maxhq/shared/bindings";
import type { Copies, FileFailure } from "@maxhq/shared/composer/copies";
import type { QueuedMessage } from "@maxhq/shared/composer/send";
import { clipboardFileOf, type StagedFile } from "@maxhq/shared/composer/staging";
import type { ClipboardFragment } from "@maxhq/shared/pills/clipboard";
import {
	excerptIds,
	fileKeys,
	fromUserInput,
	type GitItemRef,
	isBlank,
	keyedGitItems,
	markedText,
	normalize,
	type Segment,
	toUserInput,
	trimEnd,
	withExcerpts,
} from "@maxhq/shared/pills/draft";

/** What the box holds: the text with its pills, and the files in the strip by key. */
export type PhoneDraft = {
	readonly segments: readonly Segment[];
	readonly loose: readonly string[];
};

/** An empty box. */
export const EMPTY_DRAFT: PhoneDraft = { segments: [], loose: [] };

/** The draft as one message: the text, then each file in the strip after a space. */
export const wholeOf = (draft: PhoneDraft): Segment[] => {
	if (draft.loose.length === 0) return normalize(draft.segments);
	const out = trimEnd(draft.segments);
	for (const key of draft.loose) {
		const before = markedText(out);
		if (before !== "" && !/\s$/.test(before)) out.push({ type: "text", text: " " });
		out.push({ type: "pill", pill: { type: "attachment", key } });
	}
	return normalize(out);
};

/**
 * A message as the box takes it back: the files at its end, with only
 * spaces between them, go to the strip, and the text before them stays.
 */
export const splitLoose = (segments: readonly Segment[]): PhoneDraft => {
	const out = normalize(segments);
	const loose: string[] = [];
	let cut = out.length;
	for (let at = out.length - 1; at >= 0; at -= 1) {
		const segment = out[at]!;
		if (segment.type === "pill" && segment.pill.type === "attachment") {
			if (!loose.includes(segment.pill.key)) loose.unshift(segment.pill.key);
			cut = at;
			continue;
		}
		if (segment.type === "text" && segment.text.trim() === "") continue;
		break;
	}
	return loose.length === 0
		? { segments: out, loose }
		: { segments: trimEnd(out.slice(0, cut)), loose };
};

/** The keys of the files the draft holds, in the text or in the strip. */
export const draftKeys = (draft: PhoneDraft): Set<string> =>
	new Set([...fileKeys(draft.segments), ...draft.loose]);

/** Whether the draft has something to send. */
export const filled = (draft: PhoneDraft): boolean =>
	draft.loose.length > 0 || !isBlank({ segments: draft.segments, gitItems: [] });

/**
 * A draft brought back, such as a queued message to edit, put before what
 * the box holds, with a blank line between them.
 */
export const withRestored = (restored: PhoneDraft, current: PhoneDraft): PhoneDraft => {
	const loose = [...new Set([...restored.loose, ...current.loose])];
	if (isBlank({ segments: current.segments, gitItems: [] }))
		return { segments: restored.segments, loose };
	return {
		segments: normalize([
			...restored.segments,
			{ type: "text", text: "\n\n" },
			...current.segments,
		]),
		loose,
	};
};

/** A queued message as it goes to the agent again, such as when it steers. */
export const queuedInput = (message: QueuedMessage): UserInput => ({
	text: message.text,
	attachments: message.attachments.map((ref) => ref.id),
	...(message.refs && message.refs.length > 0 ? { refs: message.refs } : {}),
});

/**
 * The files of a queued message that come back with it. An agent's excerpt
 * or a note's copy was for that send only, and the next one takes a new
 * one.
 */
export const queuedFiles = (message: QueuedMessage): AttachmentRef[] => {
	const excerpts = excerptIds(message.refs);
	return message.attachments.filter((ref) => !excerpts.has(ref.id));
};

/** What comes back of a draft: the draft, and its pull requests and issues by key. */
export type Restored = {
	readonly draft: PhoneDraft;
	readonly gitItems: ReadonlyMap<string, GitItemRef>;
};

/** A queued message as a draft again, each file under the key `keyOf` gives it. */
export const queuedDraft = (
	message: QueuedMessage,
	keyOf: (id: AttachmentId) => string
): Restored => {
	const restored = fromUserInput(queuedInput(message), keyOf);
	return {
		draft: splitLoose(restored.segments),
		gitItems: keyedGitItems(markedText(restored.segments), restored.gitItems),
	};
};

/** A draft as the phone saves it. */
export const fragmentOf = (
	draft: PhoneDraft,
	files: readonly StagedFile[],
	device: DeviceId,
	gitItems: readonly GitItemRef[]
): ClipboardFragment => {
	const keys = draftKeys(draft);
	return {
		version: 1,
		origin: "",
		segments: wholeOf(draft),
		files: files
			.filter((file) => keys.has(file.key))
			.map((file) => clipboardFileOf(file, device)),
		gitItems,
	};
};

/** A saved draft as the box takes it back. */
export const draftOfFragment = (fragment: ClipboardFragment): Restored => ({
	draft: splitLoose(fragment.segments),
	gitItems: keyedGitItems(markedText(fragment.segments), fragment.gitItems),
});

/** The files a message waits on that couldn't be readied, with why. */
export const unreadyFiles = (
	whole: readonly Segment[],
	files: readonly StagedFile[]
): FileFailure[] => {
	const keys = new Set(fileKeys(whole));
	return files
		.filter((file) => keys.has(file.key) && file.state !== "ready")
		.map((file) => ({ name: file.name, error: file.error }));
};

/**
 * The message to send, in the wire format the desktop sends: each pill a
 * mark in the text with its ref, the files in the strip last, without the
 * spaces at the end, and with each copy the send took.
 */
export const messageInput = (
	whole: readonly Segment[],
	gitItems: readonly GitItemRef[],
	files: readonly StagedFile[],
	copies: Pick<Copies, "excerpts" | "notes">,
	visuals: boolean
): UserInput => {
	const ids = new Map(
		files.flatMap((file) =>
			file.state === "ready" && file.ref ? [[file.key, file.ref.id] as const] : []
		)
	);
	return withExcerpts(
		toUserInput({ segments: trimEnd(whole), gitItems }, (key) => ids.get(key) ?? null, {
			visuals,
		}),
		copies.excerpts,
		copies.notes
	);
};
