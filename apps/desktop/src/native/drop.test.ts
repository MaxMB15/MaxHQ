// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { act, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import {
	claimPageDrop,
	createDropRouter,
	DROP_OVER_ATTRIBUTE,
	type PageDrag,
	registerDropTarget,
	toCssPoint,
	useDropTarget,
} from "./drop";

/** A drag from the page in the air, and a way to end it. */
const pageDrags = (carried: PageDrag | null) => {
	let drag = carried;
	let ended = () => {};
	return {
		current: () => drag,
		take: vi.fn(() => {
			const taken = drag;
			drag = null;
			return taken;
		}),
		onEnd: (listener: () => void) => {
			ended = listener;
			return () => {};
		},
		end: () => ended(),
	};
};

const PICTURE: PageDrag = new Map([["application/x-maxhq-file", "{}"]]);

/** A page with two composers side by side and a dialog over the second. */
const page = () => {
	const left = document.createElement("div");
	const field = document.createElement("textarea");
	left.append(field);
	const right = document.createElement("div");
	const dialog = document.createElement("div");
	const at = (x: number): Element | null => {
		if (x < 100) return field;
		if (x < 200) return right;
		if (x < 300) return dialog;
		return null;
	};
	return { left, field, right, dialog, at };
};

describe("createDropRouter", () => {
	it("sends a drop only to the target under it", () => {
		const { left, right, at } = page();
		const router = createDropRouter((x) => at(x));
		const onLeft = vi.fn();
		const onRight = vi.fn();
		router.register(left, { onDrop: onLeft });
		router.register(right, { onDrop: onRight });

		router.handle({ type: "drop", paths: ["/a.png"], position: { x: 150, y: 10 } });
		expect(onRight).toHaveBeenCalledWith(["/a.png"], { x: 150, y: 10 });
		expect(onLeft).not.toHaveBeenCalled();
	});

	it("finds the nearest registered ancestor", () => {
		const { left, at } = page();
		const router = createDropRouter((x) => at(x));
		const onDrop = vi.fn();
		router.register(left, { onDrop });
		router.handle({ type: "drop", paths: ["/a"], position: { x: 50, y: 10 } });
		expect(onDrop).toHaveBeenCalledWith(["/a"], { x: 50, y: 10 });
	});

	it("drops nothing on an element covering the target", () => {
		const { left, right, at } = page();
		const router = createDropRouter((x) => at(x));
		const onDrop = vi.fn();
		router.register(left, { onDrop });
		router.register(right, { onDrop });
		router.handle({ type: "drop", paths: ["/a"], position: { x: 250, y: 10 } });
		router.handle({ type: "drop", paths: ["/a"], position: { x: 900, y: 10 } });
		expect(onDrop).not.toHaveBeenCalled();
	});

	it("marks the hovered target and moves the mark with the drag", () => {
		const { left, right, at } = page();
		const router = createDropRouter((x) => at(x));
		const hovers: string[] = [];
		router.register(left, { onDrop: vi.fn(), onHover: (over) => hovers.push(`left ${over}`) });
		router.register(right, {
			onDrop: vi.fn(),
			onHover: (over) => hovers.push(`right ${over}`),
		});

		router.handle({ type: "enter", paths: ["/a"], position: { x: 50, y: 10 } });
		router.handle({ type: "over", position: { x: 60, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		router.handle({ type: "over", position: { x: 150, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		expect(right.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		router.handle({ type: "drop", paths: ["/a"], position: { x: 150, y: 10 } });
		expect(right.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		expect(hovers).toEqual(["left true", "left false", "right true", "right false"]);
	});

	it("clears the hover when the drag leaves", () => {
		const { left, at } = page();
		const router = createDropRouter((x) => at(x));
		const onHover = vi.fn();
		router.register(left, { onDrop: vi.fn(), onHover });
		router.handle({ type: "enter", paths: ["/a"], position: { x: 50, y: 10 } });
		router.handle({ type: "leave" });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		expect(onHover.mock.calls).toEqual([[true], [false]]);
	});

	it("stops sending once undone, and clears a hover on the way out", () => {
		const { left, at } = page();
		const router = createDropRouter((x) => at(x));
		const onDrop = vi.fn();
		const onHover = vi.fn();
		const undo = router.register(left, { onDrop, onHover });
		router.handle({ type: "over", position: { x: 50, y: 10 } });
		undo();
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		expect(onHover.mock.calls).toEqual([[true], [false]]);
		router.handle({ type: "drop", paths: ["/a"], position: { x: 50, y: 10 } });
		expect(onDrop).not.toHaveBeenCalled();
	});

	it("lights nothing and drops nothing for a drag with no paths", () => {
		const { left, at } = page();
		const router = createDropRouter((x) => at(x));
		const onDrop = vi.fn();
		const onHover = vi.fn();
		router.register(left, { onDrop, onHover });
		router.handle({ type: "enter", paths: [], position: { x: 50, y: 10 } });
		router.handle({ type: "over", position: { x: 60, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		router.handle({ type: "drop", paths: [], position: { x: 60, y: 10 } });
		expect(onDrop).not.toHaveBeenCalled();
		expect(onHover).not.toHaveBeenCalled();
	});

	it("hands a drag from the page only to where a target takes it", () => {
		const { left, right, at } = page();
		const drags = pageDrags(PICTURE);
		const router = createDropRouter((x) => at(x), drags);
		const onDrop = vi.fn();
		const onPageDrop = vi.fn();
		router.register(left, {
			onDrop,
			onPageDrop,
			takesPageDrag: (drag, point) => drag === PICTURE && point.x < 80,
		});
		router.register(right, { onDrop });

		router.handle({ type: "enter", paths: [], position: { x: 50, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		router.handle({ type: "over", position: { x: 90, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		router.handle({ type: "over", position: { x: 150, y: 10 } });
		expect(right.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		router.handle({ type: "over", position: { x: 50, y: 10 } });
		router.handle({ type: "drop", paths: [], position: { x: 50, y: 10 } });

		expect(onPageDrop).toHaveBeenCalledExactlyOnceWith(PICTURE, { x: 50, y: 10 });
		expect(onDrop).not.toHaveBeenCalled();
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		expect(drags.current()).toBeNull();
	});

	it("uses up a drag from the page that lands where nothing takes it", () => {
		const { left, at } = page();
		const drags = pageDrags(PICTURE);
		const router = createDropRouter((x) => at(x), drags);
		const onPageDrop = vi.fn();
		router.register(left, { onDrop: vi.fn(), onPageDrop, takesPageDrag: () => true });
		router.handle({ type: "drop", paths: [], position: { x: 150, y: 10 } });
		router.handle({ type: "drop", paths: [], position: { x: 50, y: 10 } });
		expect(onPageDrop).not.toHaveBeenCalled();
		expect(drags.take).toHaveBeenCalledTimes(2);
	});

	it("stops lighting a target once the drag from the page ends", () => {
		const { left, at } = page();
		const drags = pageDrags(PICTURE);
		const router = createDropRouter((x) => at(x), drags);
		router.register(left, { onDrop: vi.fn(), takesPageDrag: () => true });
		router.handle({ type: "enter", paths: [], position: { x: 50, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		drags.end();
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);

		router.handle({ type: "enter", paths: ["/a"], position: { x: 50, y: 10 } });
		drags.end();
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
	});

	it("keeps a newer registration when an older undo runs late", () => {
		const { left, at } = page();
		const router = createDropRouter((x) => at(x));
		const first = vi.fn();
		const second = vi.fn();
		const undo = router.register(left, { onDrop: first });
		router.register(left, { onDrop: second });
		undo();
		router.handle({ type: "drop", paths: ["/a"], position: { x: 50, y: 10 } });
		expect(second).toHaveBeenCalled();
		expect(first).not.toHaveBeenCalled();
	});

	it("sends no files to a target that takes only drags from the page", () => {
		const { left, at } = page();
		const drags = pageDrags(PICTURE);
		const router = createDropRouter((x) => at(x), drags);
		const onHover = vi.fn();
		const onPageDrop = vi.fn();
		router.register(left, { onHover, onPageDrop, takesPageDrag: () => true });

		router.handle({ type: "enter", paths: ["/a"], position: { x: 50, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		router.handle({ type: "drop", paths: ["/a"], position: { x: 50, y: 10 } });
		expect(onHover).not.toHaveBeenCalled();

		router.handle({ type: "enter", paths: [], position: { x: 50, y: 10 } });
		expect(left.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		router.handle({ type: "drop", paths: [], position: { x: 50, y: 10 } });
		expect(onPageDrop).toHaveBeenCalledWith(PICTURE, { x: 50, y: 10 });
	});
});

describe("toCssPoint", () => {
	it("keeps points as they are on macOS and Linux", () => {
		expect(toCssPoint({ x: 300, y: 120 }, 2, false)).toEqual({ x: 300, y: 120 });
	});

	it("divides physical pixels by the ratio on Windows", () => {
		expect(toCssPoint({ x: 300, y: 120 }, 1.5, true)).toEqual({ x: 200, y: 80 });
		expect(toCssPoint({ x: 300, y: 120 }, 0, true)).toEqual({ x: 300, y: 120 });
	});
});

describe("registerDropTarget in the app", () => {
	const app = globalThis as { isTauri?: boolean };
	const elementFromPoint = document.elementFromPoint;

	beforeAll(() => {
		app.isTauri = true;
		mockWindows("main");
		mockIPC(() => null, { shouldMockEvents: true });
	});

	afterAll(() => {
		delete app.isTauri;
		document.elementFromPoint = elementFromPoint;
		clearMocks();
	});

	/** Lets the listener finish subscribing. */
	const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

	it("routes Tauri's drag events by where they land", async () => {
		const target = document.createElement("div");
		document.body.append(target);
		document.elementFromPoint = (x) => (x < 100 ? target : document.body);
		const onDrop = vi.fn();
		const undo = registerDropTarget(target, { onDrop });
		await settle();

		await emit("tauri://drag-enter", { paths: ["/a.png"], position: { x: 40, y: 8 } });
		expect(target.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		await emit("tauri://drag-drop", { paths: ["/b.png"], position: { x: 400, y: 8 } });
		expect(onDrop).not.toHaveBeenCalled();
		await emit("tauri://drag-drop", { paths: ["/a.png"], position: { x: 40, y: 8 } });
		expect(onDrop).toHaveBeenCalledWith(["/a.png"], { x: 40, y: 8 });
		undo();
		target.remove();
	});

	/** A drag of a timeline file starting at `element`. */
	const dragFile = (element: Element) => {
		const event = new Event("dragstart", { bubbles: true });
		Object.defineProperty(event, "dataTransfer", {
			value: {
				types: ["application/x-maxhq-file"],
				getData: () => '{"name":"shot.png"}',
			},
		});
		element.dispatchEvent(event);
	};

	it("hands a drag from the page to the target that takes it", async () => {
		const target = document.createElement("div");
		document.body.append(target);
		document.elementFromPoint = (x) => (x < 100 ? target : document.body);
		const onDrop = vi.fn();
		const onPageDrop = vi.fn();
		const undo = registerDropTarget(target, {
			onDrop,
			onPageDrop,
			takesPageDrag: (drag) => drag.has("application/x-maxhq-file"),
		});
		await settle();

		await emit("tauri://drag-enter", { paths: [], position: { x: 40, y: 8 } });
		expect(target.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(false);
		await emit("tauri://drag-leave", null);

		dragFile(target);
		await emit("tauri://drag-enter", { paths: [], position: { x: 40, y: 8 } });
		expect(target.hasAttribute(DROP_OVER_ATTRIBUTE)).toBe(true);
		await emit("tauri://drag-drop", { paths: [], position: { x: 40, y: 8 } });
		expect(onPageDrop).toHaveBeenCalledExactlyOnceWith(
			new Map([["application/x-maxhq-file", '{"name":"shot.png"}']]),
			{ x: 40, y: 8 }
		);
		expect(onDrop).not.toHaveBeenCalled();
		expect(claimPageDrop()).toBe(false);
		undo();
		target.remove();
	});

	it("gives a drag the page dropped itself to that drop alone", async () => {
		const target = document.createElement("div");
		document.body.append(target);
		document.elementFromPoint = () => target;
		const onPageDrop = vi.fn();
		const undo = registerDropTarget(target, {
			onDrop: vi.fn(),
			onPageDrop,
			takesPageDrag: () => true,
		});
		await settle();

		dragFile(target);
		expect(claimPageDrop()).toBe(true);
		await emit("tauri://drag-drop", { paths: [], position: { x: 40, y: 8 } });
		expect(onPageDrop).not.toHaveBeenCalled();
		undo();
		target.remove();
	});

	it("gives a component its hover state and drops", async () => {
		const dropped: string[][] = [];
		const Composer = () => {
			const { ref, over } = useDropTarget<HTMLDivElement>((paths) => dropped.push(paths));
			return createElement("div", { ref, "data-testid": "composer" }, over ? "over" : "idle");
		};
		render(createElement(Composer));
		const composer = screen.getByTestId("composer");
		document.elementFromPoint = () => composer;
		await settle();

		await act(() => emit("tauri://drag-over", { position: { x: 10, y: 10 } }));
		expect(composer).toHaveTextContent("over");
		await act(() => emit("tauri://drag-drop", { paths: ["/c"], position: { x: 10, y: 10 } }));
		expect(composer).toHaveTextContent("idle");
		expect(dropped).toEqual([["/c"]]);
	});
});
