// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level a Claude Code or Codex agent runs at. Each model lists
// the levels its CLI takes and the one it runs at when told none, and
// `maxd` names a level to every agent it starts, so what shows here is what
// runs. The composer's picker and the device's default in Settings draw
// from here. A level MaxHQ doesn't know yet shows under its own name.

import { t } from "../i18n";

import {
	type Agent,
	type DeviceId,
	EFFORT_SINCE,
	type ModelChoice,
	type Vendor,
} from "../bindings";
import type { ViewState } from "../view/replica";
import { accounts, linkForDevice } from "../view/select";

/** Whether a vendor's agents take an effort level. */
export const takesEffort = (vendor: Vendor): boolean => vendor === "claude" || vendor === "codex";

/** Whether a device's `maxd` sets effort levels, by the protocol its link speaks. */
export const setsEffort = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= EFFORT_SINCE;
};

/** What a level is called. A level MaxHQ doesn't know keeps the CLI's name. */
export const effortLabel = (level: string): string => {
	switch (level) {
		case "none":
			return t("None");
		case "minimal":
			return t("Minimal");
		case "low":
			return t("Low");
		case "medium":
			return t("Medium");
		case "high":
			return t("High");
		case "xhigh":
			return t("Extra high");
		case "max":
			return t("Max");
		default:
			return level;
	}
};

/** The model whose levels apply: the agent's own, else the account's default. */
const modelFor = (model: string | null, choices: readonly ModelChoice[]): ModelChoice | undefined =>
	(model === null ? undefined : choices.find((choice) => choice.id === model)) ??
	choices.find((choice) => choice.default);

/**
 * The level a model runs at when told none: the one its `maxd` names, else
 * high, which the CLIs use when nothing else says, else its middle level.
 * A `maxd` from before Claude models named theirs leaves it out.
 */
const defaultLevel = (found: ModelChoice | undefined): string | null => {
	const levels = found?.efforts ?? [];
	if (found?.default_effort) return found.default_effort;
	if (levels.includes("high")) return "high";
	return levels[Math.floor(levels.length / 2)] ?? null;
};

/** One level in a picker. */
export type EffortChoice = {
	readonly value: string;
	readonly label: string;
	/** Whether this is the level the model runs at when none is picked. */
	readonly isDefault: boolean;
};

/**
 * The levels in `levels`, with `current` added when it isn't one of them,
 * so a picker can still show it, and the default one marked.
 */
const choicesOf = (
	levels: readonly string[],
	current: string | null,
	fallback: string | null
): EffortChoice[] => {
	if (levels.length === 0) return [];
	const all = current === null || levels.includes(current) ? levels : [...levels, current];
	return all.map((level) => ({
		value: level,
		label: effortLabel(level),
		isDefault: level === fallback,
	}));
};

/**
 * The level the agent runs at: its own, else its model's default. Null
 * only when it has none and its model lists none.
 */
export const effortLevel = (
	effort: string | null,
	model: string | null,
	choices: readonly ModelChoice[]
): string | null => effort ?? defaultLevel(modelFor(model, choices));

/**
 * The levels the agent's model takes, in order, with the one it runs at
 * when told none marked. A level the agent runs at that the model doesn't
 * list stays, so the picker can still show it. Empty when the model lists
 * none.
 */
export const effortChoices = (
	effort: string | null,
	model: string | null,
	choices: readonly ModelChoice[]
): EffortChoice[] => {
	const found = modelFor(model, choices);
	return choicesOf(found?.efforts ?? [], effort, defaultLevel(found));
};

/** What the picker's button says: the name of the level the agent runs at. */
export const effortShown = (
	effort: string | null,
	model: string | null,
	choices: readonly ModelChoice[]
): string => {
	const level = effortLevel(effort, model, choices);
	return level === null ? "" : effortLabel(level);
};

/**
 * Whether you can change the agent's effort level: a lead Claude Code or
 * Codex agent MaxHQ holds, on a `device` whose `maxd` sets levels, whose
 * model lists some. A sub-agent runs at its lead's level.
 */
export const canSetEffort = (
	view: ViewState,
	agent: Agent,
	device: DeviceId,
	choices: readonly ModelChoice[]
): boolean => {
	if (!takesEffort(agent.vendor) || agent.control !== "held" || agent.parent !== null)
		return false;
	if (!setsEffort(view, device)) return false;
	return effortChoices(agent.effort ?? null, agent.model, choices).length > 0;
};

/** The models of each of the device's logins for `vendor`. */
const deviceModels = (view: ViewState, device: DeviceId, vendor: Vendor): ModelChoice[][] =>
	accounts(view)
		.filter((account) => account.device === device && account.vendor === vendor)
		.map((account) => account.models);

/** The level the first of `logins` with levels runs its default model at. */
const deviceDefault = (logins: readonly ModelChoice[][]): string | null => {
	for (const models of logins) {
		const level = defaultLevel(modelFor(null, models));
		if (level !== null) return level;
	}
	return null;
};

/**
 * The levels a device's default for `vendor` can take: every level a model
 * of that vendor's logins there lists, in the order they first come, with
 * the level its first login's default model runs at when told none
 * marked. Empty when it has no such login, or its `maxd` lists none.
 */
export const deviceEffortChoices = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	current: string | null
): EffortChoice[] => {
	const logins = deviceModels(view, device, vendor);
	const levels: string[] = [];
	for (const models of logins)
		for (const model of models)
			for (const level of model.efforts ?? [])
				if (!levels.includes(level)) levels.push(level);
	return choicesOf(levels, current, deviceDefault(logins));
};

/**
 * The level new agents for `vendor` on the device start at: the device's
 * own, else the one its first login's default model runs at when told
 * none, as `maxd` starts them. Null when no login there lists levels.
 */
export const deviceEffortLevel = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	current: string | null
): string | null => current ?? deviceDefault(deviceModels(view, device, vendor));
