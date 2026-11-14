// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox's notice, with its Undo, shared by every row's swipe and the
// strip that shows it, as each desktop window keeps one.

import { useStore } from "zustand";

import {
	createInboxNotices,
	type InboxNotices,
	type InboxNoticeState,
} from "@maxhq/shared/ui/inbox-notice";

import { type Services, useServices } from "../../services";

const byServices = new WeakMap<Services, InboxNotices>();

/** The app's notices. */
export const noticesOf = (services: Services): InboxNotices => {
	let notices = byServices.get(services);
	if (!notices) {
		notices = createInboxNotices(services.source, services.view);
		byServices.set(services, notices);
	}
	return notices;
};

/** Reads the notice and failures. */
export const useNotices = <T>(selector: (state: InboxNoticeState) => T): T =>
	useStore(noticesOf(useServices()).store, selector);
