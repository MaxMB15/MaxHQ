// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The accounts table: a row for each login agents use, a column for each
// linked device that says whether it's signed in there, and a button that
// opens the login's own view, with the first device to sign in on filled
// in when one isn't. OpenRouter's cells give its key's credits after
// Signed in, from the device's OpenRouter server.

import { useServices, useViewOf } from "@/app/services";
import { cn } from "@/lib/utils";
import type { EntityKind } from "@maxhq/shared/view/replica";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	type AccountCell,
	accountCellText,
	accountDevices,
	accountRows,
} from "@maxhq/shared/settings/accounts";

import { SECONDARY, Section } from "../controls";
import { showPlace } from "../store";

const KINDS: readonly EntityKind[] = [
	"device_link",
	"device_tools",
	"account",
	"credential",
	"model_server",
];

/** The table's labels, for the settings search. */
export const ACCOUNTS_TERMS: readonly string[] = [
	"Claude",
	"Codex",
	"Cursor",
	"OpenRouter",
	"GitHub",
	"Signed in",
	"Not signed in",
	"Not installed",
	"Not set up",
	"Manage",
];

const CELL: Record<AccountCell, string> = {
	signed_in: "text-status-ready-text",
	signed_out: "text-muted-foreground",
	not_installed: "text-subtle-foreground",
	not_set_up: "text-subtle-foreground",
};

/** The Logins card with its table. */
export const AccountsCard = () => {
	useI18n();
	const services = useServices();
	const view = useViewOf(KINDS);
	const devices = accountDevices(view);
	const rows = accountRows(view, devices);
	return (
		<Section title={translate("Logins")}>
			<div className="overflow-x-auto">
				<table className="w-full border-collapse text-left text-[12.5px]">
					<thead>
						<tr className="border-b border-sidebar-border text-[11.5px] text-muted-foreground">
							<th scope="col" className="px-4 py-1.5 font-[550]">
								{translate("Account")}
							</th>
							{devices.map((device) => (
								<th
									key={device.device}
									scope="col"
									className="px-4 py-1.5 font-[550]"
								>
									{device.name}
								</th>
							))}
							<th scope="col" className="px-4 py-1.5">
								<span className="sr-only">{translate("Actions")}</span>
							</th>
						</tr>
					</thead>
					<tbody>
						{rows.map((row) => (
							<tr
								key={row.login}
								className="border-t border-sidebar-border first:border-t-0"
							>
								<th
									scope="row"
									className="px-4 py-2 font-[600] whitespace-nowrap text-foreground"
								>
									{row.name}
									{row.plan ? (
										<span className="ml-1.5 font-normal text-muted-foreground">
											{row.plan}
										</span>
									) : null}
								</th>
								{row.cells.map((cell, at) => (
									<td
										key={devices[at]!.device}
										className={cn("px-4 py-2 whitespace-nowrap", CELL[cell])}
									>
										{accountCellText(cell)}
										{row.notes[at] ? (
											<span className="text-muted-foreground">
												{` · ${row.notes[at]}`}
											</span>
										) : null}
									</td>
								))}
								<td className="px-4 py-2 text-right">
									<button
										type="button"
										className={cn(SECONDARY, "h-[26px] px-2.5 text-[12px]")}
										onClick={() =>
											showPlace(services, {
												kind: "login",
												login: row.login,
												...(row.signIn
													? { device: row.signIn.device }
													: {}),
											})
										}
									>
										{row.signIn
											? translate("Sign in on {{name}}", {
													name: row.signIn.name,
												})
											: translate("Manage")}
									</button>
								</td>
							</tr>
						))}
					</tbody>
				</table>
			</div>
		</Section>
	);
};
