// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The app's renames, shared by the agent screen's title and its rename
// dialog, so a new name shows at once.

import { useStore } from "zustand";

import type { Agent } from "@maxhq/shared/bindings";
import { createRenames, type Renames, shownTitle } from "@maxhq/shared/ui/rename-agent";

import { type Services, useServices } from "../../services";

const byServices = new WeakMap<Services, Renames>();

/** The app's renames. */
export const renamesOf = (services: Services): Renames => {
	let renames = byServices.get(services);
	if (!renames) {
		renames = createRenames(services.source, services.view);
		byServices.set(services, renames);
	}
	return renames;
};

/** The agent's title, or the name you just gave it while maxd stores it. */
export const useShownTitle = (agent: Agent): string =>
	useStore(renamesOf(useServices()).store, (state) => shownTitle(state, agent));
