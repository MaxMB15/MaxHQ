// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's changes to MaxHQ itself, in words: the line on the card
// of one it made at once, such as "Renamed Fix login to Auth fix", the
// phrase for one that waits in its plan, and whether the card's Undo can
// still put it back. Both apps draw the card from this model.

import type {
	AdminAction,
	DeviceId,
	DeviceSettingChange,
	InboxChange,
	ProjectSettingChange,
	TimelineItem,
} from "../bindings";
import { VENDORS } from "../agent-tools/scope";
import { t } from "../i18n";
import { MODE_LABEL } from "../settings/permission";
import { VENDOR_NAME } from "../status/agent-kind";
import { listInWords } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName, workspaceName } from "../view/select";

/** A change the dispatcher made at once, as its card on its timeline. */
export type ActionItem = Extract<TimelineItem, { kind: "dispatcher_action" }>;

/** What a change is about, by its name now: an agent, a project, a workspace, a task or the device. */
export const actionSubject = (view: ViewState, device: DeviceId, action: AdminAction): string => {
	switch (action.action) {
		case "hide_project":
		case "set_project":
			return view.tables.project.get(action.project)?.name ?? t("a project");
		case "start_workspace":
		case "stop_workspace": {
			const workspace = view.tables.workspace.get(action.workspace);
			return workspace ? workspaceName(workspace) : t("a workspace");
		}
		case "set_task":
		case "run_task":
		case "delete_task":
			return (
				view.tables.device_tasks.get(device)?.tasks.find((task) => task.id === action.task)
					?.name ?? t("a task")
			);
		case "change_settings":
		case "update_tools":
			return deviceName(view, device);
		default:
			return view.tables.agent.get(action.agent)?.title ?? t("an agent");
	}
};

const inboxDone = (change: InboxChange, name: string): string => {
	switch (change.change) {
		case "settle":
			return t("Settled {{name}}", { name });
		case "unsettle":
			return t("Brought {{name}} back", { name });
		case "snooze":
			return t("Snoozed {{name}}", { name });
		case "wake":
			return t("Woke {{name}}", { name });
		default:
			return t("Changed {{name}} in the inbox", { name });
	}
};

/** What a change did, in a sentence, with `name` for what it changed. */
const doneLine = (action: AdminAction, name: string): string => {
	switch (action.action) {
		case "rename_agent":
			return t("Renamed {{name}} to {{title}}", { name, title: action.title });
		case "set_inbox":
			return inboxDone(action.change, name);
		case "mark_read":
			return t("Marked {{name}} as read", { name });
		case "set_mode":
			return t("Set the mode of {{name}} to {{mode}}", {
				name,
				mode: MODE_LABEL[action.mode],
			});
		case "set_model":
			return t("Set the model of {{name}} to {{model}}", { name, model: action.model });
		case "set_effort":
			return action.effort === null
				? t("Set the effort of {{name}} to its model's default", { name })
				: t("Set the effort of {{name}} to {{effort}}", { name, effort: action.effort });
		case "interrupt":
			return t("Stopped the turn of {{name}}", { name });
		case "stop_agent":
			return t("Stopped {{name}}", { name });
		case "delete_agent":
			return t("Deleted {{name}}", { name });
		case "hide_project":
			return action.hidden
				? t("Hid {{name}} from the sidebar", { name })
				: t("Showed {{name}} in the sidebar again", { name });
		case "set_project":
			return t("Changed the settings of {{name}}", { name });
		case "start_workspace":
			return t("Started the workspace {{name}}", { name });
		case "stop_workspace":
			return t("Stopped the workspace {{name}}", { name });
		case "set_task":
			return action.enabled
				? t("Turned on the task {{name}}", { name })
				: t("Turned off the task {{name}}", { name });
		case "run_task":
			return t("Ran the task {{name}}", { name });
		case "delete_task":
			return t("Deleted the task {{name}}", { name });
		case "change_settings":
			return t("Changed {{count, number}} settings on {{name}}", {
				count: action.changes.length,
				name,
			});
		case "update_tools":
			return t("Updated the agent CLIs on {{name}}", { name });
		case "grant_permission":
			return t("Let {{name}} run agents", { name });
		case "revoke_permission":
			return t("Took back what {{name}} may run", { name });
	}
};

const inboxPlanned = (change: InboxChange, name: string): string => {
	switch (change.change) {
		case "settle":
			return t("settling {{name}}", { name });
		case "unsettle":
			return t("bringing {{name}} back", { name });
		case "snooze":
			return t("snoozing {{name}}", { name });
		case "wake":
			return t("waking {{name}}", { name });
		default:
			return t("an inbox change for {{name}}", { name });
	}
};

/**
 * A change that waits in a plan, in the words of the plan's summary, such
 * as "deleting Fix login".
 */
