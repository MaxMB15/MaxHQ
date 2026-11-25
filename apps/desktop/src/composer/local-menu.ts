// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the model picker shows for local models on an agent's device: each
// local account's server as its group, with its credits when it has them,
// a fit tag beside each model, and the way to the device's Local models
// settings.

import type { Services } from "@/app/services";
import { openSettings } from "@/settings/store";
import type { DeviceId } from "@maxhq/shared/bindings";
import { localModelTag, serverHeading } from "@maxhq/shared/local-models/picker";
import { serverOf } from "@maxhq/shared/local-models/select";
import { kindName } from "@maxhq/shared/status/agent-kind";
import type { ViewState } from "@maxhq/shared/view/replica";

import type { LocalMenu } from "./pickers";

/** The model picker's local models for an agent on `device`, or none without one. */
export const localMenu = (
	services: Services,
	view: ViewState,
	device: DeviceId | undefined
): LocalMenu | undefined =>
	device === undefined
		? undefined
		: {
				group: (account) => {
					const server = serverOf(view, account);
					return server ? serverHeading(view, server) : kindName("local");
				},
				tag: (account, model) => localModelTag(view, account, model),
				onSetup: () => openSettings(services, { kind: "local-models", device }),
			};
