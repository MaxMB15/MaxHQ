// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Shell mode: a `!` typed at the very start of the composer, or after only
// pills such as the last command's output, turns it into a Shell tag, the
// text goes monospace and Send becomes Run. A second `!`
// makes it Private, whose output the agent sees only once you attach it.
// Backspace on the tag steps back, and the command goes back in the box as
// text. ↑ and ↓ walk the commands you ran in this agent's timeline.
//
// The message you were writing stays in the text box, hidden, unless it
// was one line of plain text: that becomes the command.
//
// On a device with sessions, ↩ runs the command in the agent's session and
// ⌥↩ in a fresh shell. The folder chip shows where the session is, or the
// agent's folder before there's one, and picks another.

import { ArrowRight, ChevronDown, Folder, LockKeyhole, SquareTerminal } from "lucide-react";
import {
	type KeyboardEvent,
	type ReactNode,
	type RefObject,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";

import { useKept } from "@maxhq/shared/agent-view/kept";
import type { AgentId, DeviceId } from "@maxhq/shared/bindings";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import { t, useI18n } from "@maxhq/shared/i18n";
import { isBlank } from "@maxhq/shared/pills/draft";
import { runHistory, type RunWalk, walkRuns } from "@maxhq/shared/terminal/history";
import { supportsTerminals } from "@maxhq/shared/terminal/select";
import {
	openCardOf,
	sessionFolder,
	sessionTerminalOf,
	supportsSessions,
} from "@maxhq/shared/terminal/session";
import {
	bangStep,
	backStep,
	homeShort,
	runPlace,
	type ShellMode,
} from "@maxhq/shared/terminal/shell";
import type { TimelineStore } from "@maxhq/shared/timeline/store";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices, useView } from "@/app/services";
import { withParts } from "@/lib/parts";
import { FolderBrowserDialog } from "@/paths/folder-browser";
import { useTerminalActions } from "@/terminal";

import type { PromptEditorHandle } from "./editor";
import { runSize, startRun, startSessionRun } from "./run-command";
import "./shell-mode.css";

/** The composer's mode and the command it holds. */
export type ShellState = { readonly mode: ShellMode; readonly command: string };

const OFF: ShellState = { mode: "message", command: "" };

// Commands a code block's Run menu puts in a composer, waiting until it takes them.
const waiting = new Map<AgentId, ShellState>();
const listeners = new Map<AgentId, Set<() => void>>();

/**
 * Puts `command` in the agent's composer in shell mode, privately for
 * `!!`, now or when the composer opens.
 */
export const putCommand = (agent: AgentId, command: string, privately: boolean): void => {
	waiting.set(agent, { mode: privately ? "private" : "shell", command });
	for (const listener of [...(listeners.get(agent) ?? [])]) listener();
};

const watchCommands = (agent: AgentId, listener: () => void) => {
	const set = listeners.get(agent) ?? new Set();
	set.add(listener);
	listeners.set(agent, set);
	return () => {
		set.delete(listener);
		if (set.size === 0) listeners.delete(agent);
	};
};

const takeCommand = (agent: AgentId): ShellState | undefined => {
	const put = waiting.get(agent);
	waiting.delete(agent);
	return put;
};

/** The agent's own folder, where its session opens. */
const agentFolder = (view: ViewState, agent: AgentId): string | null => {
	const record = view.tables.agent.get(agent);
	return (record && view.tables.workspace.get(record.workspace)?.location.path) ?? null;
};

/** Where the caret goes once the mode changes: the command field at an offset, or the text box. */
type Caret = { readonly to: "field"; readonly at: number | "end" } | { readonly to: "editor" };

export type ShellModeProps = {
	readonly agent: AgentId;
	readonly device: DeviceId;
	readonly timeline: TimelineStore;
	readonly editor: RefObject<PromptEditorHandle | null>;
	/** The composer's box, whose width the command's card takes. */
	readonly box: RefObject<HTMLElement | null>;
	/** Shows why a command didn't run, or clears that with null. */
	readonly onError: (error: string | null) => void;
};

/** What the composer draws for shell mode. */
export type ShellModeParts = {
	readonly mode: ShellMode;
	/** For the text box's `!` at the start or after only pills, while the device has terminals. */
	readonly onBang: (() => boolean) | undefined;
	/** The tag, the command and the hint, in shell mode. */
	readonly field: ReactNode;
	/** Run, in place of the send buttons, in shell mode. */
	readonly runButton: ReactNode;
};