export const adminPhrase = (view: ViewState, device: DeviceId, action: AdminAction): string => {
	const name = actionSubject(view, device, action);
	switch (action.action) {
		case "rename_agent":
			return t("renaming {{name}} to {{title}}", { name, title: action.title });
		case "set_inbox":
			return inboxPlanned(action.change, name);
		case "mark_read":
			return t("marking {{name}} as read", { name });
		case "set_mode":
			return t("setting the mode of {{name}} to {{mode}}", {
				name,
				mode: MODE_LABEL[action.mode],
			});
		case "set_model":
			return t("setting the model of {{name}} to {{model}}", { name, model: action.model });
		case "set_effort":
			return action.effort === null
				? t("setting the effort of {{name}} to its model's default", { name })
				: t("setting the effort of {{name}} to {{effort}}", {
						name,
						effort: action.effort,
					});
		case "interrupt":
			return t("stopping the turn of {{name}}", { name });
		case "stop_agent":
			return t("stopping {{name}}", { name });
		case "delete_agent":
			return t("deleting {{name}}", { name });
		case "hide_project":
			return action.hidden
				? t("hiding {{name}} from the sidebar", { name })
				: t("showing {{name}} in the sidebar again", { name });
		case "set_project":
			return t("changing the settings of {{name}}", { name });
		case "start_workspace":
			return t("starting the workspace {{name}}", { name });
		case "stop_workspace":
			return t("stopping the workspace {{name}}", { name });
		case "set_task":
			return action.enabled
				? t("turning on the task {{name}}", { name })
				: t("turning off the task {{name}}", { name });
		case "run_task":
			return t("running the task {{name}} now", { name });
		case "delete_task":
			return t("deleting the task {{name}}", { name });
		case "change_settings":
			return t("changing {{count, number}} settings on {{name}}", {
				count: action.changes.length,
				name,
			});
		case "update_tools":
			return t("updating the agent CLIs on {{name}}", { name });
		case "grant_permission":
			return t("letting {{name}} run agents", { name });
		case "revoke_permission":
			return t("taking back what {{name}} may run", { name });
	}
};

const valueText = (value: unknown): string => {
	if (value === null) return t("Default");
	if (typeof value === "boolean") return value ? t("On") : t("Off");
	return typeof value === "object" ? JSON.stringify(value) : String(value);
};

/**
 * Each setting a planned change sets, as "field: value", such as
 * "default permission: Full access", so you see what you confirm.
 */
export const settingLines = (action: AdminAction): string[] => {
	const changes: (DeviceSettingChange | ProjectSettingChange)[] =
		action.action === "change_settings" || action.action === "set_project"
			? action.changes
			: [];
	return changes.map((change) => {
		const shown =
			change.field === "default_permission"
				? MODE_LABEL[change.value]
				: valueText(change.value);
		return `${change.field.replaceAll("_", " ")}: ${shown}`;
	});
};

/**
 * How wide a planned grant is, line by line, such as "Devices: devbox", so
 * you see what you confirm. `maxd` makes the dispatcher's grant for every
 * vendor the forms offer, with any account and place and no limits.
 */
export const grantLines = (
	view: ViewState,
	action: AdminAction
): { readonly label: string; readonly value: string }[] =>
	action.action === "grant_permission"
		? [
				{ label: t("Lets it"), value: t("Start, message and stop agents") },
				{
					label: t("Devices"),
					value: listInWords(action.devices.map((device) => deviceName(view, device))),
				},
				{
					label: t("Vendors"),
					value: listInWords(VENDORS.map((vendor) => VENDOR_NAME[vendor])),
				},
				{ label: t("Accounts"), value: t("Any account") },
				{ label: t("Places"), value: t("Any folder or workspace") },
				{
					label: t("Depth"),
					value: t("{{count, number}} levels of agents below it", {
						count: action.depth,
					}),
				},
				{ label: t("Limits"), value: t("No limits") },
			]
		: [];

/**
 * Whether `device`'s dispatcher makes safe changes at once, as the Act
 * directly switch says, or null when its `maxd` is too old for it and its
 * dispatcher only plans.
 */
export const actsDirectly = (view: ViewState, device: DeviceId): boolean | null =>
	view.tables.device_settings.get(device)?.dispatcher_acts ?? null;

/** A change's card: its line, where it was when not here, and its Undo. */
export type ActionCard = {
	/** Such as "Renamed Fix login to Auth fix". */
	readonly line: string;
	/** The device it changed, when that isn't the dispatcher's own. */
	readonly elsewhere: string | null;
	/** The device whose dispatcher made it, which takes the Undo. */
	readonly home: DeviceId | null;
	/**
	 * "ready" while Undo can put it back, "gone" once a restart or a later
	 * change to the same thing took that away, and null when it has none.
	 */
	readonly undo: "ready" | "gone" | null;
};

/** The card for a change the dispatcher made at once. */
export const actionCard = (view: ViewState, item: ActionItem): ActionCard => {
	// The card's chat names the dispatcher, even when you're in another chat now.
	const device = view.tables.dispatcher_chat.get(item.agent)?.device;
	const dispatcher =
		(device === undefined ? undefined : view.tables.dispatcher.get(device)) ??
		all(view, "dispatcher").find((d) => d.agent === item.agent);
	const home = dispatcher?.device ?? null;
	const undoable = item.undo !== null && (dispatcher?.undo ?? []).includes(item.undo);
	return {
		line: doneLine(item.action, item.target || actionSubject(view, item.device, item.action)),
		elsewhere: home !== null && home !== item.device ? deviceName(view, item.device) : null,
		home,
		undo: item.undo === null ? null : undoable ? "ready" : "gone",
	};
};
