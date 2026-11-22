// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One terminal in the panel or in a pane of its own: the lock bar while it
// asks for a password, its live screen with find, the Send to agent button
// over a selection and the menu on it, the menu on a command's mark, and
// how it exited, with Restart. Command, or Control and Shift off a Mac,
// with Up or Down jumps between the commands its shell marked, and with
// Return sends to the agent what the menu would. With nothing picked that's
// the last marked command's output, or a run's last lines.

import { ChevronDown, ChevronUp, Copy, Lock, MessageSquarePlus, RotateCcw, X } from "lucide-react";
import {
	type KeyboardEvent,
	type MouseEvent,
	type PointerEvent,
	useEffect,
	useRef,
	useState,
} from "react";

import { ICON_BUTTON } from "@/agent-view/find/bar";
import { registerFinder } from "@/agent-view/find/registry";
import { otherAgents } from "@/agent-view/quotes";
import { useServices, useView } from "@/app/services";
import { type MenuEntry, useComposerNative } from "@/composer/native";
import { cn } from "@/lib/utils";
import { onMac } from "@/native/platform";
import type { AgentId } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { OUTPUT_LINES } from "@maxhq/shared/terminal/output";
import type { TerminalEntity } from "@maxhq/shared/terminal/select";

import { addPicked, restartTab } from "./actions";
import { CommandMenu, type MarkedAt } from "./command-menu";
import { LiveTerminalHost } from "./live-terminal";
import { registryOf } from "./live";
import type { CommandOutput, Picked, TermView } from "./term-view";

/** The bar over a terminal that asks for a password. */
export const LockBar = () => {
	useI18n();
	return (
		<div
			role="status"
			className="flex shrink-0 items-center gap-2 border-b border-border bg-status-busy/10 px-3 py-[5px] text-xs text-status-busy-text"
		>
			<Lock aria-hidden size={13} strokeWidth={2} className="shrink-0" />
			{translate(
				"Password prompt. What you type goes only to this terminal and isn't shown or recorded."
			)}
		</div>
	);
};

/** How an exited terminal ended, and a button that starts it again. */
const ExitBar = ({ terminal }: { terminal: TerminalEntity }) => {
	useI18n();
	const services = useServices();
	const { status } = terminal;
	if (status.state !== "exited") return null;
	return (
		<div className="flex shrink-0 items-center gap-2 border-t border-border bg-card px-3 py-1 text-xs">
			<span
				className={status.code === 0 ? "text-status-ready-text" : "text-status-error-text"}
			>
				{status.code === null
					? translate("Signal {{signal}}", { signal: status.signal ?? "?" })
					: translate("Exit {{exit_code}}", { exit_code: status.code })}
			</span>
			<button
				type="button"
				onClick={() => void restartTab(services, terminal)}
				className="inline-flex h-[22px] cursor-pointer items-center gap-1.5 rounded-[5px] px-[7px] text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
			>
				<RotateCcw aria-hidden size={12} strokeWidth={2} />
				{translate("Restart")}
			</button>
		</div>
	);
};

/** Find in one terminal, from where its selection is. */
const FindBar = ({
	terminal,
	asked,
	close,
}: {
	terminal: TerminalEntity;
	/** Bumps each time find is asked for, so the field takes focus again. */
	asked: number;
	close: () => void;
}) => {
	useI18n();
	const services = useServices();
	const [query, setQuery] = useState("");
	const [missed, setMissed] = useState(false);
	const input = useRef<HTMLInputElement>(null);
	useEffect(() => {
		input.current?.focus();
		input.current?.select();
	}, [asked]);
	const find = (text: string, back: boolean) => {
		const view = registryOf(services)?.view(terminal.id);
		setMissed(text !== "" && !(view?.find(text, back) ?? false));
	};
	const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
		if (event.nativeEvent.isComposing) return;
		if (event.key === "Escape") {
			event.preventDefault();
			event.stopPropagation();
			close();
		} else if (event.key === "Enter" && event.target === input.current) {
			event.preventDefault();
			find(query, event.shiftKey);
		}
	};
	return (
		<div
			role="search"
			aria-label={translate("Find in the terminal")}
			onKeyDown={onKeyDown}
			className="absolute top-2 right-3 z-20 flex h-8 w-[min(300px,calc(100%-1.5rem))] items-center gap-1 rounded-lg border border-input bg-popover pr-1 pl-2.5 shadow-md"
		>
			<input
				ref={input}
				type="text"
				value={query}
				aria-label={translate("Find in the terminal")}
				aria-invalid={missed}
				placeholder={translate("Find in the terminal")}
				spellCheck={false}
				autoComplete="off"
				onChange={(event) => {
					setQuery(event.target.value);
					find(event.target.value, false);
				}}
				className={cn(
					"h-full min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted-foreground",
					missed ? "text-destructive" : "text-foreground"
				)}
			/>
			<button
				type="button"
				aria-label={translate("Previous match")}
				onClick={() => find(query, true)}
				className={ICON_BUTTON}
			>
				<ChevronUp aria-hidden size={14} strokeWidth={2.2} />
			</button>
			<button
				type="button"
				aria-label={translate("Next match")}
				onClick={() => find(query, false)}
				className={ICON_BUTTON}
			>
				<ChevronDown aria-hidden size={14} strokeWidth={2.2} />
			</button>
			<button
				type="button"
				aria-label={translate("Close find")}
				onClick={close}
				className={ICON_BUTTON}
			>
				<X aria-hidden size={13} strokeWidth={2.2} />
			</button>
		</div>
	);
};

