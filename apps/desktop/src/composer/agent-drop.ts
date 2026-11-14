// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Agents and notes dragged from the sidebar onto a composer. The sidebar's
// drag asks what's under the pointer as it moves, the composer there lights
// up like a file drop, and letting go adds each agent, or the note, as a pill
// at that composer's caret. Dragging a picked row carries every picked agent.
// The drag is the sidebar's own pointer drag, not a native one, so this
// routes it the way native/drop.ts routes file drops.

import type { Agent } from "@maxhq/shared/bindings";

import type { NoteRef } from "@maxhq/shared/pills/draft";

import type { Point } from "@/native/drop";

/** The agents one drag carries, the row you pressed first. */
export type DraggedAgents = readonly Agent[];

/** What one drag from the sidebar carries: agents, or a note. */
export type Dragged = DraggedAgents | NoteRef;

/** The agents a drag carries, or null when it carries a note. */
export const draggedAgentsOf = (dragged: Dragged): DraggedAgents | null =>
	"type" in dragged ? null : dragged;

/** The note a drag carries, or null when it carries agents. */
export const draggedNoteOf = (dragged: Dragged): NoteRef | null =>
	"type" in dragged ? dragged : null;

/** What a composer does with agents or a note dragged onto it. */
export type AgentDropHandlers = {
	/** Whether it takes what's dragged. A composer doesn't take its own agent. */
	readonly accepts: (dragged: Dragged) => boolean;
	/** Hears what's dragged when it moves over, and null when it moves off or ends. */
	readonly onHover: (dragged: Dragged | null) => void;
	/** Takes what's dragged, and where it landed. */
	readonly onDrop: (dragged: Dragged, at: Point) => void;
};

/** Routes the sidebar's drag of agents or a note to the composers it passes over. */
export type AgentDropRouter = {
	/** Starts sending drags over `element` to `handlers`. Returns the undo. */
	readonly register: (element: Element, handlers: AgentDropHandlers) => () => void;
	/**
	 * The drag moved. Returns whether a composer under the pointer takes
	 * what's dragged, so the drag lights nothing else.
	 */
	readonly aim: (dragged: Dragged, at: Point) => boolean;
	/** The drag let go. Returns whether a composer took what's dragged. */
	readonly drop: (dragged: Dragged, at: Point) => boolean;
	/** The drag ended without a drop, such as on Escape. */
	readonly cancel: () => void;
};

/**
 * A router that finds elements with `elementAt`. A drag over a child goes to
 * the nearest registered ancestor, and one over anything covering it, such
 * as a dialog, goes nowhere.
 */
export const createAgentDropRouter = (
	elementAt: (x: number, y: number) => Element | null
): AgentDropRouter => {
	const targets = new Map<Element, AgentDropHandlers>();
	let over: Element | null = null;

	const targetAt = (dragged: Dragged, { x, y }: Point): Element | null => {
		let element = elementAt(x, y);
		while (element !== null && !targets.has(element)) element = element.parentElement;
		if (element === null || element.closest("[inert]")) return null;
		return targets.get(element)?.accepts(dragged) ? element : null;
	};

	const hover = (next: Element | null, dragged: Dragged | null) => {
		if (next === over) return;
		const left = over;
		over = next;
		if (left !== null) targets.get(left)?.onHover(null);
		if (next !== null && dragged !== null) targets.get(next)?.onHover(dragged);
	};

	return {
		register: (element, handlers) => {
			targets.set(element, handlers);
			return () => {
				if (targets.get(element) !== handlers) return;
				if (over === element) hover(null, null);
				targets.delete(element);
			};
		},
		aim: (dragged, at) => {
			const target = targetAt(dragged, at);
			hover(target, dragged);
			return target !== null;
		},
		drop: (dragged, at) => {
			const target = targetAt(dragged, at);
			hover(null, null);
			if (target === null) return false;
			targets.get(target)?.onDrop(dragged, at);
			return true;
		},
		cancel: () => hover(null, null),
	};
};

/** The window's router, which the sidebar's drag and every composer share. */
export const agentDrops: AgentDropRouter = createAgentDropRouter((x, y) =>
	typeof document.elementFromPoint === "function" ? document.elementFromPoint(x, y) : null
);
