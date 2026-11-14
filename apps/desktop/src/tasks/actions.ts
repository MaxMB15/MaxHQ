// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a task's row does without its editor: turn it on or off, which
// changes only that on the device, run it now and rotate its webhook URL.
// Each answers with why it didn't go through, or null.

import { t } from "@maxhq/shared/i18n";

import { useServices } from "@/app/services";
import { failureOf } from "@/settings/result";
import type { DeviceId, Task } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";

import { useRefreshActivity } from "./queries";

/** Turns tasks on and off, runs them now and rotates their webhook URLs. */
export const useTaskActions = () => {
	const { source } = useServices();
	const refresh = useRefreshActivity();
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
			if (!failure) await refresh(task.id);
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
	};
};
