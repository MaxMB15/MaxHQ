// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Hiding and showing the sidebar. Hiding it while focus is inside moves
// focus to the header's button that brings it back, so
// focus never falls to the page.

import type { Services } from "@/app/services";

import { prefsOf, setPrefs } from "./prefs";

/** The sidebar's id. */
export const SIDEBAR_ID = "sidebar";

/** The id of the header's button that hides and shows the sidebar. */
export const TOGGLE_ID = "sidebar-toggle";

/** Hides the sidebar, or shows it again. */
export const toggleSidebar = (services: Services): void => {
	const open = !prefsOf(services).getState().open;
	if (!open && document.getElementById(SIDEBAR_ID)?.contains(document.activeElement)) {
		document.getElementById(TOGGLE_ID)?.focus();
	}
	setPrefs(services, { open });
};
