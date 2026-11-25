// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What you type into a command you ran with `!`: a line of input, or a
// password when the command stops echoing. Neither field holds what you
// type anywhere but the page: no state, store, draft or log gets it. Enter
// sends it straight to the command and the field empties at once. The
// password field can take focus from the composer, so a password typed
// straight after its command never reaches the agent.
//
// The line can be locked, so it hides what you type and sends it the way
// the password field does. Esc, Tab and Control-C go to the command, and
// so do the arrows and Control-D while the line is empty. When the command
// waits for you, the line takes focus once from an empty composer or from
// nowhere, and offers a button for each answer its prompt names.
//
// Stop types Control-C. A command still running a moment later ignored
// it, and Force stop sits beside Stop from then on.
//
// A picker that reads keys one at a time can take your keys in the card:
// click its output and each key goes to it as you press it, until you
// click outside, press ⌘. or the picker closes. Then the keys go back to
// the composer.

import { CornerDownLeft, LockKeyhole, LockKeyholeOpen } from "lucide-react";
import {
	type FocusEvent,
	type KeyboardEvent,
	type ReactNode,
	type RefObject,
	useEffect,
	useLayoutEffect,
	useRef,
	useState,
} from "react";

import type { AgentId } from "@maxhq/shared/bindings";
import { t, useI18n } from "@maxhq/shared/i18n";

