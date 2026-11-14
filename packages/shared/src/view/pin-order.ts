// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the pin order keys,
// rewritten as arrow functions. Every pin here has a
// key, so the keyless order is left out, and `nextPinKey` and `byPin` are
// MaxHQ's: a new pin goes to the end, and ties sort by when it was pinned.

import type { Agent } from "../bindings";

import { inboxOf } from "./inbox";

const PIN_ORDER_DIGITS = "abcdefghijklmnopqrstuvwxyz";

const isValidPinOrderKey = (key: string): boolean => {
	if (key.length === 0) return false;
	for (const char of key) {
		if (!PIN_ORDER_DIGITS.includes(char)) return false;
	}
	// A trailing minimum digit would leave no room to sort a key immediately
	// before this one; generators never produce it, so treat it as corrupt.
	return key.at(-1) !== PIN_ORDER_DIGITS[0];
};

/**
 * Midpoint of two digit strings read as fractions in (0, 1). "" stands for
 * the open bound on either side. Requires a < b.
 */
const pinOrderMidpoint = (a: string, b: string): string => {
	if (b !== "" && a >= b) throw new Error("pinOrderMidpoint: bounds out of order");
	if (b !== "") {
		let n = 0;
		while ((a.charAt(n) || PIN_ORDER_DIGITS[0]) === b.charAt(n)) n += 1;
		if (n > 0) return b.slice(0, n) + pinOrderMidpoint(a.slice(n), b.slice(n));
	}
	const digitA = a === "" ? 0 : PIN_ORDER_DIGITS.indexOf(a.charAt(0));
	const digitB = b === "" ? PIN_ORDER_DIGITS.length : PIN_ORDER_DIGITS.indexOf(b.charAt(0));
	if (digitB - digitA > 1) {
		return PIN_ORDER_DIGITS.charAt(Math.round((digitA + digitB) / 2));
	}
	if (b.length > 1) return b.charAt(0);
	return PIN_ORDER_DIGITS.charAt(digitA) + pinOrderMidpoint(a.slice(1), "");
};

/**
 * A key that sorts strictly between two neighbors. A null bound is the top
 * or the bottom of the pins. Null when the keys given are corrupt or out of
 * order, and the caller rewrites the pins instead.
 */
export const pinOrderKeyBetween = (before: string | null, after: string | null): string | null => {
	const a = before ?? "";
	const b = after ?? "";
	if (a !== "" && !isValidPinOrderKey(a)) return null;
	if (b !== "" && !isValidPinOrderKey(b)) return null;
	if (b !== "" && a >= b) return null;
	return pinOrderMidpoint(a, b);
};

/** `count` keys spread evenly, in order, each with room on both sides. */
export const generateSpreadPinOrderKeys = (count: number): string[] => {
	let width = 2;
	let space = PIN_ORDER_DIGITS.length ** width;
	while (space <= (count + 1) * 2) {
		width += 1;
		space *= PIN_ORDER_DIGITS.length;
	}
	const step = space / (count + 1);
	const keys: string[] = [];
	for (let i = 0; i < count; i += 1) {
		let value = Math.round(step * (i + 1));
		if (value % PIN_ORDER_DIGITS.length === 0) value += 1;
		let key = "";
		for (let digit = 0; digit < width; digit += 1) {
			key = PIN_ORDER_DIGITS.charAt(value % PIN_ORDER_DIGITS.length) + key;
			value = Math.floor(value / PIN_ORDER_DIGITS.length);
		}
		keys.push(key);
	}
	return keys;
};

/** A key a pin moves to. */
export type PinAssignment = { readonly id: string; readonly key: string };

/**
 * The keys to write so the pins read in `orderedIds` after `movedId` moved
 * there. Usually one key for the moved pin; when its neighbors leave no
 * room, every shown pin gets a fresh key. `keysById` also holds the keys of
 * pins that aren't shown, such as snoozed ones, so no fresh key lands on
 * theirs.
 */
export const planPinnedReorder = (input: {
	readonly orderedIds: readonly string[];
	readonly keysById: ReadonlyMap<string, string | null | undefined>;
	readonly movedId: string;
}): readonly PinAssignment[] => {
	const { orderedIds, keysById, movedId } = input;
	const visibleIds = new Set(orderedIds);
	const reservedKeys = new Set(
		[...keysById].flatMap(([id, key]) => (!visibleIds.has(id) && key != null ? [key] : []))
	);
	const movedIndex = orderedIds.indexOf(movedId);
	if (movedIndex === -1) return [];
	const beforeId = movedIndex > 0 ? orderedIds[movedIndex - 1] : null;
	const afterId = movedIndex < orderedIds.length - 1 ? orderedIds[movedIndex + 1] : null;
	const beforeKey = beforeId != null ? (keysById.get(beforeId) ?? null) : null;
	const afterKey = afterId != null ? (keysById.get(afterId) ?? null) : null;
	const beforeUsable = beforeId === null || beforeKey != null;
	const afterUsable = afterId === null || afterKey != null;
	if (beforeUsable && afterUsable) {
		let key = pinOrderKeyBetween(beforeKey, afterKey);
		while (key !== null && reservedKeys.has(key)) key = pinOrderKeyBetween(key, afterKey);
		if (key !== null) return [{ id: movedId, key }];
	}
	const keys = generateSpreadPinOrderKeys(orderedIds.length + reservedKeys.size)
		.filter((key) => !reservedKeys.has(key))
		.slice(0, orderedIds.length);
	return orderedIds.flatMap((id, index) => {
		const key = keys[index]!;
		return keysById.get(id) === key ? [] : [{ id, key }];
	});
};

/**
 * A key after every pin there is, so a new pin goes to the end. A last key
 * another client made corrupt still sorts first, since anything longer that
 * starts with it sorts after it.
 */
export const nextPinKey = (keys: Iterable<string>): string => {
	let last: string | null = null;
	for (const key of keys) if (last === null || key > last) last = key;
	return pinOrderKeyBetween(last, null) ?? `${last}n`;
};

/** Sorts pinned agents by their key as text, then by when they were pinned. */
export const byPin = (a: Agent, b: Agent): number => {
	const left = inboxOf(a).pin;
	const right = inboxOf(b).pin;
	const leftKey = left?.key ?? "";
	const rightKey = right?.key ?? "";
	if (leftKey !== rightKey) return leftKey < rightKey ? -1 : 1;
	return (left?.at ?? 0) - (right?.at ?? 0) || a.id.localeCompare(b.id);
};
