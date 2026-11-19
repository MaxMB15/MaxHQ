// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Code highlighting on the phone, with Shiki as on the desktop. The phone
// has no workers and no WebAssembly, so Shiki runs on the JavaScript thread
// with its JavaScript regex engine. It loads the first time code asks for
// colours, and each grammar the first time its language does. Shiki works
// through code a slice of lines at a time, with a break for the app between
// slices, so the feed keeps scrolling while it works, and one job runs at a
// time. Code stays plain until its colours come, and for good when Shiki
// can't do it.
//
// The phone carries fewer grammars than the desktop, to keep the app small:
// JavaScript draws with TypeScript's, and a few large ones stay plain.

import { useEffect, useState } from "react";
import type { TextStyle } from "react-native";
import type { GrammarState, HighlighterCore, LanguageRegistration, RegexEngine } from "shiki/core";

import {
	CODE_THEME,
	codeColors,
	createHighlighter,
	isBold,
	isItalic,
	type Language,
	type Lines,
	type Token,
} from "@maxhq/shared/agent-view/highlight";

import { codeColor } from "../theme";
import { HAS_INDICES, withGuessedIndices } from "./match-indices";

export {
	languageOf,
	type Language,
	type Lines,
	type Token,
} from "@maxhq/shared/agent-view/highlight";

type Grammar = () => LanguageRegistration[];
type GrammarModule = { default: LanguageRegistration[] };

// Metro needs each name written out, and a require, which runs only when
// called, where an import would read every grammar as the app starts.
/* eslint-disable @typescript-eslint/no-require-imports */
const GRAMMARS: Partial<Record<Language, Grammar>> = {
	css: () => (require("shiki/langs/css.mjs") as GrammarModule).default,
	diff: () => (require("shiki/langs/diff.mjs") as GrammarModule).default,
	dockerfile: () => (require("shiki/langs/dockerfile.mjs") as GrammarModule).default,
	go: () => (require("shiki/langs/go.mjs") as GrammarModule).default,
	ini: () => (require("shiki/langs/ini.mjs") as GrammarModule).default,
	json: () => (require("shiki/langs/json.mjs") as GrammarModule).default,
	jsonc: () => (require("shiki/langs/jsonc.mjs") as GrammarModule).default,
	kotlin: () => (require("shiki/langs/kotlin.mjs") as GrammarModule).default,
	nix: () => (require("shiki/langs/nix.mjs") as GrammarModule).default,
	python: () => (require("shiki/langs/python.mjs") as GrammarModule).default,
	rust: () => (require("shiki/langs/rust.mjs") as GrammarModule).default,
	shellscript: () => (require("shiki/langs/shellscript.mjs") as GrammarModule).default,
	sql: () => (require("shiki/langs/sql.mjs") as GrammarModule).default,
	toml: () => (require("shiki/langs/toml.mjs") as GrammarModule).default,
	tsx: () => (require("shiki/langs/tsx.mjs") as GrammarModule).default,
	typescript: () => (require("shiki/langs/typescript.mjs") as GrammarModule).default,
	yaml: () => (require("shiki/langs/yaml.mjs") as GrammarModule).default,
};

// Languages drawn with another one's grammar, which reads them as well.
const STAND_INS: Partial<Record<Language, Language>> = {
	javascript: "typescript",
	jsx: "tsx",
};

/** The grammar the phone draws `lang` with, or null when it stays plain. */
export const grammarFor = (lang: Language): Language | null => {
	const name = STAND_INS[lang] ?? lang;
	return GRAMMARS[name] ? name : null;
};

const COLORS = codeColors((name) => codeColor[name]);

// Shiki stops a line after this long and leaves the rest of it plain. The
// first lines of a grammar compile its regexes, which takes a while.
const LINE_TIME_LIMIT = 1_000;

// A line longer than this stays plain, as a minified file's would take long.
const LINE_LENGTH_LIMIT = 2_000;

/**
 * Shiki's JavaScript regex engine, which guesses where groups match when the
 * engine has no `d` flag. A pattern it can't run is left out of its grammar,
 * so the rest of the grammar still works.
 */
export const engineFor = (hasIndices: boolean): RegexEngine => {
	const { createJavaScriptRegexEngine, defaultJavaScriptRegexConstructor } =
		require("shiki/engine/javascript") as typeof import("shiki/engine/javascript");
	return createJavaScriptRegexEngine({
		forgiving: true,
		regexConstructor: (pattern) => {
			const regex = defaultJavaScriptRegexConstructor(pattern, {
				target: "auto",
				hasIndices,
			});
			return hasIndices ? regex : withGuessedIndices(regex);
		},
	});
};

let core: HighlighterCore | null = null;

