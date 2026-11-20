// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Attachments from the system: the picker, the clipboard, a description
// for each chip and its thumbnail. The page only learns a path from a drop,
// a paste or the picker, and the backend refuses to describe or thumbnail
// any other path. An export goes the other way, to the file you pick in the
// save dialog.

import { useEffect, useState } from "react";

import { t } from "@maxhq/shared/i18n";

import type { FileKind, PathInfo, SavedExport } from "@maxhq/shared/bindings";
import { commands } from "@/data/tauri-commands";

import { invokeBytes, unwrap } from "./failure";
import { inApp } from "./platform";

/**
 * Opens the system picker, which takes files and folders together.
 * Resolves with the chosen paths, empty if you cancel or outside the app.
 */
export const pickAttachments = (): Promise<string[]> =>
	inApp()
		? unwrap(
				commands.pickAttachments({
					prompt: t("Attach"),
					message: t("Choose files or folders to attach."),
				})
			)
		: Promise.resolve([]);

/**
 * The files on the clipboard, such as ones copied in Finder. The webview's
 * paste event names them without their paths, so ask here first on paste
 * and fall back to `readClipboardImage`. Empty when there are none.
 */
export const readClipboardFiles = (): Promise<string[]> =>
	inApp() ? unwrap(commands.readClipboardFiles()) : Promise.resolve([]);

/**
 * The image on the clipboard as PNG bytes, such as a screenshot or an
 * image copied from another app. Null when there's none, or when the
 * clipboard holds files, since Finder puts their icons there too.
 */
export const readClipboardImage = async (): Promise<Uint8Array | null> => {
	if (!inApp()) return null;
	const png = await invokeBytes("read_clipboard_image");
	return png.length > 0 ? png : null;
};

/**
 * Moves a dropped, pasted or picked file to the Trash, where Finder can put
 * it back, such as a GitHub App key once its devices hold it. Folders and
 * links are refused. Outside the app there's nothing to move.
 */
export const moveToTrash = (path: string): Promise<void> =>
	inApp() ? unwrap(commands.trashOffered(path)).then(() => undefined) : Promise.resolve();

/**
 * Shows a download in Finder, selected. The backend refuses any path
 * outside `~/Downloads/MaxHQ`. Outside the app there's nothing to show.
 */
export const revealPath = (path: string): Promise<void> =>
	inApp() ? unwrap(commands.revealDownload(path)).then(() => undefined) : Promise.resolve();

/**
 * Asks where to save an export in the save dialog, suggesting `name` in
 * `folder`, and writes `text` there. Resolves with the file written, or
 * null when you cancel. Outside the app there's no dialog, so nothing is
 * saved.
 */
export const saveExport = (
	text: string,
	name: string,
	folder: string | null
): Promise<SavedExport | null> =>
	inApp() ? unwrap(commands.saveExport(text, name, folder)) : Promise.resolve(null);

/**
 * Asks for one folder, or one file when `file` is set, in the system
 * picker titled `title`, opening at `start`. Resolves with the path, or
 * null when you cancel or outside the app.
 */
export const pickPath = (
	title: string,
	start: string | null,
	file: boolean
): Promise<string | null> =>
	inApp() ? unwrap(commands.pickPath(title, start, file)) : Promise.resolve(null);

const IMAGE = /\.(png|jpe?g|gif|webp|heic|tiff?|bmp)$/i;

/** A description from the name alone, for the browser build. */
const guess = (path: string): PathInfo => {
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

/**
 * Describes dropped, pasted or picked paths for their chips, in the same
 * order. Outside the app it guesses from the names.
 */
export const describePaths = (paths: readonly string[]): Promise<PathInfo[]> =>
	inApp() ? unwrap(commands.describePaths([...paths])) : Promise.resolve(paths.map(guess));

/**
 * A thumbnail of a dropped, pasted or picked path as a PNG at most `size`
 * CSS pixels on a side, sharp at the screen's pixel ratio. Files without a
 * preview get their Finder icon. Null when there's nothing to show, such
 * as for a file that's gone, or outside the app.
 */
export const thumbnail = async (path: string, size: number): Promise<Blob | null> => {
	if (!inApp()) return null;
	const scale = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
	const png = await invokeBytes("thumbnail", { path, size, scale });
	return png.length > 0
		? new Blob([png as Uint8Array<ArrayBuffer>], { type: "image/png" })
		: null;
};

/**
 * A thumbnail's object URL for an `<img>`, or null while it loads and when
 * there's none. The hook revokes the URL when the path or size changes and
 * when the component unmounts.
 */
export const useThumbnail = (path: string | null, size: number): string | null => {
	const key = path === null ? null : `${size} ${path}`;
	const [shown, setShown] = useState<{ key: string; url: string | null } | null>(null);
	useEffect(() => {
		if (path === null) return;
		const loading = `${size} ${path}`;
		let live = true;
		let url: string | null = null;
		thumbnail(path, size).then(
			(png) => {
				if (!live) return;
				url = png ? URL.createObjectURL(png) : null;
				setShown({ key: loading, url });
			},
			() => {
				if (live) setShown({ key: loading, url: null });
			}
		);
		return () => {
			live = false;
			if (url !== null) URL.revokeObjectURL(url);
		};
	}, [path, size]);
	return shown !== null && shown.key === key ? shown.url : null;
};

export type { FileKind, PathInfo, SavedExport };
