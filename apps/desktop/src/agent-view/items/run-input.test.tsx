// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InputRow, keyInput } from "./run-input";

afterEach(cleanup);

/** The reply line on its own, with what it sends and interrupts. */
const renderLine = (props: Partial<Parameters<typeof InputRow>[0]> = {}) => {
	const onSend = vi.fn<(data: string) => void>();
	const onInterrupt = vi.fn<() => void>();
	const view = render(<InputRow onSend={onSend} onInterrupt={onInterrupt} {...props} />);
	const line = () =>
		screen.getByRole<HTMLInputElement>("textbox", { name: "Input for the command" });
	const hidden = () => screen.getByLabelText<HTMLInputElement>("Input for the command");
	return { onSend, onInterrupt, line, hidden, rerender: view.rerender };
};

const NONE = { ctrlKey: false, metaKey: false, altKey: false, shiftKey: false };

describe("the keys the reply line passes on", () => {
	it.each([
		["Escape", "\x1b"],
		["Tab", "\t"],
	])("sends %s whatever the line holds", (key, sent) => {
		expect(keyInput({ ...NONE, key }, true, false)).toBe(sent);
		expect(keyInput({ ...NONE, key }, false, false)).toBe(sent);
	});

	it("sends the arrows as CSI, or as SS3 once the program asks for that", () => {
		const arrows = ["ArrowUp", "ArrowDown", "ArrowRight", "ArrowLeft"];
		expect(arrows.map((key) => keyInput({ ...NONE, key }, true, false))).toEqual([
			"\x1b[A",
			"\x1b[B",
			"\x1b[C",
			"\x1b[D",
		]);
		expect(arrows.map((key) => keyInput({ ...NONE, key }, true, true))).toEqual([
			"\x1bOA",
			"\x1bOB",
			"\x1bOC",
			"\x1bOD",
		]);
	});

	it("keeps the arrows and Control-D for what you type once the line holds text", () => {
		expect(keyInput({ ...NONE, key: "ArrowLeft" }, false, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "d", ctrlKey: true }, false, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "d", ctrlKey: true }, true, false)).toBe("\x04");
	});

	it("keeps any other key, and the ones with Shift, Option or Command", () => {
		expect(keyInput({ ...NONE, key: "a" }, true, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "Tab", shiftKey: true }, true, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "ArrowUp", altKey: true }, true, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "ArrowUp", metaKey: true }, true, false)).toBeNull();
		expect(keyInput({ ...NONE, key: "e", ctrlKey: true }, true, false)).toBeNull();
	});
});

describe("the reply line", () => {
	it("types what it holds, then the key, and empties", () => {
		const { onSend, line } = renderLine();
		fireEvent.change(line(), { target: { value: "gi" } });
		fireEvent.keyDown(line(), { key: "Tab" });
		expect(onSend).toHaveBeenLastCalledWith("gi\t");
		expect(line().value).toBe("");

		fireEvent.keyDown(line(), { key: "Escape" });
		expect(onSend).toHaveBeenLastCalledWith("\x1b");
	});

	it("sends the arrows and Control-D only while it's empty", () => {
		const { onSend, line } = renderLine({ appCursor: true });
		fireEvent.keyDown(line(), { key: "ArrowDown" });
		fireEvent.keyDown(line(), { key: "d", ctrlKey: true });
		expect(onSend.mock.calls).toEqual([["\x1bOB"], ["\x04"]]);

		onSend.mockClear();
		fireEvent.change(line(), { target: { value: "ab" } });
		fireEvent.keyDown(line(), { key: "ArrowLeft" });
		fireEvent.keyDown(line(), { key: "d", ctrlKey: true });
		expect(onSend).not.toHaveBeenCalled();
		expect(line().value).toBe("ab");
	});

	it("interrupts on Control-C, unless you picked text to copy", () => {
		const { onInterrupt, line } = renderLine();
		fireEvent.change(line(), { target: { value: "copy me" } });
		line().setSelectionRange(0, 4);
		fireEvent.keyDown(line(), { key: "c", ctrlKey: true });
		expect(onInterrupt).not.toHaveBeenCalled();
		line().setSelectionRange(7, 7);
		fireEvent.keyDown(line(), { key: "c", ctrlKey: true });
		expect(onInterrupt).toHaveBeenCalledOnce();
	});

	it("offers a button for each answer, sending it with Enter", () => {
		const { onSend } = renderLine({ choices: ["y", "n"], waiting: true });
		expect(screen.getByRole("textbox")).toHaveAttribute("placeholder", "Or type a reply");
		fireEvent.click(screen.getByRole("button", { name: "Answer n" }));
		expect(onSend).toHaveBeenCalledExactlyOnceWith("n\r");
	});

	it("hides what you type once locked, in a fresh field after each send", () => {
		const { onSend, hidden } = renderLine();
		const lock = screen.getByRole("button", { name: "Hide what you type" });
		expect(lock).toHaveAttribute("aria-pressed", "false");
		fireEvent.change(hidden(), { target: { value: "half" } });

		fireEvent.click(lock);
		expect(lock).toHaveAttribute("aria-pressed", "true");
		const field = hidden();
		// Locking keeps what you typed so far.
		expect(field).toHaveAttribute("type", "password");
		expect(field.value).toBe("half");
		expect(field).toHaveAttribute("data-1p-ignore");
		expect(field).toHaveAttribute("autocomplete", "off");

		act(() => field.focus());
		fireEvent.keyDown(field, { key: "Enter" });
		expect(onSend).toHaveBeenCalledExactlyOnceWith("half\r");
		const fresh = hidden();
		expect(fresh).not.toBe(field);
		expect(field.isConnected).toBe(false);
		expect(fresh.value).toBe("");
		expect(fresh).toHaveFocus();

		fireEvent.click(lock);
		expect(hidden()).toHaveAttribute("type", "text");
	});

	it("takes focus once as a wait starts, from nowhere or an empty composer", () => {
		const composer = document.createElement("div");
		composer.className = "composer-editor";
		composer.contentEditable = "true";
		composer.tabIndex = 0;
		document.body.append(composer);
		try {
			const { line, rerender, onSend, onInterrupt } = renderLine();
			const row = (waiting: boolean) => (
				<InputRow onSend={onSend} onInterrupt={onInterrupt} waiting={waiting} />
			);
			act(() => composer.focus());
			rerender(row(true));
			expect(line()).toHaveFocus();

			// While you type in the composer, the next wait leaves you there.
			rerender(row(false));
			composer.textContent = "half a thought";
			act(() => composer.focus());
			rerender(row(true));
			expect(composer).toHaveFocus();

			// And so does another field you're in.
			const other = document.createElement("input");
			document.body.append(other);
			rerender(row(false));
			act(() => other.focus());
			rerender(row(true));
			expect(other).toHaveFocus();
			other.remove();
		} finally {
			composer.remove();
		}
	});

	it("leaves focus alone when it first shows already waiting", () => {
		renderLine({ waiting: true });
		expect(document.activeElement).toBe(document.body);
	});
});
