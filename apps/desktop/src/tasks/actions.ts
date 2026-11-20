// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a task's row does without its editor: turn it on or off, which
// changes only that on the device, run it now and rotate its webhook URL.
// Each answers with why it didn't go through, or null.

import { useServices } from "@/app/services";
import { taskActions } from "@maxhq/shared/tasks/actions";

import { useRefreshActivity } from "./queries";

/** Turns tasks on and off, runs them now and rotates their webhook URLs. */
export const useTaskActions = () => {
	const { source } = useServices();
	const refresh = useRefreshActivity();
	return taskActions(source, refresh);
};
