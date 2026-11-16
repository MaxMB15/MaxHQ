// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The scheduled tasks, as the desktop's Settings page lists them: every
// device's tasks, grouped by the device each lives on, under the address
// its webhooks are reached at. Each task has its switch, when it runs next
// and how it last went, Run now, History, Edit and Delete. A device whose
// maxd predates tasks isn't listed. Opened from the agent list's menu.

import { useI18n } from "@maxhq/shared/i18n";

import { ScrollView, StyleSheet, Text, View } from "react-native";

import type { DeviceId, DeviceTasks, FunnelState, Task, TaskProblem } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { localZone } from "@maxhq/shared/tasks/draft";
import {
	addressHint,
	addressOf,
	hiddenHookUrl,
	lastRunLine,
	needsSecret,
	nextRunLine,
	triggerLine,
	webhookLine,
} from "@maxhq/shared/tasks/format";
import { deviceTasks } from "@maxhq/shared/tasks/place";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { confirm } from "../../ui/choose";
import { Card, CardRow } from "../../ui/Card";
import { Header, IconButton } from "../../ui/Header";
import { Plus } from "../../ui/Icon";
import { Toggle } from "../../ui/Toggle";
import { OutLink } from "../pulls/look";
import { useTaskRow } from "./actions";
import { AddressRow } from "./hook";

const TaskRow = ({
	task,
	home,
	funnel,
	webhooks,
	problem,
	onEdit,
	onActivity,
}: {
	task: Task;
	home: DeviceId;
	/** Whether the device's webhooks can be reached, and where. */
	funnel: FunnelState;
	/** How the device's GitHub events tasks hear of events. */
	webhooks: DeviceTasks["webhooks"];
	/** Why the task can't run now, as its device says. */
	problem: TaskProblem | null;
	onEdit: () => void;
	onActivity: () => void;
}) => {
	const { t } = useI18n();
	const now = useNow(30_000);
	const row = useTaskRow(task, home);
	const heard = webhookLine(task, { webhooks }, now);
	return (
		<View style={styles.task}>
			<View style={styles.head}>
				<Text style={styles.name}>{task.name}</Text>
				<Toggle
					label={t("{{name}} on", { name: task.name })}
					value={task.enabled}
					onChange={(on) => void row.setEnabled(on)}
				/>
			</View>
			<Text style={styles.sub}>
				{triggerLine(task.trigger, localZone())}. {nextRunLine(task, now)}.{" "}
				{lastRunLine(task.last_run, now)}.
			</Text>
			{heard ? <Text style={styles.sub}>{heard}.</Text> : null}
			{needsSecret(task) ? (
				<Text style={styles.warn}>
					{t("Add the signing secret. Until then the webhook refuses every request.")}
				</Text>
			) : null}
			{problem ? (
				<View style={styles.problem}>
					<Text style={styles.warn}>{problem.reason}</Text>
					{problem.link ? (
						<OutLink url={problem.link}>{t("Open the GitHub App's settings")}</OutLink>
					) : null}
				</View>
			) : null}
			{task.trigger.type === "hook" ? (
				<Text style={styles.url} numberOfLines={1} ellipsizeMode="middle">
					{hiddenHookUrl(task.id, funnel)}
				</Text>
			) : null}
			<View style={styles.buttons}>
				<Button
					small
					label={row.run === "started" ? t("Started") : t("Run now")}
					hint={t("Run {{name}} now", { name: task.name })}
					disabled={row.run !== "ready"}
					onPress={() => void row.runNow()}
				/>
				<Button small label={t("History")} onPress={onActivity} />
				<Button
					small
					label={t("Edit")}
					hint={t("Edit {{name}}", { name: task.name })}
					onPress={onEdit}
				/>
				<Button
					small
					tone="danger"
					label={t("Delete")}
					onPress={() =>
						confirm(
							{ title: task.name, detail: t("Its runs and requests go too.") },
							t("Delete"),
							() => void row.remove()
						)
					}
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

export const TasksScreen = ({ navigation }: ScreenProps<"Tasks">) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const groups = deviceTasks(view);
	return (
		<View style={styles.screen}>
			<Header
				title={t("Scheduled tasks")}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				right={
					groups.length > 0 ? (
						<IconButton
							label={t("New task")}
							onPress={() => navigation.navigate("TaskEditor")}
						>
							<Plus />
						</IconButton>
					) : undefined
				}
			/>
			<ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
				{groups.length === 0 ? (
					<Card label={t("Scheduled tasks")}>
						<CardRow
							first
							label={t("No device runs scheduled tasks yet")}
							note={t("A device's maxd runs them once it's updated.")}
						/>
					</Card>
				) : (
					groups.map((reach) => {
						const { device, tasks, funnel, problems, webhooks } = reach;
						const name = deviceName(view, device);
						return (
							<Card
								key={device}
								title={name}
								note={
									tasks.some(
										(task) =>
											task.trigger.type !== "every" &&
											task.trigger.type !== "at"
									)
										? addressHint(addressOf(reach), funnel)
										: undefined
								}
							>
								<AddressRow reach={reach} name={name} />
								{tasks.length === 0 ? (
									<CardRow label={t("No tasks on this device")} />
								) : (
									tasks.map((task) => (
										<TaskRow
											key={task.id}
											task={task}
											home={device}
											funnel={funnel}
											webhooks={webhooks}
											problem={
												problems.find((p) => p.task === task.id) ?? null
											}
											onEdit={() =>
												navigation.navigate("TaskEditor", {
													task: task.id,
													home: device,
												})
											}
											onActivity={() =>
												navigation.navigate("TaskActivity", {
													task: task.id,
												})
											}
										/>
									))
								)}
							</Card>
						);
					})
				)}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	task: {
		gap: 4,
		paddingHorizontal: 14,
		paddingVertical: 10,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	head: { flexDirection: "row", alignItems: "center", gap: 12 },
	name: { flex: 1, color: color.foreground, fontSize: size.small, fontWeight: "600" },
	sub: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	warn: { color: color.busyText, fontSize: size.meta, lineHeight: 17 },
	problem: { gap: 4, alignItems: "flex-start" },
	url: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.tiny },
	buttons: { flexDirection: "row", flexWrap: "wrap", gap: 8, paddingTop: 6 },
	failed: { color: color.errorText, fontSize: size.meta },
});
