// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent CLIs on a device, on the host and in each workspace MaxHQ made:
// the version each runs, the newest one, and how it gets updated. MaxHQ
// updates the ones it installed. The host's own CLIs wait until you tick
// them, and the ones it can't update show the command to run yourself.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { Switch } from "@/components/ui/switch";
import { Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId } from "@maxhq/shared/bindings";
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
import type { ViewState } from "@maxhq/shared/view/replica";

const ToolRow = ({
	device,
	tool,
	view,
	live,
	ticked,
	onTick,
}: {
	device: DeviceId;
	tool: AgentToolInfo;
	view: ViewState;
	live: boolean;
	ticked: readonly Tool[];
	onTick: (tool: Tool, on: boolean) => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const label = toolLabel(view, tool);
	const running = tool.state.type === "running";

	const update = async () => {
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(await source.updateTools(device, [targetOf(tool)]));
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't update the CLI", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const state =
		tool.state.type === "running"
			? { text: tool.state.message, error: false }
			: tool.state.type === "failed"
				? { text: tool.state.reason, error: true }
				: note;

	return (
		<Row
			title={label}
			sub={
				tool.update.type === "manual" ? (
					<>
						{versionLine(tool)} {t("Update it yourself with")}{" "}
						<code className="font-mono text-foreground">{tool.update.command}</code>.
					</>
				) : tool.update.type === "never" ? (
					t("{{value1}} MaxHQ doesn't update it.", { value1: versionLine(tool) })
				) : (
					versionLine(tool)
				)
			}
		>
			<StatusNote note={state} />
			{tool.update.type === "manual" ? (
				<CopyButton
					text={tool.update.command}
					label={t("Copy command")}
					icon={false}
					className={SECONDARY}
				/>
			) : null}
			{tool.update.type === "needs_tick" ? (
				<span className="flex items-center gap-2 text-xs text-secondary-foreground">
					{t("Let MaxHQ update")}
					<Switch
						aria-label={t("Let MaxHQ update {{value1}}", {
							value1: TOOL_NAME[tool.kind.tool],
						})}
						checked={ticked.includes(tool.kind.tool)}
						disabled={!live}
						onCheckedChange={(on) => onTick(tool.kind.tool, on)}
					/>
				</span>
			) : null}
			{offersUpdate(tool) && hasNewer(tool) ? (
				<button
					type="button"
					className={SECONDARY}
					aria-label={t("Update {{label}}", { label: label })}
					disabled={!live || busy || running || !mayUpdate(tool, ticked)}
					onClick={() => void update()}
				>
					{t("Update")}
				</button>
			) : null}
		</Row>
	);
};

/** The device's agent CLIs, each with its version and how it updates. */
export const DeviceTools = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [note, setNote] = useState<Note | null>(null);
	const tools = agentTools(view.tables.device_tools.get(device)?.tools ?? []);
	const ticked = deviceSettingsOf(view, device).tool_updates;

	const tick = async (tool: Tool, on: boolean) => {
		const next = on
			? [...ticked.filter((t) => t !== tool), tool]
			: ticked.filter((t) => t !== tool);
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
		<Section
			title={t("Agent CLIs")}
			label={t("Agent CLIs on {{name}}", { name: name })}
			hint={note ? <StatusNote note={note} /> : t("Updated by Update everything too")}
		>
			{tools.length === 0 ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{live
						? t("{{name}} reports no agent CLIs.", { name: name })
						: t("Connect to {{name}} to see its CLIs.", { name: name })}
				</p>
			) : (
				tools.map((tool) => (
					<ToolRow
						key={`${tool.kind.tool}-${tool.place.type === "host" ? "host" : tool.place.workspace}`}
						device={device}
						tool={tool}
						view={view}
						live={live}
						ticked={ticked}
						onTick={(t, on) => void tick(t, on)}
					/>
				))
			)}
		</Section>
	);
};
