// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's copy of the ACP registry, as a list to search and install
// from. A binary the registry gives no checksum for installs once you
// confirm it for that agent and version.

import { t } from "../i18n";

import type { AcpRegistry, DeviceId, RegistryAgent } from "../bindings";
import type { ViewState } from "../view/replica";

/** A device's registry, or none before its first fetch. */
export const registryOf = (view: ViewState, device: DeviceId): AcpRegistry | undefined =>
	view.tables.acp_registry.get(device);

/** What a registry agent's row shows. */
export type RegistryRow = {
	readonly agent: RegistryAgent;
	/** Whether this device can install it at all. */
	readonly installable: boolean;
	/** Whether its install asks you to confirm a binary with no checksum. */
	readonly needsConfirm: boolean;
	/** Whether a newer version than the one installed is listed. */
	readonly update: boolean;
	/** Such as "Installed 1.8.2, 1.9.0 is out", or why it can't install. */
	readonly status: string;
};

const HOW: Readonly<Record<string, string>> = {
	get binary() {
		return t("Binary");
	},
	get npx() {
		return t("npm package");
	},
	get uvx() {
		return t("Python package");
	},
};

/** One agent's row. */
export const registryRow = (agent: RegistryAgent): RegistryRow => {
	const install = agent.install;
	const update = agent.installed !== null && agent.installed !== agent.version;
	const status = !install
		? t("Not available for this device")
		: agent.installed === null
			? `${HOW[install.type]}, ${agent.version}`
			: update
				? t("Installed {{agent_installed}}, {{agent_version}} is out", {
						agent_installed: agent.installed,
						agent_version: agent.version,
					})
				: t("Installed {{agent_installed}}", { agent_installed: agent.installed });
	return {
		agent,
		installable: install !== null,
		needsConfirm: install?.type === "binary" && !install.checked,
		update,
		status,
	};
};

/** The rows whose name, id or description holds `query`, installed ones first. */
export const registryRows = (registry: AcpRegistry, query = ""): RegistryRow[] => {
	const q = query.trim().toLowerCase();
	const found = registry.agents.filter(
		(a) =>
			q === "" ||
			a.name.toLowerCase().includes(q) ||
			a.id.toLowerCase().includes(q) ||
			(a.description ?? "").toLowerCase().includes(q)
	);
	const rows = found.map(registryRow);
	return [...rows.filter((r) => r.agent.installed), ...rows.filter((r) => !r.agent.installed)];
};

/** The registry's agent names by id, for the tool rows. */
export const registryNames = (registry: AcpRegistry | undefined): Map<string, string> =>
	new Map((registry?.agents ?? []).map((a) => [a.id, a.name]));

/** The confirm a binary with no checksum asks for. */
export const uncheckedWarning = (agent: RegistryAgent): string =>
	t(
		"The registry gives no checksum for {{agent_name}} {{agent_version}}, so MaxHQ can't check the download. Install it anyway?",
		{ agent_name: agent.name, agent_version: agent.version }
	);
