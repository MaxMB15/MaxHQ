// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What one device keeps for a project, as the desktop's card for it: its
// own form with its own Save, then its "Always allow" command rules. The
// phone can't read the Mac's git identity, so a device that keeps nothing
// yet starts with no commit author here. The Mac sends its identity the
// first time it makes a workspace or starts an agent there.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId, ProjectId, Timestamp } from "@maxhq/shared/bindings";
import {
	allowedWorkflowEnvText,
	commandRulesOn,
	deviceSettleDays,
	draftOf,
	planProjectSettings,
	projectCapabilities,
	type ProjectDraft,
	type ProjectEdit,
	projectSettingsOn,
	sameDraft,
	shownDraft,
} from "@maxhq/shared/settings/project";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { CardBody, NoteLine, useAction } from "../devices/section";
import { GatedToggle } from "../test-hosts/parts";
import { Box, Labeled, Setting, UnitBox } from "./project-fields";
import { projectRows } from "./project-form";
import { ProjectRules } from "./project-rules";

export const ProjectDevice = ({
	project,
	projectName,
	device,
	view,
	now,
}: {
	project: ProjectId;
	projectName: string;
	device: DeviceId;
	view: ViewState;
	now: Timestamp;
}) => {
	useI18n();
	const { source } = useServices();
	const name = deviceName(view, device);
	const live = isLive(view, device, now);
	const kept = projectSettingsOn(view, project, device);
	const current = draftOf(kept);
	const deviceDays = deviceSettleDays(view, device);
	const rows = projectRows({
		name,
		live,
		capabilities: projectCapabilities(view, device),
		deviceDays,
		allowed: allowedWorkflowEnvText(kept, now),
	});
	// What you typed, kept until the device reports something new.
	const [edit, setEdit] = useState<ProjectEdit | null>(null);
	const { busy, note, setNote, run } = useAction();
	const draft = shownDraft(edit, current, current);
	const set = (change: Partial<ProjectDraft>) => {
		setNote(null);
		setEdit({ draft: { ...draft, ...change }, from: current });
	};
	const plan = planProjectSettings(project, device, draft);
	const changed = !sameDraft(draft, current);

	const save = () => {
		if (!plan.ok || !changed || busy) return;
		void run(t("Saving"), () => source.setProjectSettings(plan.settings), t("Saved"));
	};

	return (
		<Card
			title={t("On {{name}}", { name: name })}
			note={rows.state}
			label={t("{{projectName}} on {{name}}", { projectName: projectName, name: name })}
		>
			<CardRow
				label={t("Resume after a restart")}
				note={t(
					"When the device or a container restarts, agents that were busy carry on with a message naming what got cut off. Off leaves them paused."
				)}
			>
				<GatedToggle
					label={t("Resume after a restart")}
					value={draft.resume}
					disabled={!live}
					onChange={(on) => set({ resume: on })}
				/>
			</CardRow>
			<Setting
				label={t("Ports never offered")}
				note={t(
					"MaxHQ offers to open a port when something listens on it in a container. It never offers these, nor the ones it always skips, such as 22."
				)}
			>
				<Box
					accessibilityLabel={t("Ports never offered")}
					value={draft.ports}
					onChangeText={(ports) => set({ ports })}
					placeholder={t("Such as 9229, 35729")}
					keyboardType="numbers-and-punctuation"
					editable={live}
				/>
			</Setting>
			<Setting label={t("Auto-settle after")} note={rows.settle.note}>
				<UnitBox
					accessibilityLabel={t("Auto-settle after")}
					unit={t("days")}
					value={draft.autoSettle}
					onChangeText={(autoSettle) => set({ autoSettle })}
					placeholder={String(deviceDays)}
					keyboardType="number-pad"
					editable={rows.settle.editable}
				/>
			</Setting>
			<CardRow label={t("Settle an agent when its PR merges")} note={rows.pulls.note}>
				<GatedToggle
					label={t("Settle an agent when its PR merges")}
					value={draft.settleOnMerge}
					disabled={!rows.pulls.editable}
					onChange={(on) => set({ settleOnMerge: on })}
				/>
			</CardRow>
			<CardRow label={t("Give agents the workflow's variables")} note={rows.workflows.note}>
				<GatedToggle
					label={t("Give agents the workflow's variables")}
					value={draft.workflowEnv}
					disabled={!rows.workflows.editable}
					onChange={(on) => set({ workflowEnv: on })}
				/>
			</CardRow>
			<Setting label={t("Memory per container")} note={rows.memory.note}>
				<UnitBox
					accessibilityLabel={t("Memory per container")}
					unit="GiB"
					value={draft.memory}
					onChangeText={(memory) => set({ memory })}
					placeholder={t("Host's")}
					keyboardType="decimal-pad"
					editable={rows.memory.editable}
				/>
			</Setting>
			<Setting
				label={t("Commit author")}
				note={t(
					"The name and email on commits agents make in this project's containers. GitHub still shows the App as the one that pushed."
				)}
			>
				<Labeled label={t("Name")}>
					<Box
						accessibilityLabel={t("Name")}
						value={draft.name}
						onChangeText={(text) => set({ name: text })}
						textContentType="name"
						editable={live}
					/>
				</Labeled>
				<Labeled label={t("Email")}>
					<Box
						accessibilityLabel={t("Email")}
						value={draft.email}
						onChangeText={(email) => set({ email })}
						keyboardType="email-address"
						textContentType="emailAddress"
						editable={live}
					/>
				</Labeled>
			</Setting>
			<CardBody>
				<View style={styles.save}>
					<View style={styles.grow}>
						{plan.ok ? (
							<NoteLine note={note} />
						) : (
							<Text accessibilityLiveRegion="polite" style={styles.error}>
								{plan.reason}
							</Text>
						)}
					</View>
					<Button
						label={t("Save")}
						tone="primary"
						small
						disabled={!live || !changed || !plan.ok || busy}
						onPress={save}
					/>
				</View>
			</CardBody>
			<ProjectRules
				project={project}
				device={device}
				rules={commandRulesOn(view, project, device)}
				live={live}
			/>
		</Card>
	);
};

const styles = StyleSheet.create({
	save: { flexDirection: "row", alignItems: "center", gap: 12 },
	grow: { flex: 1 },
	error: { color: color.errorText, fontSize: size.meta },
});
