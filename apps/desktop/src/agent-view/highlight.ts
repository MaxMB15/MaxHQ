// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Code highlighting for messages and the file viewer. Shiki runs in a worker, see
// highlight-worker.ts, and this side keeps what it sends back by a hash of
// the code, so a row that scrolls back in doesn't highlight again.

import { fnv1a } from "@maxhq/shared/lib/fnv";

const LANGUAGES = [
	"c",
	"cpp",
	"css",
	"diff",
	"dockerfile",
	"go",
	"html",
	"ini",
	"java",
	"javascript",
	"json",
	"jsonc",
	"jsx",
	"kotlin",
	"nix",
	"python",
	"ruby",
	"rust",
	"shellscript",
	"sql",
	"swift",
	"toml",
	"tsx",
	"typescript",
	"xml",
	"yaml",
] as const;

/** A language Shiki can highlight here. */
export type Language = (typeof LANGUAGES)[number];

const KNOWN: ReadonlySet<string> = new Set(LANGUAGES);

// Fence names and file extensions for the languages above.
const ALIASES: Record<string, Language> = {
	bash: "shellscript",
	cc: "cpp",
	"c++": "cpp",
	cfg: "ini",
	cjs: "javascript",
	cts: "typescript",
	cxx: "cpp",
	docker: "dockerfile",
	h: "c",
	hh: "cpp",
	hpp: "cpp",
	htm: "html",
	hxx: "cpp",
	js: "javascript",
	kt: "kotlin",
	kts: "kotlin",
	mjs: "javascript",
	mts: "typescript",
	patch: "diff",
	py: "python",
	pyi: "python",
	rb: "ruby",
	rs: "rust",
	sh: "shellscript",
	shell: "shellscript",
	ts: "typescript",
	yml: "yaml",
	zsh: "shellscript",
};

/** Code longer than this stays plain, as it would keep the worker busy too long. */
export const MAX_HIGHLIGHT = 20_000;

/** One run of text in one style. */
export type Token = {
	readonly content: string;
	readonly color?: string;
	readonly fontStyle?: number;
};

/** Highlighted code, one list of tokens per line. */
export type Lines = readonly (readonly Token[])[];

/** What the page asks the worker. */
export type HighlightJob = { readonly id: number; readonly code: string; readonly lang: Language };

/** What the worker answers, null when Shiki failed. */
export type HighlightReply = { readonly id: number; readonly lines: Lines | null };

/** The language a fence or file name names, if Shiki can do it here. */
export const languageOf = (name: string | null | undefined): Language | null => {
	if (!name) return null;
	const key = name.trim().toLowerCase();
	if (KNOWN.has(key)) return key as Language;
	return ALIASES[key] ?? null;
};

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

const CACHE_SIZE = 300;
const cache = new Map<string, Lines>();

// With the length and language beside it, two blocks sharing a key is
// unlikely enough for a display cache.
const keyOf = (code: string, lang: Language) => `${lang}:${code.length}:${fnv1a(code)}`;

/** Highlighted lines already worked out for this code, if any. */
export const cachedLines = (code: string, lang: Language): Lines | undefined => {
	const key = keyOf(code, lang);
	const lines = cache.get(key);
	if (lines) {
		// Most recently used goes last, so the oldest is the first to go.
		cache.delete(key);
		cache.set(key, lines);
	}
	return lines;
};

const remember = (key: string, lines: Lines) => {
	cache.set(key, lines);
	if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value as string);
};

// Blocks already on their way to the worker, so two rows with the same code
// share one job.
const inflight = new Map<string, Promise<Lines | null>>();

const ask = (started: Worker | null, code: string, lang: Language) =>
	new Promise<Lines | null>((resolve) => {
		if (!started) return resolve(null);
		const id = nextId++;
		waiting.set(id, resolve);
		const job: HighlightJob = { id, code, lang };
		started.postMessage(job);
	});

/**
 * Highlights code, or gives null when it's longer than `most` characters or
 * Shiki fails.
 */
export const highlight = (
	code: string,
	lang: Language,
	most: number = MAX_HIGHLIGHT
): Promise<Lines | null> => {
	if (code.length > most) return Promise.resolve(null);
	const hit = cachedLines(code, lang);
	if (hit) return Promise.resolve(hit);
	const key = keyOf(code, lang);
	let job = inflight.get(key);
	if (!job) {
		job = workerFor()
			.then((started) => ask(started, code, lang))
			.then((lines) => {
				if (lines) remember(key, lines);
				return lines;
			})
			.finally(() => inflight.delete(key));
		inflight.set(key, job);
	}
	return job;
};