/** Which way a key jumps between marked commands: Command and Up or Down, or Control and Shift off a Mac. */
export const jumpKey = (e: KeyboardEvent<HTMLElement>): "up" | "down" | null => {
	const held = onMac()
		? e.metaKey && !e.ctrlKey && !e.shiftKey
		: e.ctrlKey && e.shiftKey && !e.metaKey;
	if (!held || e.altKey) return null;
	return e.key === "ArrowUp" ? "up" : e.key === "ArrowDown" ? "down" : null;
};

/** Whether a key sends to the agent: Command and Return, or Control, Shift and Return off a Mac. */
export const sendKey = (e: KeyboardEvent<HTMLElement>): boolean => {
	const held = onMac()
		? e.metaKey && !e.ctrlKey && !e.shiftKey
		: e.ctrlKey && e.shiftKey && !e.metaKey;
	return held && !e.altKey && e.key === "Enter";
};

/**
 * What Send to agent takes from a terminal: the picked text, or with none,
 * the output of the last command its shell marked, or a run's last lines.
 * Null when there's nothing to send.
 */
export const sendable = (
	shown: TermView,
	terminal: TerminalEntity
): Picked | CommandOutput | null => {
	const picked = shown.selection();
	if (picked) return picked;
	const last = shown.commands().at(-1);
	const output = last ? shown.output(last.id) : null;
	if (output && output.text.trim() !== "") return output;
	if (terminal.kind.type !== "run") return null;
	const { text, total } = shown.text(OUTPUT_LINES);
	if (text.trim() === "") return null;
	const count = text.split("\n").length;
	return { text, lines: { first: Math.max(1, total - count + 1), last: total } };
};

/** The menu's key for Send to agent, as the native menu spells it. */
const sendAccelerator = () => (onMac() ? "Cmd+Enter" : "Ctrl+Shift+Enter");

const FLOAT_BUTTON =
	"inline-flex h-6 cursor-pointer items-center gap-1.5 rounded-[5px] px-2 text-[12px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

/**
 * One terminal and what goes with it. Keys typed in it go to the terminal,
 * and picked text can go to `agent`'s chat, or another agent's.
 */
