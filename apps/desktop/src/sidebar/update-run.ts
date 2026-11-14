// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: one run per window,
// closed once every device is done, a retry that starts the failed devices'
// clocks again and updates only them, and a dismiss that keeps the devices
// still going. MaxHQ's app does the updating and sends each step, so the run
// only follows what it hears.

import { t } from "@maxhq/shared/i18n";

import { createStore, type StoreApi } from "zustand/vanilla";

import type { Services } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import {
	applyProgress,
	closeFinishedRun,
	dismissSettled,
	failedDevices,
	type FleetRun,
	isActive,
	restartDevices,
	startRun,
	type UpdatePlan,
} from "@maxhq/shared/fleet/update-view";

/** The window's Update everything run, or null when none is showing. */
export type UpdateRunState = { readonly run: FleetRun | null };

const stores = new WeakMap<Services, StoreApi<UpdateRunState>>();

/** The Update everything run's store for one window. */
export const updateRunOf = (services: Services): StoreApi<UpdateRunState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<UpdateRunState>(() => ({ run: null }));
		stores.set(services, store);
	}
	return store;
};

const edit = (services: Services, change: (run: FleetRun) => FleetRun | null) =>
	updateRunOf(services).setState((s) => ({ run: s.run && change(s.run) }));

/** The devices still going, failed with this reason. */
const failRest =
	(devices: readonly DeviceId[], reason: string) =>
	(run: FleetRun): FleetRun => ({
		...run,
		devices: run.devices.map((d) =>
			devices.includes(d.device) && isActive(d.status)
				? { ...d, status: { status: "failed", reason } }
				: d
		),
	});

const follow = async (services: Services, devices: DeviceId[], tools: boolean) => {
	try {
		const result = await services.source.updateFleet(devices, tools, (progress) =>
			edit(services, (run) => closeFinishedRun(applyProgress(run, progress, Date.now())))
		);
		if (result.status === "error") {
			const reason =
				result.error.kind === "failed" ? result.error.message : t("The update stopped");
			edit(services, failRest(devices, reason));
		}
	} catch (error) {
		console.warn("Couldn't update the fleet", error);
		edit(services, failRest(devices, t("Couldn't reach the app")));
	}
	// A device the app never answered for didn't get updated.
	edit(services, (run) => closeFinishedRun(failRest(devices, t("No answer from the app"))(run)));
};

/** Starts a run over the plan's devices. Does nothing while one is going. */
export const startUpdate = (services: Services, plan: UpdatePlan, now: number): void => {
	const store = updateRunOf(services);
	if (store.getState().run?.devices.some((d) => isActive(d.status))) return;
	if (plan.rows.length === 0) return;
	store.setState({ run: startRun(plan.rows, plan.tools, now) });
	void follow(
		services,
		plan.rows.map((r) => r.device),
		plan.tools
	);
};

/** Updates the failed devices again. */
export const retryFailed = (services: Services, now: number): void => {
	const run = updateRunOf(services).getState().run;
	if (!run) return;
	const failed = failedDevices(run);
	if (failed.length === 0) return;
	edit(services, (r) => restartDevices(r, failed, now));
	void follow(services, failed, run.tools);
};

/** Drops the devices that finished, failed or were skipped. */
export const dismissRun = (services: Services): void =>
	edit(services, (run) => dismissSettled(run));
