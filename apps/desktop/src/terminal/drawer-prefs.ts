// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Each agent's terminal drawer in this window: whether it shows and how
// tall, which this Mac keeps in localStorage, and its active tab, the tab
// split beside it and the names you gave tabs, which last as long as the
// window. localStorage can throw or hold anything, so every read checks
// what it got and falls back to the defaults.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import type { AgentId, TerminalId } from "@maxhq/shared/bindings";

/** The storage key. */
export const DRAWERS_KEY = "maxhq.terminal-drawer";

// Bump when the saved shape changes, so old settings read as the defaults.
const VERSION = 1;

/** How many agents' drawers it keeps, the ones you used last. */
export const MAX_SAVED = 200;

/** The lowest the drawer's edge drags to. */
export const MIN_HEIGHT = 120;
/** The tallest the drawer can be saved at. The pane's own height holds it lower. */
export const MAX_HEIGHT = 1600;
/** The drawer's height until you drag it. */
export const DEFAULT_HEIGHT = 260;

/** What this Mac keeps of one agent's drawer. */
export type DrawerPrefs = { readonly open: boolean; readonly height: number };

export const DEFAULT_DRAWER: DrawerPrefs = { open: false, height: DEFAULT_HEIGHT };

/** The height held between the lowest and the tallest, in whole pixels. */
export const clampHeight = (height: number): number =>
	Math.round(Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, height)));

/** One window's drawers. */
export type DrawersState = {
	/** What this Mac keeps, by agent, the one used last at the end. */
	readonly saved: Readonly<Record<AgentId, DrawerPrefs>>;
	/** Each drawer's active tab. */
	readonly active: Readonly<Record<AgentId, TerminalId>>;
	/** The tab each drawer shows beside its active one. */
	readonly split: Readonly<Record<AgentId, TerminalId>>;
	/** The names you gave tabs in this window, which say nothing of what runs. */
	readonly named: Readonly<Record<TerminalId, string>>;
	/** The agent whose drawer takes focus once its active tab shows. */
	readonly focusing: AgentId | null;
};

const isObject = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** Reads saved drawers, keeping each one that checks out. */
export const readDrawers = (raw: string | null): Record<AgentId, DrawerPrefs> => {
	if (raw === null) return {};
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return {};
	}
	if (!isObject(saved) || saved.version !== VERSION || !isObject(saved.drawers)) return {};
	const drawers: Record<AgentId, DrawerPrefs> = {};
	for (const [agent, drawer] of Object.entries(saved.drawers).slice(-MAX_SAVED)) {
		if (!isObject(drawer)) continue;
		const { open, height } = drawer;
		drawers[agent] = {
			open: typeof open === "boolean" ? open : DEFAULT_DRAWER.open,
			height:
				typeof height === "number" && Number.isFinite(height)
					? clampHeight(height)
					: DEFAULT_DRAWER.height,
		};
	}
	return drawers;
};

const load = (): Record<AgentId, DrawerPrefs> => {
	try {
		return readDrawers(localStorage.getItem(DRAWERS_KEY));
	} catch {
		return {};
	}
};

const save = (drawers: Readonly<Record<AgentId, DrawerPrefs>>): void => {
	try {
		localStorage.setItem(DRAWERS_KEY, JSON.stringify({ version: VERSION, drawers }));
	} catch (error) {
		console.warn("Couldn't save the terminal drawers", error);
	}
};

const stores = new WeakMap<Services, StoreApi<DrawersState>>();

/** The window's drawers, read from storage the first time. */
export const drawersOf = (services: Services): StoreApi<DrawersState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<DrawersState>(() => ({
			saved: load(),
			active: {},
			split: {},
			named: {},
			focusing: null,
		}));
		stores.set(services, store);
	}
	return store;
};

/** What this Mac keeps of the agent's drawer. */
export const drawerOf = (state: DrawersState, agent: AgentId): DrawerPrefs =>
	state.saved[agent] ?? DEFAULT_DRAWER;

/** Changes the agent's drawer and saves the lot, this agent's last. */
export const setDrawer = (
	services: Services,
	agent: AgentId,
	change: Partial<DrawerPrefs>
): void => {
	const store = drawersOf(services);
	const { [agent]: before = DEFAULT_DRAWER, ...rest } = store.getState().saved;
	const kept = Object.entries(rest).slice(-(MAX_SAVED - 1));
	const height = change.height === undefined ? before.height : clampHeight(change.height);
	const saved = { ...Object.fromEntries(kept), [agent]: { ...before, ...change, height } };
	store.setState({ saved });
	save(saved);
};

/** Picks a drawer's active tab, and takes it from beside it when it was there. */
export const setActive = (services: Services, agent: AgentId, terminal: TerminalId): void => {
	const store = drawersOf(services);
	const { active, split } = store.getState();
	const before = active[agent];
	if (before === terminal) return;
	// The two split tabs swap, so both stay in sight.
	const beside =
		split[agent] === terminal && before !== undefined ? { ...split, [agent]: before } : split;
	store.setState({ active: { ...active, [agent]: terminal }, split: beside });
};

/** A copy of `record` with `key` set to `value`, or without it for null. */
const withKey = (record: Readonly<Record<string, string>>, key: string, value: string | null) => {
	const next = { ...record };
	if (value === null) delete next[key];
	else next[key] = value;
	return next;
};

/** Shows `terminal` beside the active tab, or one tab alone when null. */
export const setSplit = (services: Services, agent: AgentId, terminal: TerminalId | null): void => {
	const store = drawersOf(services);
	store.setState({ split: withKey(store.getState().split, agent, terminal) });
};

/** Notes the name you gave a tab, or that you cleared it. */
export const setNamed = (services: Services, terminal: TerminalId, name: string): void => {
	const store = drawersOf(services);
	store.setState({ named: withKey(store.getState().named, terminal, name === "" ? null : name) });
};

/** Reads the window's drawers. */
export const useDrawers = <T>(selector: (state: DrawersState) => T): T =>
	useStore(drawersOf(useServices()), selector);
