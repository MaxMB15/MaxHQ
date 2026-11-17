// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A translated sentence with drawn parts in it, such as a command in
// monospace. Each language puts the parts where its grammar wants them,
// and what a part holds can't be mistaken for the sentence around it.

import { Fragment, type ReactNode } from "react";

import { t as translate } from "@maxhq/shared/i18n";

const EDGE = "\u0000";

/**
 * The sentence `key` names, with each `{{name}}` that `parts` has filled by
 * that part. `values` fills the rest, as `t` would.
 */
export const withParts = (
	key: string,
	parts: Readonly<Record<string, ReactNode>>,
	values?: Readonly<Record<string, unknown>>
): ReactNode[] => {
	const marks = Object.fromEntries(Object.keys(parts).map((name) => [name, EDGE + name + EDGE]));
	return translate(key, { ...values, ...marks })
		.split(EDGE)
		.map((piece, i) =>
			// Pieces of one sentence, so their place is their identity.
			i % 2 === 1 ? <Fragment key={i}>{parts[piece]}</Fragment> : piece
		);
};
