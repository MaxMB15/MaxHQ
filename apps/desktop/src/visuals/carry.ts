// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual on its way to a composer: Add to chat and Add to chat in, the
// card's Copy, and a drag from the card's title row. Each carries the page
// itself, so the composer that takes it stages the page as a file. Copy
// and a drag write three types: the fenced block as plain text, the
// draft's own type with its HTML fallback, and the visual's type.

import { t } from "@maxhq/shared/i18n";

import { MessageSquarePlus, MessagesSquare } from "lucide-react";

import type { Services } from "@/app/services";
import { otherAgents } from "@/agent-view/quotes";
import { writeDraftClipboard } from "@/composer/pill-keys";
import { quoteInbox } from "@/composer/quote-inbox";
import { paneAgent } from "@/layout/pane-id";
import { panesOf } from "@/layout/panes";
import type { AgentId } from "@maxhq/shared/bindings";
import { encodeFragment } from "@maxhq/shared/pills/clipboard";
import { visualFragment, visualFrom } from "@maxhq/shared/pills/visual";
import { deviceName } from "@maxhq/shared/view/select";
import type { ViewState } from "@maxhq/shared/view/replica";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import {
	VISUAL_MIME,
	type VisualTransfer,
	writeVisualTransfer,
} from "@maxhq/shared/visuals/transfer";

import type { VisualAction } from "./actions";

/** Where a visual from an agent's reply came from: the agent, and its device's name. */
export const agentFrom = (view: ViewState, id: AgentId): string | null => {
	const agent = view.tables.agent.get(id);
	if (!agent) return null;
	const device = view.tables.workspace.get(agent.workspace)?.device;
	return visualFrom(agent.title, device === undefined ? null : deviceName(view, device));
};

/** The agent in the window's focused pane, when it takes messages. */
export const focusedAgent = (services: Services): AgentId | null => {
	const panel = panesOf(services).api()?.activePanel;
	const id = panel ? paneAgent(panel.id) : null;
	const agent = id === null ? undefined : services.view.getState().view.tables.agent.get(id);
	return agent && agent.control !== "observed" ? agent.id : null;
};

/**
 * Add to chat, into the composer of the agent whose reply holds the visual
 * or else the focused pane's, and Add to chat in, listing the other panes'
 * agents when the menu opens.
 */
export const chatActions = (
	services: Services,
	visual: VisualTransfer,
	origin: AgentId | null
): VisualAction[] => {
	const own = () => origin ?? focusedAgent(services);
	const self = own();
	return [
		...(self === null
			? []
			: [
					{
						get label() {
							return t("Add to chat");
						},
						icon: MessageSquarePlus,
						run: () => {
							const agent = own();
							if (agent !== null) quoteInbox.sendVisual(agent, visual, true);
						},
					},
				]),
		{
			get label() {
				return t("Add to chat in");
			},
			icon: MessagesSquare,
			run: () => {},
			entries: () =>
				otherAgents(services, own() ?? ("" as AgentId)).map((other) => ({
					label: other.title,
					icon: MessageSquarePlus,
					run: () => quoteInbox.sendVisual(other.id, visual, false),
				})),
		},
	];
};

/** Writes the visual's three types to a clipboard or a drag. */
export const writeVisual = (data: Pick<DataTransfer, "setData">, visual: VisualTransfer): void => {
	writeDraftClipboard(data, toVisualFence(visual), encodeFragment(visualFragment(visual)));
	data.setData(VISUAL_MIME, writeVisualTransfer(visual));
};

/**
 * Puts the visual on the clipboard with all three types. The clipboard API
 * takes only plain text and HTML, so this copies through a copy event, and
 * falls back to the fenced block alone where that's refused.
 */
export const copyVisual = async (visual: VisualTransfer): Promise<void> => {
	let wrote = false;
	const onCopy = (event: ClipboardEvent) => {
		if (!event.clipboardData) return;
		event.preventDefault();
		writeVisual(event.clipboardData, visual);
		wrote = true;
	};
	document.addEventListener("copy", onCopy, { once: true, capture: true });
	try {
		document.execCommand("copy");
	} catch {
		// Refused, which the fallback covers.
	} finally {
		document.removeEventListener("copy", onCopy, { capture: true });
	}
	if (!wrote) await navigator.clipboard.writeText(toVisualFence(visual));
};

/** Starts a drag of the visual from its card's title row. */
export const dragVisual = (
	event: { readonly dataTransfer: DataTransfer | null },
	visual: VisualTransfer
): void => {
	const data = event.dataTransfer;
	if (!data) return;
	writeVisual(data, visual);
	data.effectAllowed = "copy";
};
