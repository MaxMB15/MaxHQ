// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Dragging an agent from the sidebar into the panes. A press that moves a few
// pixels turns into a drag: the agent's name follows the pointer, the spot it
// would land on lights up the way dockview's own drags do, and letting go
// there opens the agent's pane at that spot. A pinned agent dragged over
// the pins moves among them instead, with a line where it would land, and
// one dragged onto another agent's composer goes into its message as a pill.
// A picked row carries every picked agent to a composer, and the name that
// follows the pointer says how many more. A note's row drags the same way:
// onto a composer it goes in as a pill, and onto the panes it opens there.
// Escape, or letting go anywhere else, drops nothing, and the click that
// ends a drag doesn't open the agent.

import { t } from "@maxhq/shared/i18n";

import { openAgent } from "@/app/open-agent";
import type { Services } from "@/app/services";
import { agentDrops, type Dragged } from "@/composer/agent-drop";
import { type DropTarget, targetAt } from "@/layout/drop";
import { type DropSpot, panesOf } from "@/layout/panes";
import { openNote } from "@/notes/open";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import type { NoteRef } from "@maxhq/shared/pills/draft";
import { draggedAgents } from "@maxhq/shared/ui/rows";
import { pinMoveSteps } from "@maxhq/shared/view/inbox-actions";

import { runSteps } from "./commands";

// How far the pointer moves before a press becomes a drag, in pixels.
const THRESHOLD = 4;

const GHOST =
	"pointer-events-none fixed z-[9999] max-w-56 truncate rounded-md border border-input bg-popover px-2 py-1 text-xs text-foreground-strong shadow-lg";

// The same colors dockview's drop targets take from the pane theme.
const SPOT =
	"pointer-events-none absolute z-[9999] box-border border border-(--dv-drag-over-border-color) bg-(--dv-drag-over-background-color) transition-[left,top,width,height] duration-75 ease-out";

const LINE = "pointer-events-none fixed z-[9999] h-[2px] rounded-full bg-primary";

const COUNT =
	"ml-1.5 inline-flex h-4 items-center rounded-full bg-primary px-1.5 text-[10.5px] font-semibold text-primary-foreground";

// The name that follows the pointer, with how many more come along.
const ghostOf = (name: string, more: number): HTMLElement => {
	const ghost = document.createElement("div");
	ghost.className = GHOST;
	ghost.setAttribute("aria-hidden", "true");
	ghost.textContent = name;
	if (more > 0) {
		const count = document.createElement("span");
		count.className = COUNT;
		count.textContent = `+${more}`;
		ghost.append(count);
	}
	return ghost;
};

// The gap among the pins a pinned agent would land in, and the line's place.
type PinSlot = {
	readonly shown: readonly AgentId[];
	readonly slot: number;
	readonly top: number;
	readonly left: number;
	readonly width: number;
};

// The gap among the pinned rows nearest the pointer, while it's over them.
const pinSlotAt = (x: number, y: number): PinSlot | null => {
	const rows = [...document.querySelectorAll<HTMLElement>("[data-pin-row]")];
	const first = rows[0]?.getBoundingClientRect();
	const last = rows.at(-1)?.getBoundingClientRect();
	if (!first || !last || x < first.left || x > first.right) return null;
	if (y < first.top - first.height / 2 || y > last.bottom + last.height / 2) return null;
	const boxes = rows.map((row) => row.getBoundingClientRect());
	const at = boxes.findIndex((box) => y < box.top + box.height / 2);
	const slot = at === -1 ? rows.length : at;
	return {
		shown: rows.map((row) => row.dataset.pinRow!),
		slot,
		top: (slot < boxes.length ? boxes[slot]!.top : last.bottom) - 1,
		left: first.left,
		width: first.width,
	};
};

// The grid the panes sit in, unless the devices view hides it.
const gridRoot = (): HTMLElement | null => {
	const root = document.querySelector<HTMLElement>(".dv-dockview");
	return root && !root.closest("[inert]") ? root : null;
};

// Lights up where a drop would land, or nothing when `target` is null.
const light = (spot: HTMLElement, root: HTMLElement | null, target: DropTarget | null): void => {
	if (!root || !target) {
		spot.remove();
		return;
	}
	if (spot.parentElement !== root) root.append(spot);
	const area = root.getBoundingClientRect();
	const { box } = target;
	spot.style.left = `${box.left - area.left}px`;
	spot.style.top = `${box.top - area.top}px`;
	spot.style.width = `${box.width}px`;
	spot.style.height = `${box.height}px`;
};

// Stops the click that follows letting go, which would open the agent again.
const swallowClick = (): void => {
	const swallow = (e: MouseEvent) => {
		e.preventDefault();
		e.stopPropagation();
	};
	window.addEventListener("click", swallow, { capture: true, once: true });
	setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
};

// What a row's drag carries and what letting it go outside a composer does.
type RowDrag = {
	/** What a composer would take. */
	readonly carried: Dragged;
	/** The name that follows the pointer, and how many more come along. */
	readonly name: string;
	readonly more: number;
	/** Opens the row's pane at a spot in the panes. */
	readonly open: (at: DropSpot) => void;
	/** Moves the row to a slot among the pins, for a pinned row. */
	readonly moveTo?: (shown: readonly AgentId[], slot: number) => void;
};

