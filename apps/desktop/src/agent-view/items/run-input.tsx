// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What you type into a command you ran with `!`: a line of input, or a
// password when the command stops echoing. Neither field holds what you
// type anywhere but the page: no state, store, draft or log gets it. Enter
// sends it straight to the command and the field empties at once. The
// password field can take focus from the composer, so a password typed
// straight after its command never reaches the agent.

import { CornerDownLeft, LockKeyhole } from "lucide-react";
import { type KeyboardEvent, useEffect, useLayoutEffect, useRef, useState } from "react";

import { t, useI18n } from "@maxhq/shared/i18n";

import { cn } from "@/lib/utils";

const KBD =
	"rounded-[4px] border border-border px-1 font-mono text-[10.5px] text-subtle-foreground";

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

/** The line under a running command that types into it. */
export const InputRow = ({
	onSend,
	onInterrupt,
}: {
	/** Types the line and its Enter into the command. */
	onSend: (data: string) => void;
	/** Control-C in the field, with no text picked. */
	onInterrupt: () => void;
}) => {
	useI18n();
	const field = useRef<HTMLInputElement>(null);
	const send = () => {
		const input = field.current;
		if (!input) return;
		onSend(`${input.value}\r`);
		input.value = "";
	};
	return (
		<div className="flex items-center gap-2 border-t border-border py-1.5 pr-2.5 pl-3 text-xs text-subtle-foreground">
			<span aria-hidden className="font-mono">
				›
			</span>
			<input
				ref={field}
				type="text"
				aria-label={t("Input for the command")}
				placeholder={t("Type to send input to the command")}
				autoComplete="off"
				autoCorrect="off"
				autoCapitalize="off"
				spellCheck={false}
				className="min-w-0 flex-1 bg-transparent font-mono text-xs text-foreground outline-none placeholder:text-subtle-foreground"
				onKeyDown={(event) => {
					const input = event.currentTarget;
					const picked = input.selectionStart !== input.selectionEnd;
					if (event.key === "c" && event.ctrlKey && !event.metaKey && !picked) {
						event.preventDefault();
						event.stopPropagation();
						onInterrupt();
						return;
					}
					ownKeys(event, send);
				}}
			/>
			<kbd aria-hidden className={KBD}>
				⏎
			</kbd>
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
	// Counts the passwords sent, so each one gets a fresh element.
	const [round, setRound] = useState(0);
	// Whether the field had focus when a password went, so the fresh one takes it.
	const kept = useRef(false);
	// Where focus was before the field took it.
	const back = useRef<HTMLElement | null>(null);

	useEffect(() => {
		if (!takeFocus) return;
		const at = strayFocus();
		if (at === undefined) return;
		back.current = at;
		field.current?.focus();
	}, [takeFocus]);

	useEffect(() => {
		if (!kept.current) return;
		kept.current = false;
		field.current?.focus();
	}, [round]);

	// It runs before the field leaves the page, while it still has focus.
	useLayoutEffect(() => {
		const current = field;
		return () => {
			const to = back.current;
			if (document.activeElement === current.current && to?.isConnected) to.focus();
		};
	}, []);

	const send = () => {
		const input = field.current;
		if (!input) return;
		onSend(`${input.value}\r`);
		input.value = "";
		kept.current = document.activeElement === input;
		setRound((sent) => sent + 1);
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
				data-1p-ignore=""
				data-lpignore="true"
				data-bwignore=""
				data-form-type="other"
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
