// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's settings, as the desktop's General tab has them: what each
// device keeps for it, each in its own card with its own Save, and the
// commands you allowed there. A row on top opens the project's skills, as
// the desktop's Skills tab.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useMemo } from "react";
import { ScrollView, StyleSheet, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import { projectDevices } from "@maxhq/shared/settings/project";

import type { ScreenProps } from "../../navigation";
import { useView } from "../../services";
import { color } from "../../theme";
import { Card, CardRow } from "../../ui/Card";
import { Header } from "../../ui/Header";
import { Muted } from "../devices/section";
import { ProjectDevice } from "./project-device";

export const ProjectSettingsScreen = ({ navigation, route }: ScreenProps<"ProjectSettings">) => {
	useI18n();
	const id = route.params.project;
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const project = view.tables.project.get(id);
	const devices = useMemo(() => projectDevices(view, id), [view, id]);
	const back = { label: t("Back"), onPress: () => navigation.goBack() };

	return (
		<View style={styles.screen}>
			<Header
				title={project?.name ?? t("Settings")}
				subtitle={project ? (project.repo ?? t("No remote")) : undefined}
				back={back}
			/>
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				automaticallyAdjustKeyboardInsets
			>
				{project ? (
					<Card label={t("Skills")}>
						<CardRow
							first
							label={t("Skills")}
							onPress={() => navigation.navigate("Skills", { project: id })}
						/>
					</Card>
				) : null}
				{!project ? (
					<Muted>{t("That project is gone.")}</Muted>
				) : devices.length === 0 ? (
					<Muted>
						{t("A device keeps a project's settings once it hosts workspaces.")}
					</Muted>
				) : (
					devices.map((device) => (
						<ProjectDevice
							key={device}
							project={id}
							projectName={project.name}
							device={device}
							view={view}
							now={now}
						/>
					))
				)}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
});
