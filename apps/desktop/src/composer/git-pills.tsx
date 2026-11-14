// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pull requests and issues in the text. A `#123`, or `owner/repo#123` for
// another repo, stays text: the composer looks each new one up after a
// short pause, and one that resolves is drawn as a pill in its state's
// shade, with a preview on hover. The caret still goes inside, so changing
// the digits looks the new number up. Keep as text stops drawing a number
// that means something else, such as a step.

import type { AgentId, Timestamp } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { type GitItemRef, type GitTarget, gitKey, gitNumbers } from "@maxhq/shared/pills/draft";
import { sameRepo } from "@maxhq/shared/pills/git-link";
import { middleTruncate } from "@maxhq/shared/pills/label";
import { Extension } from "@tiptap/core";
import type { Node as ProseNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { Popover, PopoverPopup } from "@/components/ui/popover";
import { gitFamily, gitIconMask } from "@/pills/pill";
import { GitItemDetails } from "@/pills/registry";
import { PREVIEW_DELAY_MS } from "@/pills/shown";

import { markedTextOf, posOf } from "./doc";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

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

const decorate = (doc: ProseNode, state: GitPillsState): DecorationSet => {
	const decorations = gitNumbers(markedTextOf(doc)).flatMap((span) => {
		const key = gitKey(span);
		const item = shownItem(state, key);
		if (!item) return [];
		return [
			Decoration.inline(posOf(doc, span.from), posOf(doc, span.to), {
				class: "pill pill-text",
				"data-git-key": key,
				"data-git-number": String(span.number),
				"data-family": gitFamily(item.state),
				"data-state": "ready",
				"data-title": middleTruncate(item.title),
				style: `--pill-icon: ${gitIconMask(item.kind, item.state)}`,
			}),
		];
	});
	return DecorationSet.create(doc, decorations);
};

const GIT_PILLS = new PluginKey<DecorationSet>("gitPills");

/** Draws each `#123` and `owner/repo#123` that resolved as its pill. */
export const GitPills = Extension.create<{ store: GitPillsStore | null }>({
	name: "gitPills",
	addOptions: () => ({ store: null }),
	addProseMirrorPlugins() {
		const { store } = this.options;
		if (!store) return [];
		return [
			new Plugin<DecorationSet>({
				key: GIT_PILLS,
				state: {
					init: (_, state) => decorate(state.doc, store.getState()),
					apply: (tr, old, _, state) =>
						tr.docChanged || tr.getMeta(GIT_PILLS)
							? decorate(state.doc, store.getState())
							: old,
				},
				props: { decorations: (state) => GIT_PILLS.getState(state) },
				view: (view) => ({
					destroy: store.subscribe((next, previous) => {
						if (next.lookups === previous.lookups && next.kept === previous.kept)
							return;
						if (!view.isDestroyed)
							view.dispatch(view.state.tr.setMeta(GIT_PILLS, true));
					}),
				}),
			}),
		];
	},
});

/** A drawn `#123` under the pointer, by its key. */
export type GitHover = { readonly key: string; readonly element: Element };

/** The drawn `#123` an event happened on, or null. */
export const gitHoverAt = (target: EventTarget | null): GitHover | null => {
	const element =
		target instanceof Element ? target.closest<HTMLElement>("[data-git-key]") : null;
	const key = element?.dataset.gitKey;
	return element && key ? { key, element } : null;
};

const CLOSE_DELAY_MS = 120;

/**
 * The preview for the drawn `#123` under the pointer, which opens after a
 * short pause and stays while the pointer is on it.
 */
export const useGitHover = () => {
	const [shown, setShown] = useState<GitHover | null>(null);
	const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
	const clear = () => {
		if (timer.current !== null) clearTimeout(timer.current);
		timer.current = null;
	};
	useEffect(() => clear, []);
	const later = (next: GitHover | null, delay: number) => {
		clear();
		timer.current = setTimeout(() => setShown(next), delay);
	};
	return {
		shown,
		/** From the text box: the pill under the pointer, or null once it leaves. */
		hover: (next: GitHover | null) => {
			if (next && next.element === shown?.element) clear();
			else if (next) later(next, shown ? 0 : PREVIEW_DELAY_MS);
			else later(null, CLOSE_DELAY_MS);
		},
		/** The pointer moved onto the preview, or off it. */
		hold: (held: boolean) => (held ? clear() : later(null, CLOSE_DELAY_MS)),
		close: () => {
			clear();
			setShown(null);
		},
	};
};

const KEEP =
	"mt-2 h-6 rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

/** The preview of a drawn `#123`, with Keep as text. */
export const GitPillPreview = ({
	store,
	shown,
	now,
	onHold,
	onClose,
}: {
	store: GitPillsStore;
	shown: GitHover | null;
	now: Timestamp;
	onHold: (held: boolean) => void;
	onClose: () => void;
}) => {
	useI18n();
	const item = useStore(store, (state) => (shown ? shownItem(state, shown.key) : null));
	if (!shown || !item) return null;
	return (
		<Popover
			open
			onOpenChange={(open) => {
				if (!open) onClose();
			}}
		>
			<PopoverPopup
				anchor={shown.element}
				side="top"
				align="start"
				initialFocus={false}
				finalFocus={false}
				className="p-2.5 text-xs shadow-lg"
				onMouseEnter={() => onHold(true)}
				onMouseLeave={() => onHold(false)}
			>
				<GitItemDetails item={item} now={now} />
				<button
					type="button"
					className={KEEP}
					onMouseDown={(event) => event.preventDefault()}
					onClick={() => {
						store.getState().keep(shown.key);
						onClose();
					}}
				>
					{translate("Keep as text")}
				</button>
			</PopoverPopup>
		</Popover>
	);
};
