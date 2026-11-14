// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Shiki in a worker, so building the highlighter, compiling a grammar and
// tokenizing a long block never cost the page a frame. It uses Shiki's
// JavaScript regex engine, since the app's content policy doesn't allow
// WebAssembly. Each grammar loads the first time a block in its language
// asks for it.

import type { HighlighterCore, LanguageRegistration } from "shiki/core";

import type { HighlightJob, HighlightReply, Language, Lines } from "./highlight";
import { regexFor } from "./highlight-regex";

type Grammar = () => Promise<{ default: LanguageRegistration[] }>;

// Vite needs each dynamic import spelled out to find it. The dev server loads
// each one on demand, and a build folds them all into the worker's one file.
export const GRAMMARS = {
	c: () => import("shiki/langs/c.mjs"),
	cpp: () => import("shiki/langs/cpp.mjs"),
	css: () => import("shiki/langs/css.mjs"),
	diff: () => import("shiki/langs/diff.mjs"),
	dockerfile: () => import("shiki/langs/dockerfile.mjs"),
	go: () => import("shiki/langs/go.mjs"),
	html: () => import("shiki/langs/html.mjs"),
	ini: () => import("shiki/langs/ini.mjs"),
	java: () => import("shiki/langs/java.mjs"),
	javascript: () => import("shiki/langs/javascript.mjs"),
	json: () => import("shiki/langs/json.mjs"),
	jsonc: () => import("shiki/langs/jsonc.mjs"),
	jsx: () => import("shiki/langs/jsx.mjs"),
	kotlin: () => import("shiki/langs/kotlin.mjs"),
	nix: () => import("shiki/langs/nix.mjs"),
	python: () => import("shiki/langs/python.mjs"),
	ruby: () => import("shiki/langs/ruby.mjs"),
	rust: () => import("shiki/langs/rust.mjs"),
	shellscript: () => import("shiki/langs/shellscript.mjs"),
	sql: () => import("shiki/langs/sql.mjs"),
	swift: () => import("shiki/langs/swift.mjs"),
	toml: () => import("shiki/langs/toml.mjs"),
	tsx: () => import("shiki/langs/tsx.mjs"),
	typescript: () => import("shiki/langs/typescript.mjs"),
	xml: () => import("shiki/langs/xml.mjs"),
	yaml: () => import("shiki/langs/yaml.mjs"),
} satisfies Record<Language, Grammar>;

const THEME = "github-dark-default";

// The theme's colours each stand for one of the app's `--code-*` tokens, so
// highlighted code takes the app's theme, and a new theme needs no new pass.
// The near white and near black ones mark text that's otherwise plain.
export const CODE_COLORS: Readonly<Record<string, string>> = {
	"#e6edf3": "var(--code-text)",
	"#f0f6fc": "var(--code-text)",
	"#161b22": "var(--code-text)",
	"#8b949e": "var(--code-comment)",
	"#ff7b72": "var(--code-keyword)",
	"#79c0ff": "var(--code-constant)",
	"#ffa657": "var(--code-variable)",
	"#d2a8ff": "var(--code-function)",
	"#7ee787": "var(--code-tag)",
	"#a5d6ff": "var(--code-string)",
	"#ffa198": "var(--code-invalid)",
};

// Shiki stops a line after 500 ms by default and leaves the rest of it plain.
// A grammar's regexes compile the first time they run, and in WebKit that
// alone can take a cold first line past 500 ms. The limit is only checked
// between matches, and the worker never holds up the page, so this one only
// cuts lines that are truly slow.
const LINE_TIME_LIMIT = 5_000;

let core: Promise<HighlighterCore> | null = null;

const highlighter = (): Promise<HighlighterCore> => {
	core ??= (async () => {
		const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, theme] =
			await Promise.all([
				import("shiki/core"),
				import("shiki/engine/javascript"),
				import("shiki/themes/github-dark-default.mjs"),
			]);
		return createHighlighterCore({
			themes: [theme.default],
			langs: [],
			engine: createJavaScriptRegexEngine({ regexConstructor: regexFor }),
		});
	})();
	return core;
};

const loaded = new Map<Language, Promise<void>>();

const loadGrammar = (shiki: HighlighterCore, lang: Language): Promise<void> => {
	let loading = loaded.get(lang);
	if (!loading) {
		loading = GRAMMARS[lang]().then((grammar) => shiki.loadLanguage(...grammar.default));
		loaded.set(lang, loading);
	}
	return loading;
};

/** Highlights code, or gives null when Shiki can't. */
export const tokenize = async ({ code, lang }: HighlightJob): Promise<Lines | null> => {
	if (!(lang in GRAMMARS)) return null;
	try {
		const shiki = await highlighter();
		await loadGrammar(shiki, lang);
		return shiki
			.codeToTokensBase(code, {
				lang,
				theme: THEME,
				colorReplacements: CODE_COLORS,
				tokenizeTimeLimit: LINE_TIME_LIMIT,
			})
			.map((line) =>
				line.map((t) => ({ content: t.content, color: t.color, fontStyle: t.fontStyle }))
			);
	} catch (error) {
		console.warn("Couldn't highlight code", error);
		return null;
	}
};

self.addEventListener("message", (event: MessageEvent<HighlightJob>) => {
	void tokenize(event.data).then((lines) => {
		const reply: HighlightReply = { id: event.data.id, lines };
		self.postMessage(reply);
	});
});
