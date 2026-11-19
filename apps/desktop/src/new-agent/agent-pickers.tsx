// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pickers that say where a new agent runs and as what: the project or
// a local folder, the workspace or device, the login, model and mode. The
// new agent form and a scheduled task's editor both start agents from them.

import type { PermissionMode } from "@maxhq/shared/bindings";
import {
	accountOptions,
	type Choices,
	deviceOptions,
	modelOptions,
	modeOptions,
	type Option,
	projectOptions,
	type Resolved,
	workspaceOptions,
} from "@maxhq/shared/new-agent/spec";
import type { ViewState } from "@maxhq/shared/view/replica";
import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { PathField } from "@/paths/path-field";
import { pickMode } from "@/settings/auto-consent";

import { Picker } from "./picker";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// The place picker's entry for a folder, which no project ID can match.
const FOLDER = "*folder";

/** The look of the form's text fields. */
export const AGENT_FIELD =
	"w-full rounded-[7px] border border-border bg-subpanel px-2.5 text-[12.5px] text-foreground outline-none placeholder:text-subtle-foreground focus:border-primary focus:shadow-focus";

/** The pickers for a new agent, with the folder's field when it runs in one. */
export const AgentPickers = ({
	view,
	choices,
	resolved: r,
	now,
	onPick,
	children,
}: {
	view: ViewState;
	choices: Choices;
	resolved: Resolved;
	now: number;
	onPick: (patch: Partial<Choices>) => void;
	/** What shows between the pickers and the folder's field. */
	children?: React.ReactNode;
}) => {
	useI18n();
	const services = useServices();
	const folder = r.place === "folder";
	const places: Option[] = [
		...projectOptions(view).map((o) => ({ ...o, group: "Projects" })),
		{
			value: FOLDER,
			label: translate("Use a local folder"),
			detail: translate("On any connected device"),
		},
	];
	return (
		<>
			<div className="flex flex-wrap gap-2">
				<Picker
					label={translate("Project")}
					value={folder ? FOLDER : (r.project ?? "")}
					options={places}
					onChange={(value) =>
						value === FOLDER
							? onPick({ place: "folder" })
							: onPick({ place: "project", project: value, workspace: null })
					}
				/>
				{folder ? (
					<Picker
						label={translate("Device")}
						value={r.device ?? ""}
						options={deviceOptions(view)}
						onChange={(device) => onPick({ device })}
					/>
				) : (
					<Picker
						label={translate("Runs on")}
						value={r.workspace?.id ?? ""}
						options={r.project ? workspaceOptions(view, r.project) : []}
						onChange={(workspace) => onPick({ workspace })}
						disabled={!r.project}
					/>
				)}
				<Picker
					label={translate("Agent")}
					value={r.account?.id ?? ""}
					options={r.device ? accountOptions(view, r.device, now) : []}
					onChange={(account) => onPick({ account })}
					disabled={!r.account}
				/>
				<Picker
					label={translate("Model")}
					value={r.shownModel ?? ""}
					options={modelOptions(view, r.account)}
					onChange={(model) => onPick({ model: model || null })}
				/>
				<Picker
					label={translate("Mode")}
					value={r.mode}
					options={modeOptions(r.account?.vendor, r.account?.acp)}
					onChange={(picked) => {
						// Auto asks the first time you pick it.
						const mode = picked as PermissionMode;
						pickMode(services, mode, () => onPick({ mode }));
					}}
				/>
			</div>
			{children}
			{folder ? (
				<PathField
					device={r.device}
					value={choices.path}
					onChange={(path) => onPick({ path })}
					label={translate("Folder")}
					title={translate("Choose the agent's folder")}
					placeholder={translate("/Users/you/code/project")}
					className={cn(AGENT_FIELD, "h-8")}
				/>
			) : null}
		</>
	);
};
