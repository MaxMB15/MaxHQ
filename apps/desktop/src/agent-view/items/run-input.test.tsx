// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { onMac } from "@/native/platform";

import {
	focusComposer,
	focusFree,
	heldLook,
	InputRow,
	isGiveBack,
	KeysHint,
	keyInput,
	pickerKey,
	useHeldKeys,
} from "./run-input";

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
		expect(onSend).toHaveBeenCalledExactlyOnceWith("half\r", { secret: true });
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

describe("the keys a picker takes", () => {
	it("sends the arrows as CSI, or as SS3 once the program asks for that", () => {
		const arrows = ["ArrowUp", "ArrowDown", "ArrowRight", "ArrowLeft"];
		expect(arrows.map((key) => pickerKey({ ...NONE, key }, false))).toEqual([
			"\x1b[A",
			"\x1b[B",
			"\x1b[C",
			"\x1b[D",
		]);
		expect(arrows.map((key) => pickerKey({ ...NONE, key }, true))).toEqual([
			"\x1bOA",
			"\x1bOB",
			"\x1bOC",
			"\x1bOD",
		]);
	});

	it.each([
		["Enter", false, "\r"],
		[" ", false, " "],
		["Tab", false, "\t"],
		["Tab", true, "\x1b[Z"],
		["Escape", false, "\x1b"],
		["Backspace", false, "\x7f"],
		["j", false, "j"],
		["Y", true, "Y"],
		["é", false, "é"],
	])("sends %s (Shift %s) as a terminal does", (key, shiftKey, sent) => {
		expect(pickerKey({ ...NONE, key, shiftKey }, false)).toBe(sent);
	});

	it("sends Control with a letter as its control character", () => {
		expect(pickerKey({ ...NONE, key: "a", ctrlKey: true }, false)).toBe("\x01");
		expect(pickerKey({ ...NONE, key: "D", ctrlKey: true }, false)).toBe("\x04");
		expect(pickerKey({ ...NONE, key: "1", ctrlKey: true }, false)).toBeNull();
	});

	it("leaves Command, Option and keys with long names to the app", () => {
		expect(pickerKey({ ...NONE, key: "k", metaKey: true }, false)).toBeNull();
		expect(pickerKey({ ...NONE, key: "ArrowUp", altKey: true }, false)).toBeNull();
		expect(pickerKey({ ...NONE, key: "F5" }, false)).toBeNull();
		expect(pickerKey({ ...NONE, key: "Shift" }, false)).toBeNull();
	});

	it("gives the keys back on ⌘. alone", () => {
		const command = onMac() ? { metaKey: true } : { ctrlKey: true };
		expect(isGiveBack({ ...NONE, ...command, key: ".", code: "Period" })).toBe(true);
		expect(isGiveBack({ ...NONE, ...command, shiftKey: true, key: ">", code: "Period" })).toBe(
			false
		);
		expect(isGiveBack({ ...NONE, ...command, key: ",", code: "Comma" })).toBe(false);
		expect(isGiveBack({ ...NONE, key: ".", code: "Period" })).toBe(false);
	});
});

/** Two agents' panes, each with a composer, one with a field of its own, and a button. */
const panes = () => {
	const root = document.createElement("div");
	root.innerHTML = `
		<div data-agent-pane="agent-1">
			<div class="answer-field"><div class="composer-editor" contenteditable="true" tabindex="0" data-testid="answer"></div></div>
			<div class="composer-editor" contenteditable="true" tabindex="0" data-testid="composer-1"></div>
			<input data-testid="find-1" />
		</div>
		<div data-agent-pane="agent-2">
			<div class="composer-editor" contenteditable="true" tabindex="0" data-testid="composer-2"></div>
		</div>
		<button data-testid="button">Button</button>`;
	document.body.append(root);
	const at = (id: string) => root.querySelector<HTMLElement>(`[data-testid="${id}"]`)!;
	return { root, at };
};

describe("where focus may go", () => {
	it("is free with focus nowhere, on a button, or in this agent's composer", () => {
		const { root, at } = panes();
		try {
			expect(focusFree("agent-1")).toBe(true);
			act(() => at("button").focus());
			expect(focusFree("agent-1")).toBe(true);
			act(() => at("composer-1").focus());
			expect(focusFree("agent-1")).toBe(true);
		} finally {
			root.remove();
		}
	});

	it("is busy while you type in another agent's composer or any other field", () => {
		const { root, at } = panes();
		try {
			act(() => at("composer-2").focus());
			expect(focusFree("agent-1")).toBe(false);
			act(() => at("find-1").focus());
			expect(focusFree("agent-1")).toBe(false);
		} finally {
			root.remove();
		}
	});

	it("goes back to the agent's own composer, not an answer field or another agent's", () => {
		const { root, at } = panes();
		try {
			focusComposer("agent-1");
			expect(at("composer-1")).toHaveFocus();
			focusComposer("agent-2");
			expect(at("composer-2")).toHaveFocus();
			focusComposer("agent-3");
			expect(at("composer-2")).toHaveFocus();
		} finally {
			root.remove();
		}
	});
});

