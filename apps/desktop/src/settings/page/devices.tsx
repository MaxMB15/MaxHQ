// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Devices and phones group: a tab for each linked device, this Mac's
// first, and one for phones. Only the tab you're on renders. A search
// shows only the tabs with a match, on the one you picked when it has one.

import { Tabs } from "@base-ui/react/tabs";

import { useServices, useViewOf } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import type { EntityKind } from "@maxhq/shared/view/replica";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { accountDevices } from "@maxhq/shared/settings/accounts";
import { LOGIN_HINT } from "@maxhq/shared/settings/login-steps";

import { ApnsLogin } from "../apns-login";
import { CardScope, SEG, SEG_ITEM } from "../controls";
import { DeviceSettings } from "../device-settings";
import { LOGIN_STEPS } from "../login-steps";
import { PhonesSettingsPage } from "../phones-settings";
import { deviceCardId, settingsOf, useSettings } from "../store";
import { type Results, searchTab } from "./find";
import { Subheading } from "./subheading";

const KINDS: readonly EntityKind[] = ["device_link"];

/** The tabs and the one that shows. */
export const DevicesGroup = ({ results }: { results: Results | null }) => {
	useI18n();
	const services = useServices();
	const view = useViewOf(KINDS);
	const picked = useSettings((s) => s.tab);
	const all = accountDevices(view);
	// A device that's gone, or none picked, shows this Mac, listed first.
	const chosen: DeviceId | "phones" =
		picked === "phones" || all.some((d) => d.device === picked)
			? (picked as DeviceId | "phones")
			: (all[0]?.device ?? "phones");
	const tab = results ? searchTab(chosen, results) : chosen;
	if (tab === null) return null;
	const devices = results ? all.filter((d) => results.devices.has(d.device)) : all;
	const phones = !results || results.phones.size > 0;
	const shown = all.find((d) => d.device === tab);

	return (
		<Tabs.Root
			value={tab}
			onValueChange={(next: DeviceId | "phones") =>
				settingsOf(services).setState({ tab: next })
			}
			className="flex flex-col gap-3"
		>
			<Tabs.List aria-label={translate("Device")} className={SEG}>
				{devices.map((device) => (
					<Tabs.Tab key={device.device} value={device.device} className={SEG_ITEM}>
						{device.name}
					</Tabs.Tab>
				))}
				{phones ? (
					<Tabs.Tab value="phones" className={SEG_ITEM}>
						{translate("Phones")}
					</Tabs.Tab>
				) : null}
			</Tabs.List>
			<Tabs.Panel value={tab} className="flex flex-col gap-3 outline-none">
				{shown ? (
					<DeviceSettings
						key={shown.device}
						device={shown.device}
						name={shown.name}
						only={results?.devices.get(shown.device) ?? null}
					/>
				) : (
					<PhonesTab results={results} />
				)}
			</Tabs.Panel>
		</Tabs.Root>
	);
};

// Today's Phones page, then the Apple key the devices push to phones with.
const PhonesTab = ({ results }: { results: Results | null }) => {
	useI18n();
	const shows = (card: "phones" | "apns") => !results || results.phones.has(card);
	return (
		<CardScope.Provider value={translate("Every device")}>
			{shows("phones") ? (
				<div id={deviceCardId("phones")} className="scroll-mt-[60px]">
					<PhonesSettingsPage />
				</div>
			) : null}
			{shows("apns") ? (
				<div id={deviceCardId("apns")} className="flex scroll-mt-[60px] flex-col gap-3">
					<Subheading title={LOGIN_STEPS.apns.title} hint={LOGIN_HINT.apns} />
					<ApnsLogin />
				</div>
			) : null}
		</CardScope.Provider>
	);
};