// Builds Shiki the first time it's needed.
const shiki = (): HighlighterCore => {
	if (core) return core;
	const { createHighlighterCoreSync } = require("shiki/core") as typeof import("shiki/core");
	const theme = (
		require("shiki/themes/github-dark-default.mjs") as typeof import("shiki/themes/github-dark-default.mjs")
	).default;
	core = createHighlighterCoreSync({
		themes: [theme],
		langs: [],
		engine: engineFor(HAS_INDICES),
	});
	return core;
};
/* eslint-enable @typescript-eslint/no-require-imports */

// How long each slice should take, so a frame or two goes by between them.
const SLICE_MS = 12;

/** How many lines the next slice takes, from how long the last one took. */
export const nextStep = (step: number, took: number): number =>
	Math.max(1, Math.min(500, Math.round((step * SLICE_MS) / Math.max(1, took))));

// A break that lets the app handle touches and draw before the next slice.
const breather = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** How a slice of code becomes tokens, and how the next slice picks up. */
type Tokenizer = Pick<HighlighterCore, "codeToTokens">;

/**
 * Highlights `code` a slice of lines at a time, each slice picking up the
 * grammar's state where the one before it left off. `step` says how many
 * lines each slice takes, from the last one's and how long it took.
 */
export const tokenizeSliced = async (
	tokenizer: Tokenizer,
	code: string,
	lang: string,
	{
		colors = COLORS,
		pause = breather,
		step = nextStep,
	}: {
		colors?: Readonly<Record<string, string>>;
		pause?: () => Promise<void>;
		step?: (step: number, took: number) => number;
	} = {}
): Promise<Lines> => {
	const lines = code.split(/\r\n|\r|\n/);
	const out: Token[][] = [];
	let state: GrammarState | undefined;
	let size = step(20, SLICE_MS);
	for (let at = 0; at < lines.length;) {
		const started = Date.now();
		const slice = lines.slice(at, at + size);
		const result = tokenizer.codeToTokens(slice.join("\n"), {
			lang,
			theme: CODE_THEME,
			colorReplacements: { ...colors },
			grammarState: state,
			tokenizeTimeLimit: LINE_TIME_LIMIT,
			tokenizeMaxLineLength: LINE_LENGTH_LIMIT,
		});
		state = result.grammarState;
		slice.forEach((line, i) => {
			const tokens = result.tokens[i];
			out.push(
				tokens
					? tokens.map((t) => ({
							content: t.content,
							color: t.color,
							fontStyle: t.fontStyle,
						}))
					: [{ content: line }]
			);
		});
		at += slice.length;
		size = step(size, Date.now() - started);
		if (at < lines.length) await pause();
	}
	return out;
};

// The job running now, so the next waits its turn.
let turn: Promise<unknown> = Promise.resolve();

const inTurn = <T>(job: () => Promise<T>): Promise<T> => {
	const run = turn.then(job, job);
	turn = run.catch(() => undefined);
	return run;
};

const highlighter = createHighlighter((code, lang) =>
	inTurn(async () => {
		const name = grammarFor(lang);
		if (!name) return null;
		// The screen draws the code plain before Shiki takes the thread.
		await breather();
		try {
			const loaded = shiki();
			if (!loaded.getLoadedLanguages().includes(name)) {
				loaded.loadLanguageSync(...GRAMMARS[name]!());
			}
			return await tokenizeSliced(loaded, code, name);
		} catch (error) {
			console.warn("Couldn't highlight code", error);
			return null;
		}
	})
);

/** Highlights code, or gives null when it's longer than `most` characters or Shiki fails. */
export const highlight = highlighter.highlight;

/** Highlighted lines already worked out for this code, if any. */
export const cachedLines = highlighter.cachedLines;

/**
 * Colours for `code` in `lang`: null until they're ready, and for good when
 * `lang` is null, `wanted` is false, or Shiki can't. `most` is the longest
 * code it highlights.
 */
export const useHighlight = (
	code: string,
	lang: Language | null,
	wanted = true,
	most?: number
): Lines | null => {
	const cached = lang && wanted ? (cachedLines(code, lang) ?? null) : null;
	const [found, setFound] = useState<{ code: string; lang: Language; lines: Lines } | null>(null);
	useEffect(() => {
		if (!lang || !wanted || cached) return;
		let current = true;
		void highlight(code, lang, most).then((lines) => {
			if (current && lines) setFound({ code, lang, lines });
		});
		return () => {
			current = false;
		};
	}, [code, lang, wanted, cached, most]);
	if (cached) return cached;
	return found && found.code === code && found.lang === lang ? found.lines : null;
};

const styles = new Map<string, TextStyle>();

/** A token's style, one object for each look, so rows can compare them. */
export const tokenStyle = (token: Token): TextStyle => {
	const key = `${token.color ?? ""}\n${token.fontStyle ?? 0}`;
	let style = styles.get(key);
	if (!style) {
		style = {
			color: token.color,
			fontStyle: isItalic(token) ? "italic" : undefined,
			fontWeight: isBold(token) ? "600" : undefined,
		};
		styles.set(key, style);
	}
	return style;
};
