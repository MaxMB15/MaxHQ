// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: when a selection
// settles enough to offer actions on it, where they go, and what puts them
// away. The rest is MaxHQ's: a drag that starts on the selection, which quotes
// it, puts the actions away too.

/** A point in the window, in CSS pixels. */
export type SelectionActionPoint = { x: number; y: number };

/** How long a double or triple click takes to finish picking. */
export const SELECTION_MULTI_CLICK_INTERVAL_MS = 500;
const SELECTION_SCROLL_SETTLE_MS = 150;

type SelectionRect = { left: number; top: number; right: number; bottom: number };

/**
 * Where the actions go: at the pointer when it let go on the text, else
 * below the selection's end, kept inside its bounds and the window.
 */
export const resolveSelectionActionPosition = ({
	bounds,
	selectionRect,
	pointer,
	viewport,
}: {
	bounds: { left: number; top: number; width: number; height: number };
	selectionRect: { right: number; bottom: number } | null;
	pointer: SelectionActionPoint | null;
	viewport: { width: number; height: number };
}): SelectionActionPoint => {
	const preferred = pointer ?? {
		x: selectionRect?.right ?? bounds.left + bounds.width - 140,
		y: selectionRect ? selectionRect.bottom + 4 : bounds.top + 12,
	};
	return {
		x: Math.max(
			8,
			Math.min(
				Math.max(bounds.left, preferred.x),
				bounds.left + bounds.width,
				viewport.width - 8
			)
		),
		y: Math.max(
			8,
			Math.min(
				Math.max(bounds.top, preferred.y),
				bounds.top + bounds.height,
				viewport.height - 8
			)
		),
	};
};

/** Whether any of a rectangle shows inside every clip. */
export const isSelectionRectVisible = (
	rect: SelectionRect,
	clips: readonly SelectionRect[]
): boolean => {
	let { left, top, right, bottom } = rect;
	for (const clip of clips) {
		left = Math.max(left, clip.left);
		top = Math.max(top, clip.top);
		right = Math.min(right, clip.right);
		bottom = Math.min(bottom, clip.bottom);
	}
	return right > left && bottom > top;
};

/** What `observeSelectionActions` hands back. */
export type SelectionActions = {
	/** Whether the actions are about to show. */
	readonly pending: boolean;
	/** Forgets the selection, so the actions stay away until the next one. */
	readonly cancel: () => void;
	/** Hears the document's `selectionchange`. */
	readonly selectionChanged: () => void;
	readonly dispose: () => void;
};

/**
 * Watches selections in `element` and says when to show their actions and
 * when to put them away. `getActionElement` names the actions, so using
 * them doesn't count as picking something else.
 */
