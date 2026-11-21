// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The More button in an agent's header. Its sheet opens find over the
// feed and the list of your prompts, renames the agent in a small dialog,
// picks its permission mode and model in sheets of their own and its effort
// level on a slider in a small dialog, as the desktop's composer does,
// links a pull request to it in a sheet, opens its details, its sub-agents
// and its ports and processes, copies its session ID and exports the
// conversation, revokes its permission to run agents, opens the Move sheet,
// and stops it or deletes one MaxHQ holds after asking. The screen goes back
// to the agents once it's gone, and a control that fails says why in an
// alert.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import * as Clipboard from "expo-clipboard";
import { useState } from "react";
import { Alert } from "react-native";

import { STOP_AGENT } from "@maxhq/shared/agent-view/controls";
import { agentDevice } from "@maxhq/shared/agent-view/move-agent";
import type { Agent, Permission } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { AUTO_QUESTION } from "@maxhq/shared/settings/auto-mode";
import { deleteAgents, deleteQuestion } from "@maxhq/shared/ui/delete-agents";

import type { Screens } from "../../navigation";
import { useLetsEach } from "../phone/everything";
import { LinkPullSheet } from "../pulls";
import { useServices } from "../../services";
import { choose, confirm } from "../../ui/choose";
import { IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { autoConsent } from "./auto-consent";
import type { ExportTo } from "./ExportConversation";
import {
	type CopyOut,
	copyOutSheet,
	createRenameDialog,
	MORE_LABEL,
	type MoreChoice,
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
import { type PermissionSource, permissions, revokeHeld, revokeQuestion } from "./permissions";
import { RenameDialog } from "./RenameDialog";
import { renamesOf, useShownTitle } from "./renames";

const show = (sheet: Sheet) => choose(sheet.title, sheet.choices, sheet.message);

/** Asks first, then revokes `held` and says why when it didn't. */
export const askRevoke = (source: PermissionSource, held: Permission): void =>
	confirm(
		revokeQuestion(held, Date.now()),
		t("Revoke"),
		() =>
			void revokeHeld(permissions, source, held).then((why) => {
				if (why) Alert.alert(why);
			})
	);

export const AgentMore = ({
	agent,
	held,
	onFind,
	onPrompts,
	onExport,
	onDeleted,
}: {
	agent: Agent;
	/** The permission to run agents it holds, which the sheet can revoke. */
	held: Permission | undefined;
	onFind: () => void;
	/** Opens the list of your prompts. */
	onPrompts: () => void;
	onExport: (to: ExportTo) => void;
	onDeleted: () => void;
}) => {
	const { t } = useI18n();

	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const services = useServices();
	const { source } = services;
	const [dialog] = useState(() => createRenameDialog(renamesOf(services)));
	const [effortOpen, setEffortOpen] = useState(false);
	const [linking, setLinking] = useState(false);
	const title = useShownTitle(agent);
	const lets = useLetsEach();

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
	const confirmRevoke = () => held && askRevoke(source, held);
	const copyOut = (now: Agent) => {
		const sheet = copyOutSheet(now, (what: CopyOut) => {
			if (what !== "session") return onExport(what);
			const id = now.vendor_session_id;
			if (id)
				void Clipboard.setStringAsync(id).then(
					() => Alert.alert(t("Copied"), id),
					() => Alert.alert(t("Couldn't copy"))
				);
		});
		choose(title, sheet.choices, sheet.message);
	};
	// The sheets read the view as it is when you tap, so they list what's there now.
	const open = () => {
		const view = services.view.getState().view;
		const now = view.tables.agent.get(agent.id) ?? agent;
		const device = agentDevice(view, now);
		const picks: Record<MoreChoice, () => void> = {
			find: onFind,
			prompts: onPrompts,
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
						),
						device !== null && lets(device)
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
			effort: () => setEffortOpen(true),
			link: () => setLinking(true),
			details: () => navigation.navigate("AgentInfo", { agent: now.id }),
			subAgents: () => navigation.navigate("SubAgents", { agent: now.id }),
			running: () => navigation.navigate("Running", { agent: now.id }),
			copyOut: () => copyOut(now),
			allowed: confirmRevoke,
			move: () => navigation.navigate("MoveAgent", { agent: now.id }),
			stop: confirmStop,
			delete: confirmDelete,
		};
		choose(
			title,
			moreChoices(view, now, held !== undefined).map((choice) => ({
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
			<LinkPullSheet agent={agent} open={linking} onClose={() => setLinking(false)} />
			<EffortDialog
				pick={effortOpen ? effortPick(services.view.getState().view, agent) : null}
				onPick={(level) =>
					run("change the effort level", () => source.setEffort(agent.id, level))
				}
				close={() => setEffortOpen(false)}
			/>
		</>
	);
};
