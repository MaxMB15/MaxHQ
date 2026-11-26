// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The accounts table in Settings: a row for Claude, Codex, Cursor,
// OpenRouter and GitHub, a column for each linked device, and in each cell
// whether that device is signed in, from what the view holds. A device that
// reports its CLIs and has none for the vendor reads Not installed. The row
// offers to sign in on the first device that has the CLI and no login.
// OpenRouter has no CLI: a device with an OpenRouter server that holds a
// key is signed in, with the key's credits beside it, and one without such
// a server reads Not set up. Its row offers to sign in only while no device
// is, since most devices never need a key of their own.

import { t } from "../i18n";

import type { DeviceId, Vendor } from "../bindings";
import { creditsShort } from "../local-models/credits";
import { openRouterServerOf } from "../local-models/openrouter";
import { runsLocalModels } from "../local-models/select";
import { VENDOR_NAME } from "../status/agent-kind";
import { accountName } from "../status/usage";
import { all, type ViewState } from "../view/replica";

import { loggedOut } from "./cli-sign-in";
import type { LoginView } from "../guide/settings-link";

/** A device the table has a column for. */
export type AccountDevice = { readonly device: DeviceId; readonly name: string };

/** What a cell says. */
export type AccountCell = "signed_in" | "signed_out" | "not_installed" | "not_set_up";

/** One row of the table. */
export type AccountRow = {
	/** The login view its Manage button opens. */
	readonly login: LoginView;
	readonly name: string;
	/** The plan a signed-in account reports, such as "Max". */
	readonly plan: string | null;
	/** One cell for each of the table's devices, in their order. */
	readonly cells: readonly AccountCell[];
	/**
	 * What follows each cell, in the same order, such as "$7.42 left" for
	 * an OpenRouter key's credits, or null.
	 */
	readonly notes: readonly (string | null)[];
	/** The device to sign in on, when one has the CLI and no login. */
	readonly signIn: AccountDevice | null;
};

/** The table's devices: each linked one once, this Mac first, then by name. */
export const accountDevices = (view: ViewState): AccountDevice[] => {
	const seen = new Map<DeviceId, AccountDevice & { readonly local: boolean }>();
	for (const link of all(view, "device_link")) {
		if (link.device === null || link.claim) continue;
		const local = link.route.type === "local";
		const held = seen.get(link.device);
		if (!held || (local && !held.local))
			seen.set(link.device, { device: link.device, name: link.name, local });
	}
	return [...seen.values()]
		.sort((a, b) => Number(b.local) - Number(a.local) || a.name.localeCompare(b.name))
		.map(({ device, name }) => ({ device, name }));
};

const VENDORS: readonly (Vendor & LoginView)[] = ["claude", "codex", "cursor"];

/**
 * Whether `vendor`'s CLI is on a device: true or false once the device
 * reports its CLIs, and undefined before.
 */
export const installed = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor
): boolean | undefined => {
	const tools = view.tables.device_tools.get(device)?.tools;
	if (!tools) return undefined;
	return tools.some(
		(tool) =>
			tool.kind.type === "agent" &&
			tool.kind.tool === vendor &&
			(tool.path !== null || tool.version !== null)
	);
};

const vendorCell = (view: ViewState, device: DeviceId, vendor: Vendor): AccountCell => {
	const signed = all(view, "account").some(
		(a) =>
			a.device === device &&
			a.vendor === vendor &&
			a.login !== "model_server" &&
			a.login !== "acp_command" &&
			!loggedOut(a)
	);
	if (signed) return "signed_in";
	return installed(view, device, vendor) === false ? "not_installed" : "signed_out";
};

const githubCell = (view: ViewState, device: DeviceId): AccountCell =>
	all(view, "credential").some(
		(c) =>
			c.device === device && (c.kind.type === "github_user" || c.kind.type === "github_app")
	)
		? "signed_in"
		: "signed_out";

/** What a cell says, with its note after it, such as "Signed in · $7.42 left". */
export const accountCellText = (cell: AccountCell, note: string | null = null): string => {
	const text = {
		signed_in: t("Signed in"),
		signed_out: t("Not signed in"),
		not_installed: t("Not installed"),
		not_set_up: t("Not set up"),
	}[cell];
	return note ? `${text} · ${note}` : text;
};

/**
 * A device's OpenRouter cell: signed in with its server's credits, such as
 * "$7.42 left", while the server holds a key; not signed in for a server
 * without one; and not set up without a server.
 */
export const openRouterCell = (
	view: ViewState,
	device: DeviceId
): { readonly cell: AccountCell; readonly note: string | null } => {
	const server = openRouterServerOf(view, device);
	if (!server) return { cell: "not_set_up", note: null };
	if (server.token !== true) return { cell: "signed_out", note: null };
	return { cell: "signed_in", note: creditsShort(server) };
};

/**
 * The OpenRouter row. It offers to sign in on a device whose server lost
 * its key, else, while no device is signed in, on the first that runs
 * models; otherwise its button manages.
 */
const openRouterRow = (view: ViewState, devices: readonly AccountDevice[]): AccountRow => {
	const shown = devices.map((d) => openRouterCell(view, d.device));
	const cells = shown.map((s) => s.cell);
	const signIn =
		devices[cells.indexOf("signed_out")] ??
		(cells.includes("signed_in")
			? undefined
			: devices.find((d) => runsLocalModels(view, d.device)));
	return {
		login: "openrouter",
		name: "OpenRouter",
		plan: null,
		cells,
		notes: shown.map((s) => s.note),
		signIn: signIn ?? null,
	};
};

// The plan of the vendor's first signed-in account that reports one.
const planOf = (view: ViewState, vendor: Vendor): string | null => {
	const account = all(view, "account")
		.filter((a) => a.vendor === vendor && a.plan?.trim() && !loggedOut(a))
		.sort((a, b) => a.id.localeCompare(b.id))[0];
	if (!account) return null;
	const name = accountName(account);
	const prefix = `${VENDOR_NAME[vendor]} `;
	return name.startsWith(prefix) ? name.slice(prefix.length) : null;
};

/** The table's rows for `devices`, in the order Settings shows them. */
export const accountRows = (view: ViewState, devices: readonly AccountDevice[]): AccountRow[] => {
	const row = (
		login: LoginView,
		name: string,
		plan: string | null,
		cells: AccountCell[]
	): AccountRow => {
		const at = cells.indexOf("signed_out");
		return {
			login,
			name,
			plan,
			cells,
			notes: cells.map(() => null),
			signIn: at >= 0 ? devices[at]! : null,
		};
	};
	return [
		...VENDORS.map((vendor) =>
			row(
				vendor,
				VENDOR_NAME[vendor],
				planOf(view, vendor),
				devices.map((d) => vendorCell(view, d.device, vendor))
			)
		),
		openRouterRow(view, devices),
		// GitHub has no CLI to install, and one sign-in covers every device
		// that holds it, so its row only manages.
		{
			...row(
				"github",
				"GitHub",
				null,
				devices.map((d) => githubCell(view, d.device))
			),
			signIn: null,
		},
	];
};
