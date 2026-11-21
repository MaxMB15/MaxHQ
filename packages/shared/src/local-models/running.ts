// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a local agent shows while it runs: "Loading model" while its turn
// waits for the model to load into memory, and the composer footer's speed
// line with how fast the server reads the prompt and writes, and how full
// its context is. The line stays, dimmed, after the turn. An agent on
// another device's server takes the speeds that server last measured.

import type { Activity, Agent, ModelServer, ServerModel } from "../bindings";
import { formatNumber, t } from "../i18n";
import { isLocalAccount } from "../status/agent-kind";
import { formatContext } from "../status/format";
import type { ViewState } from "../view/replica";
import { serverModelChoices, serverModelName, serverOf } from "./models";
import { localModelName } from "./select";

// The states in which the agent's own turn runs, so it needs its model.
const WORKING: ReadonlySet<Activity["state"]> = new Set(["starting", "busy"]);

/** Whether the agent's own turn runs now, or is about to start. */
export const turnRuns = (agent: Pick<Agent, "activity">): boolean =>
	WORKING.has(agent.activity.state);

/** The server and model a local agent runs: the one it picked, else the server's default. */
const runningModel = (
	view: ViewState,
	agent: Pick<Agent, "account" | "model">
): { server: ModelServer; model: ServerModel } | undefined => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	if (!account || !isLocalAccount(account)) return undefined;
	const server = serverOf(view, account);
	if (!server) return undefined;
	const id = agent.model ?? serverModelChoices(view, account).find((c) => c.default)?.id;
	const model = server.models.find((m) => m.id === id);
	return model ? { server, model } : undefined;
};

/** A local agent waiting for its model to load. */
export type LocalLoading = {
	/** The model's name, when the agent's server lists it. */
	readonly model: string | null;
	/**
	 * How much of the model is in memory, 0 to 99, when its server says.
	 * At 99 the file is in and the server gets ready: see `warmingUp`.
	 */
	readonly percent: number | null;
};

/**
 * Whether a local agent's turn waits for its model to load, with the
 * model's name and how far the load is. It shows as soon as a turn starts on
 * a model that isn't in memory, before the server says it's loading, so
 * there's no blank wait. The model may be on another device's server, whose
 * load shows here the same. Null for an agent that isn't local or whose
 * model is ready.
 */
export const localLoading = (
	view: ViewState,
	agent: Pick<Agent, "account" | "model" | "local" | "activity">
): LocalLoading | null => {
	const running = runningModel(view, agent);
	const model = running
		? serverModelName(view, running.server, running.model)
		: localModelName(view, agent);
	const load = running?.model.load;
	const percent = load?.state === "loading" ? (load.percent ?? null) : null;
	if (agent.local?.loading) return { model, percent };
	if (!running || !turnRuns(agent)) return null;
	return load?.state === "unloaded" || load?.state === "loading" ? { model, percent } : null;
};

/** The footer's speed line for a local agent. */
export type SpeedLine = {
	/** The line, such as "1,180 / 74 tok/s · 31k of 66k". */
	readonly text: string;
	/** The line folded for a narrow footer: the writing speed, else the context. */
	readonly short: string;
	/** The line in words, for its tooltip and screen readers. */
	readonly full: string;
	/** Whether the turn it measures runs now. After it, the line shows dimmed. */
	readonly live: boolean;
};

const perSecond = (value: number): string => formatNumber(Math.round(value));

/**
 * The speed line from what the agent's model server reports: prompt and
 * writing speeds in tokens per second, and the context used against the
 * server's context size. Null until the server reports any of them. An
 * agent whose `maxd` doesn't measure the speeds, because the model runs on
 * another device's server, takes the speeds that server last measured for
 * the model.
 */
export const localSpeed = (
	view: ViewState,
	agent: Pick<Agent, "account" | "model" | "local" | "activity">
): SpeedLine | null => {
	const run = agent.local;
	if (!run) return null;
	const running = runningModel(view, agent);
	const device = agent.account ? view.tables.account.get(agent.account)?.device : undefined;
	const measured = running?.server.device !== device ? running?.model.speed : undefined;
	const prompt = run.prompt_per_second ?? measured?.prompt_per_second ?? null;
	const decode = run.decode_per_second ?? measured?.decode_per_second ?? null;
	const used = run.context_used ?? null;
	const size = run.context_size ?? null;

	const speed =
		prompt !== null && decode !== null
			? t("{{prompt}} / {{decode}} tok/s", {
					prompt: perSecond(prompt),
					decode: perSecond(decode),
				})
			: decode !== null
				? t("{{decode}} tok/s", { decode: perSecond(decode) })
				: null;
	const context = used !== null && size !== null ? formatContext(used, size) : null;
	const text = [speed, context].filter((part) => part !== null).join(" · ");
	if (text === "") return null;

	const facts = [
		prompt !== null &&
			t("Reads the prompt at {{value}} tokens/s", { value: perSecond(prompt) }),
		decode !== null && t("Writes {{value}} tokens/s", { value: perSecond(decode) }),
		context !== null && t("Context {{context}} in use", { context }),
	].filter((part): part is string => part !== false);
	const live = turnRuns(agent);
	const full = facts.join(" · ");
	return {
		text,
		short:
			decode !== null
				? t("{{decode}} tok/s", { decode: perSecond(decode) })
				: (context ?? text),
		full: live ? full : t("Last turn: {{facts}}", { facts: full }),
		live,
	};
};
