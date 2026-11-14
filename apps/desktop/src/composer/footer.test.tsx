// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { EFFORT_SINCE } from "@maxhq/shared/bindings";
import { account, link } from "@maxhq/shared/test/build";
import { FOLD_SLACK } from "@maxhq/shared/ui/fold";

import {
	DEVICE,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	renderComposer,
	type,
} from "./testing";

// jsdom lays nothing out, so the row is as wide as the test makes the pane,
// and what it wants is a rough sum of what it shows: 7 pixels a letter, 16
// an icon or a control's padding, and 4 letters for a model name cut short.
let pane = 2000;

const isRow = (node: Element) => node.hasAttribute("data-footer-row");

const wanted = (row: Element): number => {
	let width = 12;
	const walk = (node: Node): void => {
		if (node instanceof Element) {
			if (node.classList.contains("sr-only")) return;
			if (node.tagName.toLowerCase() === "svg") {
				width += 16;
				return;
			}
			if (node.classList.contains("min-w-[4ch]")) {
				width += 4 * 7;
				return;
			}
			if (node.tagName === "BUTTON") width += 16;
		} else if (node.nodeType === Node.TEXT_NODE) {
			width += (node.textContent ?? "").length * 7;
		}
		node.childNodes.forEach(walk);
	};
	row.childNodes.forEach(walk);
	return width;
};

// The rows a ResizeObserver watches, each with the call that reports its size.
const watched = new Map<Element, { by: object; report: () => void }>();

class FakeObserver {
	constructor(private readonly report: ResizeObserverCallback) {}
	observe(target: Element) {
		if (!isRow(target)) return;
		watched.set(target, {
			by: this,
			report: () =>
				this.report(
					[{ target, contentRect: { width: pane } } as ResizeObserverEntry],
					this as unknown as ResizeObserver
				),
		});
	}
	unobserve() {}
	disconnect() {
		for (const [target, { by }] of watched) if (by === this) watched.delete(target);
	}
}

const resize = (width: number) =>
	act(() => {
		pane = width;
		for (const { report } of watched.values()) report();
	});

beforeAll(() => {
	installBrowserFakes();
	vi.stubGlobal("ResizeObserver", FakeObserver);
	Object.defineProperty(HTMLElement.prototype, "clientWidth", {
		configurable: true,
		get(this: HTMLElement) {
			return isRow(this) ? pane : 0;
		},
	});
	Object.defineProperty(HTMLElement.prototype, "scrollWidth", {
		configurable: true,
		get(this: HTMLElement) {
			return isRow(this) ? Math.max(pane, wanted(this)) : 0;
		},
	});
});

afterAll(() => {
	vi.unstubAllGlobals();
	Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
	Reflect.deleteProperty(HTMLElement.prototype, "scrollWidth");
});

afterEach(() => {
	cleanup();
	pane = 2000;
});

const row = (): HTMLElement => {
	const found = document.querySelector<HTMLElement>("[data-footer-row]");
	if (!found) throw new Error("No footer row");
	return found;
};

const button = (name: string | RegExp) => within(row()).getByRole("button", { name });

/** Narrows the pane to a pixel short of what the row wants. */
const narrow = () => resize(wanted(row()) - 1);

/** What opens a control's tooltip: the control, or a send button's wrapper. */
const triggerOf = (control: HTMLElement) => control.closest("[data-base-ui-tooltip-trigger]");

/** What a control's tooltip says, opened as focus opens it. */
const tipOf = async (control: HTMLElement): Promise<string> => {
	act(() => control.focus());
	await waitFor(() => expect(triggerOf(control)).toHaveAttribute("data-popup-open"));
	const open = document.querySelectorAll('[role="presentation"][data-open] > [data-open]');
	return [...open].at(-1)?.textContent ?? "";
};

const claude = account({
	id: "acc",
	plan: "max",
	models: [
		{
			id: "opus",
			label: "Opus 5.5",
			default: true,
			efforts: ["low", "medium", "high"],
			default_effort: "high",
		},
	],
});

/** A Claude agent that names its model by id, with an effort level and a permission mode. */
const renderClaude = () =>
	renderComposer({
		agent: heldAgent({
			vendor: "claude",
			account: "acc",
			model: "claude-opus-5-5",
			effort: "high",
		}),
		account: claude,
		links: [link({ id: "local", device: DEVICE, protocol: EFFORT_SINCE })],
	});

