// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tools a device's maxd installs beside the agent CLIs: helpers such
// as Node.js and gh, and ACP agents from the registry, on the device
// itself and in each container workspace. Each row says where its install
// stands, with a bar while it downloads, and offers Install, Remove, Try
// again or the licences it waits on.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { ConfirmButton, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId, HelperTool, ToolKind, ToolPlace } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { registryNames, registryOf } from "@maxhq/shared/hosts/registry";
import {
	HELPER_NAME,
	type InstallProgress,
	installRows,
	missingHelpers,
	offersToolInstalls,
	type ToolRow,
} from "@maxhq/shared/hosts/tools";
import type { ViewState } from "@maxhq/shared/view/replica";

import { LicenceDialog } from "./licence-dialog";

const HOST: ToolPlace = { type: "host" };

/** A thin bar for a running install, sliding while its size isn't known. */
export const ProgressBar = ({ progress, label }: { progress: InstallProgress; label: string }) => {
	useI18n();
	return (
		<div
			role="progressbar"
			aria-label={label}
			aria-valuetext={progress.text}
			aria-valuemin={progress.fraction === null ? undefined : 0}
			aria-valuemax={progress.fraction === null ? undefined : 100}
			aria-valuenow={
				progress.fraction === null ? undefined : Math.round(progress.fraction * 100)
			}
			className="relative h-1 w-[120px] overflow-hidden rounded-full bg-track"
		>
			{progress.fraction === null ? (
				<span className="absolute inset-y-0 left-0 w-1/3 animate-pulse rounded-full bg-primary" />
			) : (
				<span
					className="absolute inset-y-0 left-0 rounded-full bg-primary transition-[width]"
					style={{ width: `${progress.fraction * 100}%` }}
				/>
			)}
		</div>
	);
};

/** Sends one install or removal, with the note that says how it went. */
const useSend = () => {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const send = async (what: string, call: () => Promise<ActionResult>) => {
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(await call());
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn(`Couldn't ${what}`, error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return { busy, note, send };
};

const InstallRow = ({
	device,
	row,
	live,
	onLicences,
}: {
	device: DeviceId;
	row: ToolRow;
	live: boolean;
	onLicences: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, send } = useSend();
	const { tool, name, status, progress } = row;
	const install = () =>
		void send(t("install the tool"), () => source.installTool(device, tool.place, tool.kind));
	const remove = () =>
		void send(t("remove the tool"), () => source.removeTool(device, tool.place, tool.kind));
	const state = tool.state.type;
	const installed = state === "idle" && tool.version !== null;
	const sub =
		installed && tool.owner !== "max_hq"
			? t("{{status}}. You installed it.", { status: status })
			: (progress?.text ?? status);
	return (
		<Row title={name} sub={sub}>
			<StatusNote note={note} />
			{progress ? (
				<ProgressBar progress={progress} label={t("Installing {{name}}", { name: name })} />
			) : null}
			{state === "licences" ? (
				<button type="button" className={SECONDARY} disabled={!live} onClick={onLicences}>
					{t("Read licences")}
				</button>
			) : null}
			{state === "failed" ? (
				<button
					type="button"
					className={SECONDARY}
					aria-label={t("Try installing {{name}} again", { name: name })}
					disabled={!live || busy}
					onClick={install}
				>
					{t("Try again")}
				</button>
			) : null}
			{state === "idle" && tool.version === null ? (
				<button
					type="button"
					className={SECONDARY}
					aria-label={t("Install {{name}}", { name: name })}
					disabled={!live || busy}
					onClick={install}
				>
					{t("Install")}
				</button>
			) : null}
			{installed && tool.owner === "max_hq" ? (
				<ConfirmButton
					label={t("Remove")}
					question={t("Remove {{name}}?", { name: name })}
					confirm={t("Remove {{name}}", { name: name })}
					disabled={!live || busy}
					onConfirm={remove}
				/>
			) : null}
		</Row>
	);
};

/** A helper the device doesn't have, with Install. */
const MissingRow = ({
	device,
	tool,
	live,
}: {
	device: DeviceId;
	tool: HelperTool;
	live: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, send } = useSend();
	const name = HELPER_NAME[tool];
	const kind: ToolKind = { type: "helper", tool };
	return (
		<Row title={name} sub={t("Not installed")}>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				aria-label={t("Install {{name}}", { name: name })}
				disabled={!live || busy}
				onClick={() =>
					void send(t("install the tool"), () => source.installTool(device, HOST, kind))
				}
			>
				{t("Install")}
			</button>
		</Row>
	);
};

const placeKey = (place: ToolPlace): string => (place.type === "host" ? "host" : place.workspace);

const kindKey = (kind: ToolKind): string =>
	kind.type === "acp" ? `acp-${kind.id}` : `${kind.type}-${kind.tool}`;

/** The helpers and ACP agents on a device and in its workspaces. */
export const DeviceInstalls = ({
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

	const [reading, setReading] = useState(false);
	const offered = offersToolInstalls(view, device);
	const rows = installRows(view, device, registryNames(registryOf(view, device)));
	const groups = new Map<string, { label: string; rows: ToolRow[] }>([
		["host", { label: `On ${name}`, rows: [] }],
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
		<Section
			title={t("Tools")}
			label={t("Tools on {{name}}", { name: name })}
			hint={t("Installed and kept up to date by MaxHQ")}
		>
			{!offered ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{live
						? t("{{name}}'s maxd is too old to install tools. Update it first.", {
								name: name,
							})
						: t("Connect to {{name}} to see its tools.", { name: name })}
				</p>
			) : (
				[...groups].map(([key, group]) =>
					group.rows.length === 0 && (key !== "host" || missing.length === 0) ? null : (
						<div key={key} role="group" aria-label={group.label}>
							<h3 className="border-t border-sidebar-border px-4 pt-3 text-[11.5px] font-[600] text-secondary-foreground first:border-t-0">
								{group.label}
							</h3>
							{group.rows.map((row) => (
								<InstallRow
									key={kindKey(row.tool.kind)}
									device={device}
									row={row}
									live={live}
									onLicences={() => setReading(true)}
								/>
							))}
							{key === "host"
								? missing.map((tool) => (
										<MissingRow
											key={tool}
											device={device}
											tool={tool}
											live={live}
										/>
									))
								: null}
						</div>
					)
				)
			)}
			<LicenceDialog
				device={device}
				view={view}
				open={reading}
				onClose={() => setReading(false)}
			/>
		</Section>
	);
};
