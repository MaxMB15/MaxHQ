// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The skills each CLI sees in a place, as the place's device last scanned
// them. A scan isn't part of the view, so the window keeps the last answer
// for each place and asks again when a view mounts, after an apply or when
// you press Rescan. It lasts as long as the window does.

import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentPlace, DeviceId, EffectiveSkill } from "../bindings";
import type { DataSource } from "../data/source";
import { readOf } from "../pulls/read";

/** Where one scan stands. */
export type SkillScan =
	| { readonly state: "scanning"; readonly skills: readonly EffectiveSkill[] | null }
	| { readonly state: "done"; readonly skills: readonly EffectiveSkill[] }
	| {
			readonly state: "failed";
			readonly error: string;
			readonly skills: readonly EffectiveSkill[] | null;
	  };

type ScanState = { readonly scans: ReadonlyMap<string, SkillScan> };

const EMPTY: ScanState = { scans: new Map() };

const store = createStore<ScanState>(() => EMPTY);

/** The key a place's scan goes under. */
export const scanKey = (device: DeviceId, place: AgentPlace): string =>
	place.type === "workspace"
		? `${device}\u0000workspace\u0000${place.workspace}`
		: `${device}\u0000folder\u0000${place.path}`;

const put = (key: string, scan: SkillScan) =>
	store.setState((state) => ({ scans: new Map(state.scans).set(key, scan) }));

/**
 * Scans a place on `device` again, keeping the last answer on show while
 * it runs. A scan already running for the place is left to finish.
 */
export const rescanSkills = async (
	source: Pick<DataSource, "scanSkills">,
	device: DeviceId,
	place: AgentPlace
): Promise<void> => {
	const key = scanKey(device, place);
	const last = store.getState().scans.get(key);
	if (last?.state === "scanning") return;
	put(key, { state: "scanning", skills: last?.skills ?? null });
	try {
		const { skills } = readOf(await source.scanSkills(device, place), "skills");
		put(key, { state: "done", skills });
	} catch (error) {
		put(key, {
			state: "failed",
			error: error instanceof Error ? error.message : String(error),
			skills: last?.skills ?? null,
		});
	}
};

/** The last scan of a place, or none before the first. */
export const skillScanOf = (device: DeviceId, place: AgentPlace): SkillScan | undefined =>
	store.getState().scans.get(scanKey(device, place));

/**
 * A place's scan, asking for one when there's none yet. Pass null for
 * `place` while there's nothing to scan.
 */
export const useSkillScan = (
	source: Pick<DataSource, "scanSkills">,
	device: DeviceId | null,
	place: AgentPlace | null
): SkillScan | undefined => {
	const key = device && place ? scanKey(device, place) : null;
	const scan = useStore(store, (state) => (key ? state.scans.get(key) : undefined));
	useEffect(() => {
		// A new object for a place already scanned asks nothing.
		if (!device || !place || store.getState().scans.has(scanKey(device, place))) return;
		void rescanSkills(source, device, place);
	}, [source, device, place]);
	return scan;
};

/** Forgets every scan, for tests. */
export const clearSkillScans = () => store.setState(EMPTY, true);