describe("a card holding your keys", () => {
	type Props = { takes: boolean; appCursor?: boolean };

	/** A card's output that may hold the keys, in agent-1's pane with its composer. */
	const renderHold = (props: Props) => {
		const onSend = vi.fn<(data: string) => void>();
		const onStop = vi.fn<() => void>();
		const Output = ({ takes, appCursor = false }: Props) => {
			const keys = useHeldKeys({ agent: "agent-1", takes, appCursor, onSend, onStop });
			return (
				<div data-agent-pane="agent-1">
					<div {...keys.box} data-testid="box">
						<div className={heldLook(keys)} data-testid="output">
							? Select a framework
						</div>
						{keys.held ? (
							<KeysHint>
								<button type="button">Stop</button>
							</KeysHint>
						) : null}
					</div>
					<div
						className="composer-editor"
						contentEditable
						suppressContentEditableWarning
						tabIndex={0}
						data-testid="composer"
					/>
					<button type="button">Elsewhere</button>
				</div>
			);
		};
		const view = render(<Output {...props} />);
		const box = () => screen.getByTestId("box");
		return {
			onSend,
			onStop,
			box,
			composer: () => screen.getByTestId("composer"),
			rerender: (next: Props) => view.rerender(<Output {...next} />),
		};
	};

	const command = () => (onMac() ? { metaKey: true } : { ctrlKey: true });

	it("offers the output, and holds the keys once you click in it", () => {
		const { box } = renderHold({ takes: true });
		expect(box()).toHaveAttribute("data-keys", "offered");
		expect(box()).toHaveAccessibleName("Click to give the program your keys");
		expect(screen.getByTestId("output")).toHaveClass("cursor-pointer");

		act(() => box().focus());

		expect(box()).toHaveAttribute("data-keys", "held");
		expect(screen.getByTestId("output")).toHaveClass("outline-primary");
		expect(box()).toHaveTextContent("↑↓↩go to the program");
	});

	it("sends each key to the program, the arrows as it asked", () => {
		const { box, onSend } = renderHold({ takes: true, appCursor: true });
		act(() => box().focus());

		fireEvent.keyDown(box(), { key: "ArrowDown" });
		fireEvent.keyDown(box(), { key: " " });
		fireEvent.keyDown(box(), { key: "Enter" });

		expect(onSend.mock.calls).toEqual([["\x1bOB"], [" "], ["\r"]]);
	});

	it("stops the program on Control-C", () => {
		const { box, onSend, onStop } = renderHold({ takes: true });
		act(() => box().focus());

		fireEvent.keyDown(box(), { key: "c", ctrlKey: true });

		expect(onStop).toHaveBeenCalledOnce();
		expect(onSend).not.toHaveBeenCalled();
	});

	it("leaves the keys of the buttons under the output to them", () => {
		const { box, onSend } = renderHold({ takes: true });
		act(() => box().focus());
		const stop = within(box()).getByRole("button", { name: "Stop" });
		act(() => stop.focus());

		fireEvent.keyDown(stop, { key: "Enter" });

		expect(onSend).not.toHaveBeenCalled();
		expect(box()).toHaveAttribute("data-keys", "held");
	});

	it("gives the keys back to the composer on ⌘.", () => {
		const { box, composer, onSend } = renderHold({ takes: true });
		act(() => box().focus());

		fireEvent.keyDown(box(), { ...command(), key: ".", code: "Period" });

		expect(box()).toHaveAttribute("data-keys", "offered");
		expect(composer()).toHaveFocus();
		expect(onSend).not.toHaveBeenCalled();
	});

	it("gives the keys back on a click outside, where the click goes", () => {
		const { box } = renderHold({ takes: true });
		act(() => box().focus());
		const elsewhere = screen.getByRole("button", { name: "Elsewhere" });

		act(() => elsewhere.focus());

		expect(box()).toHaveAttribute("data-keys", "offered");
		expect(elsewhere).toHaveFocus();
	});

	it("gives the keys back to the composer when focus goes nowhere", () => {
		const { box, composer } = renderHold({ takes: true });
		act(() => box().focus());

		act(() => box().blur());

		expect(box()).toHaveAttribute("data-keys", "offered");
		expect(composer()).toHaveFocus();
	});

	it("gives the keys back to the composer once the program stops taking them", () => {
		const { box, composer, rerender, onSend } = renderHold({ takes: true });
		act(() => box().focus());

		rerender({ takes: false });

		expect(box()).not.toHaveAttribute("data-keys");
		expect(box()).not.toHaveAttribute("tabindex");
		expect(composer()).toHaveFocus();
		fireEvent.keyDown(box(), { key: "ArrowDown" });
		expect(onSend).not.toHaveBeenCalled();
	});

	it("offers nothing while the program takes no keys", () => {
		const { box, onSend } = renderHold({ takes: false });

		expect(box()).not.toHaveAttribute("role");
		expect(screen.getByTestId("output")).toHaveAttribute("class", "");
		fireEvent.keyDown(box(), { key: "ArrowDown" });
		expect(onSend).not.toHaveBeenCalled();
	});
});
