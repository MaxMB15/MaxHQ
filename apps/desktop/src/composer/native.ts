// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Everything the composer asks of the system: spell check, the file picker,
// the clipboard, file details and drops. It all goes through one interface,
// which the app's native module fills as it stands, so the browser build
// and the tests can stand in for it. The browser stub finds nothing and
// picks nothing, and leaves spelling to the webview.

import { createContext, useContext } from "react";

import type {
	DropHandlers,
	FileKind,
	MenuEntry,
	Misspelling,
	PathInfo,
	SpellingChoice,
} from "@/native";

// The shapes come from the native module, so the composer and the commands
// can't drift apart.
export { DROP_OVER_ATTRIBUTE } from "@/native";
export type { DropHandlers, FileKind, MenuEntry, Misspelling, PathInfo, SpellingChoice };

/**
 * The native calls the composer makes. Each one rejects only when the call
 * itself goes wrong, and resolves with an empty result when there's nothing.
 */
export type ComposerNative = {
	/** Whether spell check comes from the system. When false, the webview's own runs instead. */
	readonly hasNativeSpelling: () => boolean;
	/** The misspelled words in `text`. */
	readonly checkSpelling: (text: string) => Promise<Misspelling[]>;
	/**
	 * Shows the spelling menu at a point in the webview, for the misspelling
	 * under the pointer or for none. Resolves once the menu closes.
	 */
	readonly spellingMenu: (
		misspelling: Misspelling | null,
		at: { readonly x: number; readonly y: number }
	) => Promise<SpellingChoice>;
	/** Opens the system picker for files and folders. Empty when you cancel. */
	readonly pickAttachments: () => Promise<string[]>;
	/** Files copied in Finder, as paths. */
	readonly readClipboardFiles: () => Promise<string[]>;
	/** An image on the clipboard as PNG bytes, or null. */
	readonly readClipboardImage: () => Promise<Uint8Array | null>;
	/** Describes dropped, pasted or picked paths, in the same order. Never pass a typed path. */
	readonly describePaths: (paths: readonly string[]) => Promise<PathInfo[]>;
	/** A PNG of a dropped, pasted or picked path, at most `size` pixels on a side. */
	readonly thumbnail: (path: string, size: number) => Promise<Blob | null>;
	/** Sends file drops on `element` to `handlers`. Returns the undo. */
	readonly registerDropTarget: (element: Element, handlers: DropHandlers) => () => void;
	/**
	 * Shows a file on this Mac in Finder, such as a download. Unset while
	 * the app has no command for it, and then nothing offers it.
	 */
	readonly revealPath?: (path: string) => Promise<void>;
	/**
	 * Shows a native context menu at a point in the webview. Returns false
	 * when none can show, and then the webview's own menu stays. Unset in
	 * the browser build.
	 */
	readonly contextMenu?: (
		entries: readonly MenuEntry[],
		at: { readonly x: number; readonly y: number }
	) => boolean;
};

const IMAGE = /\.(png|jpe?g|gif|webp|heic|tiff?|bmp)$/i;

/** Describes a path from its name alone, the way the browser build does. */
export const guessPath = (path: string): PathInfo => {
	const name = path.replace(/\/+$/, "").split("/").pop() || path;
	const kind: FileKind = path.endsWith("/")
		? "folder"
		: /\.pdf$/i.test(name)
			? "pdf"
			: IMAGE.test(name)
				? "image"
				: "file";
	return { path, name, kind, size: null, mime: null };
};

/** The stand-in outside the app. */
export const browserNative: ComposerNative = {
	hasNativeSpelling: () => false,
	checkSpelling: async () => [],
	spellingMenu: async () => ({ kind: "none" }),
	pickAttachments: async () => [],
	readClipboardFiles: async () => [],
	readClipboardImage: async () => null,
	describePaths: async (paths) => paths.map(guessPath),
	thumbnail: async () => null,
	registerDropTarget: () => () => {},
};

const NativeContext = createContext<ComposerNative>(browserNative);

/** Hands a set of native calls to every composer below it. */
export const ComposerNativeProvider = NativeContext.Provider;

/** The native calls from the nearest `ComposerNativeProvider`, or the browser stub. */
export const useComposerNative = (): ComposerNative => useContext(NativeContext);

/** What a failed native call says, for the error line. */
export const nativeErrorText = (error: unknown): string =>
	error instanceof Error ? error.message : String(error);
