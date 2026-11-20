// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Quoting the conversation. A selection in one row becomes a quote pill:
// Add to chat over it puts the pill in this agent's composer, the right
// click menu also offers the other open agents, and dragging it carries
// the pill to any composer or to an agent in the sidebar.

import { useCallback, useEffect } from "react";

import { type Services, useServices } from "@/app/services";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { type MenuEntry, useComposerNative } from "@/composer/native";
import { paneAgent } from "@/layout/pane-id";
import { panesOf } from "@/layout/panes";
import { onText } from "@/native/context-guard";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { makeQuote, QUOTE_MIME } from "@maxhq/shared/pills/quote";
import type { TimelineStore } from "@maxhq/shared/timeline/store";

import { provideJumpServices } from "./jump";
import { QuoteToolbar, type SelectedQuote } from "./quote-toolbar";
import { captureRowSelection } from "./selection";
import "./quotes.css";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// What drags itself, such as a picture, which the timeline's file drag takes.
const OWN_DRAG = "img, video, [draggable=true]";

/** The agents in the window's other panes that take messages, by pane. */
export const otherAgents = (
	services: Services,
	self: AgentId
): { readonly id: AgentId; readonly title: string }[] => {
	const agents = services.view.getState().view.tables.agent;
	return (panesOf(services).api()?.panels ?? []).flatMap((panel) => {
		const id = paneAgent(panel.id);
		const agent = id === null ? undefined : agents.get(id);
		return agent && agent.id !== self && agent.control !== "observed"
			? [{ id: agent.id, title: agent.title }]
			: [];
	});
};

/** Adds a quote to an agent's composer and puts the selection away. */
const send = (agent: AgentId, quote: QuoteRef, focus: boolean) => {
	quoteInbox.send(agent, quote, focus);
	window.getSelection()?.removeAllRanges();
};

/**
 * Quoting in the conversation in `viewport`: the Add to chat button over a
 * selection, the menu on it and dragging it. An agent you only watch
 * offers no Add to chat, since it has no composer.
 */
export const TimelineQuotes = ({
	agent,
	viewport,
	store,
}: {
	agent: Agent;
	viewport: HTMLElement | null;
	store: TimelineStore;
}) => {
	useI18n();
	const services = useServices();
	const native = useComposerNative();
	const own = agent.control !== "observed";

	useEffect(() => provideJumpServices(services), [services]);

	const quoteOf = useCallback(
		(selection: Selection | null): SelectedQuote | null => {
			if (!viewport) return null;
			const picked = captureRowSelection(viewport, selection);
			const item = picked ? store.getState().items.get(picked.item) : undefined;
			if (!picked || !item) return null;
			return {
				quote: makeQuote({ id: agent.id, title: agent.title }, item, picked.anchor),
				range: picked.range,
			};
		},
		[viewport, store, agent.id, agent.title]
	);

	useEffect(() => {
		if (!viewport) return;
		const onContextMenu = (event: MouseEvent) => {
			const selected = quoteOf(window.getSelection());
			if (!selected || !onText(selected.range, event.clientX, event.clientY)) return;
			const { quote } = selected;
			const addTo = (other: AgentId, text: string, focus: boolean): MenuEntry => ({
				kind: "item",
				text,
				action: () => send(other, quote, focus),
			});
			const others = otherAgents(services, agent.id);
			const entries: MenuEntry[] = [
				...(own ? [addTo(agent.id, translate("Add to chat"), true)] : []),
				{
					kind: "submenu",
					text: translate("Add to chat in"),
					entries: others.map((other) => addTo(other.id, other.title, false)),
				},
				{ kind: "separator" },
				{ kind: "copy", text: window.getSelection()?.toString() || quote.text },
			];
			if (native.contextMenu?.(entries, { x: event.clientX, y: event.clientY }))
				event.preventDefault();
		};
		// The drop router notes what the drag carries once it reaches the
		// document, so it goes on from here.
		const onDragStart = (event: DragEvent) => {
			const node = event.target as Node | null;
			const element = node instanceof Element ? node : (node?.parentElement ?? null);
			if (!node || !event.dataTransfer || element?.closest(OWN_DRAG)) return;
			const selected = quoteOf(window.getSelection());
			if (!selected || !selected.range.intersectsNode(node)) return;
			event.dataTransfer.setData(QUOTE_MIME, JSON.stringify(selected.quote));
			event.dataTransfer.setData("text/plain", selected.quote.text);
			event.dataTransfer.effectAllowed = "copy";
		};
		viewport.addEventListener("contextmenu", onContextMenu);
		viewport.addEventListener("dragstart", onDragStart);
		return () => {
			viewport.removeEventListener("contextmenu", onContextMenu);
			viewport.removeEventListener("dragstart", onDragStart);
		};
	}, [viewport, quoteOf, own, agent.id, services, native]);

	return own ? (
		<QuoteToolbar
			viewport={viewport}
			quoteOf={quoteOf}
			onAdd={(quote) => quoteInbox.send(agent.id, quote, true)}
		/>
	) : null;
};
