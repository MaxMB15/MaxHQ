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

import { CornerDownLeft, LockKeyhole, LockKeyholeOpen } from "lucide-react";
import { type KeyboardEvent, useEffect, useLayoutEffect, useRef, useState } from "react";

import { t, useI18n } from "@maxhq/shared/i18n";

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
	/** Types the line and its Enter into the command. */
	onSend: (data: string) => void;
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
		onSend(`${input.value}${key}`);
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
	/** Types the password and its Enter into the command. */
	onSend: (data: string) => void;
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
		onSend(`${input.value}\r`);
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
