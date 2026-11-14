// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the byte size
// formatter, as an arrow function. It moved here from the
// device load card, so the Running list and the card show sizes alike.

import { formatNumber, t } from "../i18n";

/** A size in bytes, such as "14.0 GB". */
export const formatMemory = (value: number): string => {
	if (value < 1024) return t("{{value}} B", { value: formatNumber(value) });
	const units = ["KB", "MB", "GB"] as const;
	let unit = -1;
	let next = value;
	do {
		next /= 1024;
		unit += 1;
	} while (next >= 1024 && unit < units.length - 1);
	return t("{{value}} {{unit}}", {
		value: formatNumber(next, {
			minimumFractionDigits: next >= 10 ? 1 : 2,
			maximumFractionDigits: next >= 10 ? 1 : 2,
		}),
		unit: units[unit],
	});
};
