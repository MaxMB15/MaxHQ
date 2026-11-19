// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pull requests and issues in a draft's text. A `#123`, or `owner/repo#123`
// for another repo, stays text: a box looks each new one up after a short
// pause, and one that resolves shows as a pill. Keep as text stops showing a
// number that means something else, such as a step.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AgentId } from "../bindings";
import type { DataSource } from "../data/source";
import { type GitItemRef, type GitTarget, gitKey, gitNumbers } from "./draft";
import { sameRepo } from "./git-link";

/** How long typing pauses before a new number is looked up. */
export const GIT_LOOKUP_DELAY_MS = 180;

/** What a `#123` or `owner/repo#123` in the text turned out to be. */
export type GitLookup =
	| { readonly state: "pending" }
	| { readonly state: "found"; readonly item: GitItemRef }
	| { readonly state: "missing" };

export type GitPillsState = {
	/** By each one's `gitKey`. */
	readonly lookups: ReadonlyMap<string, GitLookup>;
	/** The keys of those you chose to keep as text. */
	readonly kept: ReadonlySet<string>;
	/**
	 * Adds what's already known by key, such as a pick from the menu or a
	 * queued message's refs, and draws each as a pill again if it was kept
	 * as text.
	 */
	readonly seed: (items: ReadonlyMap<string, GitItemRef>) => void;
	/** Draws one as text from now on. */
	readonly keep: (key: string) => void;
	/** Looks up, after a pause, each one in the text not asked about yet. */
	readonly want: (targets: readonly GitTarget[]) => void;
	/** Stops a lookup that hasn't started. */
	readonly dispose: () => void;
};

export type GitPillsStore = StoreApi<GitPillsState>;

/**
 * Looks a pull request or issue up by its number, in the agent's repo or
 * the one the target names, or gives null for none.
 */
export const gitFinder =
	(source: DataSource, agent: AgentId) =>
	async ({ repo, number }: GitTarget): Promise<GitItemRef | null> => {
		const search = await (repo === null
			? source.findGitItems(agent, String(number))
			: source.findGitItems(agent, String(number), repo));
		return (
			search.items.find(
				(item) => item.number === number && (repo === null || sameRepo(item.repo, repo))
			) ?? null
		);
	};

/** A store that finds each one with `find`, which gives null for one that isn't there. */
export const createGitPills = (find: (target: GitTarget) => Promise<GitItemRef | null>) => {
	let timer: ReturnType<typeof setTimeout> | null = null;
	let wanted = new Map<string, GitTarget>();

	return createStore<GitPillsState>()((set, get) => {
		const settle = (key: string, lookup: GitLookup) => {
			const lookups = new Map(get().lookups);
			if (lookups.get(key)?.state !== "pending") return;
			lookups.set(key, lookup);
			set({ lookups });
		};

		const lookUp = () => {
			timer = null;
			const current = wanted;
			const lookups = new Map(get().lookups);
			for (const [key, lookup] of lookups)
				if (lookup.state === "pending" && !current.has(key)) lookups.delete(key);
			set({ lookups });
			for (const [key, target] of current) {
				if (lookups.get(key)?.state !== "pending") continue;
				find(target).then(
					(item) => settle(key, item ? { state: "found", item } : { state: "missing" }),
					() => settle(key, { state: "missing" })
				);
			}
		};

		return {
			lookups: new Map(),
			kept: new Set(),
			seed: (items) => {
				if (items.size === 0) return;
				const lookups = new Map(get().lookups);
				const kept = new Set(get().kept);
				for (const [key, item] of items) {
					lookups.set(key, { state: "found", item });
					kept.delete(key);
				}
				set({ lookups, kept });
			},
			keep: (key) => set({ kept: new Set(get().kept).add(key) }),
			want: (targets) => {
				wanted = new Map(targets.map((target) => [gitKey(target), target]));
				const fresh = [...wanted.keys()].filter((key) => !get().lookups.has(key));
				if (fresh.length > 0) {
					const lookups = new Map(get().lookups);
					for (const key of fresh) lookups.set(key, { state: "pending" });
					set({ lookups });
				}
				if (timer !== null) clearTimeout(timer);
				timer = [...get().lookups.values()].some((lookup) => lookup.state === "pending")
					? setTimeout(lookUp, GIT_LOOKUP_DELAY_MS)
					: null;
			},
			dispose: () => {
				if (timer !== null) clearTimeout(timer);
				timer = null;
			},
		};
	});
};

/** The item a key shows as a pill, or null while it shows as text. */
export const shownItem = (state: GitPillsState, key: string): GitItemRef | null => {
	const lookup = state.lookups.get(key);
	return lookup?.state === "found" && !state.kept.has(key) ? lookup.item : null;
};

/** The pull requests and issues the text shows as pills, in the order it names them. */
export const shownItems = (state: GitPillsState, text: string): GitItemRef[] => {
	const seen = new Set<string>();
	return gitNumbers(text).flatMap((span) => {
		const key = gitKey(span);
		const item = seen.has(key) ? null : shownItem(state, key);
		seen.add(key);
		return item ? [item] : [];
	});
};
