// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A new random ID, such as a note's or a scheduled task's, made on any
// client, the phone included.

/**
 * A random version 4 UUID. React Native may have no `crypto.randomUUID`,
 * so it falls back to `crypto.getRandomValues`, and to `Math.random` where
 * neither is there.
 */
export const newUuid = (): string => {
	const crypto = (
		globalThis as {
			crypto?: {
				randomUUID?: () => string;
				getRandomValues?: (bytes: Uint8Array) => Uint8Array;
			};
		}
	).crypto;
	if (typeof crypto?.randomUUID === "function") return crypto.randomUUID();
	const bytes = new Uint8Array(16);
	if (typeof crypto?.getRandomValues === "function") crypto.getRandomValues(bytes);
	else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
	bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
	bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
	const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
};
