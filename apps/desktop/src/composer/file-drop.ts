// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A picture or file pulled out of a timeline and dropped on a composer. In
// the app, Tauri's drop brings it, as a drag from the page the router
// hands over. Only the composer's own box takes one, even where the whole
// pane takes files, since the drag starts inside that pane. In a browser,
// the text box's own drop brings it instead. The file stays on its
// agent's device until it's dropped, and then copies from there.

import { type DraggedFile, FILE_DRAG_TYPE, readDraggedFile } from "@/agent-view/file-drag";
import { claimPageDrop, type PageDrag, type Point } from "@/native/drop";

/** The timeline file a drag from the page carries, or null. */
export const timelineFileOf = (drag: PageDrag): DraggedFile | null =>
	readDraggedFile(drag.get(FILE_DRAG_TYPE));

/** Whether `at` is over `element`'s box. */
export const isOver = (element: Element | null, at: Point): boolean => {
	if (element === null) return false;
	const box = element.getBoundingClientRect();
	return at.x >= box.left && at.x <= box.right && at.y >= box.top && at.y <= box.bottom;
};

/**
 * Takes a timeline file the webview dropped on the text box itself, and
 * hands it to `onFile`. Returns whether the drop was one, so nothing else
 * takes it.
 */
export const dropTimelineFile = (
	event: DragEvent,
	onFile: (file: DraggedFile) => void
): boolean => {
	const file = readDraggedFile(event.dataTransfer?.getData(FILE_DRAG_TYPE));
	if (file === null) return false;
	event.preventDefault();
	if (claimPageDrop()) onFile(file);
	return true;
};
