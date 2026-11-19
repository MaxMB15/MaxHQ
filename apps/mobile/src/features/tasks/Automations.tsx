// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Automations under an agent's header, as the desktop's: the scheduled
// tasks that send this agent their prompt, each with when it runs next, a
// switch that pauses it and Run now. It shows only when a task targets the
// agent. A task's name opens it in the editor.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { Agent } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { localZone } from "@maxhq/shared/tasks/draft";
import { nextRunLine, triggerLine } from "@maxhq/shared/tasks/format";
import { type HomedTask, tasksTargeting } from "@maxhq/shared/tasks/place";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Toggle } from "../../ui/Toggle";
import { useTaskRow } from "./actions";

const Automation = ({ homed: { task, home } }: { homed: HomedTask }) => {
	const { t } = useI18n();

	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const now = useNow(30_000);
	const row = useTaskRow(task, home);
	return (
		<View style={styles.automation}>
			<View style={styles.line}>
				<Pressable
					accessibilityRole="button"
					accessibilityHint={t("Edit task")}
					onPress={() => navigation.navigate("TaskEditor", { task: task.id, home })}
					style={({ pressed }) => [styles.text, pressed && styles.pressed]}
				>
					<Text style={styles.name} numberOfLines={1}>
						{task.name}
					</Text>
					<Text style={styles.when} numberOfLines={1}>
						{triggerLine(task.trigger, localZone())}. {nextRunLine(task, now)}
					</Text>
				</Pressable>
				<Button
					small
					label={row.run === "started" ? t("Started") : t("Run now")}
					hint={t("Run {{name}} now", { name: task.name })}
					disabled={row.run !== "ready"}
					onPress={() => void row.runNow()}
				/>
				<Toggle
					label={t("{{name}} on", { name: task.name })}
					value={task.enabled}
					onChange={(on) => void row.setEnabled(on)}
				/>
			</View>
			{row.failure ? (
				<Text style={styles.failed} accessibilityRole="alert">
					{row.failure}
				</Text>
			) : null}
		</View>
	);
};

/** The tasks that reach an agent, or nothing when none does. */
export const Automations = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const homed = tasksTargeting(view, agent.id);
	if (homed.length === 0) return null;
	return (
		<View style={styles.section} accessibilityLabel={t("Automations")}>
			<Text accessibilityRole="header" style={styles.heading}>
				{t("Automations")}
			</Text>
			{homed.map((h) => (
				<Automation key={h.task.id} homed={h} />
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	section: {
		gap: 4,
		paddingHorizontal: 14,
		paddingVertical: 8,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	heading: {
		color: color.mutedForeground,
		fontSize: size.pill,
		fontWeight: "600",
		letterSpacing: 0.5,
		textTransform: "uppercase",
	},
	automation: { gap: 2 },
	line: { flexDirection: "row", alignItems: "center", gap: 10 },
	text: { flex: 1, minWidth: 0, minHeight: 44, justifyContent: "center" },
	pressed: { opacity: 0.6 },
	name: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	when: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
});
