// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The commands you answered "Always allow" for in a project, which a
// device answers for you from then on. Each one can go, without asking
// first, as on the desktop.

import { t, useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import type { CommandRule, DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { ruleKey } from "@maxhq/shared/settings/project";

import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Buttons, CopyTextButton, Muted, NoteLine, useAction } from "../devices/section";
import { Setting } from "./project-fields";

export const ProjectRules = ({
	project,
	device,
	rules,
	live,
}: {
	project: ProjectId;
	device: DeviceId;
	rules: readonly CommandRule[];
	/** Whether the device's maxd answers now. */
	live: boolean;
}) => {
	useI18n();
	const { source } = useServices();
	const { busy, note, run } = useAction();
	return (
		<Setting
			label={t("Allowed commands")}
			note={t(
				'Commands you answered "Always allow" for. Agents here run them without asking, except in Plan. A rule keeps a command and its subcommand, so "git show" covers any git show. One marked exact covers only that line.'
			)}
		>
			{rules.length === 0 ? <Muted>{t("None yet.")}</Muted> : null}
			{rules.map((rule) => (
				<View key={ruleKey(rule)} style={styles.rule}>
					<Text selectable numberOfLines={3} style={styles.command}>
						{rule.command}
					</Text>
					<View style={styles.actions}>
						{rule.exact ? <Text style={styles.exact}>{t("Exact")}</Text> : null}
						<View style={styles.grow} />
						<Buttons>
							<CopyTextButton label={t("Copy")} text={rule.command} />
							<Button
								label={t("Remove")}
								hint={t("Remove {{command}}", { command: rule.command })}
								tone="danger"
								small
								disabled={!live || busy}
								onPress={() =>
									void run(null, () =>
										source.removeCommandRule(device, project, rule)
									)
								}
							/>
						</Buttons>
					</View>
				</View>
			))}
			<NoteLine note={note} />
		</Setting>
	);
};

const styles = StyleSheet.create({
	rule: {
		gap: 6,
		paddingTop: 8,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	command: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	actions: { flexDirection: "row", alignItems: "center", gap: 8 },
	exact: { color: color.mutedForeground, fontSize: size.tiny },
	grow: { flex: 1 },
});
