// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Automations under an agent's header: the scheduled tasks that send this
// agent their prompt, each with when it runs next, a switch that pauses it and
// Run now. It shows only when a task targets the agent. A task's name opens it
// in the editor.

import { useI18n } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";

import { useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import type { Agent } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { localZone } from "@maxhq/shared/tasks/draft";
import { nextRunLine, triggerLine } from "@maxhq/shared/tasks/format";
import { type HomedTask, tasksTargeting } from "@maxhq/shared/tasks/place";

import { useTaskActions } from "./actions";
import { type Editing, TaskEditor } from "./editor";

// How long Run now says "Started" after a run starts.
const STARTED_MS = 3000;

// Run now's state: ready, waiting on the request, or showing that it started.
type Run = "ready" | "running" | "started";

const Automation = ({
	homed: { task, home },
	onEdit,
}: {
	homed: HomedTask;
	onEdit: (editing: Editing) => void;
}) => {
	const { t } = useI18n();

	const now = useNow(30_000);
	const actions = useTaskActions();
	const [failure, setFailure] = useState<string | null>(null);
	const [run, setRun] = useState<Run>("ready");
	useEffect(() => {
		if (run !== "started") return;
		const timer = setTimeout(() => setRun("ready"), STARTED_MS);
		return () => clearTimeout(timer);
	}, [run]);
	const runNow = async () => {
		setRun("running");
		setFailure(null);
		const failed = await actions.runNow(task);
		setFailure(failed);
		setRun(failed ? "ready" : "started");
	};
	return (
		<li className="flex flex-col gap-0.5">
			<div className="flex items-center gap-2.5">
				<span className="min-w-0 flex-1 truncate">
					<button
						type="button"
						title={t("Edit task")}
						onClick={() => onEdit({ task, home })}
						className="rounded-[3px] text-foreground hover:underline"
					>
						{task.name}
					</button>{" "}
					{triggerLine(task.trigger, localZone())}. {nextRunLine(task, now)}
				</span>
				<button
					type="button"
					aria-label={t("Run {{name}} now", { name: task.name })}
					disabled={run !== "ready"}
					onClick={() => void runNow()}
					className="h-[22px] shrink-0 rounded-[5px] px-[7px] text-[11.5px] text-secondary-foreground enabled:hover:bg-accent enabled:hover:text-foreground disabled:opacity-60"
				>
					{run === "started" ? t("Started") : t("Run now")}
				</button>
				<span aria-hidden className="h-3.5 w-px shrink-0 bg-track" />
				<Switch
					aria-label={t("{{name}} on", { name: task.name })}
					checked={task.enabled}
					onCheckedChange={(on) =>
						void actions.setEnabled(task, home, on).then(setFailure)
					}
				/>
			</div>
			{failure ? (
				<span role="status" className="selectable text-status-error-text">
					{failure}
				</span>
			) : run === "started" ? (
				<span role="status" className="sr-only">
					{t("{{name}} started.", { name: task.name })}
				</span>
			) : null}
		</li>
	);
};

/** The tasks that reach an agent, or nothing when none does. */
export const Automations = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const [editing, setEditing] = useState<Editing | null>(null);
	const homed = tasksTargeting(view, agent.id);
	if (homed.length === 0) return null;
	return (
		<section
			aria-label={t("Automations")}
			className="flex flex-col gap-1 pr-3 pb-2 pl-3.5 text-[11.5px] text-muted-foreground"
		>
			<h3 className="text-[11px] font-[650] tracking-[0.04em] uppercase">
				{t("Automations")}
			</h3>
			<ul className="flex flex-col gap-1">
				{homed.map((h) => (
					<Automation key={h.task.id} homed={h} onEdit={setEditing} />
				))}
			</ul>
			<TaskEditor editing={editing} onClose={() => setEditing(null)} />
		</section>
	);
};
