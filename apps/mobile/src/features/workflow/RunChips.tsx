// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A lead's latest run as a chip, such as "pipeline 11/18", on its row in
// the agent list and under the agent screen's header, as the desktop's.
// Each opens the run's screen. Amber while the run waits on you.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useMemo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { AgentId } from "@maxhq/shared/bindings";
import { chipOf, type RunChip } from "@maxhq/shared/workflow/select";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, radius, size, TOUCH } from "../../theme";
import { Workflow } from "../../ui/glyphs";
import { chipLook } from "./runs";

const useOpenRun = () => {
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	return (chip: RunChip) => navigation.navigate("Run", chip.run);
};

// Slop that makes a chip `height` tall and about `width` wide a 44 point target.
const slopOf = (height: number, width = TOUCH) => {
	const y = Math.max(0, (TOUCH - height) / 2);
	const x = Math.max(4, (TOUCH - width) / 2);
	return { top: y, bottom: y, left: x, right: x };
};

/**
 * The chip on an agent's row. A tap on it opens the run, and a tap on the
 * rest of the row opens the agent. VoiceOver reads the row as one, so it
 * reaches the run through the agent screen's chip.
 */
export const RowRunChip = ({ chip }: { chip: RunChip }) => {
	const { t } = useI18n();
	const open = useOpenRun();
	const look = chipLook(chip);
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={t("Workflow, {{spoken}}", { spoken: chip.spoken })}
			hitSlop={slopOf(18, 64)}
			onPress={() => open(chip)}
			style={({ pressed }) => [
				styles.rowChip,
				{ borderColor: look.border },
				pressed && styles.pressed,
			]}
		>
			<Text numberOfLines={1} style={[styles.rowChipText, { color: look.text }]}>
				{chip.text}
			</Text>
		</Pressable>
	);
};

/** The chip under the agent screen's header, while the agent leads a run. */
export const RunChipBar = ({ agent }: { agent: AgentId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const open = useOpenRun();
	const chip = useMemo(() => chipOf(view, agent), [view, agent]);
	if (!chip) return null;
	const look = chipLook(chip);
	return (
		<View style={styles.bar}>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={t("Workflow, {{spoken}}", { spoken: chip.spoken })}
				hitSlop={slopOf(22, 120)}
				onPress={() => open(chip)}
				style={({ pressed }) => [
					styles.chip,
					{ borderColor: look.border },
					pressed && styles.pressed,
				]}
			>
				<Workflow size={12} strokeWidth={2} color={look.text} />
				<Text numberOfLines={1} style={[styles.chipText, { color: look.text }]}>
					{chip.text}
				</Text>
			</Pressable>
		</View>
	);
};

const styles = StyleSheet.create({
	rowChip: {
		height: 18,
		maxWidth: 96,
		flexShrink: 1,
		justifyContent: "center",
		paddingHorizontal: 5,
		borderRadius: 4,
		borderWidth: StyleSheet.hairlineWidth,
		backgroundColor: color.secondary,
	},
	rowChipText: { fontSize: size.pill, fontWeight: "600", fontVariant: ["tabular-nums"] },
	bar: {
		flexDirection: "row",
		paddingHorizontal: 14,
		paddingVertical: 6,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
	chip: {
		height: 22,
		maxWidth: 220,
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		paddingHorizontal: 7,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		backgroundColor: color.secondary,
	},
	chipText: { flexShrink: 1, fontSize: size.tiny, fontVariant: ["tabular-nums"] },
	pressed: { backgroundColor: color.accent },
});
