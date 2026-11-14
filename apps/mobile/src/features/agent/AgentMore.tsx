// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The More button in an agent's header. Its sheet opens find over the
// feed, renames the agent in a small dialog, picks its permission mode and
// model in sheets of their own and its effort level in a small dialog, as
// the desktop's composer does, and stops it or deletes one MaxHQ holds after
// asking. The screen goes back to the agents once it's gone,
// and a control that fails says why in an alert.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Alert } from "react-native";

import { STOP_AGENT } from "@maxhq/shared/agent-view/controls";
import type { Agent } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { AUTO_QUESTION } from "@maxhq/shared/settings/auto-mode";
import { deleteAgents, deleteQuestion } from "@maxhq/shared/ui/delete-agents";

import { useServices } from "../../services";
import { choose, confirm } from "../../ui/choose";
import { IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { autoConsent } from "./auto-consent";
import {
	createRenameDialog,
	MORE_LABEL,
	type MoreChoice,
	type EffortPick,
	effortPick,
	modelSheet,
	moreChoices,
	permissionSheet,
	pickPermission,
	runControl,
	type Sheet,
	stopQuestion,
} from "./more";
import { EffortDialog } from "./EffortDialog";
import { RenameDialog } from "./RenameDialog";
import { renamesOf, useShownTitle } from "./renames";

const show = (sheet: Sheet) => choose(sheet.title, sheet.choices, sheet.message);

export const AgentMore = ({
	agent,
	onFind,
	onDeleted,
}: {
	agent: Agent;
	onFind: () => void;
	onDeleted: () => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const [dialog] = useState(() => createRenameDialog(renamesOf(services)));
	const [effort, setEffort] = useState<EffortPick | null>(null);
	const title = useShownTitle(agent);

	const run = (what: string, action: () => Promise<ActionResult>) =>
		void runControl(what, action).then((why) => {
			if (why) Alert.alert(why);
		});
	const remove = async () => {
		const { deleted, failures } = await deleteAgents(source, [agent]);
		if (deleted.has(agent.id)) onDeleted();
		else if (failures.length > 0) Alert.alert(failures.join(" "));
	};
	const confirmDelete = () => confirm(deleteQuestion([agent]), t("Delete"), () => void remove());
	const confirmStop = () =>
		confirm(stopQuestion(title), STOP_AGENT.label, () =>
			run("stop the agent", () => source.stopAgent(agent.id))
		);
	const askAuto = (yes: () => void) => confirm(AUTO_QUESTION, AUTO_QUESTION.action, yes);
	// The sheets read the view as it is when you tap, so they list what's there now.
	const open = () => {
		const view = services.view.getState().view;
		const now = view.tables.agent.get(agent.id) ?? agent;
		const picks: Record<MoreChoice, () => void> = {
			find: onFind,
			rename: () => dialog.open(now),
			permission: () =>
				now.permission &&
				show(
					permissionSheet(
						view,
						now,
						now.permission,
						pickPermission(autoConsent, askAuto, (mode) =>
							run("change the permission mode", () =>
								source.setPermission(now.id, mode)
							)
						)
					)
				),
			model: () =>
				show(
					modelSheet(
						view,
						now,
						(model, to) =>
							run("change the model", () => source.setModel(now.id, model, to)),
						show
					)
				),
			effort: () => setEffort(effortPick(view, now)),
			stop: confirmStop,
			delete: confirmDelete,
		};
		choose(
			title,
			moreChoices(view, now).map((choice) => ({
				label: t(MORE_LABEL[choice]),
				destructive: choice === "delete",
				pick: picks[choice],
			}))
		);
	};

	return (
		<>
			<IconButton label={t("More")} onPress={open}>
				<More />
			</IconButton>
			<RenameDialog dialog={dialog} />
			<EffortDialog
				pick={effort}
				onPick={(level) =>
					run("change the effort level", () => source.setEffort(agent.id, level))
				}
				close={() => setEffort(null)}
			/>
		</>
	);
};
