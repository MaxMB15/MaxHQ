// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pull requests and issues in the text. A `#123`, or `owner/repo#123` for
// another repo, stays text: the composer looks each new one up after a
// short pause, and one that resolves is drawn as a pill in its state's
// shade, with a preview on hover. The caret still goes inside, so changing
// the digits looks the new number up. Keep as text stops drawing a number
// that means something else, such as a step. The store that looks them up
// is the shared one in @maxhq/shared/pills/git-pills.

import type { Timestamp } from "@maxhq/shared/bindings";
import { gitKey, gitNumbers } from "@maxhq/shared/pills/draft";
import { type GitPillsState, type GitPillsStore, shownItem } from "@maxhq/shared/pills/git-pills";
import { middleTruncate } from "@maxhq/shared/pills/label";
import { Extension } from "@tiptap/core";
import type { Node as ProseNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";

import { Popover, PopoverPopup } from "@/components/ui/popover";
import { gitFamily, gitIconMask } from "@/pills/pill";
import { GitItemDetails } from "@/pills/registry";
import { PREVIEW_DELAY_MS } from "@/pills/shown";

import { markedTextOf, posOf } from "./doc";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

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
