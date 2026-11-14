// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A task's runs and webhook requests, read from its home device when the
// Deliveries and runs dialog opens. The tasks themselves come with the
// view, so a change shows without asking again.

import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useServices } from "@/app/services";
import type { TaskId } from "@maxhq/shared/bindings";

const RUNS = "task-runs";
const DELIVERIES = "task-deliveries";

/** A task's latest runs. */
export const useRuns = (task: TaskId) => {
	const { source } = useServices();
	return useQuery({ queryKey: [RUNS, task], queryFn: () => source.listRuns(task) });
};

/** The latest requests a task's webhook or GitHub events took. */
export const useDeliveries = (task: TaskId) => {
	const { source } = useServices();
	return useQuery({ queryKey: [DELIVERIES, task], queryFn: () => source.listDeliveries(task) });
};

/** Asks for a task's runs and requests again. */
export const useRefreshActivity = () => {
	const queries = useQueryClient();
	return (task: TaskId) =>
		Promise.all(
			[RUNS, DELIVERIES].map((key) => queries.invalidateQueries({ queryKey: [key, task] }))
		);
};
