// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A note's visual blocks, each drawn as the visual's card. The editor's look
// hands each block's node to a host made per editor, and the editor renders
// the cards into those nodes through portals, so they sit inside the app's
// providers like the rest of the pane. A card is drawn once per node, so
// typing elsewhere in the note leaves its frame alone.
//
// The </> button shows the block's fenced text to edit, until the caret
// leaves it. A press on the title row, outside its buttons, selects the
// whole block, so Delete, Copy and Cut take all of it.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, type MouseEvent, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";

import type { VisualControls, VisualHost } from "@/composer/markdown";
import { VisualCard } from "@/visuals/visual-card";
import type { Visual } from "@maxhq/shared/visuals/fence";

type Drawing = {
	readonly dom: HTMLElement;
	readonly visual: Visual;
	readonly controls: VisualControls;
};

/** The cards one editor draws, and the host its look draws them through. */
export type NoteVisuals = {
	readonly host: VisualHost;
	readonly subscribe: (listener: () => void) => () => void;
	readonly drawings: () => readonly Drawing[];
};

/**
 * Makes the store a note's cards are drawn from. The editor draws as it
 * renders, so the cards change a moment after, never inside its update.
 */
export const noteVisuals = (): NoteVisuals => {
	const drawn = new Map<HTMLElement, Drawing>();
	let list: readonly Drawing[] = [];
	const listeners = new Set<() => void>();
	let waiting = false;
	const changed = () => {
		if (waiting) return;
		waiting = true;
		queueMicrotask(() => {
			waiting = false;
			list = [...drawn.values()];
			for (const listener of [...listeners]) listener();
		});
	};
	return {
		host: {
			draw: (dom, visual, controls) => {
				drawn.set(dom, { dom, visual, controls });
				changed();
			},
			remove: (dom) => {
				if (drawn.delete(dom)) changed();
			},
		},
		subscribe: (listener) => {
			listeners.add(listener);
			return () => {
				listeners.delete(listener);
			};
		},
		drawings: () => list,
	};
};

const SOURCE_BUTTON =
	"inline-flex h-[22px] items-center rounded-[5px] px-[7px] font-mono text-[11.5px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

/** A press on the title row, but not on one of its buttons, picks the block. */
const pressTitle = (controls: VisualControls) => (event: MouseEvent) => {
	if (event.button !== 0) return;
	const target = event.target as Element;
	if (!target.closest("figcaption") || target.closest("button, a, [role='menuitem']")) return;
	event.preventDefault();
	controls.select();
};

type BlockProps = { readonly drawing: Drawing; readonly readOnly: boolean };

const NoteVisualBlockView = ({ drawing, readOnly }: BlockProps) => {
	const { t } = useI18n();
	const { dom, visual, controls } = drawing;
	return createPortal(
		// biome-ignore lint/a11y/noStaticElementInteractions: the title row picks the block for the keys, which reach it through the editor.
		<div onMouseDown={pressTitle(controls)}>
			<VisualCard
				html={visual.html}
				title={visual.title}
				buttons={
					readOnly ? null : (
						<button
							type="button"
							title={t("Edit the source")}
							aria-label={t("Edit the source")}
							className={SOURCE_BUTTON}
							onClick={controls.showSource}
						>
							{"</>"}
						</button>
					)
				}
			/>
		</div>,
		dom
	);
};

const NoteVisualBlock = memo(NoteVisualBlockView);

/** The cards of a note's visual blocks, wherever its editor put them. */
export const NoteVisualBlocks = ({
	visuals,
	readOnly,
}: {
	visuals: NoteVisuals;
	readOnly: boolean;
}) => {
	const drawings = useSyncExternalStore(visuals.subscribe, visuals.drawings);
	return drawings.map((drawing) => (
		<NoteVisualBlock key={keyOf(drawing.dom)} drawing={drawing} readOnly={readOnly} />
	));
};

// Each node's key, for as long as it's drawn.
const keys = new WeakMap<HTMLElement, string>();
let nextKey = 0;
const keyOf = (dom: HTMLElement): string => {
	let key = keys.get(dom);
	if (key === undefined) {
		key = `visual-${nextKey++}`;
		keys.set(dom, key);
	}
	return key;
};

/** Makes a note editor's store of cards once. */
export const useNoteVisuals = (): NoteVisuals => useState(noteVisuals)[0];
