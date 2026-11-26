// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// OpenRouter in Accounts: a pay-as-you-go key for cloud models, one for
// each device that runs agents on them. The page shows one device, picked
// at its top. Without an OpenRouter server there, it has OpenRouter's form
// from the Local models page: Sign in with OpenRouter with a spending
// limit, or a pasted key. With one, it has that server's row from the
// same page, with its credits, admin key, Sign in again, Test and Remove,
// then its models and a link to them in Local models. The key never
// reaches this Mac's app: the device's maxd keeps it.

import { useServices, useView } from "@/app/services";
import { Picker } from "@/new-agent/picker";
import type { DeviceId } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { openRouterServerOf } from "@maxhq/shared/local-models/openrouter";
import { localDeviceOptions, openRouterPageDevice } from "@maxhq/shared/local-models/settings-page";
import { accountCellText, openRouterCell } from "@maxhq/shared/settings/accounts";
import { settingsWhy } from "@maxhq/shared/settings/device-settings";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";

import { Row, Section } from "./controls";
import { ExternalRow, OpenRouterForm } from "./local-models-servers";
import { Steps } from "./login-steps";
import { showPlace } from "./store";

/** The page's headings and labels, for the settings search. */
export const OPENROUTER_LOGIN_TERMS: readonly string[] = [
	"Device",
	"Sign in with OpenRouter",
	"Spending limit",
	"API key",
	"Get a key",
	"Sign in again",
	"Admin key",
	"Models in Local models",
	"Not set up",
];

/** One device's OpenRouter: its server and what it holds, or the form that sets one up. */
const DeviceBody = ({ device, view }: { device: DeviceId; view: ViewState }) => {
	useI18n();
	const services = useServices();
	const name = deviceName(view, device);
	const live = linkForDevice(view, device)?.state.state === "connected";
	const why = settingsWhy(view, device, name, live).local;
	const server = openRouterServerOf(view, device);
	const { cell, note } = openRouterCell(view, device);
	return (
		<Section
			title={translate("Sign in")}
			hint={translate("Once on each device")}
			label={translate("OpenRouter on {{name}}", { name })}
		>
			<Row title={translate("Device")} sub={why ?? accountCellText(cell, note)}>
				<Picker
					label={translate("Device")}
					value={device}
					options={localDeviceOptions(view)}
					onChange={(next) =>
						showPlace(services, { kind: "login", login: "openrouter", device: next })
					}
				/>
			</Row>
			{server ? (
				<>
					<ExternalRow device={device} server={server} disabled={why} />
					<div className="flex items-center gap-3 border-t border-sidebar-border px-4 py-3 text-xs">
						<span className="text-muted-foreground">
							{translate("{{count, number}} model", { count: server.models.length })}
						</span>
						<button
							type="button"
							className="ml-auto text-primary hover:underline"
							onClick={() => showPlace(services, { kind: "local-models", device })}
						>
							{translate("Models in Local models")}
						</button>
					</div>
				</>
			) : why === null ? (
				<div className="flex flex-col gap-3 border-t border-sidebar-border px-4 py-3">
					<OpenRouterForm device={device} view={view} />
				</div>
			) : null}
		</Section>
	);
};

/**
 * The OpenRouter page. `device` is the one it opens on, such as the one the
 * Accounts table offers to sign in on; without it, the page opens on the
 * first device with an OpenRouter server.
 */
export const OpenRouterLogin = ({ device }: { device?: DeviceId }) => {
	useI18n();
	const view = useView((s) => s.view);
	const shown = openRouterPageDevice(view, device);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="openrouter" />
			{shown ? (
				<DeviceBody key={shown} device={shown} view={view} />
			) : (
				<p className="px-1 py-6 text-center text-[12.5px] text-muted-foreground">
					{translate("Link a device to sign in with OpenRouter on it.")}
				</p>
			)}
		</div>
	);
};
