// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the button that
// floats over a selection in the conversation, where it goes, when it shows,
// and Tab and Escape on it. The rest is MaxHQ's: it makes a quote pill from
// any row, not a citation of a reply.

import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { TextQuote } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import {
	isSelectionRectVisible,
	observeSelectionActions,
	resolveSelectionActionPosition,
	type SelectionActionPoint,
	type SelectionActions,
} from "./selection-actions";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

type Clip = { left: number; top: number; right: number; bottom: number };

/** Whether any of the picked text shows, inside the viewport and every box that clips it. */
const showsAny = (range: Range, viewport: HTMLElement): boolean => {
	const viewportRect = viewport.getBoundingClientRect();
	if (!isSelectionRectVisible(range.getBoundingClientRect(), [viewportRect])) return false;
	// A selection can cross blocks with different clipping ancestors.
	const common = range.commonAncestorContainer;
	const document = viewport.ownerDocument;
	const view = document.defaultView!;
	const walker = document.createTreeWalker(common, NodeFilter.SHOW_TEXT);
	let node = common.nodeType === Node.TEXT_NODE ? common : walker.nextNode();
	for (; node; node = walker.nextNode()) {
		if (!range.intersectsNode(node)) continue;
		if (node.parentElement && view.getComputedStyle(node.parentElement).userSelect === "none")
			continue;
		const fragment = document.createRange();
		fragment.selectNodeContents(node);
		if (node === range.startContainer) fragment.setStart(node, range.startOffset);
		if (node === range.endContainer) fragment.setEnd(node, range.endOffset);
		if (!fragment.toString().trim()) continue;
		const lines = Array.from(fragment.getClientRects());
		if (!lines.some((line) => isSelectionRectVisible(line, [viewportRect]))) continue;
		const clips: Clip[] = [viewportRect];
		for (
			let element = node.parentElement;
			element && element !== viewport;
			element = element.parentElement
		) {
			const style = view.getComputedStyle(element);
			const clipX = style.overflowX !== "visible";
			const clipY = style.overflowY !== "visible";
			if (!clipX && !clipY) continue;
			const bounds = element.getBoundingClientRect();
			const left = bounds.left + element.clientLeft;
			const top = bounds.top + element.clientTop;
			clips.push({
				left: clipX ? left : -Infinity,
				right: clipX ? left + element.clientWidth : Infinity,
				top: clipY ? top : -Infinity,
				bottom: clipY ? top + element.clientHeight : Infinity,
			});
		}
		if (lines.some((line) => isSelectionRectVisible(line, clips))) return true;
	}
	return false;
};

/** A quote made from the page's selection, and the text it picked. */
export type SelectedQuote = { readonly quote: QuoteRef; readonly range: Range };

/**
 * An Add to chat button over a selection in `viewport`, which makes a quote
 * with `quoteOf` and hands it to `onAdd`.
 */
export const QuoteToolbar = ({
	viewport,
	quoteOf,
	onAdd,
}: {
	viewport: HTMLElement | null;
	quoteOf: (selection: Selection | null) => SelectedQuote | null;
	onAdd: (quote: QuoteRef) => void;
}) => {
	useI18n();
	const [shown, setShown] = useState<{
		quote: QuoteRef;
		position: SelectionActionPoint;
	} | null>(null);
	const toolbarRef = useRef<HTMLButtonElement>(null);
	const actionsRef = useRef<SelectionActions | null>(null);
	// The latest `quoteOf`, so a new one doesn't start the watch over.
	const quoteOfRef = useRef(quoteOf);
	useEffect(() => {
		quoteOfRef.current = quoteOf;
	});

	// Keeps the button inside the window, measured before it paints.
	useLayoutEffect(() => {
		const toolbar = toolbarRef.current;
		if (!toolbar || !shown) return;
		const rect = toolbar.getBoundingClientRect();
		toolbar.style.left = `${Math.max(8, Math.min(shown.position.x, window.innerWidth - rect.width - 8))}px`;
		toolbar.style.top = `${Math.max(8, Math.min(shown.position.y, window.innerHeight - rect.height - 8))}px`;
	}, [shown]);

	useEffect(() => {
		if (!viewport) return;
		const clear = () => setShown(null);
		const update = (pointer: SelectionActionPoint | null) => {
			const selected = quoteOfRef.current(window.getSelection());
			if (!selected || !showsAny(selected.range, viewport)) return clear();
			const rects = selected.range.getClientRects();
			const pointerOnText =
				pointer !== null &&
				Array.from(rects).some(
					(line) =>
						pointer.x >= line.left - 8 &&
						pointer.x <= line.right + 8 &&
						pointer.y >= line.top - 8 &&
						pointer.y <= line.bottom + 8
				);
			setShown({
				quote: selected.quote,
				position: resolveSelectionActionPosition({
					bounds: viewport.getBoundingClientRect(),
					selectionRect:
						rects.item(rects.length - 1) ?? selected.range.getBoundingClientRect(),
					pointer: pointerOnText ? pointer : null,
					viewport: { width: window.innerWidth, height: window.innerHeight },
				}),
			});
		};
		const actions = observeSelectionActions({
			element: viewport,
			getActionElement: () => toolbarRef.current,
			restoreAfterScroll: true,
			onSelection: update,
			onDismiss: clear,
		});
		actionsRef.current = actions;
		// Tab from the selection goes to the button first.
		const focusActions = (event: KeyboardEvent) => {
			const toolbar = toolbarRef.current;
			if (
				event.key !== "Tab" ||
				event.shiftKey ||
				event.altKey ||
				event.ctrlKey ||
				event.metaKey ||
				event.isComposing ||
				event.defaultPrevented ||
				!toolbar ||
				toolbar.contains(event.target as Node)
			)
				return;
			event.preventDefault();
			event.stopPropagation();
			toolbar.focus({ preventScroll: true });
		};
		document.addEventListener("keydown", focusActions, true);
		document.addEventListener("selectionchange", actions.selectionChanged);
		return () => {
			document.removeEventListener("keydown", focusActions, true);
			document.removeEventListener("selectionchange", actions.selectionChanged);
			actions.dispose();
			actionsRef.current = null;
		};
	}, [viewport]);

	if (!shown) return null;
	const dismiss = () => {
		actionsRef.current?.cancel();
		setShown(null);
	};
	const add = () => {
		onAdd(shown.quote);
		window.getSelection()?.removeAllRanges();
		dismiss();
	};
	return createPortal(
		<button
			ref={toolbarRef}
			type="button"
			className="fixed z-50 inline-flex h-7 max-w-[calc(100vw-1rem)] cursor-pointer items-center gap-1.5 rounded-full border border-input bg-popover px-3 text-[12px] text-foreground shadow-md outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
			style={{ left: shown.position.x, top: shown.position.y }}
			onPointerDown={(event) => event.preventDefault()}
			onClick={add}
			onKeyDown={(event) => {
				event.stopPropagation();
				if (event.key === "Escape" && !event.nativeEvent.isComposing) {
					event.preventDefault();
					dismiss();
				}
			}}
		>
			<TextQuote size={13} strokeWidth={1.9} aria-hidden />
			{translate("Add to chat")}
		</button>,
		document.body
	);
};
