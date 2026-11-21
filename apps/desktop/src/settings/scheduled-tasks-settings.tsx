// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The scheduled tasks view: one device's tasks at a time, picked above
// them, with on and off, when each runs next, how it last went and Run
// now, under the address the device's webhooks are reached at. A
// GitHub events task says whether a webhook brings its events at once. A
// device whose `maxd` predates tasks isn't listed.

import { useState } from "react";

import { useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { TaskActivity } from "@/tasks/activity";
import { useTaskActions } from "@/tasks/actions";
import { type Editing, TaskEditor } from "@/tasks/editor";
import { AddressRow } from "@/tasks/hook-address";
import { HookUrl } from "@/tasks/hook-fields";
import type { DeviceId, DeviceTasks, FunnelState, Task, TaskProblem } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { localZone } from "@maxhq/shared/tasks/draft";
import {
	addressHint,
	addressOf,
	hiddenHookUrl,
	lastRunLine,
	needsSecret,
	nextRunLine,
	triggerLine,
	webhookLine,
} from "@maxhq/shared/tasks/format";
import { deviceTasks, shownTaskDevice } from "@maxhq/shared/tasks/place";
import { deviceName } from "@maxhq/shared/view/select";
import { cn } from "@/lib/utils";

import {
	ConfirmButton,
	PRIMARY,
	Row,
	SECONDARY,
	Section,
	SEG,
	SEG_ITEM,
	StatusNote,
} from "./controls";
import type { Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const TaskRow = ({
	task,
	home,
	funnel,
	webhooks,
	problem,
	onEdit,
	onActivity,
}: {
	task: Task;
	home: DeviceId;
	/** Whether the device's webhooks can be reached, and where. */
	funnel: FunnelState;
	/** How the device's GitHub events tasks hear of events. */
	webhooks: DeviceTasks["webhooks"];
	/** Why the task can't run now, as its device says. */
	problem: TaskProblem | null;
	onEdit: () => void;
	onActivity: () => void;
}) => {
	useI18n();
	const now = useNow(30_000);
	const actions = useTaskActions();
	const [note, setNote] = useState<Note | null>(null);
	// The webhook's whole URL, shown once after a rotate.
	const [url, setUrl] = useState<string | null>(null);
	const answer = (failure: string | null) =>
		setNote(failure ? { text: failure, error: true } : null);
	const heard = webhookLine(task, { webhooks }, now);
	return (
		<Row
			title={<span className="selectable">{task.name}</span>}
			sub={
				<>
					<span className="block">
						{triggerLine(task.trigger, localZone())}. {nextRunLine(task, now)}.{" "}
						{lastRunLine(task.last_run, now)}.
					</span>
					{heard ? <span className="block">{heard}.</span> : null}
					{needsSecret(task) ? (
						<span className="block text-status-busy-text">
							{translate(
								"Add the signing secret. Until then the webhook refuses every request."
							)}
						</span>
					) : null}
					{problem ? (
						<span className="block text-status-busy-text">
							{problem.reason}
							{problem.link ? (
								<>
									{" "}
									<a
										href={problem.link}
										target="_blank"
										rel="noreferrer"
										className="text-primary hover:underline"
									>
										{translate("Open the GitHub App's settings")}
									</a>
								</>
							) : null}
						</span>
					) : null}
					{task.trigger.type === "hook" && url === null ? (
						<span className="block truncate font-mono select-none">
							{hiddenHookUrl(task.id, funnel)}
						</span>
					) : null}
					{url ? (
						<span className="mt-1.5 block">
							<HookUrl url={url} />
						</span>
					) : null}
				</>
			}
		>
			<StatusNote note={note} />
			{task.trigger.type === "hook" ? (
				<ConfirmButton
					label={translate("Rotate URL")}
					question="The current URL stops working."
					confirm={translate("Rotate")}
					onConfirm={() => void actions.rotate(task, setUrl).then(answer)}
				/>
			) : null}
			<button type="button" className={SECONDARY} onClick={onActivity}>
				{translate("History")}
			</button>
			<button
				type="button"
				className={SECONDARY}
				aria-label={translate("Run {{name}} now", { name: task.name })}
				onClick={() => void actions.runNow(task).then(answer)}
			>
				{translate("Run now")}
			</button>
			<button
				type="button"
				className={SECONDARY}
				aria-label={translate("Edit {{name}}", { name: task.name })}
				onClick={onEdit}
			>
				{translate("Edit")}
			</button>
			<Switch
				aria-label={translate("{{name}} on", { name: task.name })}
				checked={task.enabled}
				onCheckedChange={(on) => void actions.setEnabled(task, home, on).then(answer)}
			/>
		</Row>
	);
};

/** The page's headings and labels, and its task form's, for the settings search. */
export const SCHEDULED_TASKS_TERMS: readonly string[] = [
	"New task",
	"Prompt",
	"When it runs",
	"Webhook",
	"Rotate URL",
	"Requests it takes",
	"Where it goes",
	"History",
];

/** The Scheduled tasks page. */
export const ScheduledTasksSettingsPage = () => {
	useI18n();
	const view = useView((s) => s.view);
	const groups = deviceTasks(view);
	const [picked, setPicked] = useState<DeviceId | null>(null);
	const shown = shownTaskDevice(view, picked);
	const [editing, setEditing] = useState<Editing | null>(null);
	const [activity, setActivity] = useState<Task | null>(null);
	return (
		<>
			{groups.length === 0 ? (
				<Section title={translate("Scheduled tasks")}>
					<Row
						title={translate("No device runs scheduled tasks yet")}
						sub={translate("A device's maxd runs them once it's updated.")}
					/>
				</Section>
			) : (
				<>
					<div className="flex items-center gap-2">
						{groups.length > 1 ? (
							<div role="group" aria-label={translate("Device")} className={SEG}>
								{groups.map(({ device }) => (
									<button
										key={device}
										type="button"
										aria-pressed={device === shown}
										className={SEG_ITEM}
										onClick={() => setPicked(device)}
									>
										{deviceName(view, device)}
									</button>
								))}
							</div>
						) : null}
						<button
							type="button"
							className={cn(PRIMARY, "ml-auto")}
							onClick={() => setEditing({ task: null, home: null })}
						>
							{translate("New task")}
						</button>
					</div>
					{groups
						.filter((reach) => reach.device === shown)
						.map((reach) => {
							const { device, tasks, funnel, problems, webhooks } = reach;
							return (
								<Section
									key={device}
									title={deviceName(view, device)}
									hint={
										tasks.some(
											(t) =>
												t.trigger.type !== "every" &&
												t.trigger.type !== "at"
										)
											? addressHint(addressOf(reach), funnel)
											: undefined
									}
								>
									<AddressRow reach={reach} name={deviceName(view, device)} />
									{tasks.length === 0 ? (
										<Row title={translate("No tasks on this device")} />
									) : (
										tasks.map((task) => (
											<TaskRow
												key={task.id}
												task={task}
												home={device}
												funnel={funnel}
												webhooks={webhooks}
												problem={
													problems.find((p) => p.task === task.id) ?? null
												}
												onEdit={() => setEditing({ task, home: device })}
												onActivity={() => setActivity(task)}
											/>
										))
									)}
								</Section>
							);
						})}
				</>
			)}
			<TaskEditor editing={editing} onClose={() => setEditing(null)} />
			<TaskActivity task={activity} onClose={() => setActivity(null)} />
		</>
	);
};
