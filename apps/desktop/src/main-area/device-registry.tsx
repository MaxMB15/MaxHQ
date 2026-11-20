// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The ACP registry as a device sees it: a search over its agents, each
// with how it would install and whether it's in, and Install or Update.
// A binary the registry gives no checksum for asks once before it goes.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useServices } from "@/app/services";
import { DANGER, FIELD, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId } from "@maxhq/shared/bindings";
import {
	registryOf,
	type RegistryRow,
	registryRows,
	uncheckedWarning,
} from "@maxhq/shared/hosts/registry";
import { offersToolInstalls } from "@maxhq/shared/hosts/tools";
import type { ViewState } from "@maxhq/shared/view/replica";

const AgentRow = ({ device, row, live }: { device: DeviceId; row: RegistryRow; live: boolean }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [asking, setAsking] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const { agent } = row;
	const verb = row.update ? t("Update") : t("Install");

	const install = async (confirmUnchecked: boolean) => {
		setAsking(false);
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(
				await source.installAcpAgent(
					device,
					agent.id,
					agent.version,
					{ type: "host" },
					confirmUnchecked
				)
			);
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't install the agent", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};

	const offers = row.installable && (agent.installed === null || row.update);
	return (
		<Row
			title={agent.name}
			sub={
				<>
					{agent.description ? <span className="block">{agent.description}</span> : null}
					{asking ? (
						<span role="alert" className="block text-status-error-text">
							{uncheckedWarning(agent)}
						</span>
					) : (
						<span className="block">{row.status}</span>
					)}
				</>
			}
		>
			<StatusNote note={note} />
			{asking ? (
				<>
					<button type="button" className={SECONDARY} onClick={() => setAsking(false)}>
						{t("Cancel")}
					</button>
					<button
						type="button"
						className={DANGER}
						disabled={!live || busy}
						onClick={() => void install(true)}
					>
						{t("Install anyway")}
					</button>
				</>
			) : offers ? (
				<button
					type="button"
					className={SECONDARY}
					aria-label={`${verb} ${agent.name}`}
					disabled={!live || busy}
					onClick={() => (row.needsConfirm ? setAsking(true) : void install(false))}
				>
					{verb}
				</button>
			) : null}
		</Row>
	);
};

/** The ACP registry on a device, to search and install from. */
export const DeviceRegistry = ({
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

	const [query, setQuery] = useState("");
	const registry = registryOf(view, device);
	if (!offersToolInstalls(view, device)) return null;
	const rows = registry ? registryRows(registry, query) : [];
	return (
		<Section
			title={t("ACP registry")}
			label={t("ACP registry for {{name}}", { name: name })}
			hint={t("Agents to install")}
		>
			<div className="flex flex-col gap-2 px-4 py-3">
				<input
					type="search"
					aria-label={t("Search the registry")}
					placeholder={t("Search by name or what it does")}
					className={FIELD}
					value={query}
					onChange={(event) => setQuery(event.target.value)}
				/>
				{registry?.problem ? (
					<p role="status" className="selectable text-xs text-status-error-text">
						{registry.problem}
					</p>
				) : null}
			</div>
			{!registry ? (
				<p className="px-4 pb-3 text-xs text-muted-foreground">
					{live
						? t("{{name}} hasn't fetched the registry yet.", { name: name })
						: t("Connect to {{name}} to see the registry.", { name: name })}
				</p>
			) : rows.length === 0 ? (
				<p className="px-4 pb-3 text-xs text-muted-foreground">
					{query.trim() ? t("No agent matches.") : t("The registry lists no agents.")}
				</p>
			) : (
				rows.map((row) => (
					<AgentRow key={row.agent.id} device={device} row={row} live={live} />
				))
			)}
		</Section>
	);
};
