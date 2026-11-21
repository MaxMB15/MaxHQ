// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The facts at the top of a device's tab in Settings: how this client
// reaches it, the hosts on the way, the `maxd` it runs and what that
// speaks, its system and its ID. Each comes from the view, so the card
// shows only what the device has said, and leaves out what it hasn't.

import type { DeviceId } from "../bindings";
import { t } from "../i18n";
import { OS_NAME } from "../status/device-page";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** One fact: what it is and its value, which you can select and copy. */
export type ConnectionFact = { readonly label: string; readonly value: string };

/** What the view knows about how `device` is reached and what it runs, in order. */
export const connectionFacts = (view: ViewState, device: DeviceId): readonly ConnectionFact[] => {
	const facts: ConnectionFact[] = [];
	const add = (label: string, value: string | null | undefined) => {
		if (value) facts.push({ label, value });
	};
	const link = linkForDevice(view, device);
	const record = view.tables.device.get(device);
	if (link) {
		add(t("Reached by"), link.route.type === "ssh" ? `ssh ${link.route.host}` : t("This Mac"));
		add(t("Through"), link.via.join(", "));
	}
	add(t("maxd"), record?.maxd_version);
	add(t("Protocol"), link?.protocol == null ? null : String(link.protocol));
	add(t("Build ID"), record?.build);
	add(t("System"), record ? (OS_NAME[record.os] ?? record.os) : null);
	add(t("Built for"), record?.target);
	add(t("Device ID"), device);
	return facts;
};
