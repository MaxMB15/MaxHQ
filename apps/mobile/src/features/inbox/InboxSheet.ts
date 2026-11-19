// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheets the agent list opens, in the system's own sheet. A long press
// on a row opens the inbox menu for the agent, Rename, Copy and Delete;
// the selection's Actions opens the same menu for every agent picked, with
// Delete for the ones MaxHQ holds. Delete asks first, and what goes wrong
// shows in the list's notice.

import { formatNumber, t } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";

import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { deletableAgents, deleteAgents, deleteQuestion } from "@maxhq/shared/ui/delete-agents";
import { changeInbox, showFailure } from "@maxhq/shared/ui/inbox-notice";
import { agentCopies } from "@maxhq/shared/view/copy-agent";
import { inboxMenu, inboxTargets } from "@maxhq/shared/view/inbox-actions";
import { swipeMenu } from "@maxhq/shared/view/inbox-list";
import { inboxSheet, type InboxSheetEntry } from "@maxhq/shared/view/inbox-sheet";

import type { Services } from "../../services";
import { type Choice, choose, confirm } from "../../ui/choose";
import type { RenameDialog } from "../agent/more";
import { noticesOf } from "./notices";
import { pickSnooze } from "./SnoozeSheet";

const confirmDelete = (services: Services, agents: readonly Agent[]) => {
	const remove = async () => {
		const { failures } = await deleteAgents(services.source, agents);
		if (failures.length > 0) noticesOf(services).store.setState({ failures });
	};
	confirm(deleteQuestion(agents), t("Delete"), () => void remove());
};

/** Opens a sheet with what there is to copy for `agent`: its path, branch and session ID. */
const pickCopy = (services: Services, agent: Agent) => {
	const { view } = services.view.getState();
	choose(
		t("Copy"),
		agentCopies(view, agent).map((copy) => ({
			label: copy.label,
			pick: () =>
				void Clipboard.setStringAsync(copy.text).then(
					(copied) => {
						if (!copied) showFailure(noticesOf(services), t("Couldn't copy"));
					},
					() => showFailure(noticesOf(services), t("Couldn't copy"))
				),
		}))
	);
};

const changeChoice = (services: Services, entry: InboxSheetEntry): Choice | null => {
	const notices = noticesOf(services);
	switch (entry.kind) {
		case "change":
			return {
				label: entry.label,
				pick: () => void changeInbox(notices, entry.choice.steps),
			};
		case "snooze":
			return {
				label: entry.label,
				pick: () => pickSnooze(notices, entry.agents, entry.untilLimit),
			};
		default:
			return null;
	}
};

/** Opens the sheet for the agent, renaming it in `rename`. */
export const openInboxSheet = (services: Services, agent: Agent, rename: RenameDialog): void => {
	const { view } = services.view.getState();
	const deletable = deletableAgents(view, [agent.id]).length > 0;
	const copies = agentCopies(view, agent).length > 0;
	choose(
		agent.title,
		inboxSheet(swipeMenu(view, agent, Date.now()), deletable).flatMap((entry): Choice[] => {
			switch (entry.kind) {
				case "rename":
					return [
						{ label: entry.label, pick: () => rename.open(agent) },
						...(copies
							? [{ label: t("Copy"), pick: () => pickCopy(services, agent) }]
							: []),
					];
				case "delete":
					return [
						{
							label: entry.label,
							destructive: true,
							pick: () => confirmDelete(services, [agent]),
						},
					];
				default: {
					const choice = changeChoice(services, entry);
					return choice ? [choice] : [];
				}
			}
		})
	);
};

/**
 * Opens the selection's Actions: the inbox menu for every agent picked,
 * then Delete for the ones MaxHQ holds. The selection lets go of deleted
 * agents itself, once the view no longer has them.
 */
export const openSelectionSheet = (services: Services, ids: readonly AgentId[]): void => {
	const { view } = services.view.getState();
	const targets = inboxTargets(view, ids);
	const menu = targets.length > 0 ? inboxMenu(view, targets, Date.now()) : null;
	const removable = deletableAgents(view, ids);
	const several = ids.length > 1;
	const count = t("{{countText}} agents selected", {
		count: ids.length,
		countText: formatNumber(ids.length),
	});
	choose(
		count,
		inboxSheet(menu, removable.length > 0).flatMap((entry): Choice[] => {
			switch (entry.kind) {
				case "rename":
					return [];
				case "delete":
					return [
						{
							label: several
								? t("Delete ({{length}})", {
										length: formatNumber(removable.length),
									})
								: entry.label,
							destructive: true,
							pick: () => confirmDelete(services, removable),
						},
					];
				case "snooze":
					return [
						{
							label: several
								? t("Snooze ({{length}})", {
										length: formatNumber(entry.agents.length),
									})
								: entry.label,
							pick: () =>
								pickSnooze(noticesOf(services), entry.agents, entry.untilLimit),
						},
					];
				default: {
					const choice = changeChoice(services, entry);
					return choice ? [choice] : [];
				}
			}
		})
	);
};
