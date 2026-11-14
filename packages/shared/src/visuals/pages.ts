// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Visual pages read from their files, kept by attachment ID. A card in the
// timeline and a sent pill's preview both read the page this way, so one
// read serves every place a page shows. A staged page goes in as it's
// added, so its pill never reads it back. The newest pages stay, up to a
// bound, and a failed read stays failed until it's tried again.

import { t } from "../i18n";

import type { AgentId, AttachmentId, AttachmentRef } from "../bindings";

/** Where a page is: on its way, read, or not read with the reason. */
export type PageState =
	| { readonly status: "loading" }
	| { readonly status: "ready"; readonly html: string }
	| { readonly status: "failed"; readonly error: string };

/** Reads a page's file through the agent that has it. */
export type PageReader = (agent: AgentId, file: AttachmentRef) => Promise<string>;

export type VisualPages = {
	/** The page's state, or undefined when nothing asked for it yet. */
	readonly get: (id: AttachmentId) => PageState | undefined;
	/** Starts reading the page unless it's read or on its way, and gives its state. */
	readonly load: (agent: AgentId, file: AttachmentRef) => PageState;
	/** Reads the page again, after a failed read. */
	readonly retry: (agent: AgentId, file: AttachmentRef) => PageState;
	/** Keeps a page this app already has, such as one it staged. */
	readonly put: (id: AttachmentId, html: string) => void;
	/** Hears every change. Returns the undo. */
	readonly subscribe: (listener: () => void) => () => void;
};

/** How many pages stay read. */
export const KEPT_PAGES = 64;

const LOADING: PageState = { status: "loading" };

const reason = (error: unknown): string =>
	error instanceof Error
		? error.message
		: typeof error === "string"
			? error
			: t("Couldn't read it");

export const createVisualPages = (read: PageReader, kept = KEPT_PAGES): VisualPages => {
	const pages = new Map<AttachmentId, PageState>();
	const listeners = new Set<() => void>();
	const changed = () => {
		for (const listener of [...listeners]) listener();
	};
	const set = (id: AttachmentId, state: PageState) => {
		pages.delete(id);
		pages.set(id, state);
		while (pages.size > kept) {
			const oldest = pages.keys().next().value;
			if (oldest === undefined) break;
			pages.delete(oldest);
		}
		changed();
	};
	const start = (agent: AgentId, file: AttachmentRef): PageState => {
		set(file.id, LOADING);
		read(agent, file).then(
			(html) => {
				if (pages.get(file.id) === LOADING) set(file.id, { status: "ready", html });
			},
			(error: unknown) => {
				if (pages.get(file.id) === LOADING)
					set(file.id, { status: "failed", error: reason(error) });
			}
		);
		return LOADING;
	};
	return {
		get: (id) => pages.get(id),
		load: (agent, file) => pages.get(file.id) ?? start(agent, file),
		retry: (agent, file) => {
			const state = pages.get(file.id);
			return state && state.status !== "failed" ? state : start(agent, file);
		},
		put: (id, html) => set(id, { status: "ready", html }),
		subscribe: (listener) => {
			listeners.add(listener);
			return () => listeners.delete(listener);
		},
	};
};
