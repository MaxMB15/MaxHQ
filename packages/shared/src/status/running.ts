// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What an idle agent left running: the jobs it started that still run, and
// what it waits on, such as a background task it watches or its next
// wakeup. An older maxd sends none of it, and the agent looks as it did.

import { t } from "../i18n";

import type { Agent, AgentRunning, AgentWait, Timestamp } from "../bindings";

import type { Fact } from "./card";
import { formatDuration, formatWhen, listInWords } from "./format";

/** What the agent waits on now, leaving out a wakeup that's past. */
export const agentWait = (
	running: AgentRunning | null | undefined,
	now: Timestamp
): AgentWait | null => {
	const wait = running?.waiting ?? null;
	if (wait?.kind === "wakeup" && wait.at <= now) return null;
	return wait;
};

/** How many jobs still run, 0 when maxd doesn't say. */
export const jobCount = (running: AgentRunning | null | undefined): number => running?.count ?? 0;

/** The names of the jobs that still run, such as "pnpm dev, cargo watch and 2 more". */
export const jobNames = (running: AgentRunning): string => {
	const names = [...running.names];
	const more = running.count - names.length;
	if (more > 0) names.push(t("{{more}} more", { more }));
	return listInWords(names);
};

/** The "Still running" fact, when jobs still run. */
export const jobsFact = (running: AgentRunning | null | undefined): Fact[] =>
	running && running.count > 0 ? [{ label: t("Still running"), value: jobNames(running) }] : [];

/** The sentence for the jobs that still run, such as "2 processes still running: pnpm dev and cargo watch.". */
export const jobsSentence = (running: AgentRunning): string =>
	t("Processes still running", { count: running.count, names: jobNames(running) });

/** The title and body for what an idle agent waits on. */
export const waitWords = (wait: AgentWait, now: Timestamp): { title: string; body: string } =>
	wait.kind === "task"
		? {
				title: t("Waiting on a background task"),
				body: t("It watches {{name}} and goes on when the task ends.", { name: wait.name }),
			}
		: {
				title: t("Waiting on a wakeup"),
				body: t("It wakes itself at {{when}}, in {{left}}.", {
					when: formatWhen(wait.at, now),
					left: formatDuration(Math.max(0, wait.at - now)),
				}),
			};

/** Whether an idle agent waits on something, left something running, or neither. */
export const idleExtra = (
	agent: Pick<Agent, "running">,
	now: Timestamp
): "idle_waiting" | "idle_running" | null =>
	agentWait(agent.running, now)
		? "idle_waiting"
		: jobCount(agent.running) > 0
			? "idle_running"
			: null;
