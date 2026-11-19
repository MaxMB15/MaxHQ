// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent CLIs on a device, on the host and in each workspace MaxHQ made,
// as the desktop shows them: the version each runs, the newest one, and how
// it gets updated. Ticking a host CLI and updating one change the device,
// so both need the Mac's switch for this phone.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import {
	agentTools,
	type AgentToolInfo,
	hasNewer,
	mayUpdate,
	offersUpdate,
	targetOf,
	TOOL_NAME,
	toolLabel,
	versionLine,
} from "@maxhq/shared/fleet/update-view";
import type { Tool } from "@maxhq/shared/fleet/wire";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { GatedToggle } from "../test-hosts/parts";
import {
	Buttons,
	CardBody,
	CopyTextButton,
	Muted,
	type Note,
	NoteLine,
	type SectionProps,
	useAction,
} from "./section";

const ToolRow = memo(function ToolRow({
	device,
	tool,
	view,
	live,
	lets,
	ticked,
	onTick,
}: {
	device: DeviceId;
	tool: AgentToolInfo;
	view: ViewState;
	live: boolean;
	lets: boolean;
	ticked: readonly Tool[];
	onTick: (tool: Tool, on: boolean) => void;
}) {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	const label = toolLabel(view, tool);
	const running = tool.state.type === "running";
	const state: Note | null =
		tool.state.type === "running"
			? { text: tool.state.message, error: false }
			: tool.state.type === "failed"
				? { text: tool.state.reason, error: true }
				: note;
	const updates = offersUpdate(tool) && hasNewer(tool);

	return (
		<CardBody>
			<Text style={styles.title}>{label}</Text>
			{tool.update.type === "manual" ? (
				<Muted>
					{versionLine(tool)} {t("Update it yourself with")}{" "}
					<Text style={styles.command}>{tool.update.command}</Text>.
				</Muted>
			) : tool.update.type === "never" ? (
				<Muted>
					{t("{{value1}} MaxHQ doesn't update it.", { value1: versionLine(tool) })}
				</Muted>
			) : (
				<Muted>{versionLine(tool)}</Muted>
			)}
			{tool.update.type === "needs_tick" ? (
				<View style={styles.tick}>
					<Text style={styles.tickLabel}>{t("Let MaxHQ update")}</Text>
					<GatedToggle
						label={t("Let MaxHQ update {{value1}}", {
							value1: TOOL_NAME[tool.kind.tool],
						})}
						value={ticked.includes(tool.kind.tool)}
						disabled={!live || !lets}
						onChange={(on) => onTick(tool.kind.tool, on)}
					/>
				</View>
			) : null}
			{tool.update.type === "manual" || updates ? (
				<Buttons>
					{tool.update.type === "manual" ? (
						<CopyTextButton label={t("Copy command")} text={tool.update.command} />
					) : null}
					{updates ? (
						<Button
							label={t("Update")}
							hint={t("Update {{label}}", { label: label })}
							small
							disabled={!live || busy || running || !lets || !mayUpdate(tool, ticked)}
							onPress={() =>
								void run(t("Sending"), () =>
									source.updateTools(device, [targetOf(tool)])
								)
							}
						/>
					) : null}
				</Buttons>
			) : null}
			<NoteLine note={state} />
		</CardBody>
	);
});

/** The device's agent CLIs, each with its version and how it updates. */
export const DeviceTools = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const lets = useLetsPhone(device);
	const [note, setNote] = useState<Note | null>(null);
	const tools = agentTools(view.tables.device_tools.get(device)?.tools ?? []);
	const ticked = deviceSettingsOf(view, device).tool_updates;
	// The switch and the Update button change the device; the rest reads.
	const gated = tools.some(
		(tool) => tool.update.type === "needs_tick" || (offersUpdate(tool) && hasNewer(tool))
	);

	const tick = async (tool: Tool, on: boolean) => {
		const next = on
			? [...ticked.filter((x) => x !== tool), tool]
			: ticked.filter((x) => x !== tool);
		setNote({ text: t("Saving"), error: false });
		try {
			const failure = failureOf(
				await source.setDeviceSettings({
					device,
					...deviceSettingsOf(view, device),
					tool_updates: next,
				})
			);
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't save the device's settings", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		}
	};

	return (
		<Card
			title={t("Agent CLIs")}
			label={t("Agent CLIs on {{name}}", { name: name })}
			note={note ? undefined : t("Updated by Update everything too")}
		>
			{note ? (
				<CardBody>
					<NoteLine note={note} />
				</CardBody>
			) : null}
			{tools.length === 0 ? (
				<CardBody>
					<Muted>
						{live
							? t("{{name}} reports no agent CLIs.", { name: name })
							: t("Connect to {{name}} to see its CLIs.", { name: name })}
					</Muted>
				</CardBody>
			) : (
				tools.map((tool) => (
					<ToolRow
						key={`${tool.kind.tool}-${tool.place.type === "host" ? "host" : tool.place.workspace}`}
						device={device}
						tool={tool}
						view={view}
						live={live}
						lets={lets}
						ticked={ticked}
						onTick={(which, on) => void tick(which, on)}
					/>
				))
			)}
			{gated && !lets ? (
				<CardBody>
					<NeedsSwitch />
				</CardBody>
			) : null}
		</Card>
	);
};

const styles = StyleSheet.create({
	title: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	command: { color: color.foreground, fontFamily: font.mono },
	tick: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 10 },
	tickLabel: { color: color.secondaryForeground, fontSize: size.meta },
});
