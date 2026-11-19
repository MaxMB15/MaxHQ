// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The list a field's `@`, `#` or `$` opens above the keyboard: what the
// menu offers, under a heading for each kind, or why it has nothing yet.
// A tap puts the option in place of the word, and the keyboard stays up.

import {
	groupName,
	type MenuContents,
	type MenuFinds,
	menuGroups,
	menuLabel,
	type MenuOption,
	menuStatus,
	optionText,
} from "@maxhq/shared/composer/menu";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";
import { memo, useMemo } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import { color, size } from "../../theme";

type Row =
	| { readonly type: "heading"; readonly kind: MenuOption["kind"] }
	| { readonly type: "option"; readonly option: MenuOption };

const HEADING = 30;
const OPTION = 40;

const keyOf = (row: Row, index: number): string =>
	row.type === "heading" ? `heading:${row.kind}` : `option:${index}`;

const Heading = memo(({ kind }: { kind: MenuOption["kind"] }) => (
	<Text style={styles.heading} accessibilityRole="header" numberOfLines={1}>
		{groupName(kind)}
	</Text>
));
Heading.displayName = "Heading";

const Option = memo(
	({ option, onPick }: { option: MenuOption; onPick: (option: MenuOption) => void }) => {
		const { label, detail } = optionText(option);
		return (
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={detail ? `${label}, ${detail}` : label}
				onPress={() => onPick(option)}
				hitSlop={{ top: 2, bottom: 2 }}
				style={({ pressed }) => [styles.option, pressed && styles.pressed]}
			>
				<Text style={styles.label} numberOfLines={1}>
					{label}
				</Text>
				{detail ? (
					<Text style={styles.detail} numberOfLines={1}>
						{detail}
					</Text>
				) : null}
			</Pressable>
		);
	}
);
Option.displayName = "Option";

/** The open menu's options, or its status while it has none. */
export const SuggestionList = ({
	kind,
	contents,
	finds,
	onPick,
}: {
	kind: TriggerKind;
	contents: MenuContents;
	/** What the field's `@` finds, for the words that name it. */
	finds: MenuFinds;
	onPick: (option: MenuOption) => void;
}) => {
	const rows = useMemo(
		(): Row[] =>
			menuGroups(contents.options).flatMap((group): Row[] => [
				{ type: "heading", kind: group.kind },
				...group.options.map((option): Row => ({ type: "option", option })),
			]),
		[contents.options]
	);
	const status = menuStatus(kind, contents, finds);
	return (
		<View style={styles.panel} accessibilityLabel={menuLabel(kind, finds)}>
			{status !== null ? (
				<Text style={styles.status} accessibilityLiveRegion="polite">
					{status}
				</Text>
			) : (
				<FlatList
					data={rows}
					keyExtractor={keyOf}
					keyboardShouldPersistTaps="always"
					renderItem={({ item }) =>
						item.type === "heading" ? (
							<Heading kind={item.kind} />
						) : (
							<Option option={item.option} onPick={onPick} />
						)
					}
				/>
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	panel: {
		maxHeight: 220,
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: 10,
		backgroundColor: color.card,
		overflow: "hidden",
	},
	heading: {
		height: HEADING,
		lineHeight: HEADING,
		paddingHorizontal: 12,
		color: color.mutedForeground,
		fontSize: size.tiny,
		fontWeight: "600",
	},
	option: {
		height: OPTION,
		paddingHorizontal: 12,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
	},
	pressed: { backgroundColor: color.accent },
	label: { flexShrink: 1, color: color.foreground, fontSize: size.body },
	detail: { flexShrink: 0, maxWidth: "45%", color: color.mutedForeground, fontSize: size.meta },
	status: { padding: 12, color: color.mutedForeground, fontSize: size.small },
});
