// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A run shown in run mode moves to its lead's Workflow tab once an agent
// binds it.

import type { Unsubscribe } from "../data/source";
import type { UiStore } from "../ui/store";
import type { ViewStore } from "../view/store";
import { leadOf } from "./select";

/** Hands a run in run mode over to its lead as soon as the view names one. */
export const watchHandOver = (view: ViewStore, ui: UiStore): Unsubscribe => {
	const check = () => {
		const shown = ui.getState().slideOut;
		if (!shown || shown.lead !== null || !shown.run) return;
		const lead = leadOf(view.getState().view, shown.run);
		if (lead) ui.getState().handOver(lead);
	};
	check();
	const stopView = view.subscribe(check);
	const stopUi = ui.subscribe(check);
	return () => {
		stopView();
		stopUi();
	};
};
