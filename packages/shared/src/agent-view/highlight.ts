// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What code highlighting is the same for on every client: the languages
// Shiki highlights, the names that pick one, the theme colours each stand for
// a code colour of the app's, and a cache of highlighted code by a hash of
// it, so a row that scrolls back in doesn't highlight again. Each client runs
// Shiki its own way, the desktop in a worker and the phone between frames.

import { fnv1a } from "../lib/fnv";

export const LANGUAGES = [
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

/** Code longer than this stays plain, as it would keep the highlighter busy too long. */
export const MAX_HIGHLIGHT = 20_000;

/** One run of text in one style. */
export type Token = {
	readonly content: string;
	readonly color?: string;
	readonly fontStyle?: number;
};

/** Highlighted code, one list of tokens per line. */
export type Lines = readonly (readonly Token[])[];

/** The language a fence or file name names, if Shiki can do it here. */
export const languageOf = (name: string | null | undefined): Language | null => {
	if (!name) return null;
	const key = name.trim().toLowerCase();
	if (KNOWN.has(key)) return key as Language;
	return ALIASES[key] ?? null;
};

/** The theme every client highlights with. */
export const CODE_THEME = "github-dark-default";

/** One of the app's code colours. */
export type CodeColor =
	| "text"
	| "comment"
	| "keyword"
	| "constant"
	| "variable"
	| "function"
	| "tag"
	| "string"
	| "invalid";

// The theme's colours each stand for one of the app's code colours, so
// highlighted code takes the app's theme, and a new theme needs no new pass.
// The near white and near black ones mark text that's otherwise plain.
const THEME_COLORS: Readonly<Record<string, CodeColor>> = {
	"#e6edf3": "text",
	"#f0f6fc": "text",
	"#161b22": "text",
	"#8b949e": "comment",
	"#ff7b72": "keyword",
	"#79c0ff": "constant",
	"#ffa657": "variable",
	"#d2a8ff": "function",
	"#7ee787": "tag",
	"#a5d6ff": "string",
	"#ffa198": "invalid",
};

/** Shiki's colour replacements that turn the theme's colours into a client's own. */
export const codeColors = (
	colorOf: (color: CodeColor) => string
): Readonly<Record<string, string>> =>
	Object.fromEntries(Object.entries(THEME_COLORS).map(([hex, name]) => [hex, colorOf(name)]));

/** Whether a token's font style asks for italics, as Shiki's bit 1. */
export const isItalic = (token: Token): boolean => !!token.fontStyle && (token.fontStyle & 1) !== 0;

/** Whether a token's font style asks for bold, as Shiki's bit 2. */
export const isBold = (token: Token): boolean => !!token.fontStyle && (token.fontStyle & 2) !== 0;

/** Highlights code, or gives null when it can't. */
export type Highlight = (code: string, lang: Language) => Promise<Lines | null>;

/** A client's highlighter: code highlighted once, cached and shared between asks. */
export type Highlighter = {
	/**
	 * Highlights code, or gives null when it's longer than `most` characters
	 * or Shiki fails.
	 */
	readonly highlight: (code: string, lang: Language, most?: number) => Promise<Lines | null>;
	/** Highlighted lines already worked out for this code, if any. */
	readonly cachedLines: (code: string, lang: Language) => Lines | undefined;
};

const CACHE_SIZE = 300;

// With the length and language beside it, two blocks sharing a key is
// unlikely enough for a display cache.
const keyOf = (code: string, lang: Language) => `${lang}:${code.length}:${fnv1a(code)}`;

/** A highlighter that runs `run` for code it hasn't highlighted yet. */
export const createHighlighter = (run: Highlight): Highlighter => {
	const cache = new Map<string, Lines>();
	// Code already on its way, so two rows with the same code share one job.
	const inflight = new Map<string, Promise<Lines | null>>();

	const cachedLines = (code: string, lang: Language): Lines | undefined => {
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

	const highlight = (
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
			job = run(code, lang)
				.catch(() => null)
				.then((lines) => {
					if (lines) remember(key, lines);
					return lines;
				})
				.finally(() => inflight.delete(key));
			inflight.set(key, job);
		}
		return job;
	};

	return { highlight, cachedLines };
};