describe("the composer's footer", () => {
	it("shows everything where it fits, and the model by its name", async () => {
		renderClaude();
		await findTextbox();
		expect(button("Model: Opus 5.5")).toHaveTextContent("Opus 5.5");
		expect(button("Effort: High")).toHaveTextContent("High");
		expect(button("Permission mode: Ask first")).toHaveTextContent("Ask first");
		expect(button(/^Send/)).toHaveTextContent("Send⏎");
		expect(await tipOf(button("Model: Opus 5.5"))).toBe("Opus 5.5claude-opus-5-5");
	});

	it("folds a step at a time as the pane narrows, and stays on one line", async () => {
		renderClaude();
		await findTextbox();

		narrow();
		expect(button(/^Send/)).toHaveTextContent(/^Send$/);
		expect(button("Permission mode: Ask first")).toHaveTextContent("Ask first");

		narrow();
		expect(button("Permission mode: Ask first")).toHaveTextContent(/^$/);
		expect(button("Effort: High")).toHaveTextContent("High");

		narrow();
		expect(button("Effort: High")).toHaveTextContent(/^$/);
		const name = within(button("Model: Opus 5.5")).getByText("Opus 5.5");
		expect(name).not.toHaveClass("min-w-[4ch]");

		narrow();
		expect(within(button("Model: Opus 5.5")).getByText("Opus 5.5")).toHaveClass("min-w-[4ch]");
		expect(within(button("Send")).getByText("Send")).not.toHaveClass("sr-only");

		narrow();
		expect(within(button("Send")).getByText("Send")).toHaveClass("sr-only");

		// Folded all the way, it fits a narrow pane.
		resize(240);
		expect(row().scrollWidth).toBeLessThanOrEqual(row().clientWidth);
	});

	it("keeps the full text in every folded control's name and tooltip", async () => {
		pane = 200;
		renderClaude();
		// A disabled button takes no focus, so the message has words to send.
		type(await findTextbox(), "hi");
		expect(await tipOf(button("Permission mode: Ask first"))).toBe(
			"Permission mode: Ask first"
		);
		expect(await tipOf(button("Effort: High"))).toBe("Effort: High");
		expect(await tipOf(button("Model: Opus 5.5"))).toBe("Opus 5.5claude-opus-5-5");
		expect(await tipOf(button("Send"))).toBe("Send ⏎");
	});

	it("opens a disabled send button's tooltip on hover", async () => {
		pane = 200;
		renderClaude();
		await findTextbox();
		const send = button("Send");
		expect(send).toBeDisabled();
		const trigger = triggerOf(send);
		if (!(trigger instanceof HTMLElement)) throw new Error("No trigger");
		expect(trigger).not.toBe(send);
		fireEvent.pointerEnter(trigger, { pointerType: "mouse" });
		fireEvent.mouseEnter(trigger);
		fireEvent.mouseMove(trigger);
		await waitFor(() => expect(trigger).toHaveAttribute("data-popup-open"), { timeout: 2000 });
	});

	it("unfolds only once the wider row fits with room to spare", async () => {
		renderClaude();
		await findTextbox();
		const whole = wanted(row());
		narrow();
		expect(button(/^Send/)).toHaveTextContent(/^Send$/);

		resize(whole + FOLD_SLACK - 1);
		expect(button(/^Send/)).toHaveTextContent(/^Send$/);
		resize(whole + FOLD_SLACK);
		expect(button(/^Send/)).toHaveTextContent("Send⏎");
	});

	it("folds the attachments' size first, then moves their count onto the paperclip", async () => {
		const { drop } = renderClaude();
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/work/a.rs", "/work/b.rs"], { x: 0, y: 0 }));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(2));
		const attach = button("Attach files");
		expect(within(row()).getByText(/^2 attachments, /)).not.toHaveClass("sr-only");

		narrow();
		expect(within(row()).getByText("2 attachments")).toBeInTheDocument();
		expect(within(row()).getByText(/^2 attachments, /)).toHaveClass("sr-only");
		expect(attach).toHaveTextContent(/^$/);

		narrow();
		expect(within(row()).queryByText("2 attachments")).not.toBeInTheDocument();
		expect(attach).toHaveTextContent("2");
		const summary = within(row()).getByText(/^2 attachments, /);
		expect(summary).toHaveClass("sr-only");
		expect(await tipOf(attach)).toBe(`${summary.textContent}Attach files or folders (Ctrl+U)`);
		// The key hints fold only after the attachments.
		expect(button(/^Send/)).toHaveTextContent("Send⏎");
	});

	it("folds the steer and queue buttons to their icons during a turn", async () => {
		pane = 200;
		renderComposer({ agent: heldAgent({ activity: { state: "busy" } }) });
		type(await findTextbox(), "hi");
		expect(within(button("Steer")).getByText("Steer")).toHaveClass("sr-only");
		expect(within(button("Queue")).getByText("Queue")).toHaveClass("sr-only");
		expect(await tipOf(button("Steer"))).toBe("Steer Ctrl+Enter");
		expect(await tipOf(button("Stop"))).toBe("Stop this turn Ctrl+.");
	});

	it("says what Send now does while a prompt waits, folded or not", async () => {
		const prompted = heldAgent({ activity: { state: "needs_input", attention: "att-1" } });
		renderComposer({ agent: prompted });
		type(await findTextbox(), "hi");
		expect(row()).toHaveTextContent("⏎ to queue, Ctrl+Enter to send now");
		expect(button("Send now")).toHaveTextContent(/^Send now$/);
		expect(await tipOf(button("Send now"))).toBe("Skips what's waiting and sends this now");
		cleanup();

		pane = 200;
		renderComposer({ agent: prompted });
		type(await findTextbox(), "hi");
		expect(within(button("Send now")).getByText("Send now")).toHaveClass("sr-only");
		expect(await tipOf(button("Send now"))).toBe(
			"Send now Ctrl+EnterSkips what's waiting and sends this now"
		);
	});
});
