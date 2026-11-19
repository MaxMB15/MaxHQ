// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Code highlighting for messages and the file viewer. Shiki runs in a worker,
// see highlight-worker.ts, and the shared highlighter keeps what it sends
// back by a hash of the code, so a row that scrolls back in doesn't
// highlight again.

import { createHighlighter, type Language, type Lines } from "@maxhq/shared/agent-view/highlight";

export {
	languageOf,
	MAX_HIGHLIGHT,
	type Language,
	type Lines,
	type Token,
} from "@maxhq/shared/agent-view/highlight";

/** What the page asks the worker. */
export type HighlightJob = { readonly id: number; readonly code: string; readonly lang: Language };

/** What the worker answers, null when Shiki failed. */
export type HighlightReply = { readonly id: number; readonly lines: Lines | null };

const waiting = new Map<number, (lines: Lines | null) => void>();
let nextId = 0;
let worker: Promise<Worker | null> | null = null;

// Starts the worker the first time a block needs it. Where there are no
// workers, as in tests, code stays plain.
const workerFor = (): Promise<Worker | null> => {
	worker ??=
		typeof Worker === "undefined"
			? Promise.resolve(null)
			: import("./highlight-worker?worker").then(
					({ default: HighlightWorker }) => {
						const started = new HighlightWorker();
						started.addEventListener(
							"message",
							(event: MessageEvent<HighlightReply>) => {
								waiting.get(event.data.id)?.(event.data.lines);
								waiting.delete(event.data.id);
							}
						);
						started.addEventListener("error", (event) => {
							console.warn("The highlighter stopped", event.message);
							for (const done of waiting.values()) done(null);
							waiting.clear();
						});
						return started;
					},
					(error: unknown) => {
						console.warn("Couldn't start the highlighter", error);
						return null;
					}
				);
	return worker;
};

const ask = (started: Worker | null, code: string, lang: Language) =>
	new Promise<Lines | null>((resolve) => {
		if (!started) return resolve(null);
		const id = nextId++;
		waiting.set(id, resolve);
		const job: HighlightJob = { id, code, lang };
		started.postMessage(job);
	});

const highlighter = createHighlighter((code, lang) =>
	workerFor().then((started) => ask(started, code, lang))
);

/**
 * Highlights code, or gives null when it's longer than `most` characters or
 * Shiki fails.
 */
export const highlight = highlighter.highlight;

/** Highlighted lines already worked out for this code, if any. */
export const cachedLines = highlighter.cachedLines;
