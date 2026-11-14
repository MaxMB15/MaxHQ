// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { agent } from "@maxhq/shared/test/build";
import { describe, expect, it, vi } from "vitest";

import {
	type AgentDropHandlers,
	createAgentDropRouter,
	draggedAgentsOf,
	draggedNoteOf,
} from "./agent-drop";

const handlers = (own = "a1") => ({
	accepts: vi.fn<AgentDropHandlers["accepts"]>(
		(dragged) => draggedAgentsOf(dragged)?.some((a) => a.id !== own) ?? false
	),
	onHover: vi.fn<AgentDropHandlers["onHover"]>(),
	onDrop: vi.fn<AgentDropHandlers["onDrop"]>(),
});

describe("the agent drop router", () => {
	const dragged = agent({ id: "a2", title: "Sidebar sort" });

	const setup = () => {
		const composer = document.createElement("div");
		const inner = document.createElement("p");
		composer.append(inner);
		const outside = document.createElement("div");
		document.body.append(composer, outside);
		let under: Element | null = inner;
		const router = createAgentDropRouter(() => under);
		const composerHandlers = handlers();
		router.register(composer, composerHandlers);
		return {
			router,
			composer,
			composerHandlers,
			over: (element: Element | null) => {
				under = element;
			},
			inner,
			outside,
		};
	};

	it("lights the composer under the pointer, through its children", () => {
		const { router, composerHandlers } = setup();
		expect(router.aim([dragged], { x: 10, y: 20 })).toBe(true);
		expect(composerHandlers.onHover).toHaveBeenCalledWith([dragged]);
		router.aim([dragged], { x: 12, y: 20 });
		expect(composerHandlers.onHover).toHaveBeenCalledTimes(1);
	});

	it("puts the light out when the drag moves off, and on Escape", () => {
		const { router, composerHandlers, over, inner, outside } = setup();
		router.aim([dragged], { x: 10, y: 20 });
		over(outside);
		expect(router.aim([dragged], { x: 400, y: 20 })).toBe(false);
		expect(composerHandlers.onHover).toHaveBeenLastCalledWith(null);

		over(inner);
		router.aim([dragged], { x: 10, y: 20 });
		router.cancel();
		expect(composerHandlers.onHover).toHaveBeenLastCalledWith(null);
	});

	it("drops the agent where the pointer let go", () => {
		const { router, composerHandlers } = setup();
		router.aim([dragged], { x: 10, y: 20 });
		expect(router.drop([dragged], { x: 11, y: 21 })).toBe(true);
		expect(composerHandlers.onDrop).toHaveBeenCalledWith([dragged], { x: 11, y: 21 });
		expect(composerHandlers.onHover).toHaveBeenLastCalledWith(null);
	});

	it("turns away an agent dragged onto its own composer", () => {
		const { router, composerHandlers } = setup();
		const self = agent({ id: "a1" });
		expect(router.aim([self], { x: 10, y: 20 })).toBe(false);
		expect(router.drop([self], { x: 10, y: 20 })).toBe(false);
		expect(composerHandlers.onHover).not.toHaveBeenCalled();
		expect(composerHandlers.onDrop).not.toHaveBeenCalled();
	});

	it("ignores a composer behind a dialog", () => {
		const { router, composer, composerHandlers } = setup();
		composer.setAttribute("inert", "");
		expect(router.aim([dragged], { x: 10, y: 20 })).toBe(false);
		expect(router.drop([dragged], { x: 10, y: 20 })).toBe(false);
		expect(composerHandlers.onDrop).not.toHaveBeenCalled();
	});

	it("stops sending drags once the composer goes", () => {
		const composer = document.createElement("div");
		const router = createAgentDropRouter(() => composer);
		const first = handlers();
		const undo = router.register(composer, first);
		router.aim([dragged], { x: 0, y: 0 });
		undo();
		expect(first.onHover).toHaveBeenLastCalledWith(null);
		expect(router.drop([dragged], { x: 0, y: 0 })).toBe(false);
		expect(first.onDrop).not.toHaveBeenCalled();
	});

	it("carries a note to a composer that takes one", () => {
		const composer = document.createElement("div");
		const router = createAgentDropRouter(() => composer);
		const note = {
			type: "note",
			project: "p1",
			note: "n1",
			title: "Release steps",
			copy: null,
		} as const;
		const takesNotes = handlers();
		takesNotes.accepts.mockImplementation((each) => draggedNoteOf(each) !== null);
		router.register(composer, takesNotes);
		expect(router.aim(note, { x: 10, y: 20 })).toBe(true);
		expect(takesNotes.onHover).toHaveBeenCalledWith(note);
		expect(router.drop(note, { x: 10, y: 20 })).toBe(true);
		expect(takesNotes.onDrop).toHaveBeenCalledWith(note, { x: 10, y: 20 });
		expect(draggedAgentsOf(note)).toBeNull();
		expect(draggedNoteOf([dragged])).toBeNull();
	});
});
