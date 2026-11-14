// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The native pieces the composer needs. In a browser each one answers with
// nothing to give, so the sample data build still runs.

export { NativeFailure } from "./failure";
export {
	checkSpelling,
	hasNativeSpelling,
	spellingMenu,
	type Misspelling,
	type SpellingChoice,
} from "./spelling";
export {
	describePaths,
	moveToTrash,
	pickAttachments,
	readClipboardFiles,
	readClipboardImage,
	revealPath,
	saveExport,
	thumbnail,
	useThumbnail,
	type FileKind,
	type PathInfo,
	type SavedExport,
} from "./files";
export {
	claimPageDrop,
	DROP_OVER_ATTRIBUTE,
	registerDropTarget,
	useDropTarget,
	type DropHandlers,
	type PageDrag,
} from "./drop";
export { contextMenu, type MenuEntry } from "./menu";
