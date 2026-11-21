// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The accounts table in Settings: a row for Claude, Codex, Cursor and
// GitHub, a column for each linked device, and in each cell whether that
// device is signed in, from what the view holds. A device that reports its
// CLIs and has none for the vendor reads Not installed. The row offers to
// sign in on the first device that has the CLI and no login.

import type { DeviceId, Vendor } from "../bindings";
import { VENDOR_NAME } from "../status/agent-kind";
import { accountName } from "../status/usage";
import { all, type ViewState } from "../view/replica";

import { loggedOut } from "./cli-sign-in";
import type { LoginView } from "../guide/settings-link";

/** A device the table has a column for. */
export type AccountDevice = { readonly device: DeviceId; readonly name: string };

/** What a cell says. */
export type AccountCell = "signed_in" | "signed_out" | "not_installed";

/** One row of the table. */
export type AccountRow = {
	/** The login view its Manage button opens. */
	readonly login: LoginView;
	readonly name: string;
	/** The plan a signed-in account reports, such as "Max". */
	readonly plan: string | null;
	/** One cell for each of the table's devices, in their order. */
	readonly cells: readonly AccountCell[];
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

// The CLI's state on a device: true or false once the device reports its
// CLIs, and undefined before.
const installed = (view: ViewState, device: DeviceId, vendor: Vendor): boolean | undefined => {
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
	const row = (login: LoginView, name: string, plan: string | null, cells: AccountCell[]) => {
		const at = cells.indexOf("signed_out");
		return { login, name, plan, cells, signIn: at >= 0 ? devices[at]! : null };
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
