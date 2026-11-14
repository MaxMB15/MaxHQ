// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher bar at the foot of the home screen, without React Native:
// the setting that keeps it shown or folds it to a grabber, what it says,
// whether it takes a request, and where a drag leaves the sheet above it.

import { t } from "@maxhq/shared/i18n";

import type { AccountId, Agent, Plan } from "@maxhq/shared/bindings";
import { isWorking, WORKING_HINT } from "@maxhq/shared/dispatch/conversation";
import { type DispatchState, inUse, shownPlan, talkingTo } from "@maxhq/shared/dispatch/flow";
import type { ResultRow } from "@maxhq/shared/dispatch/plan";
import { type DispatcherReach, dispatcherReach } from "@maxhq/shared/dispatch/reach";
import type { ViewState } from "@maxhq/shared/view/replica";

/** Whether the bar always shows, or folds to a grabber while the dispatcher is idle. */
export type BarSetting = "always" | "in_use";

/** The settings in the order the phone offers them. */
export const BAR_SETTINGS: readonly { readonly setting: BarSetting; readonly label: string }[] = [
	{ setting: "always", label: "Always show" },
	{ setting: "in_use", label: "Only when in use" },
];

export const barSettingLabel = (setting: BarSetting): string =>
	t(BAR_SETTINGS.find((s) => s.setting === setting)?.label ?? "");

/** What the phone keeps for the dispatcher. */
export type DispatcherPrefs = {
	readonly bar: BarSetting;
	/** The account picked to run as, or null for the Mac's default. */
	readonly account: AccountId | null;
};

export const DEFAULT_PREFS: DispatcherPrefs = { bar: "always", account: null };

/** The prefs kept as `text`, with the defaults for what's missing or unreadable. */
export const readPrefs = (text: string | null): DispatcherPrefs => {
	if (!text) return DEFAULT_PREFS;
	try {
		const kept: unknown = JSON.parse(text);
		if (typeof kept !== "object" || kept === null) return DEFAULT_PREFS;
		const { bar, account } = kept as Record<string, unknown>;
		return {
			bar: bar === "in_use" ? "in_use" : "always",
			account: typeof account === "string" ? account : null,
		};
	} catch {
		return DEFAULT_PREFS;
	}
};

/** What the empty box says. */
export const ASK_PLACEHOLDER = "Ask anything, or describe a task";

/** The bar as the home screen draws it. */
export type DispatcherBar = {
	readonly reach: DispatcherReach;
	/** The conversation's agent, unless you started over. */
	readonly talking: Agent | undefined;
	/** The plan waiting on you. */
	readonly plan: Plan | null;
	/** Whether the dispatcher is in use: a call on its way, working, asking, or a plan waiting. */
	readonly inUse: boolean;
	/** The bar with its box, or only the grabber. */
	readonly shows: "bar" | "grabber";
	/** Whether Send sends. */
	readonly canSend: boolean;
	/** Next to the grabber while the sheet is shut: what the dispatcher is up to. */
	readonly status: string | null;
};

const statusOf = (talking: Agent | undefined, plan: Plan | null): string | null => {
	if (plan) return t("A plan waits on you");
	if (talking?.activity.state === "needs_input") return t("The dispatcher asks you something");
	if (talking && isWorking(talking)) return t(WORKING_HINT);
	return null;
};

/**
 * The bar over `view`, with what you typed and asked in `state`. It always
 * shows unless you keep it only for when it's in use; then it shows while
 * the sheet is open, the dispatcher is in use, or the box holds a draft.
 */
export const dispatcherBar = (
	view: ViewState,
	state: DispatchState,
	prefs: DispatcherPrefs,
	open: boolean
): DispatcherBar => {
	const reach = dispatcherReach(view, prefs.account);
	const talking = talkingTo(view, state, reach.dispatcher);
	const plan = shownPlan(state, reach.dispatcher);
	const using = inUse(state, talking, plan);
	const draft = state.text.trim() !== "";
	return {
		reach,
		talking,
		plan,
		inUse: using,
		shows: prefs.bar === "always" || open || using || draft ? "bar" : "grabber",
		canSend: reach.off === null && !state.busy && draft,
		status: statusOf(talking, plan),
	};
};

/** How an item of a plan that ran went, with the name of the task it saved. */
export const resultLine = (row: ResultRow): string =>
	row.task === null
		? row.line
		: t("{{result}} as {{task}}", { result: row.line, task: row.task });

// A flick faster than this, in points per millisecond, opens or shuts the
// sheet however far it went.
const FLICK = 0.5;

/**
 * Whether the sheet ends open once you let go: a flick decides, else
 * whether it's more than half up. `dy` is how far the finger went down,
 * `vy` how fast, and `height` how far the sheet travels.
 */
export const sheetOpens = (wasOpen: boolean, dy: number, vy: number, height: number): boolean => {
	if (vy < -FLICK) return true;
	if (vy > FLICK) return false;
	return (wasOpen ? 1 : 0) - dy / Math.max(height, 1) > 0.5;
};
