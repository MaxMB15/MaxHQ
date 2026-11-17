// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Running list's reads and signals, for the desktop's ports popover and
// the phone's Ports and processes screen. The agent's processes are read
// now and every few seconds after, while its device can be reached. Stop
// goes to a row, a process or a command with everything under it, and a
// row Stop doesn't end within a few seconds is offered Kill. Each row
// tracks why its last signal failed.

import { t } from "../i18n";

import { useEffect, useRef, useState } from "react";

import type { AgentId, AgentProcess, ProcessList, ProcessSignal } from "../bindings";
import { failureOf } from "../data/failure";
import type { DataSource } from "../data/source";
import { errorText } from "../timeline/store";

import { cpuShares, KILL_AFTER_MS, PROCESS_REFRESH_MS } from "./processes";

/** One read of an agent's processes. */
export type ProcessRead = {
	readonly agent: AgentId;
	readonly list: ProcessList;
	/** Each process's share of a core since the read before. */
	readonly cpu: ReadonlyMap<string, number>;
};

/**
 * The agent's processes, read now and every few seconds after while
 * `live`, and a way to read them again at once, which settles once that
 * read has. A failed read shows no list until one works, and says why.
 */
export const useProcesses = (
	source: Pick<DataSource, "listProcesses">,
	agent: AgentId,
	live: boolean
) => {
	const [read, setRead] = useState<ProcessRead | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	const [reads, setReads] = useState(0);
	const before = useRef<readonly AgentProcess[]>([]);
	// Refreshes that wait for the next read to settle.
	const waiting = useRef<(() => void)[]>([]);

	useEffect(() => {
		const settle = () => {
			const done = waiting.current;
			waiting.current = [];
			for (const resolve of done) resolve();
		};
		if (!live) return settle();
		let gone = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const next = () => {
			if (!gone) timer = setTimeout(load, PROCESS_REFRESH_MS);
		};
		const load = () =>
			void source.listProcesses(agent).then(
				(result) => {
					if (gone) return;
					settle();
					if (result.status === "error") {
						setRead(null);
						setFailure(errorText(result.error));
						return next();
					}
					setFailure(null);
					const list = result.data;
					// A maxd too old to list them stays so until it's updated.
					if (list.status === "too_old") return setRead({ agent, list, cpu: new Map() });
					setRead({ agent, list, cpu: cpuShares(list.processes, before.current) });
					before.current = list.processes;
					next();
				},
				(error: unknown) => {
					console.warn("Couldn't list the agent's processes", error);
					if (gone) return;
					settle();
					setRead(null);
					setFailure(error instanceof Error ? error.message : String(error));
					next();
				}
			);
		load();
		return () => {
			gone = true;
			clearTimeout(timer);
		};
	}, [source, agent, live, reads]);

	return {
		read: live && read?.agent === agent ? read : null,
		/** Why the last read failed, while no read has worked since. */
		failure: live ? failure : null,
		refresh: () =>
			new Promise<void>((resolve) => {
				if (!live) return resolve();
				waiting.current.push(resolve);
				setReads((n) => n + 1);
			}),
	};
};

/** Where a row's Stop stands: sent, or offered Kill since it didn't end the row. */
export type Stopping = "stopping" | "kill";

const put =
	<T>(key: string, value: T | null) =>
	(was: ReadonlyMap<string, T>): ReadonlyMap<string, T> => {
		const next = new Map(was);
		if (value === null) next.delete(key);
		else next.set(key, value);
		return next;
	};

/**
 * Stop and Kill for the agent's processes, one row at a time: a process,
 * or a command with everything under it. A row Stop doesn't end within a
 * few seconds is offered Kill. Each row tracks why its last signal failed.
 */
export const useStops = (
	source: Pick<DataSource, "signalProcess">,
	agent: AgentId,
	refresh: () => void
) => {
	const [stops, setStops] = useState<ReadonlyMap<string, Stopping>>(() => new Map());
	const [failures, setFailures] = useState<ReadonlyMap<string, string>>(() => new Map());

	const send = (process: AgentProcess, signal: ProcessSignal) =>
		source
			.signalProcess(agent, process.pid, process.start, signal)
			.then(failureOf, (error: unknown) =>
				error instanceof Error && error.message
					? t("Couldn't stop that process: {{message}}", { message: error.message })
					: t("Couldn't stop that process.")
			);

	/**
	 * Sends `signal` to each of `processes` in turn, for the row `key`. It
	 * fails only when none got it, since some may end on their own first.
	 */
	const signal = (key: string, processes: readonly AgentProcess[], signal: ProcessSignal) => {
		setStops(put<Stopping>(key, "stopping"));
		setFailures(put<string>(key, null));
		void (async () => {
			let sent = false;
			let failure: string | null = null;
			for (const process of processes) {
				const failed = await send(process, signal);
				if (failed === null) sent = true;
				else failure ??= failed;
			}
			return sent ? null : failure;
		})().then((failure) => {
			refresh();
			if (failure !== null) {
				setStops(put<Stopping>(key, null));
				setFailures(put(key, failure));
				return;
			}
			setTimeout(
				() => setStops((was) => (was.has(key) ? put<Stopping>(key, "kill")(was) : was)),
				KILL_AFTER_MS
			);
		});
	};

	return {
		state: (key: string): Stopping | undefined => stops.get(key),
		failure: (key: string): string | null => failures.get(key) ?? null,
		signal,
	};
};

/** Stop and Kill for an agent's processes, from `useStops`. */
export type Stops = ReturnType<typeof useStops>;
