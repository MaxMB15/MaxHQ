// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

/**
 * FNV-1a over a text's UTF-16 code units, as an unsigned 32-bit number. It's
 * quick and the same on every device, which is all a display cache needs.
 */
export const fnv1a = (text: string): number => {
	let h = 0x811c9dc5;
	for (let i = 0; i < text.length; i++) {
		h ^= text.charCodeAt(i);
		h = Math.imul(h, 0x01000193);
	}
	return h >>> 0;
};
