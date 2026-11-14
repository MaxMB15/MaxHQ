// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheet a long press on an inbox row opens, in the system's own sheet:
// the inbox menu for the agent, Rename and Delete. Delete asks first, and
// what goes wrong shows in the inbox's notice.

import { t } from "@maxhq/shared/i18n";

import type { Agent } from "@maxhq/shared/bindings";
import { deletableAgents, deleteAgents, deleteQuestion } from "@maxhq/shared/ui/delete-agents";
import { changeInbox } from "@maxhq/shared/ui/inbox-notice";
import { swipeMenu } from "@maxhq/shared/view/inbox-list";
import { inboxSheet, type InboxSheetEntry } from "@maxhq/shared/view/inbox-sheet";

import type { Services } from "../../services";
import { choose, confirm } from "../../ui/choose";
import type { RenameDialog } from "../agent/more";
import { noticesOf } from "./notices";
import { pickSnooze } from "./SnoozeSheet";

const confirmDelete = (services: Services, agent: Agent) => {
	const remove = async () => {
		const { failures } = await deleteAgents(services.source, [agent]);
		if (failures.length > 0) noticesOf(services).store.setState({ failures });
	};
	confirm(deleteQuestion([agent]), t("Delete"), () => void remove());
};

/** Opens the sheet for the agent, renaming it in `rename`. */
export const openInboxSheet = (services: Services, agent: Agent, rename: RenameDialog): void => {
	const { view } = services.view.getState();
	const notices = noticesOf(services);
	const deletable = deletableAgents(view, [agent.id]).length > 0;
	const pick = (entry: InboxSheetEntry) => {
		switch (entry.kind) {
			case "change":
				return void changeInbox(notices, entry.choice.steps);
			case "snooze":
				return pickSnooze(notices, entry.agents, entry.untilLimit);
			case "rename":
				return rename.open(agent);
			case "delete":
				return confirmDelete(services, agent);
		}
	};
	choose(
		agent.title,
		inboxSheet(swipeMenu(view, agent, Date.now()), deletable).map((entry) => ({
			label: entry.label,
			destructive: entry.kind === "delete",
			pick: () => pick(entry),
		}))
	);
};
