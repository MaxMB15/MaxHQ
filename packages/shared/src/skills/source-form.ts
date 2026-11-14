// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The library page's form for a new source, read into what the request
// sends. An absolute path is a folder on the holder; anything else is a
// git repo to clone, written as a URL or as `owner/repo` on GitHub.

import { t } from "../i18n";

import type { SkillLibrary, SourceKind, SourceTrust } from "../bindings";

/** What you typed into the form. */
export type SourceDraft = {
	readonly name: string;
	readonly where: string;
	readonly ref: string;
	readonly thirdParty: boolean;
};

/** A blank form. */
export const EMPTY_SOURCE: SourceDraft = { name: "", where: "", ref: "", thirdParty: false };

/** The request a form makes, or why it can't make one yet. */
export type SourcePlan =
	| {
			readonly ok: true;
			readonly name: string;
			readonly origin: SourceKind;
			readonly ref: string | null;
			readonly trust: SourceTrust;
	  }
	| { readonly ok: false; readonly reason: string };

const OWNER_REPO = /^[\w.-]+\/[\w.-]+$/u;

/** Where a source's skills come from, from what you typed. */
export const originOf = (where: string): SourceKind | null => {
	const text = where.trim();
	if (text.startsWith("/")) return { kind: "local", path: text.replace(/\/+$/u, "") || "/" };
	if (OWNER_REPO.test(text)) return { kind: "git", url: `https://github.com/${text}` };
	if (/^(https?|ssh|git):\/\//u.test(text) || /^[\w.-]+@[\w.-]+:/u.test(text)) {
		return { kind: "git", url: text };
	}
	return null;
};

/** The last part of a URL or path, without `.git`, as a name to start from. */
const nameFrom = (origin: SourceKind): string => {
	const where = origin.kind === "git" ? origin.url : origin.path;
	const parts = where.replace(/\.git$/u, "").split(/[/:]/u);
	return parts.slice(-2).filter(Boolean).join("/");
};

/** What a form asks the holder to add. */
export const planSource = (library: SkillLibrary, draft: SourceDraft): SourcePlan => {
	const origin = originOf(draft.where);
	if (!origin) {
		return {
			ok: false,
			reason: t("Give a git URL, owner/repo on GitHub, or an absolute folder path."),
		};
	}
	const name = draft.name.trim() || nameFrom(origin);
	if (library.sources.some((source) => source.name === name)) {
		return {
			ok: false,
			reason: t("The library already has a source called {{name}}.", { name: name }),
		};
	}
	const ref = draft.ref.trim();
	if (ref && origin.kind === "local") {
		return { ok: false, reason: t("A folder has no branch or tag to fetch.") };
	}
	return {
		ok: true,
		name,
		origin,
		ref: ref || null,
		trust: draft.thirdParty ? "third_party" : "own",
	};
};
