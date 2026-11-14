// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window's inbox notices: the one the sidebar shows under its list, with
// its Undo, and the failures beside it. The menus, the shortcuts and the
// notice itself all reach the same one through `noticesOf`.

import { useStore } from "zustand";

import { type Services, useServices } from "@/app/services";
import {
	createInboxNotices,
	type InboxNotices,
	type InboxNoticeState,
} from "@maxhq/shared/ui/inbox-notice";

const byServices = new WeakMap<Services, InboxNotices>();

/** The notices for one window. */
export const noticesOf = (services: Services): InboxNotices => {
	let notices = byServices.get(services);
	if (!notices) {
		notices = createInboxNotices(services.source, services.view);
		byServices.set(services, notices);
	}
	return notices;
};

/** Reads the window's notice and failures. */
export const useNotices = <T>(selector: (state: InboxNoticeState) => T): T =>
	useStore(noticesOf(useServices()).store, selector);
