// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's container workspaces on its page, as the desktop shows them:
// a row for each one MaxHQ made, a note on the containers it leaves alone,
// New workspace, which opens the form, and Copy to another device, which
// opens the copy form with this device to copy onto. Both need the Mac's
// switch for this phone.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { StyleSheet, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import { deviceWorkspaces, showsSnapshots } from "@maxhq/shared/workspaces/device";
import { canCopy, workspaceActions } from "@maxhq/shared/workspaces/state";

import type { Screens } from "../../navigation";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { Buttons, CardBody, Muted, type SectionProps } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { WorkspaceRow } from "./WorkspaceRow";

export const DeviceWorkspaces = ({ device, name, view }: SectionProps) => {
	const { t } = useI18n();

	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const now = useNow(5000);
	const lets = useLetsPhone(device);
	const section = deviceWorkspaces(view, device, now);
	if (!section) return null;
	const { made, makes, live } = section;

	return (
		<Card title={t("Workspaces on {{name}}", { name: name })} note={section.hint}>
			{made.length > 0 ? (
				made.map((w) => (
					<WorkspaceRow
						key={w.id}
						workspace={w}
						project={
							w.project
								? (view.tables.project.get(w.project)?.name ?? w.project)
								: null
						}
						where={name}
						live={live}
						lets={lets}
						snapshots={showsSnapshots(
							view,
							w,
							live ? workspaceActions(w.managed.state) : []
						)}
					/>
				))
			) : (
				<CardBody>
					<Muted>
						{t(
							"No workspaces yet. Make one for a project and its agents run in a container here."
						)}
					</Muted>
				</CardBody>
			)}
			<CardBody>
				{section.footer ? <Muted>{section.footer}</Muted> : null}
				{makes ? (
					<Buttons>
						<Button
							label={t("New workspace")}
							tone="primary"
							disabled={!live || !lets}
							onPress={() => navigation.navigate("NewWorkspace", { device })}
						/>
					</Buttons>
				) : null}
				<View style={styles.copy}>
					<Button
						label={t("Copy to another device…")}
						small
						disabled={!live || !canCopy(view, device) || !lets}
						onPress={() => navigation.navigate("CopyProject", { device })}
					/>
				</View>
				{lets ? null : <NeedsSwitch />}
			</CardBody>
		</Card>
	);
};

const styles = StyleSheet.create({
	// A row, so the small button keeps its own width.
	copy: { flexDirection: "row" },
});
