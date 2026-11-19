// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a picker shows: the label of what's picked, and its list with each
// group's heading above its entries, in the order the options come.

import type { Option } from "@maxhq/shared/new-agent/spec";

/** One line in a picker's list: a group's heading or an option. */
export type PickerRow =
	| { readonly kind: "group"; readonly key: string; readonly label: string }
	| { readonly kind: "option"; readonly key: string; readonly option: Option };

/** The options with a heading wherever the group changes. */
export const pickerRows = (options: readonly Option[]): PickerRow[] => {
	const rows: PickerRow[] = [];
	let group: string | undefined;
	for (const option of options) {
		if (option.group && option.group !== group) {
			rows.push({ kind: "group", key: `g:${option.group}`, label: option.group });
		}
		group = option.group;
		rows.push({ kind: "option", key: `o:${option.value}`, option });
	}
	return rows;
};

/** What the picker shows for `value`: its option's label, or `empty`. */
export const pickedLabel = (options: readonly Option[], value: string, empty: string): string =>
	options.find((o) => o.value === value)?.label ?? empty;