export const TerminalBody = ({
	terminal,
	className,
}: {
	terminal: TerminalEntity;
	className?: string;
}) => {
	useI18n();
	const services = useServices();
	const native = useComposerNative();
	// An agent you only watch has no composer for picked text.
	const own = useView((s) => s.view.tables.agent.get(terminal.agent)?.control !== "observed");
	const body = useRef<HTMLDivElement>(null);
	// Where the Send to agent button shows, while there's picked text.
	const [float, setFloat] = useState<{ x: number; y: number } | null>(null);
	const [finding, setFinding] = useState(0);
	const [marked, setMarked] = useState<MarkedAt | null>(null);
	const id = terminal.id;

	const view = () => registryOf(services)?.view(id) ?? null;

	useEffect(() => {
		const element = body.current;
		return element ? registerFinder(element, () => setFinding((n) => n + 1), true) : undefined;
	}, []);

	// The view is placed by the host below, whose effect runs first.
	useEffect(() => {
		const shown = registryOf(services)?.view(id);
		return shown?.onSelection(() => {
			if (!shown.selection()) setFloat(null);
		});
	}, [services, id]);

	useEffect(
		() =>
			registryOf(services)
				?.view(id)
				?.onMark((mark, anchor) => setMarked({ id: mark, anchor })),
		[services, id]
	);

	const add = (to: AgentId, focus: boolean) => {
		const shown = view();
		const picked = shown ? sendable(shown, terminal) : null;
		if (!shown || !picked) return;
		addPicked(services, to, terminal, picked, shown.text(0).total, focus);
		shown.clearSelection();
		setFloat(null);
	};

	const copy = () => {
		const picked = view()?.selection();
		if (picked) void navigator.clipboard?.writeText(picked.text).catch(() => {});
		view()?.focus();
	};

	// The button shows where you let go of the mouse, kept inside the terminal.
	const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
		const box = body.current?.getBoundingClientRect();
		if (!box || !view()?.selection()) return setFloat(null);
		const x = event.clientX - box.left;
		const y = event.clientY - box.top;
		setFloat({
			x: Math.max(4, Math.min(x, box.width - 170)),
			y: y + 44 > box.height ? Math.max(4, y - 36) : y + 10,
		});
	};

	const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
		if (event.nativeEvent.isComposing) return;
		if (own && sendKey(event)) {
			event.preventDefault();
			add(terminal.agent, true);
			return;
		}
		const way = jumpKey(event);
		if (!way) return;
		event.preventDefault();
		view()?.jump(way === "up");
	};

	const onContextMenu = (event: MouseEvent<HTMLDivElement>) => {
		const shown = view();
		if (!shown) return;
		const picked = shown.selection();
		const something = sendable(shown, terminal) !== null;
		const addTo = (to: AgentId, text: string, focus: boolean): MenuEntry => ({
			kind: "item",
			text,
			enabled: something,
			...(focus ? { accelerator: sendAccelerator() } : {}),
			action: () => add(to, focus),
		});
		const entries: MenuEntry[] = [
			...(own ? [addTo(terminal.agent, translate("Send to agent"), true)] : []),
			{
				kind: "submenu",
				text: translate("Send to another agent"),
				entries: something
					? otherAgents(services, terminal.agent).map((other) =>
							addTo(other.id, other.title, false)
						)
					: [],
			},
			...(picked
				? ([{ kind: "separator" }, { kind: "copy", text: picked.text }] as const)
				: []),
		];
		if (native.contextMenu?.(entries, { x: event.clientX, y: event.clientY }))
			event.preventDefault();
	};

	return (
		<div
			ref={body}
			className={cn("relative flex min-h-0 min-w-0 flex-col", className)}
			onPointerUp={onPointerUp}
			onContextMenu={onContextMenu}
			onKeyDown={onKeyDown}
		>
			{terminal.password_prompt ? <LockBar /> : null}
			<LiveTerminalHost terminal={id} interactive className="min-h-0 flex-1" />
			{float ? (
				<div
					className="absolute z-10 inline-flex gap-1 rounded-[7px] border border-input bg-popover p-[3px] shadow-md"
					style={{ left: float.x, top: float.y }}
					onPointerDown={(event) => event.preventDefault()}
					onPointerUp={(event) => event.stopPropagation()}
				>
					{own ? (
						<button
							type="button"
							className={FLOAT_BUTTON}
							onClick={() => add(terminal.agent, true)}
						>
							<MessageSquarePlus aria-hidden size={13} strokeWidth={1.9} />
							{translate("Send to agent")}
						</button>
					) : null}
					<button
						type="button"
						aria-label={translate("Copy")}
						title={translate("Copy")}
						className={FLOAT_BUTTON}
						onClick={copy}
					>
						<Copy aria-hidden size={13} strokeWidth={1.9} />
					</button>
				</div>
			) : null}
			{finding > 0 ? (
				<FindBar
					terminal={terminal}
					asked={finding}
					close={() => {
						setFinding(0);
						view()?.clearSelection();
						view()?.focus();
					}}
				/>
			) : null}
			{marked ? (
				<CommandMenu
					key={marked.id}
					terminal={terminal}
					marked={marked}
					own={own}
					close={() => setMarked(null)}
				/>
			) : null}
			<ExitBar terminal={terminal} />
		</div>
	);
};
