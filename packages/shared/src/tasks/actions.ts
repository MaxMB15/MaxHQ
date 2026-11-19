// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a task does on every client: turn it on or off, which changes only
// that on the device, run it now, rotate its webhook URL, delete it, and
// save the editor's draft. A save sends the task, then the signing secret
// on its own call so it never goes into the task, then whether the webhook
// takes requests. Each answers with why it didn't go through, or null.

import { t } from "../i18n";

import type { DeviceId, Task, TaskId } from "../bindings";
import { failureOf, scrub } from "../data/failure";
import type { ActionResult, DataSource } from "../data/source";
import type { ViewState } from "../view/replica";
import { type TaskDraft, taskFromDraft } from "./draft";

type TaskCalls = Pick<
	DataSource,
	"setTaskEnabled" | "runTaskNow" | "rotateHookToken" | "deleteTask"
>;

/**
 * Turns tasks on and off, runs them now and rotates their webhook URLs.
 * `ran` hears of each run, so its runs can be read again.
 */
export const taskActions = (
	source: TaskCalls,
	ran: (task: TaskId) => unknown = () => undefined
) => {
	const attempt = async (what: string, call: () => Promise<ActionResult>) => {
		try {
			return failureOf(await call());
		} catch (error) {
			console.warn(`Couldn't ${what}`, error);
			return t("Couldn't {{what}}. Try again.", { what: what });
		}
	};
	return {
		setEnabled: (task: Task, home: DeviceId, enabled: boolean) =>
			attempt(enabled ? t("turn it on") : t("pause it"), () =>
				source.setTaskEnabled(home, task.id, enabled)
			),
		runNow: async (task: Task) => {
			const failure = await attempt(t("run it"), () => source.runTaskNow(task.id));
			if (!failure) await ran(task.id);
			return failure;
		},
		/** Makes a new URL for a task's webhook, and hands it to `show`. */
		rotate: (task: Task, show: (url: string) => void) =>
			attempt(t("rotate it"), async () => {
				const result = await source.rotateHookToken(task.id);
				if (result.status === "ok" && result.data.status === "hook_url")
					show(result.data.url);
				return result;
			}),
		/** Deletes a task, with its runs and requests. */
		remove: async (task: Task) => {
			try {
				return failureOf(await source.deleteTask(task.id));
			} catch (error) {
				console.warn("Couldn't delete the task", error);
				return t("Couldn't delete it. Try again.");
			}
		},
	};
};

/** What saving a draft came to. */
export type SavedDraft =
	/** Nothing was saved, for `reason`. */
	| { readonly state: "refused"; readonly reason: string }
	/** The task was saved but its secret or its on and off wasn't, for `reason`. */
	| { readonly state: "partly"; readonly task: Task; readonly reason: string }
	/** All of it was saved. A new webhook's URL shows this once. */
	| { readonly state: "saved"; readonly task: Task; readonly hookUrl: string | null };

/**
 * Saves `draft` as task `id` on `home`, then sends its signing secret and
 * whether its webhook takes requests (`listen`).
 */
export const saveDraft = async (
	source: Pick<DataSource, "saveTask" | "setHookSecret" | "setHookOn">,
	view: ViewState,
	draft: TaskDraft,
	home: DeviceId | null,
	listen: boolean,
	id: TaskId
): Promise<SavedDraft> => {
	const built = taskFromDraft(view, draft, id);
	if (!built.ok) return { state: "refused", reason: built.reason };
	if (!home) return { state: "refused", reason: t("Pick the device it lives on.") };
	const secret = draft.signed ? draft.secret.trim() : "";
	try {
		const result = await source.saveTask(home, built.task);
		const failure = failureOf(result);
		if (failure) return { state: "refused", reason: failure };
		const answer =
			result.status === "ok" && result.data.status === "task_saved" ? result.data : null;
		const task = answer?.task ?? built.task;
		const problems: string[] = [];
		if (task.trigger.type === "hook") {
			if (secret) {
				const sent = failureOf(await source.setHookSecret(task.id, secret));
				if (sent) problems.push(scrub(sent, secret));
			}
			if (task.trigger.on !== listen) {
				const turned = failureOf(await source.setHookOn(task.id, listen));
				if (turned) problems.push(turned);
			}
		}
		if (problems.length > 0) {
			return {
				state: "partly",
				task,
				reason: t("Saved, but {{value1}}", { value1: problems.join(" ") }),
			};
		}
		return { state: "saved", task, hookUrl: answer?.hook_url ?? null };
	} catch (error) {
		console.warn("Couldn't save the task", error);
		return { state: "refused", reason: t("Couldn't save it. Try again.") };
	}
};
