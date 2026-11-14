// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES. Only the size rule
// and the file names are kept, since MaxHQ has no input limit and no paste as
// text shortcut.

/** Pasted text this big or bigger goes in as a text file instead of into the message. */
export const PASTED_TEXT_ATTACHMENT_THRESHOLD_BYTES = 32 * 1024;

const textEncoder = new TextEncoder();

/**
 * Whether pasted text goes in as a file. The rule counts bytes, since a
 * character count understates text heavy in non-Latin scripts.
 */
export const pastesAsFile = (text: string): boolean =>
	text.length >= PASTED_TEXT_ATTACHMENT_THRESHOLD_BYTES ||
	textEncoder.encode(text).byteLength >= PASTED_TEXT_ATTACHMENT_THRESHOLD_BYTES;

/** The first free name among `pasted-text.txt`, `pasted-text-2.txt` and on. */
export const nextPastedTextFileName = (existingNames: readonly string[]): string => {
	const names = new Set(existingNames.map((name) => name.toLowerCase()));
	if (!names.has("pasted-text.txt")) return "pasted-text.txt";
	for (let sequence = 2; ; sequence += 1) {
		const candidate = `pasted-text-${sequence}.txt`;
		if (!names.has(candidate)) return candidate;
	}
};
