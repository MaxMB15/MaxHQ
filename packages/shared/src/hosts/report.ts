// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's host report as the views show it: its items grouped by what
// they're about, each with how it stands and the fix to offer. A fix maxd
// can do itself is applied from the app. One that needs root becomes a
// line to run in Terminal, over SSH for a device the app reaches that way.

import { t } from "../i18n";

import type { DeviceId, HostFix, HostItem, HostReport, HostSource } from "../bindings";
import { shellQuote } from "../agent-tools/scope";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Whether a device's `maxd` checks its host against a spec. */
export const offersHostSpec = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.host_spec === true;

/** A device's host report, or none before its first check. */
export const hostReportOf = (view: ViewState, device: DeviceId): HostReport | undefined =>
	view.tables.host_report.get(device);

/** What each group of items is called, by the first part of their ids. */
const GROUP_LABEL: Readonly<Record<string, string>> = {
	get incus() {
		return t("Incus");
	},
	get caps() {
		return t("Limits");
	},
	get system() {
		return t("System");
	},
	get tools() {
		return t("Tools");
	},
};

/** How an item's fix shows. */
export type FixView =
	| { readonly kind: "none" }
	| { readonly kind: "now" }
	| { readonly kind: "terminal"; readonly line: string }
	| { readonly kind: "nix"; readonly change: string };

/** One item as a row shows it. */
export type HostItemView = {
	readonly id: string;
	readonly label: string;
	readonly ok: boolean;
	/** How it stands, such as "Missing" or "Off, should be on". */
	readonly status: string;
	readonly fix: FixView;
};

/** The items about one thing, problems first. */
export type HostGroup = {
	readonly id: string;
	readonly label: string;
	readonly items: readonly HostItemView[];
	/** How many items in it need something done. */
	readonly problems: number;
};

/** How an item stands, in words. */
export const itemStatus = (item: HostItem): string => {
	switch (item.state.state) {
		case "ok":
			return t("Ok");
		case "drift":
			return t("{{item_state_have}}, should be {{item_state_want}}", {
				item_state_have: item.state.have,
				item_state_want: item.state.want,
			});
		case "missing":
			return t("Missing");
		case "unknown":
			return item.state.why;
	}
};

/**
 * The line that runs a root command on `device` from a Mac terminal:
 * the command alone for the Mac itself, or through `ssh -t` along the
 * device's route, so sudo can ask for the password. A WSL distro's goes
 * through `wsl.exe` on its PC. None when there's no route to the device.
 */
export const terminalLine = (view: ViewState, device: DeviceId, command: string): string | null => {
	const route = linkForDevice(view, device)?.route;
	if (!route) return null;
	switch (route.type) {
		case "local":
			return command;
		case "ssh":
			return `ssh -t ${shellQuote(route.host)} ${shellQuote(command)}`;
		case "wsl": {
			const line = `wsl.exe -d ${route.distro} -- ${command}`;
			return route.via === null
				? line
				: `ssh -t ${shellQuote(route.via)} ${shellQuote(line)}`;
		}
	}
};

const fixView = (view: ViewState, device: DeviceId, fix: HostFix | null): FixView => {
	switch (fix?.type) {
		case undefined:
			return { kind: "none" };
		case "now":
			return { kind: "now" };
		case "root_command": {
			const line = terminalLine(view, device, fix.command);
			return line === null ? { kind: "none" } : { kind: "terminal", line };
		}
		case "nix_change":
			return { kind: "nix", change: fix.change };
	}
};

/** A report's items in groups, in the order the report first names each. */
export const hostGroups = (view: ViewState, report: HostReport): HostGroup[] => {
	const groups = new Map<string, HostItemView[]>();
	for (const item of report.items) {
		const group = item.id.split(".")[0] ?? item.id;
		const row: HostItemView = {
			id: item.id,
			label: item.label,
			ok: item.state.state === "ok",
			status: itemStatus(item),
			fix: fixView(view, report.device, item.fix),
		};
		groups.set(group, [...(groups.get(group) ?? []), row]);
	}
	return [...groups].map(([id, items]) => ({
		id,
		label: GROUP_LABEL[id] ?? id.charAt(0).toUpperCase() + id.slice(1),
		items: [...items.filter((i) => !i.ok), ...items.filter((i) => i.ok)],
		problems: items.filter((i) => !i.ok).length,
	}));
};

/** The ids of the items `maxd` can fix now, for Apply. */
export const fixableNow = (report: HostReport): string[] =>
	report.items.filter((i) => i.state.state !== "ok" && i.fix?.type === "now").map((i) => i.id);

/** How many of a report's items need something done. */
export const hostProblems = (report: HostReport): number =>
	report.items.filter((i) => i.state.state !== "ok").length;

/** Where the spec came from, in words. */
export const sourceText = (source: HostSource): string => {
	switch (source.type) {
		case "file":
			return t("From {{source_path}}", { source_path: source.path });
		case "nix":
			return t("From the NixOS module");
		case "defaults":
			return t("No spec file, so the defaults");
	}
};

/** What a row says about an item's fix, when it doesn't show the fix itself. */
export const FIX_TEXT: Readonly<Record<FixView["kind"], string>> = {
	none: "",
	get now() {
		return t("Apply fixes it");
	},
	get terminal() {
		return t("The root command below fixes it");
	},
	nix: "",
};

/** One root command and the items it fixes. */
export type RootPart = { readonly line: string; readonly item: string; readonly labels: string[] };

/** The distinct root commands in `groups`, each with the items it fixes. */
export const rootParts = (groups: readonly HostGroup[]): RootPart[] => {
	const parts = new Map<string, RootPart>();
	for (const item of groups.flatMap((g) => g.items)) {
		if (item.ok || item.fix.kind !== "terminal") continue;
		const part = parts.get(item.fix.line);
		if (part) part.labels.push(item.label);
		else parts.set(item.fix.line, { line: item.fix.line, item: item.id, labels: [item.label] });
	}
	return [...parts.values()];
};