/**
 * Follows a press on an agent's row, which turns into a drag once the
 * pointer moves. A `pinned` row can move among the pins too.
 */
export const pressAgent = (
	services: Services,
	agent: Agent,
	down: PointerEvent,
	pinned = false
): void => {
	// The agents a composer would take: the picked ones when this row is one.
	const carried = draggedAgents(
		services.view.getState().view,
		agent,
		services.ui.getState().picked
	);
	pressRow(services, down, {
		carried,
		name: agent.title,
		more: carried.length - 1,
		open: (at) => openAgent(services, agent.id, { at }),
		...(pinned
			? {
					moveTo: (shown: readonly AgentId[], slot: number) => {
						const steps = pinMoveSteps(
							services.view.getState().view,
							shown,
							agent.id,
							slot
						);
						if (steps.length > 0)
							runSteps(services, steps, () =>
								t("Moved {{title}}", { title: agent.title })
							);
					},
				}
			: {}),
	});
};

/**
 * Follows a press on a note's row, which turns into a drag once the
 * pointer moves. `name` is the title the row shows.
 */
export const pressNote = (
	services: Services,
	note: NoteRef,
	name: string,
	down: PointerEvent
): void =>
	pressRow(services, down, {
		carried: note,
		name,
		more: 0,
		open: (at) => openNote(services, note.project, note.note, { at }),
	});

const pressRow = (services: Services, down: PointerEvent, drag: RowDrag): void => {
	if (down.button !== 0 || down.pointerType === "touch") return;
	const { carried, moveTo } = drag;
	let state: "pressed" | "dragging" | "cancelled" = "pressed";
	let target: DropTarget | null = null;
	let pin: PinSlot | null = null;
	const line = document.createElement("div");
	line.className = LINE;
	line.setAttribute("aria-hidden", "true");
	const ghost = ghostOf(drag.name, drag.more);
	const spot = document.createElement("div");
	spot.className = SPOT;
	spot.dataset.dropSpot = "";
	const body = document.body.style;
	const before = { cursor: body.cursor, userSelect: body.userSelect };

	const hide = () => {
		ghost.remove();
		spot.remove();
		line.remove();
		body.cursor = before.cursor;
		body.userSelect = before.userSelect;
	};
	const aim = (x: number, y: number) => {
		ghost.style.left = `${x + 12}px`;
		ghost.style.top = `${y + 8}px`;
		// A composer under the pointer takes the drop before the pane it sits in.
		const composer = agentDrops.aim(carried, { x, y });
		pin = moveTo && !composer ? pinSlotAt(x, y) : null;
		if (pin) {
			line.style.top = `${pin.top}px`;
			line.style.left = `${pin.left}px`;
			line.style.width = `${pin.width}px`;
			if (!line.isConnected) document.body.append(line);
		} else {
			line.remove();
		}
		const api = pin || composer ? null : panesOf(services).api();
		const root = gridRoot();
		target = api && root ? targetAt(api, root, x, y) : null;
		light(spot, root, target);
	};
	const move = (e: PointerEvent) => {
		if (e.pointerId !== down.pointerId || state === "cancelled") return;
		if (state === "pressed") {
			if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < THRESHOLD) return;
			state = "dragging";
			window.getSelection()?.removeAllRanges();
			body.cursor = "grabbing";
			body.userSelect = "none";
			document.body.append(ghost);
		}
		aim(e.clientX, e.clientY);
	};
	// Puts the row where the drag let go: into a composer, among the pins or in a pane.
	const land = (x: number, y: number) => {
		if (agentDrops.drop(carried, { x, y })) return;
		if (pin) moveTo?.(pin.shown, pin.slot);
		else if (target) drag.open(target.spot);
	};
	const up = (e: PointerEvent) => {
		if (e.pointerId !== down.pointerId) return;
		if (state === "dragging") land(e.clientX, e.clientY);
		if (state !== "pressed") swallowClick();
		end();
	};
	const cancel = (e: PointerEvent) => {
		if (e.pointerId === down.pointerId) end();
	};
	const key = (e: KeyboardEvent) => {
		if (e.key !== "Escape" || state !== "dragging") return;
		e.preventDefault();
		e.stopPropagation();
		state = "cancelled";
		target = null;
		pin = null;
		agentDrops.cancel();
		hide();
	};
	const end = () => {
		window.removeEventListener("pointermove", move, { capture: true });
		window.removeEventListener("pointerup", up, { capture: true });
		window.removeEventListener("pointercancel", cancel, { capture: true });
		window.removeEventListener("keydown", key, { capture: true });
		window.removeEventListener("blur", end);
		agentDrops.cancel();
		hide();
	};

	// Before the panes see them, so a pane that stops the events can't lose the drag.
	window.addEventListener("pointermove", move, { capture: true });
	window.addEventListener("pointerup", up, { capture: true });
	window.addEventListener("pointercancel", cancel, { capture: true });
	window.addEventListener("keydown", key, { capture: true });
	window.addEventListener("blur", end);
};
