// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Running section of the ports popover: what the agent started and
// still runs, read from its device every few seconds while the popover is
// open. Each command is one row with the totals of everything under it,
// which a chevron unfolds. Each row has Stop, and Kill once a process
// outlasts Stop. A command's Stop reaches everything under it. A command
// that only keeps the machine awake goes last, marked so. The agent's
// CLI and MCP servers wait in a collapsed group at the bottom. A device
// whose maxd is too old says so, and one that can't be reached shows
// nothing.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { ChevronRight } from "lucide-react";
import { Fragment, useEffect, useRef, useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import type { AgentId, AgentProcess, ProcessList, ProcessSignal } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { formatMemory } from "@maxhq/shared/fleet/memory";
import {
	cpuShares,
	formatCpu,
	KILL_AFTER_MS,
	PROCESS_REFRESH_MS,
	type ProcessGroup,
	processKey,
	runningList,
	shortCommand,
	stopOrder,
	turnEnded,
} from "@maxhq/shared/ports/processes";
import { formatDuration } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";

import { Failure, Section, TEXT_BUTTON } from "./parts";

type Read = {
	readonly agent: AgentId;
	readonly list: ProcessList;
	/** Each process's share of a core since the read before. */
	readonly cpu: ReadonlyMap<string, number>;
};

/**
 * The agent's processes, read now and every few seconds after while
 * `live`, and a way to read them again at once. A failed read shows
 * nothing until one works.
 */
const useProcesses = (agent: AgentId, live: boolean) => {
	const { source } = useServices();
	const [read, setRead] = useState<Read | null>(null);
	const [reads, setReads] = useState(0);
	const before = useRef<readonly AgentProcess[]>([]);

	useEffect(() => {
		if (!live) return;
		let gone = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const next = () => {
			if (!gone) timer = setTimeout(load, PROCESS_REFRESH_MS);
		};
		const load = () =>
			void source.listProcesses(agent).then(
				(result) => {
					if (gone) return;
					if (result.status === "error") {
						setRead(null);
						return next();
					}
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
					setRead(null);
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
		refresh: () => setReads((n) => n + 1),
	};
};

type Stopping = "stopping" | "kill";

const put =
	<T,>(key: string, value: T | null) =>
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
const useStops = (agent: AgentId, refresh: () => void) => {
	const { source } = useServices();
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
		state: (key: string) => stops.get(key),
		failure: (key: string) => failures.get(key) ?? null,
		signal,
	};
};

type Stops = ReturnType<typeof useStops>;

const Ports = ({ ports }: { ports: readonly number[] }) => {
	const { t } = useI18n();
	return ports.map((port) => (
		<span
			key={port}
			title={t("Listening on port {{port}}", {
				port: formatNumber(port, { useGrouping: false }),
			})}
			className="shrink-0 rounded-[4px] bg-secondary px-1 font-mono text-[10.5px] text-secondary-foreground tabular-nums"
		>
			{formatNumber(port, { useGrouping: false })}
		</span>
	));
};

/** Stop, then Kill once Stop hasn't ended the row: `processes` in the order they get each signal. */
const StopButton = ({
	id,
	processes,
	name,
	stops,
}: {
	id: string;
	processes: () => readonly AgentProcess[];
	name: string;
	stops: Stops;
}) => {
	const { t } = useI18n();
	return stops.state(id) === "kill" ? (
		<button
			type="button"
			aria-label={t("Kill {{name}}", { name: name })}
			title={t("Still running after Stop. Kill ends it at once.")}
			className={cn(TEXT_BUTTON, "text-status-error-text")}
			onClick={() => stops.signal(id, processes(), "kill")}
		>
			{t("Kill")}
		</button>
	) : (
		<button
			type="button"
			aria-label={t("Stop {{name}}", { name: name })}
			className={TEXT_BUTTON}
			disabled={stops.state(id) === "stopping"}
			onClick={() => stops.signal(id, processes(), "term")}
		>
			{stops.state(id) === "stopping" ? t("Stopping") : t("Stop")}
		</button>
	);
};

const Chevron = ({ open }: { open: boolean }) => {
	useI18n();
	return (
		<ChevronRight
			aria-hidden
			size={12}
			strokeWidth={2.2}
			className={cn("transition-transform", open && "rotate-90")}
		/>
	);
};

/**
 * One line of the list: a process, or a command with the totals of
 * everything under it and a chevron that unfolds them.
 */
const Row = ({
	process,
	inset,
	cpu,
	after,
	stops,
	group,
}: {
	process: AgentProcess;
	/** Its left padding, which shows how far down a tree it sits. */
	inset: string;
	cpu: number;
	/** Whether to mark it as running after the agent's turn, unless it only keeps the machine awake. */
	after: boolean;
	/** Stop and Kill, for what the agent started. */
	stops?: Stops;
	/** For a command, everything under it and whether that shows. */
	group?: { of: ProcessGroup; open: boolean; toggle: () => void };
}) => {
	const { t } = useI18n();

	const name = shortCommand(process);
	const id = processKey(process);
	const under = group?.of.under.length ?? 0;
	return (
		<li
			className={cn(
				"grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-2 gap-y-0.5 py-1.5 pr-3.5 text-[12px]",
				inset
			)}
		>
			<span className="flex min-w-0 items-center gap-1.5">
				{group ? (
					under > 0 ? (
						<button
							type="button"
							aria-expanded={group.open}
							aria-label={t("The {{value1}} processes of {{name}}", {
								value1: formatNumber(under + 1),
								name: name,
							})}
							className="-my-1 grid size-4 shrink-0 cursor-pointer place-items-center rounded-[4px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
							onClick={group.toggle}
						>
							<Chevron open={group.open} />
						</button>
					) : (
						<span aria-hidden className="size-4 shrink-0" />
					)
				) : null}
				<span
					className="truncate font-mono text-[11.5px] text-foreground"
					title={process.command.trim() || process.name}
				>
					{name}
				</span>
				<Ports ports={group?.of.ports ?? process.ports} />
			</span>
			{stops ? (
				<StopButton
					id={id}
					processes={() => (group ? stopOrder(group.of) : [process])}
					name={name}
					stops={stops}
				/>
			) : (
				<span />
			)}
			<span
				className={cn(
					"flex min-w-0 flex-wrap gap-x-2 text-[11px] text-muted-foreground tabular-nums",
					group && "pl-[22px]"
				)}
			>
				<span>{formatDuration(process.run_ms)}</span>
				<span>{t("{{value1}} CPU", { value1: formatCpu(cpu) })}</span>
				<span>{formatMemory(group?.of.memory ?? process.memory)}</span>
				{under > 0 ? (
					<span>{t("{{value1}} processes", { value1: formatNumber(under + 1) })}</span>
				) : null}
				{group?.of.keepsAwake ? (
					<span
						className="italic"
						title={t("It only keeps the computer awake and does no work.")}
					>
						{t("keeps awake")}
					</span>
				) : after ? (
					<span
						className="italic"
						title={t("It's still running after the agent's turn ended.")}
					>
						{t("after the turn")}
					</span>
				) : null}
			</span>
			{stops ? <Failure text={stops.failure(id)} /> : null}
		</li>
	);
};

/** What the agent started and still runs, with its own processes in a group below. */
export const Running = ({
	agent,
	view,
	live,
}: {
	agent: AgentId;
	view: ViewState;
	/** Whether its device can be reached, so the list can be read. */
	live: boolean;
}) => {
	const { t } = useI18n();

	const { read, refresh } = useProcesses(agent, live);
	const stops = useStops(agent, refresh);
	const [open, setOpen] = useState(false);
	const [unfolded, setUnfolded] = useState<ReadonlySet<string>>(() => new Set());
	const toggle = (key: string) =>
		setUnfolded((was) => {
			const next = new Set(was);
			if (!next.delete(key)) next.add(key);
			return next;
		});

	if (!read) return null;
	if (read.list.status === "too_old") {
		return (
			<Section title={t("Running")}>
				<li className="px-3.5 py-1.5 text-[12px] text-muted-foreground">
					{t("This device's maxd is too old to list processes.")}
				</li>
			</Section>
		);
	}
	const { started, own } = runningList(read.list.processes);
	if (started.length === 0 && own.length === 0) return null;
	const ended = turnEnded(view, agent);
	const cpu = (process: AgentProcess) => read.cpu.get(processKey(process)) ?? 0;

	return (
		<Section title={t("Running")}>
			{started.map((group) => {
				const key = processKey(group.head);
				const shown = unfolded.has(key);
				return (
					<Fragment key={key}>
						<Row
							process={group.head}
							inset="pl-2"
							cpu={[group.head, ...group.under].reduce((sum, p) => sum + cpu(p), 0)}
							after={ended}
							stops={stops}
							group={{ of: group, open: shown, toggle: () => toggle(key) }}
						/>
						{shown
							? group.under.map((process) => (
									<Row
										key={processKey(process)}
										process={process}
										inset="pl-[42px]"
										cpu={cpu(process)}
										after={false}
										stops={stops}
									/>
								))
							: null}
					</Fragment>
				);
			})}
			{started.length === 0 ? (
				<li className="px-3.5 py-1.5 text-[12px] text-muted-foreground">
					{t("Nothing the agent started is running.")}
				</li>
			) : null}
			{own.length > 0 ? (
				<li>
					<button
						type="button"
						aria-expanded={open}
						className="flex w-full cursor-pointer items-center gap-1 px-3.5 py-1 text-left text-[11.5px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
						onClick={() => setOpen((was) => !was)}
					>
						<Chevron open={open} />
						{t("Agent")}
						<span aria-hidden className="tabular-nums">
							{formatNumber(own.length)}
						</span>
					</button>
					{open ? (
						<ul aria-label={t("Agent")}>
							{own.map(({ process, child }) => (
								<Row
									key={processKey(process)}
									process={process}
									inset={child ? "pl-7" : "pl-3.5"}
									cpu={cpu(process)}
									after={false}
								/>
							))}
							<li className="px-3.5 pt-0.5 pb-1 text-[11px] text-muted-foreground">
								{t("These stop with the agent.")}
							</li>
						</ul>
					) : null}
				</li>
			) : null}
		</Section>
	);
};
