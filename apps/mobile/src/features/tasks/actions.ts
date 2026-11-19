// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a task's row does without its editor, on the Tasks screen and in an
// agent's Automations: turn it on or off, run it now, which says "Started"
// for a moment once the run starts, and delete it. A row shows why the
// last of them didn't go through.

import { useEffect, useMemo, useState } from "react";

import type { DeviceId, Task } from "@maxhq/shared/bindings";
import { taskActions } from "@maxhq/shared/tasks/actions";

import { useServices } from "../../services";

/** Turns tasks on and off, runs them now, rotates and deletes them. */
export const useTaskActions = () => {
	const { source } = useServices();
	return useMemo(() => taskActions(source), [source]);
};

// How long Run now says "Started" after a run starts.
const STARTED_MS = 3000;

/** Run now's state: ready, waiting on the request, or showing that it started. */
export type RunState = "ready" | "running" | "started";

/** A task row's actions, and why the last one failed. */
export const useTaskRow = (task: Task, home: DeviceId) => {
	const actions = useTaskActions();
	const [failure, setFailure] = useState<string | null>(null);
	const [run, setRun] = useState<RunState>("ready");
	useEffect(() => {
		if (run !== "started") return;
		const timer = setTimeout(() => setRun("ready"), STARTED_MS);
		return () => clearTimeout(timer);
	}, [run]);
	return {
		run,
		failure,
		runNow: async () => {
			setRun("running");
			setFailure(null);
			const failed = await actions.runNow(task);
			setFailure(failed);
			setRun(failed ? "ready" : "started");
		},
		setEnabled: async (on: boolean) => setFailure(await actions.setEnabled(task, home, on)),
		remove: async () => setFailure(await actions.remove(task)),
	};
};
