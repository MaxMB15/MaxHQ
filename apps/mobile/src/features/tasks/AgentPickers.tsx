// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pickers that say where a task's new agent runs and as what, as the
// desktop's: the project or a local folder, the workspace or device, the
// login, model and mode, and the folder's field when it runs in one. Auto
// asks the first time you pick it, as it does everywhere on the phone.

import { useI18n } from "@maxhq/shared/i18n";

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
import { AUTO_QUESTION, whenModeAllowed } from "@maxhq/shared/settings/auto-mode";
import type { ViewState } from "@maxhq/shared/view/replica";

import { confirm } from "../../ui/choose";
import { autoConsent } from "../agent/auto-consent";
import { TextField } from "./fields";
import { Picker } from "./Picker";

// The place picker's entry for a folder, which no project ID can match.
const FOLDER = "*folder";

const askAuto = (yes: () => void) => confirm(AUTO_QUESTION, AUTO_QUESTION.action, yes);

/** The pickers for a new agent, with the folder's field when it runs in one. */
export const AgentPickers = ({
	view,
	choices,
	resolved: r,
	now,
	onPick,
}: {
	view: ViewState;
	choices: Choices;
	resolved: Resolved;
	now: number;
	onPick: (patch: Partial<Choices>) => void;
}) => {
	const { t } = useI18n();
	const folder = r.place === "folder";
	const places: Option[] = [
		...projectOptions(view).map((o) => ({ ...o, group: t("Projects") })),
		{ value: FOLDER, label: t("Use a local folder"), detail: t("On any connected device") },
	];
	return (
		<>
			<Picker
				label={t("Project")}
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
					label={t("Device")}
					value={r.device ?? ""}
					options={deviceOptions(view)}
					onChange={(device) => onPick({ device })}
				/>
			) : (
				<Picker
					label={t("Runs on")}
					value={r.workspace?.id ?? ""}
					options={r.project ? workspaceOptions(view, r.project) : []}
					onChange={(workspace) => onPick({ workspace })}
					disabled={!r.project}
				/>
			)}
			<Picker
				label={t("Agent")}
				value={r.account?.id ?? ""}
				options={r.device ? accountOptions(view, r.device, now) : []}
				onChange={(account) => onPick({ account })}
				disabled={!r.account}
			/>
			<Picker
				label={t("Model")}
				value={r.shownModel ?? ""}
				options={modelOptions(view, r.account)}
				onChange={(model) => onPick({ model: model || null })}
			/>
			<Picker
				label={t("Mode")}
				value={r.mode}
				options={modeOptions(r.account?.vendor, r.account?.acp)}
				onChange={(picked) => {
					const mode = picked as PermissionMode;
					whenModeAllowed(mode, autoConsent, askAuto, () => onPick({ mode }));
				}}
			/>
			{folder ? (
				<TextField
					label={t("Folder")}
					value={choices.path}
					onChange={(path) => onPick({ path })}
					placeholder={t("/Users/you/code/project")}
					mono
				/>
			) : null}
		</>
	);
};
