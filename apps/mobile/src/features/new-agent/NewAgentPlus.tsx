// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Plus at the end of a project's group header in the agents list, which
// opens New agent in that project. The header itself folds the group, so
// with VoiceOver or TalkBack the same choice is an action on the header,
// which `newAgentAction` gives it.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { Pressable, StyleSheet } from "react-native";
import type { AccessibilityActionEvent } from "react-native";

import type { ProjectId } from "@maxhq/shared/bindings";

import type { Screens } from "../../navigation";
import { color, radius, TOUCH } from "../../theme";
import { Plus } from "../../ui/Icon";

const SIZE = 24;
const SLOP = (TOUCH - SIZE) / 2;

type Navigation = NativeStackNavigationProp<Screens>;

export const NewAgentPlus = ({ name, project }: { name: string; project: ProjectId }) => {
	const { t } = useI18n();

	const navigation = useNavigation<Navigation>();
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={t("New agent in {{name}}", { name })}
			hitSlop={SLOP}
			onPress={() => navigation.navigate("NewAgent", { project })}
			style={({ pressed }) => [styles.plus, pressed && styles.pressed]}
		>
			<Plus size={13} tint={color.mutedForeground} />
		</Pressable>
	);
};

/**
 * The New agent action for a project group's header, for screen readers,
 * which read the header as one button and so skip the Plus inside it.
 */
export const newAgentAction = (
	navigation: Pick<Navigation, "navigate">,
	name: string,
	project: ProjectId | null
) =>
	project === null
		? {}
		: {
				accessibilityActions: [
					{ name: "newAgent", label: t("New agent in {{name}}", { name }) },
				],
				onAccessibilityAction: (event: AccessibilityActionEvent) => {
					if (event.nativeEvent.actionName === "newAgent")
						navigation.navigate("NewAgent", { project });
				},
			};

const styles = StyleSheet.create({
	plus: {
		width: SIZE,
		height: SIZE,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: radius.sm,
	},
	pressed: { backgroundColor: color.accent },
});
