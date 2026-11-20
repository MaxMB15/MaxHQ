// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a device's `maxd` lists folders, and how a path field's Browse…
// reaches that device's folders.

import { type DeviceId, FOLDERS_SINCE } from "../bindings";
import { t } from "../i18n";
import { isThisMac } from "../settings/app-keys";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/**
 * Whether the device's `maxd` lists and makes folders: it speaks a
 * protocol that has them. A device with no link, or one whose version
 * isn't known yet, doesn't.
 */
export const supportsFolders = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= FOLDERS_SINCE;
};

/**
 * How Browse… picks a path on `device`: the system picker on this Mac, in
 * the app; MaxHQ's own browser on a device that lists folders; or not at
 * all, with why.
 */
export type BrowseWay =
	| { readonly way: "native" }
	| { readonly way: "browser" }
	| { readonly way: "none"; readonly why: string };

/** How Browse… picks a path on `device`. `native` says the app has the system picker. */
export const browseWay = (view: ViewState, device: DeviceId, native: boolean): BrowseWay => {
	if (native && isThisMac(view, device)) return { way: "native" };
	if (supportsFolders(view, device)) return { way: "browser" };
	return {
		way: "none",
		why: t("Update maxd on {{device}} to browse its folders", {
			device: deviceName(view, device),
		}),
	};
};
