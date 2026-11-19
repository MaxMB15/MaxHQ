// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tools a device's maxd installs beside the agent CLIs, as the desktop
// shows them: helpers such as Node.js and gh, and ACP agents from the
// registry, on the device and in each container workspace. Each row says
// where its install stands, with a bar while it downloads. Reading and
// accepting licences works from any phone; Install, Try again and Remove
// need the Mac's switch for this phone.

import { useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { memo } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId, HelperTool, ToolKind, ToolPlace } from "@maxhq/shared/bindings";
import { registryNames, registryOf } from "@maxhq/shared/hosts/registry";
import {
	HELPER_NAME,
	type InstallProgress,
	installRows,
	missingHelpers,
	offersToolInstalls,
	type ToolRow,
} from "@maxhq/shared/hosts/tools";

import type { Screens } from "../../navigation";
import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Buttons, CardBody, Muted, NoteLine, type SectionProps, useAction } from "./section";

const HOST: ToolPlace = { type: "host" };

/** A thin bar for a running install, a still third while its size isn't known. */
const ProgressBar = ({ progress, label }: { progress: InstallProgress; label: string }) => (
	<View
		accessible
		accessibilityRole="progressbar"
		accessibilityLabel={label}
		accessibilityValue={
			progress.fraction === null
				? { text: progress.text }
				: {
						min: 0,
						max: 100,
						now: Math.round(progress.fraction * 100),
						text: progress.text,
					}
		}
		style={styles.track}
	>
		<View
			style={[
				styles.fill,
				{ width: progress.fraction === null ? "33%" : `${progress.fraction * 100}%` },
				progress.fraction === null && styles.unknown,
			]}
		/>
	</View>
);

const InstallRow = memo(function InstallRow({
	device,
	row,
	live,
	lets,
}: {
	device: DeviceId;
	row: ToolRow;
	live: boolean;
	lets: boolean;
}) {
	const { t } = useI18n();

	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const { source } = useServices();
	const { busy, note, run } = useAction();
	const { tool, name, status, progress } = row;
	const install = () =>
		void run(t("Sending"), () => source.installTool(device, tool.place, tool.kind));
	const remove = () =>
		void run(t("Sending"), () => source.removeTool(device, tool.place, tool.kind));
	const state = tool.state.type;
	const installed = state === "idle" && tool.version !== null;
	const sub =
		installed && tool.owner !== "max_hq"
			? t("{{status}}. You installed it.", { status: status })
			: (progress?.text ?? status);
	const off = !live || busy || !lets;
	return (
		<CardBody>
			<Text style={styles.title}>{name}</Text>
			<Muted>{sub}</Muted>
			{progress ? (
				<ProgressBar progress={progress} label={t("Installing {{name}}", { name: name })} />
			) : null}
			<Buttons>
				{state === "licences" ? (
					<Button
						label={t("Read licences")}
						small
						disabled={!live}
						onPress={() => navigation.navigate("Licences", { device })}
					/>
				) : null}
				{state === "failed" ? (
					<Button
						label={t("Try again")}
						hint={t("Try installing {{name}} again", { name: name })}
						small
						disabled={off}
						onPress={install}
					/>
				) : null}
				{state === "idle" && tool.version === null ? (
					<Button
						label={t("Install")}
						hint={t("Install {{name}}", { name: name })}
						small
						disabled={off}
						onPress={install}
					/>
				) : null}
				{installed && tool.owner === "max_hq" ? (
					<Button
						label={t("Remove")}
						tone="danger"
						small
						disabled={off}
						onPress={() =>
							confirm(
								{ title: t("Remove {{name}}?", { name: name }), detail: "" },
								t("Remove {{name}}", { name: name }),
								remove
							)
						}
					/>
				) : null}
			</Buttons>
			<NoteLine note={note} />
		</CardBody>
	);
});

/** A helper the device doesn't have, with Install. */
const MissingRow = memo(function MissingRow({
	device,
	tool,
	live,
	lets,
}: {
	device: DeviceId;
	tool: HelperTool;
	live: boolean;
	lets: boolean;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	const name = HELPER_NAME[tool];
	const kind: ToolKind = { type: "helper", tool };
	return (
		<CardBody>
			<Text style={styles.title}>{name}</Text>
			<Muted>{t("Not installed")}</Muted>
			<Buttons>
				<Button
					label={t("Install")}
					hint={t("Install {{name}}", { name: name })}
					small
					disabled={!live || busy || !lets}
					onPress={() =>
						void run(t("Sending"), () => source.installTool(device, HOST, kind))
					}
				/>
			</Buttons>
			<NoteLine note={note} />
		</CardBody>
	);
});

const placeKey = (place: ToolPlace): string => (place.type === "host" ? "host" : place.workspace);

const kindKey = (kind: ToolKind): string =>
	kind.type === "acp" ? `acp-${kind.id}` : `${kind.type}-${kind.tool}`;

/** The helpers and ACP agents on a device and in its workspaces. */
export const DeviceInstalls = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const lets = useLetsPhone(device);
	const offered = offersToolInstalls(view, device);
	const rows = installRows(view, device, registryNames(registryOf(view, device)));
	const groups = new Map<string, { label: string; rows: ToolRow[] }>([
		["host", { label: t("On {{name}}", { name: name }), rows: [] }],
	]);
	for (const row of rows) {
		const key = placeKey(row.tool.place);
		if (!groups.has(key)) {
			const workspace =
				row.tool.place.type === "workspace"
					? view.tables.workspace.get(row.tool.place.workspace)
					: undefined;
			const where =
				workspace?.location.type === "container" ? workspace.location.instance : key;
			groups.set(key, { label: t("In {{where}}", { where: where }), rows: [] });
		}
		groups.get(key)?.rows.push(row);
	}
	const missing = offered ? missingHelpers(view, device) : [];

	return (
		<Card
			title={t("Tools")}
			label={t("Tools on {{name}}", { name: name })}
			note={t("Installed and kept up to date by MaxHQ")}
		>
			{!offered ? (
				<CardBody>
					<Muted>
						{live
							? t("{{name}}'s maxd is too old to install tools. Update it first.", {
									name: name,
								})
							: t("Connect to {{name}} to see its tools.", { name: name })}
					</Muted>
				</CardBody>
			) : (
				[...groups].map(([key, group]) =>
					group.rows.length === 0 && (key !== "host" || missing.length === 0) ? null : (
						<View key={key} accessibilityLabel={group.label}>
							<Text style={styles.group}>{group.label}</Text>
							{group.rows.map((row) => (
								<InstallRow
									key={kindKey(row.tool.kind)}
									device={device}
									row={row}
									live={live}
									lets={lets}
								/>
							))}
							{key === "host"
								? missing.map((tool) => (
										<MissingRow
											key={tool}
											device={device}
											tool={tool}
											live={live}
											lets={lets}
										/>
									))
								: null}
						</View>
					)
				)
			)}
			{offered && !lets && (rows.length > 0 || missing.length > 0) ? (
				<CardBody>
					<NeedsSwitch />
				</CardBody>
			) : null}
		</Card>
	);
};

const styles = StyleSheet.create({
	group: {
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
		paddingHorizontal: 14,
		paddingTop: 12,
		color: color.secondaryForeground,
		fontSize: size.tiny,
		fontWeight: "600",
	},
	title: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	track: {
		height: 4,
		width: 140,
		borderRadius: 2,
		backgroundColor: color.track,
		overflow: "hidden",
	},
	fill: { height: "100%", borderRadius: 2, backgroundColor: color.primary },
	unknown: { opacity: 0.6 },
});