export const observeSelectionActions = ({
	element,
	getActionElement,
	restoreAfterScroll = false,
	onSelection,
	onDismiss,
}: {
	element: HTMLElement;
	getActionElement?: () => HTMLElement | null;
	restoreAfterScroll?: boolean;
	onSelection: (pointer: SelectionActionPoint | null) => void;
	onDismiss: (reason: "interaction" | "cancel") => void;
}): SelectionActions => {
	const document = element.ownerDocument;
	const view = document.defaultView!;
	let pointerDown = false;
	let gestureActive = false;
	let dismissed = false;
	let restorable = false;
	let pointer: SelectionActionPoint | null = null;
	let timer: number | null = null;
	let frame: number | null = null;

	const isActionTarget = (target: EventTarget | null) =>
		target !== null && (getActionElement?.()?.contains(target as Node) ?? false);

	const cancelPending = () => {
		if (timer !== null) view.clearTimeout(timer);
		if (frame !== null) view.cancelAnimationFrame(frame);
		timer = frame = null;
	};
	const cancel = () => {
		cancelPending();
		pointerDown = gestureActive = false;
		pointer = null;
		dismissed = true;
		restorable = false;
	};
	const dismiss = () => {
		cancel();
		onDismiss("cancel");
	};
	const onInteraction = () => {
		cancel();
		onDismiss("interaction");
	};
	const schedule = (delay: number) => {
		restorable = true;
		cancelPending();
		timer = view.setTimeout(() => {
			timer = null;
			frame = view.requestAnimationFrame(() => {
				frame = null;
				onSelection(pointer);
			});
		}, delay);
	};
	const onPointerDown = (event: PointerEvent) => {
		if (!event.isPrimary) return;
		if (isActionTarget(event.target)) {
			cancel();
			return;
		}
		cancelPending();
		pointerDown = event.button === 0;
		const inside = element.contains(event.target as Node);
		gestureActive = false;
		dismissed = true;
		restorable = false;
		pointer = null;
		if (inside) onDismiss("interaction");
		else if (getActionElement?.()) onDismiss("cancel");
	};
	const onSelectionStart = (event: PointerEvent) => {
		if (!event.isPrimary) return;
		const target = event.target as Element | null;
		if (target?.closest("button, [role=button]")) return;
		gestureActive = event.button === 0 && !event.defaultPrevented;
		dismissed = !gestureActive;
	};
	const onPointerUp = (event: PointerEvent) => {
		// Preventing pointerdown, as the actions do, can keep mouseup from coming.
		if (event.isPrimary && event.button === 0) pointerDown = false;
	};
	const onMouseUp = (event: MouseEvent) => {
		if (event.button !== 0) return;
		pointerDown = false;
		if (!gestureActive) return;
		gestureActive = false;
		pointer = { x: event.clientX, y: event.clientY };
		schedule(event.detail >= 2 ? SELECTION_MULTI_CLICK_INTERVAL_MS : 0);
	};
	const onKeyDown = (event: KeyboardEvent) => {
		if (isActionTarget(event.target) && event.key !== "Escape") return;
		if (event.key === "Escape") {
			dismiss();
		} else if (!pointerDown) {
			cancelPending();
			gestureActive = false;
			pointer = null;
			dismissed = false;
		}
	};
	const onFocusIn = (event: FocusEvent) => {
		if (isActionTarget(event.target)) cancel();
		else if (!pointerDown && getActionElement?.()) dismiss();
	};
	const onScroll = () => {
		cancelPending();
		// The scroll a drag makes as it picks must not lose the release that ends it.
		if (!pointerDown) {
			pointer = null;
			if (restoreAfterScroll && !dismissed && restorable)
				schedule(SELECTION_SCROLL_SETTLE_MS);
			else dismissed = true;
		}
		onDismiss("cancel");
	};

	document.addEventListener("pointerdown", onPointerDown, true);
	element.addEventListener("pointerdown", onSelectionStart);
	view.addEventListener("pointerup", onPointerUp);
	view.addEventListener("mouseup", onMouseUp);
	view.addEventListener("pointercancel", dismiss);
	document.addEventListener("keydown", onKeyDown);
	document.addEventListener("focusin", onFocusIn);
	element.addEventListener("contextmenu", onInteraction, true);
	element.addEventListener("dragstart", onInteraction, true);
	element.addEventListener("scroll", onScroll, true);
	view.addEventListener("blur", dismiss);
	view.addEventListener("resize", dismiss);

	return {
		get pending() {
			return timer !== null || frame !== null;
		},
		cancel,
		selectionChanged: () => {
			if (isActionTarget(document.activeElement)) return;
			if (pointerDown || gestureActive) {
				onDismiss("cancel");
			} else if (!dismissed && timer === null && frame === null) {
				schedule(0);
			}
		},
		dispose: () => {
			cancel();
			document.removeEventListener("pointerdown", onPointerDown, true);
			element.removeEventListener("pointerdown", onSelectionStart);
			view.removeEventListener("pointerup", onPointerUp);
			view.removeEventListener("mouseup", onMouseUp);
			view.removeEventListener("pointercancel", dismiss);
			document.removeEventListener("keydown", onKeyDown);
			document.removeEventListener("focusin", onFocusIn);
			element.removeEventListener("contextmenu", onInteraction, true);
			element.removeEventListener("dragstart", onInteraction, true);
			element.removeEventListener("scroll", onScroll, true);
			view.removeEventListener("blur", dismiss);
			view.removeEventListener("resize", dismiss);
		},
	};
};
