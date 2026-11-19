// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Exports an agent's whole conversation as Markdown. It loads the older
// pages first, then saves the text in a file you pick in the save dialog,
// or puts it on the clipboard. The dialog suggests the agent's title and
// the day as the name, in the folder the last export went to. The
// clipboard only takes a write during the click, so the click starts the
// write at once with the text still to come, and the loading finishes
// after.

import { exportFileName } from "@maxhq/shared/timeline/export";

import { saveExport } from "@/native";
import { t as translate } from "@maxhq/shared/i18n";

export { conversationText } from "@maxhq/shared/timeline/whole";

// Starts a clipboard write now for text that comes later. A webview with
// no `ClipboardItem` writes once the text is in, which only works where
// the clipboard doesn't need the click.
const writeLater = (text: Promise<string>): Promise<void> => {
	try {
		if (
			typeof ClipboardItem === "function" &&
			typeof navigator.clipboard.write === "function"
		) {
			const blob = text.then((value) => new Blob([value], { type: "text/plain" }));
			return navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
		}
		return text.then((value) => navigator.clipboard.writeText(value));
	} catch (error) {
		return Promise.reject(error);
	}
};

/** How an export went, with the reason when it failed. */
export type ExportResult = { readonly ok: true } | { readonly ok: false; readonly reason: string };

const reasonOf = (error: unknown): string =>
	error instanceof Error && error.message ? error.message : translate("It didn't load.");

/**
 * Copies the text to the clipboard once it's ready. Call it during the
 * click. A failed load gives its own reason, and a clipboard that says no
 * gives a general one.
 */
export const copyWhenReady = async (text: Promise<string>): Promise<ExportResult> => {
	const written = writeLater(text).then(
		() => null,
		(error: unknown) => error ?? new Error(translate("The clipboard said no."))
	);
	try {
		await text;
	} catch (error) {
		return { ok: false, reason: reasonOf(error) };
	}
	const failed = await written;
	if (failed !== null) {
		console.warn(translate("Couldn't copy the conversation"), failed);
		return { ok: false, reason: translate("The clipboard didn't take it. Try again.") };
	}
	return { ok: true };
};

/** Where the folder the last export went to is kept. */
export const EXPORT_FOLDER_KEY = "maxhq.export-folder";

const lastFolder = (): string | null => {
	try {
		return localStorage.getItem(EXPORT_FOLDER_KEY);
	} catch {
		return null;
	}
};

const keepFolder = (folder: string | null) => {
	if (folder === null) return;
	try {
		localStorage.setItem(EXPORT_FOLDER_KEY, folder);
	} catch {
		// The next dialog opens where the system picks.
	}
};

/** How a save went: the file's name, cancelled, or why it failed. */
export type SaveResult =
	| { readonly step: "saved"; readonly name: string }
	| { readonly step: "cancelled" }
	| { readonly step: "failed"; readonly reason: string };

/**
 * Asks where to save the text once it's ready, and saves it there.
 * `onAsking` runs as the dialog opens. A failed load gives its own reason.
 */
export const saveWhenReady = async (
	text: Promise<string>,
	title: string,
	onAsking?: () => void
): Promise<SaveResult> => {
	let ready: string;
	try {
		ready = await text;
	} catch (error) {
		return { step: "failed", reason: reasonOf(error) };
	}
	onAsking?.();
	try {
		const saved = await saveExport(ready, exportFileName(title, new Date()), lastFolder());
		if (saved === null) return { step: "cancelled" };
		keepFolder(saved.folder);
		return { step: "saved", name: saved.name };
	} catch (error) {
		console.warn(translate("Couldn't save the conversation"), error);
		return {
			step: "failed",
			reason:
				error instanceof Error && error.message
					? error.message
					: translate("It didn't save."),
		};
	}
};