/** Shell mode for one agent's composer. */
export const useShellMode = ({
	agent,
	device,
	timeline,
	editor,
	box,
	onError,
}: ShellModeProps): ShellModeParts => {
	useI18n();
	const services = useServices();
	const actions = useTerminalActions();
	const view = useView((state) => state.view);
	const supported = supportsTerminals(view, device);
	const sessions = supported && supportsSessions(view, device);
	const place = useMemo(() => runPlace(view, agent), [view, agent]);
	const session = sessions ? sessionTerminalOf(view, agent) : undefined;
	// The card the next command goes in, and where the session is.
	const card = useSyncExternalStore(timeline.subscribe, () =>
		sessions ? openCardOf(timeline.getState()) : undefined
	);
	const sessionAt = useSyncExternalStore(timeline.subscribe, () =>
		sessionFolder(session, timeline.getState())
	);
	const [browsing, setBrowsing] = useState(false);
	const [state, setState] = useKept<ShellState>(`shell:${agent}`, OFF);
	const [running, setRunning] = useState(false);
	const field = useRef<HTMLTextAreaElement>(null);
	const walk = useRef<RunWalk | null>(null);
	const caret = useRef<Caret | null>(null);
	const mode = supported ? state.mode : "message";

	const change = (next: ShellState, to: Caret | null) => {
		caret.current = to;
		if (next.command !== state.command) walk.current = null;
		setState(next);
	};

	// The caret moves once the field shows or hides.
	useLayoutEffect(() => {
		const to = caret.current;
		caret.current = null;
		if (to === null) return;
		if (to.to === "editor") {
			editor.current?.focus();
			return;
		}
		const input = field.current;
		if (!input) return;
		input.focus();
		const at = to.at === "end" ? input.value.length : to.at;
		input.setSelectionRange(at, at);
	}, [state, editor]);

	// The field grows with the lines of a command, up to its limit.
	useLayoutEffect(() => {
		const input = field.current;
		if (!input) return;
		input.style.height = "auto";
		input.style.height = `${input.scrollHeight}px`;
	}, [state.command, mode]);

	// A command from a code block's Run menu.
	useEffect(() => {
		const take = () => {
			const put = takeCommand(agent);
			if (put) {
				walk.current = null;
				caret.current = { to: "field", at: "end" };
				setState(put);
			}
		};
		take();
		return watchCommands(agent, take);
	}, [agent, setState]);

	const onBang = () => {
		const handle = editor.current;
		if (!handle) return false;
		const segments = handle.segments();
		// One line of plain text becomes the command; anything more waits, hidden.
		const plain = segments.every((segment) => segment.type === "text");
		const text = handle.text();
		const line = plain && !text.includes("\n");
		if (line && text !== "") handle.setSegments([]);
		change({ mode: "shell", command: line ? text : "" }, { to: "field", at: 0 });
		return true;
	};

	/** Back to a message, with the command as its text, before what's there. */
	const leave = () => {
		const { command } = state;
		const handle = editor.current;
		if (handle && command !== "") {
			if (isBlank({ segments: handle.segments(), gitItems: [] }))
				handle.setSegments([{ type: "text", text: command }]);
			else handle.replaceRange(0, 0, [{ type: "text", text: `${command} ` }]);
		}
		change(OFF, { to: "editor" });
	};

	/** Runs the command, in the session unless `fresh` asks for a shell of its own. */
	const run = async (fresh = false) => {
		const { command } = state;
		if (running || command.trim() === "") return;
		const privately = mode === "private";
		setRunning(true);
		onError(null);
		const current = () => services.view.getState().view;
		const size = runSize(box.current?.clientWidth ?? 0, actions.cellSize().width);
		const started = sessions
			? await startSessionRun(services.source, current, {
					agent,
					command,
					private: privately,
					fresh,
					size,
				})
			: await startRun(services.source, current, {
					agent,
					command,
					private: privately,
					size,
				});
		setRunning(false);
		if (started.error !== undefined) {
			onError(started.error);
			return;
		}
		change(OFF, { to: "editor" });
	};

	/** Moves the session to a folder you picked, or opens it there. */
	const moveTo = async (path: string) => {
		onError(null);
		const reason = failureOf(await attempt(() => services.source.sessionCd(agent, path)));
		if (reason !== null)
			onError(
				reason
					? t("Couldn't move the session. {{reason}}.", { reason })
					: t("Couldn't move the session.")
			);
	};

	const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
		if (event.nativeEvent.isComposing) return;
		const input = event.currentTarget;
		const atStart = input.selectionStart === 0 && input.selectionEnd === 0;
		const plainKey = !event.metaKey && !event.ctrlKey && !event.altKey;
		if (event.key === "!" && atStart && plainKey) {
			const next = bangStep(mode);
			if (next === null) return;
			event.preventDefault();
			change({ ...state, mode: next }, { to: "field", at: 0 });
			return;
		}
		if (event.key === "Backspace" && atStart && plainKey) {
			event.preventDefault();
			const back = backStep(mode);
			if (back === "message") leave();
			else change({ ...state, mode: back }, { to: "field", at: 0 });
			return;
		}
		if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
			event.preventDefault();
			void run();
			return;
		}
		if (event.key === "Enter" && event.altKey && sessions && !event.shiftKey) {
			event.preventDefault();
			void run(true);
			return;
		}
		const up = event.key === "ArrowUp";
		if ((up || event.key === "ArrowDown") && plainKey && !event.shiftKey) {
			const before = input.value.slice(0, input.selectionStart);
			const after = input.value.slice(input.selectionEnd);
			// ↑ walks only from the first line and ↓ only from the last.
			if (up ? before.includes("\n") : after.includes("\n")) return;
			const step = walkRuns(
				runHistory(timeline.getState()),
				walk.current,
				up ? "backward" : "forward",
				state.command
			);
			if (step === null) return;
			event.preventDefault();
			caret.current = { to: "field", at: "end" };
			walk.current = step.walk;
			setState({ ...state, command: step.text });
		}
	};

	if (mode === "message")
		return { mode, onBang: supported ? onBang : undefined, field: null, runButton: null };

	const privately = mode === "private";
	const folder = sessionAt ?? agentFolder(view, agent);
	const sessionWhere = place && (
		<div className="shell-hint shell-where">
			<button
				type="button"
				className="shell-chip"
				aria-label={t("Pick the folder the session runs in")}
				title={folder ?? undefined}
				onClick={() => setBrowsing(true)}
			>
				<Folder aria-hidden className="size-3 shrink-0" strokeWidth={2} />
				<span className="shell-folder">{folder ? homeShort(folder) : place.folder}</span>
				<ChevronDown aria-hidden className="size-3 shrink-0" strokeWidth={2} />
			</button>
			<span>{withParts("on {{device}}", { device: <b>{place.deviceName}</b> })}</span>
			{card ? (
				<span className="shell-count">
					{t("Session · {{count, number}} commands", { count: card.blocks.length })}
				</span>
			) : null}
			<FolderBrowserDialog
				device={place.device}
				start={folder ?? "~"}
				open={browsing}
				onOpenChange={setBrowsing}
				onChoose={(path) => void moveTo(path)}
			/>
		</div>
	);
	const sessionHint = (
		<div className="shell-hint">
			<span>
				{withParts("{{key}} run in the session", {
					key: <kbd className="shell-key">↩</kbd>,
				})}
			</span>
			<span>
				{withParts("{{key}} run in a fresh shell", {
					key: <kbd className="shell-key">⌥↩</kbd>,
				})}
			</span>
			{privately ? (
				<span>{t("The agent won't see the output")}</span>
			) : (
				<span>
					{withParts("{{bangs}} keeps this one private", {
						bangs: <span className="font-mono">!!</span>,
					})}
				</span>
			)}
		</div>
	);
	const where = place && (
		<span>
			{withParts(
				place.container === null
					? "Runs in {{folder}} on {{device}}"
					: "Runs in {{folder}} in {{container}} on {{device}}",
				{
					folder: <b className="shell-folder">{place.folder}</b>,
					device: <b>{place.deviceName}</b>,
					...(place.container === null ? {} : { container: <b>{place.container}</b> }),
				}
			)}
		</span>
	);
	return {
		mode,
		onBang: undefined,
		field: (
			<div className="shell-field">
				<div className="shell-line">
					<span className="shell-tag">
						{privately ? (
							<LockKeyhole aria-hidden className="size-3" strokeWidth={2.2} />
						) : (
							<SquareTerminal aria-hidden className="size-3" strokeWidth={2.2} />
						)}
						{privately ? t("Private") : t("Shell")}
					</span>
					<textarea
						ref={field}
						className="shell-command"
						rows={1}
						value={state.command}
						aria-label={privately ? t("Command to run privately") : t("Command to run")}
						spellCheck={false}
						autoCorrect="off"
						autoCapitalize="off"
						autoComplete="off"
						onChange={(event) => {
							walk.current = null;
							setState({ ...state, command: event.target.value });
						}}
						onKeyDown={onKeyDown}
					/>
				</div>
				{sessions ? (
					<>
						{sessionWhere}
						{sessionHint}
					</>
				) : (
					<div className="shell-hint">
						{where}
						{privately ? (
							<span>{t("The agent won't see the output")}</span>
						) : (
							<>
								<span>{t("Output goes with your next message")}</span>
								<span>
									{withParts("{{bangs}} keeps it private", {
										bangs: <span className="font-mono">!!</span>,
									})}
								</span>
								<span>
									{withParts("{{key}} earlier commands", {
										key: <kbd className="shell-key">↑</kbd>,
									})}
								</span>
							</>
						)}
					</div>
				)}
			</div>
		),
		runButton: (
			<button
				type="button"
				className="shell-run"
				disabled={running || state.command.trim() === ""}
				onClick={() => void run()}
			>
				<ArrowRight aria-hidden className="size-3" strokeWidth={2.4} />
				{t("Run")}
			</button>
		),
	};
};