import { holdsCommand, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { onMac } from "@/native/platform";
import { cn } from "@/lib/utils";

const KBD =
	"rounded-[4px] border border-border px-1 font-mono text-[10.5px] text-subtle-foreground";

/** Attributes that keep the browser and password managers off a hidden field. */
const IGNORED = {
	"data-1p-ignore": "",
	"data-lpignore": "true",
	"data-bwignore": "",
	"data-form-type": "other",
} as const;

/** How long a command gets after Stop before the card says it ignored it. */
export const IGNORED_AFTER = 2000;

/**
 * Keys typed in a field go only to it, past the app's own shortcuts, and
 * Enter sends.
 */
const ownKeys = (event: KeyboardEvent<HTMLInputElement>, send: () => void) => {
	event.stopPropagation();
	if (event.key === "Enter" && !event.nativeEvent.isComposing) {
		event.preventDefault();
		send();
	}
};

/** The last letter of each arrow key's escape sequence. */
const ARROWS: Readonly<Record<string, string>> = {
	ArrowUp: "A",
	ArrowDown: "B",
	ArrowRight: "C",
	ArrowLeft: "D",
};

/** A key as the line reads it. */
type Key = Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">;

/**
 * What a key pressed in the reply line types into the command after the
 * line so far, or null for a key the line keeps. The arrows go as `ESC O A`
 * when the program asked for application cursor keys, and as `ESC [ A`
 * otherwise. They and Control-D go only while the line is empty, so they
 * still move and delete in what you're typing.
 */
export const keyInput = (key: Key, empty: boolean, appCursor: boolean): string | null => {
	if (key.metaKey || key.altKey) return null;
	if (key.ctrlKey) return key.key.toLowerCase() === "d" && !key.shiftKey && empty ? "\x04" : null;
	if (key.shiftKey) return null;
	if (key.key === "Escape") return "\x1b";
	if (key.key === "Tab") return "\t";
	const arrow = ARROWS[key.key];
	if (arrow === undefined || !empty) return null;
	return `${appCursor ? "\x1bO" : "\x1b["}${arrow}`;
};

/**
 * What a key pressed while a card holds your keys types into the program,
 * or null for one it leaves to the app, such as ⌘ shortcuts. The arrows go
 * as `ESC O A` when the program asked for application cursor keys, and as
 * `ESC [ A` otherwise. Enter, Space, Tab, Shift-Tab, Esc, Backspace,
 * letters and Control with a letter go as a terminal sends them. Off a Mac,
 * Control and Shift go to the app, whose shortcuts reach a card that way.
 */
export const pickerKey = (key: Key, appCursor: boolean): string | null => {
	if (key.metaKey || key.altKey) return null;
	if (key.ctrlKey) {
		if (key.shiftKey && !onMac()) return null;
		const letter = key.key.toLowerCase();
		return /^[a-z]$/.test(letter) ? String.fromCharCode(letter.charCodeAt(0) - 96) : null;
	}
	const arrow = ARROWS[key.key];
	if (arrow !== undefined) return `${appCursor ? "\x1bO" : "\x1b["}${arrow}`;
	switch (key.key) {
		case "Enter":
			return "\r";
		case "Tab":
			return key.shiftKey ? "\x1b[Z" : "\t";
		case "Escape":
			return "\x1b";
		case "Backspace":
			return "\x7f";
		default:
			return [...key.key].length === 1 ? key.key : null;
	}
};

/** Whether a key is ⌘., which gives held keys back while a card has them. */
export const isGiveBack = (key: Key & Pick<KeyboardEvent, "code">): boolean =>
	holdsCommand(key) && !key.shiftKey && !key.altKey && key.code === SHORTCUTS.stop.code;

/** Whether an element is one you type in. */
const typesText = (at: HTMLElement): boolean =>
	at.isContentEditable ||
	at.matches('input, textarea, select, [contenteditable]:not([contenteditable="false"])');

/** The agent's pane, which carries its ID. */
const paneOf = (agent: AgentId): HTMLElement | undefined =>
	[...document.querySelectorAll<HTMLElement>("[data-agent-pane]")].find(
		(pane) => pane.dataset.agentPane === agent
	);

/**
 * Whether focus is free for the terminal panel to take: nowhere, on
 * something you don't type in, or in this agent's composer. Never while
 * you type in another agent's composer or any other field.
 */
export const focusFree = (agent: AgentId): boolean => {
	const at = document.activeElement;
	if (!(at instanceof HTMLElement) || at === document.body) return true;
	if (!typesText(at)) return true;
	const pane = at.closest<HTMLElement>("[data-agent-pane]");
	return pane?.dataset.agentPane === agent && at.closest(".composer-editor") !== null;
};

/** Puts focus in the agent's composer, which takes your keys back. */
export const focusComposer = (agent: AgentId): void => {
	const editors = [
		...(paneOf(agent)?.querySelectorAll<HTMLElement>(".composer-editor") ?? []),
	].filter((editor) => !editor.closest(".answer-field"));
	editors.at(-1)?.focus();
};

/** Whether focus is nowhere, so the composer may take it. */
const focusNowhere = (): boolean => {
	const at = document.activeElement;
	return at === null || at === document.body;
};

/** What a card does with the keys it holds for a program. */
type HeldKeysOptions = {
	readonly agent: AgentId;
	/** Whether the program takes keys in the card now. */
	readonly takes: boolean;
	/** Whether the program asked for application cursor keys. */
	readonly appCursor: boolean;
	/** Types a key into the program, the way the reply line does. */
	readonly onSend: (data: string) => void;
	/** Control-C, which stops the program the way Stop does. */
	readonly onStop: () => void;
};

/** A card's hold on your keys, and what its output needs to take them. */
export type HeldKeys = {
	/** Whether the output has your keys now. */
	readonly held: boolean;
	/** What the output's box takes, or nothing while the program takes no keys. */
	readonly box: {
		readonly ref: RefObject<HTMLDivElement | null>;
		readonly role?: "group";
		readonly tabIndex?: number;
		readonly "aria-label"?: string;
		readonly "data-keys"?: "held" | "offered";
		readonly onFocus?: () => void;
		readonly onBlur?: (event: FocusEvent<HTMLDivElement>) => void;
		readonly onKeyDownCapture?: (event: KeyboardEvent<HTMLDivElement>) => void;
	};
};

/**
 * Lets a card's output hold your keys for a picker. Focus in the output
 * holds them, and each key you press goes to the program. Focus leaving it,
 * ⌘. or the program no longer taking keys gives them back, to the
 * composer when focus is left nowhere.
 */
export const useHeldKeys = ({
	agent,
	takes,
	appCursor,
	onSend,
	onStop,
}: HeldKeysOptions): HeldKeys => {
	const ref = useRef<HTMLDivElement>(null);
	const [holding, setHolding] = useState(false);
	const held = holding && takes;
	// Whether a pointer is down, so a click elsewhere gives the keys back only
	// once it ends, and never cuts a selection short.
	const pressed = useRef(false);
	useEffect(() => {
		if (!holding) return;
		const down = () => (pressed.current = true);
		const up = () => (pressed.current = false);
		window.addEventListener("pointerdown", down, true);
		window.addEventListener("pointerup", up, true);
		return () => {
			window.removeEventListener("pointerdown", down, true);
			window.removeEventListener("pointerup", up, true);
		};
	}, [holding]);

	// The picker closed: the keys go back to the composer.
	const [took, setTook] = useState(takes);
	if (took !== takes) {
		setTook(takes);
		if (!takes && holding) setHolding(false);
	}
	const wasHeld = useRef(false);
	useEffect(() => {
		const was = wasHeld.current;
		wasHeld.current = held;
		if (!was || held || takes) return;
		const at = document.activeElement;
		if (focusNowhere() || (at !== null && ref.current?.contains(at))) focusComposer(agent);
	}, [held, takes, agent]);

	const giveBack = () => {
		setHolding(false);
		focusComposer(agent);
	};

	if (!takes) return { held: false, box: { ref } };
	return {
		held,
		box: {
			ref,
			role: "group",
			tabIndex: 0,
			"aria-label": held
				? t(
						"Your keys go to the program. Click outside or press {{keyHint}} to give them back.",
						{
							keyHint: keyHint(SHORTCUTS.stop),
						}
					)
				: t("Click to give the program your keys"),
			"data-keys": held ? "held" : "offered",
			onFocus: () => setHolding(true),
			onBlur: (event) => {
				const to = event.relatedTarget;
				if (to instanceof Node && event.currentTarget.contains(to)) return;
				setHolding(false);
				if (to !== null) return;
				// A click on nothing gives the keys back to the composer once it ends.
				const back = () => {
					if (focusNowhere() && (window.getSelection()?.isCollapsed ?? true))
						focusComposer(agent);
				};
				if (pressed.current) window.addEventListener("pointerup", back, { once: true });
				else back();
			},
			onKeyDownCapture: (event) => {
				if (!held || event.nativeEvent.isComposing) return;
				if (isGiveBack(event)) {
					event.preventDefault();
					event.stopPropagation();
					giveBack();
					return;
				}
				// The buttons under the output keep their own keys.
				const target = event.target;
				if (target instanceof Element && target.closest("[data-keys-hint]")) return;
				if (event.key === "c" && event.ctrlKey && !event.metaKey && !event.altKey) {
					event.preventDefault();
					event.stopPropagation();
					onStop();
					return;
				}
				const data = pickerKey(event, appCursor);
				if (data === null) return;
				event.preventDefault();
				event.stopPropagation();
				onSend(data);
			},
		},
	};
};

/** The look of a card's output while it offers or holds your keys. */
export const heldLook = (keys: HeldKeys): string =>
	keys.box["data-keys"] === undefined
		? ""
		: keys.held
			? "cursor-default outline-2 outline-solid outline-primary -outline-offset-2"
			: "cursor-pointer outline-none";

/**
 * The row under a card's output while it holds your keys: which keys go to
 * the program, then the card's own buttons.
 */
export const KeysHint = ({ children }: { children: ReactNode }) => {
	useI18n();
	return (
		<div
			data-keys-hint
			title={t("Click outside or press {{keyHint}} to give the keys back.", {
				keyHint: keyHint(SHORTCUTS.stop),
			})}
			className="flex flex-wrap items-center gap-2 border-t border-border bg-primary/5 py-1.5 pr-2 pl-3 text-xs text-muted-foreground"
		>
			<span className="inline-flex items-center gap-1">
				<kbd aria-hidden className={KBD}>
					↑
				</kbd>
				<kbd aria-hidden className={KBD}>
					↓
				</kbd>
				<kbd aria-hidden className={KBD}>
					↩
				</kbd>
				<span>{t("go to the program")}</span>
			</span>
			<span className="ml-auto flex flex-wrap items-center gap-1">{children}</span>
		</div>
	);
};

/**
 * Where focus is, when it's somewhere a password typed now would go
 * astray: a composer, or `null` for nowhere. `undefined` when it's
 * anywhere else, such as another field you're typing in.
 */
const strayFocus = (): HTMLElement | null | undefined => {
	const at = document.activeElement;
	if (!(at instanceof HTMLElement) || at === document.body) return null;
	return at.closest(".composer-editor") ? at : undefined;
};

/**
 * Where focus is, when a line taking it now cuts nothing short: nowhere,
 * or a composer you haven't typed in. `undefined` when it's anywhere else.
 */
const idleFocus = (): HTMLElement | null | undefined => {
	const at = strayFocus();
	if (!at) return at;
	const draft = at.closest(".composer-editor")?.textContent ?? "";
	return draft.trim() === "" ? at : undefined;
};

/**
 * A field taking focus as `take` turns true, or as it shows with it true
 * when `onShow`, from nowhere or a composer (one you haven't typed in when
 * `idle`), and giving it back as the field leaves the page with focus.
 */
const useTakeFocus = (
	field: { readonly current: HTMLInputElement | null },
	take: boolean,
	{ idle, onShow }: { idle: boolean; onShow: boolean }
) => {
	// Where focus was before the field took it.
	const back = useRef<HTMLElement | null>(null);
	const was = useRef(onShow ? false : take);
	useEffect(() => {
		const rose = take && !was.current;
		was.current = take;
		if (!rose) return;
		const at = idle ? idleFocus() : strayFocus();
		if (at === undefined) return;
		back.current = at;
		field.current?.focus();
	}, [take, idle, field]);
	// It runs before the field leaves the page, while it still has focus.
	useLayoutEffect(() => {
		const current = field;
		return () => {
			const to = back.current;
			if (document.activeElement === current.current && to?.isConnected) to.focus();
		};
	}, [field]);
};

/**
 * Counts what a hidden field sent, so each send gets a fresh element that
 * takes focus when the old one had it.
 */
const useRounds = (field: { readonly current: HTMLInputElement | null }) => {
	const [round, setRound] = useState(0);
	const kept = useRef(false);
	useEffect(() => {
		if (!kept.current) return;
		kept.current = false;
		field.current?.focus();
	}, [round, field]);
	const next = (input: HTMLInputElement) => {
		kept.current = document.activeElement === input;
		setRound((sent) => sent + 1);
	};
	return [round, next] as const;
};

/** The line under a running command that types into it. */
export const InputRow = ({
	onSend,
	onInterrupt,
	choices = [],
	appCursor = false,
	waiting = false,
}: {
	/**
	 * Types the line and its Enter into the command, as a secret when you
	 * locked the line.
	 */
	onSend: (data: string, options?: { readonly secret: boolean }) => void;
	/** Control-C in the field, with no text picked. */
	onInterrupt: () => void;
	/** The answers the prompt the command waits at offers, as you type them. */
	choices?: readonly string[];
	/** Whether the program asked for application cursor keys. */
	appCursor?: boolean;
	/** Whether the command waits for you. */
	waiting?: boolean;
}) => {
	useI18n();
	const field = useRef<HTMLInputElement>(null);
	const [locked, setLocked] = useState(false);
	const [round, next] = useRounds(field);
	// The line takes focus once, as a wait starts, and never from someone typing.
	useTakeFocus(field, waiting, { idle: true, onShow: false });

	/** Types the line so far and then `key` into the command, and empties the line. */
	const send = (key: string) => {
		const input = field.current;
		if (!input) return;
		const text = `${input.value}${key}`;
		if (locked) onSend(text, { secret: true });
		else onSend(text);
		input.value = "";
		if (locked) next(input);
	};

	const hint = t(
		"Enter sends the line. Esc, Tab and Ctrl-C go to the command, and so do the arrows and Ctrl-D while the line is empty."
	);

	return (
		<div
			data-waiting={waiting || undefined}
			className={cn(
				"flex flex-wrap items-center gap-2 border-t border-border py-1.5 pr-2.5 pl-3 text-xs text-subtle-foreground",
				waiting && "bg-status-busy/7"
			)}
		>
			<span aria-hidden className="font-mono">
				›
			</span>
			{choices.map((choice) => (
				<button
					key={choice}
					type="button"
					aria-label={t("Answer {{choice}}", { choice })}
					className="inline-flex h-[22px] shrink-0 cursor-pointer items-center rounded-md border border-border bg-background px-2 font-mono text-xs text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
					onClick={() => onSend(`${choice}\r`)}
				>
					{choice}
				</button>
			))}
			<input
				key={round}
				ref={field}
				type={locked ? "password" : "text"}
				name={locked ? `run-input-${round}` : undefined}
				aria-label={t("Input for the command")}
				title={hint}
				placeholder={
					choices.length > 0
						? t("Or type a reply")
						: t("Type to send input to the command")
				}
				autoComplete="off"
				autoCorrect="off"
				autoCapitalize="off"
				spellCheck={false}
				{...(locked ? IGNORED : {})}
				className="min-w-[120px] flex-1 bg-transparent font-mono text-xs text-foreground outline-none placeholder:text-subtle-foreground"
				onKeyDown={(event) => {
					const input = event.currentTarget;
					const picked = input.selectionStart !== input.selectionEnd;
					if (event.key === "c" && event.ctrlKey && !event.metaKey && !picked) {
						event.preventDefault();
						event.stopPropagation();
						onInterrupt();
						return;
					}
					const key = event.nativeEvent.isComposing
						? null
						: keyInput(event, input.value === "", appCursor);
					if (key !== null) {
						event.preventDefault();
						event.stopPropagation();
						send(key);
						return;
					}
					ownKeys(event, () => send("\r"));
				}}
				onPaste={locked ? (event) => event.stopPropagation() : undefined}
			/>
			<button
				type="button"
				aria-pressed={locked}
				aria-label={t("Hide what you type")}
				title={t("Hide what you type")}
				className={cn(
					"inline-flex size-[22px] shrink-0 cursor-pointer items-center justify-center rounded-md outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
					locked
						? "text-status-busy-text"
						: "text-subtle-foreground hover:text-foreground"
				)}
				onClick={() => setLocked(!locked)}
			>
				{locked ? (
					<LockKeyhole aria-hidden className="size-3" strokeWidth={2.2} />
				) : (
					<LockKeyholeOpen aria-hidden className="size-3" strokeWidth={2} />
				)}
			</button>
			<kbd aria-hidden className={KBD}>
				⏎
			</kbd>
		</div>
	);
};

/**
 * Stop for a command: each press types Control-C, and a command still
 * running `IGNORED_AFTER` after the first press ignored it. That holds
 * until the command ends.
 */
export const useStop = (running: boolean, interrupt: () => void) => {
	const [ignored, setIgnored] = useState(false);
	// A command that ends forgets that it ignored Stop.
	const [ran, setRan] = useState(running);
	if (ran !== running) {
		setRan(running);
		if (!running) setIgnored(false);
	}
	const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
	useEffect(() => {
		if (running || timer.current === null) return;
		clearTimeout(timer.current);
		timer.current = null;
	}, [running]);
	useEffect(
		() => () => {
			if (timer.current !== null) clearTimeout(timer.current);
		},
		[]
	);
	const stop = () => {
		interrupt();
		if (timer.current !== null || ignored) return;
		timer.current = setTimeout(() => {
			timer.current = null;
			setIgnored(true);
		}, IGNORED_AFTER);
	};
	return { stop, ignored: running && ignored };
};

/** "python3 ignored Stop", beside the card's Stop once a command did. */
export const IgnoredStop = ({ program }: { program: string }) => {
	useI18n();
	return (
		<span data-ignored-stop className="text-[11.5px] whitespace-nowrap text-status-busy-text">
			{t("{{program}} ignored Stop", { program })}
		</span>
	);
};

/**
 * The secure field a command asking for a password gets in place of the
 * input row. It's uncontrolled and no form holds it, so the browser and
 * password managers leave it alone, and after Send a new element takes
 * its place, with focus when the old one had it. With `takeFocus` it
 * takes focus from a composer or from nowhere when it shows, and hands it
 * back once the command stops asking.
 */
export const PasswordField = ({
	label,
	onSend,
	takeFocus = false,
}: {
	/** What the field asks for, such as "Password for sudo on devbox". */
	label: string;
	/** Types the password and its Enter into the command, as a secret. */
	onSend: (data: string, options: { readonly secret: boolean }) => void;
	/** Whether it takes focus when it shows, for a command this window started. */
	takeFocus?: boolean;
}) => {
	useI18n();
	const field = useRef<HTMLInputElement>(null);
	const [round, next] = useRounds(field);
	useTakeFocus(field, takeFocus, { idle: false, onShow: true });

	const send = () => {
		const input = field.current;
		if (!input) return;
		onSend(`${input.value}\r`, { secret: true });
		input.value = "";
		next(input);
	};
	return (
		<div className="flex flex-wrap items-center gap-2 border-t border-border bg-status-busy/7 py-2 pr-2.5 pl-3">
			<LockKeyhole
				aria-hidden
				className="size-3.5 shrink-0 text-status-busy-text"
				strokeWidth={2}
			/>
			<input
				key={round}
				ref={field}
				type="password"
				name={`run-password-${round}`}
				aria-label={label}
				placeholder={label}
				autoComplete="off"
				autoCorrect="off"
				autoCapitalize="off"
				spellCheck={false}
				{...IGNORED}
				className={cn(
					"h-7 min-w-[200px] flex-1 rounded-[7px] border border-primary bg-background px-[9px] text-[13px] text-foreground outline-none",
					"ring-[3px] ring-primary/20 placeholder:text-xs placeholder:text-subtle-foreground"
				)}
				onKeyDown={(event) => ownKeys(event, send)}
				onPaste={(event) => event.stopPropagation()}
			/>
			<button
				type="button"
				className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-primary px-2.5 text-xs font-semibold text-primary-foreground outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-ring"
				onClick={send}
			>
				<CornerDownLeft aria-hidden className="size-3" strokeWidth={2.2} />
				{t("Send")}
			</button>
			<p className="w-full pl-[22px] text-[11.5px] text-muted-foreground">
				{t(
					"Goes straight to this command over the encrypted link. MaxHQ doesn't keep or log it, and no agent can see it or type into this command."
				)}
			</p>
		</div>
	);
};
