// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A "You ran" card: a command you ran with `!` in the agent's folder.
// While it runs it shows the live terminal, a line to type into it, Stop
// and Move to terminal. A command asking for a password gets a secure
// field instead. Once it ends it shows the output, folded to its last
// lines, with Ask about this and Run again. A command run privately with
// `!!` starts folded and reaches the agent only once you attach it. Moved
// to the terminal, it shrinks to a line that links to its tab. The first
// time a command this window started asks for a password, its secure field
// takes focus from the composer.

import { Menu } from "@base-ui/react/menu";
import {
	ArrowRight,
	ChevronUp,
	CircleCheck,
	CircleX,
	Ellipsis,
	LockKeyhole,
	MessageSquare,
	PanelBottom,
	Square,
	SquareTerminal,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { useRow } from "@maxhq/shared/agent-view/context";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import { formatTook } from "@maxhq/shared/agent-view/format";
import type { Agent, TerminalStatus, Timestamp } from "@maxhq/shared/bindings";
import {
	attempt,
	type BusySend,
	failureOf,
	sendModeFor,
	turnOf,
} from "@maxhq/shared/composer/send";
import type { ActionResult } from "@maxhq/shared/data/source";
import { t, useI18n } from "@maxhq/shared/i18n";
import { type OutputRef, type Segment, toUserInput } from "@maxhq/shared/pills/draft";
import { stripAnsi } from "@maxhq/shared/timeline/ansi";
import {
	fillOutput,
	OUTPUT_LINES,
	runStatus,
	startedOutput,
	type UserRunItem,
} from "@maxhq/shared/terminal/output";
import { type TerminalEntity, terminalById } from "@maxhq/shared/terminal/select";
import { freshNote } from "@maxhq/shared/terminal/session";
import { homeShort, programOf } from "@maxhq/shared/terminal/shell";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";

import { type Services, useServices, useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { takeOutputBack, useOutputHeld } from "@/composer/held-pills";
import { MENU_ITEM, MENU_POPUP } from "@/composer/pickers";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { askedHere, firstAskHere, runSize, startRun } from "@/composer/run-command";
import { useTypingPrefs } from "@/composer/typing-prefs";
import { withParts } from "@/lib/parts";
import { cn } from "@/lib/utils";
import { agreedSend } from "@/settings/auto-consent";
import { showDeviceCard } from "@/settings/store";
import { LiveTerminal, useTerminalActions } from "@/terminal";

import { AnsiLines, ansiLines } from "../ansi-text";
import { Elapsed } from "../elapsed";
import { Spinner } from "../icons";
import { InputRow, PasswordField } from "./run-input";

/** How many of its last lines a finished command shows until you ask for all. */
export const FOLDED_LINES = 20;

/** The rows the live terminal takes at most. */
export const LIVE_ROWS = 24;

export const BUTTON =
	"inline-flex h-[22px] shrink-0 cursor-pointer items-center gap-[5px] rounded-md border border-transparent px-2 text-xs whitespace-nowrap text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

export const BOXED = cn(BUTTON, "border-border");

const KBD =
	"rounded-[4px] border border-border px-1 font-mono text-[10.5px] text-subtle-foreground";

/** A running clock, such as "0:08" or "1:02:03". */
export const runClock = (ms: number): string => {
	const total = Math.max(0, Math.floor(ms / 1000));
	const hours = Math.floor(total / 3600);
	const minutes = Math.floor((total % 3600) / 60);
	const seconds = String(total % 60).padStart(2, "0");
	return hours > 0
		? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
		: `${minutes}:${seconds}`;
};

/** How a command ended, such as "exit 1 · 4.2 s". */
export const endedText = (
	status: Extract<TerminalStatus, { state: "exited" }>,
	took: number | null
) => {
	const how = status.code !== null ? t("exit {{code}}", { code: status.code }) : t("Stopped");
	return took === null ? how : `${how} · ${formatTook(took)}`;
};

/** What a card's button tried, when it didn't go. */
type Failed = "stop" | "kill" | "move";

/** Why a button's action didn't go, with the reason when there is one. */
const failedText = (failed: Failed, reason: string): string => {
	switch (failed) {
		case "stop":
			return reason
				? t("Couldn't stop the command. {{reason}}.", { reason })
				: t("Couldn't stop the command.");
		case "kill":
			return reason
				? t("Couldn't kill the command. {{reason}}.", { reason })
				: t("Couldn't kill the command.");
		case "move":
			return reason
				? t("Couldn't move the command to the terminal. {{reason}}.", { reason })
				: t("Couldn't move the command to the terminal.");
	}
};

/** The command's output pill, as the composer or a message takes it now. */
const outputOf = (
	item: UserRunItem,
	terminal: TerminalEntity | undefined,
	device: string,
	live: { readonly text: string; readonly total: number } | null
): OutputRef =>
	fillOutput(
		startedOutput({
			command: item.command,
			deviceName: device,
			cwd: item.cwd,
			terminal: item.terminal,
			at: terminal?.started_at ?? item.started_at ?? Date.now(),
		}),
		{ item, terminal, live, now: Date.now() }
	);

/** How a message a card sent on its own ended: whether it went, and the error to show. */
export type SentAlone = { readonly sent: boolean; readonly error: string | null };

/**
 * Sends a message of its own to the agent once you agree to it, holding
 * the segments `segments` gives then, so live output is as fresh as it
 * gets. `word` says why the device refused it.
 */
export const sendAlone = async (
	services: Services,
	agent: Agent,
	busySend: BusySend,
	segments: () => Segment[],
	word: (result: ActionResult) => string | null
): Promise<SentAlone> => {
	const agreed = await agreedSend(services, agent);
	if (!agreed.go) return { sent: false, error: agreed.failure };
	const input = toUserInput({ segments: segments(), gitItems: [] }, () => null);
	const mode = sendModeFor(turnOf(agent.activity), busySend, false);
	const error = word(await attempt(() => services.source.sendInput(agent.id, input, mode)));
	return { sent: error === null, error };
};

/** The card for a command you ran with `!`. */
export const UserRunCard = ({ item }: { item: UserRunItem }) => {
	const view = useView((state) => state.view);
	const terminal = terminalById(view, item.terminal);
	const status = runStatus(terminal, item) ?? item.process;
	const moved = item.moved || terminal?.moved === true;
	if (moved && status.state === "running") return <MovedStub item={item} />;
	return <RunCard item={item} terminal={terminal} status={status} />;
};

/** A command moved to the terminal, as a line that links to its tab. */
const MovedStub = ({ item }: { item: UserRunItem }) => {
	useI18n();
	const { agent } = useRow();
	const actions = useTerminalActions();
	return (
		<div
			data-user-run="moved"
			className="my-1 flex flex-wrap items-center gap-2 rounded-lg border border-dashed border-border px-3 py-1.5 text-xs text-muted-foreground"
		>
			<SquareTerminal aria-hidden className="size-3.5 shrink-0" strokeWidth={1.9} />
			<span className="min-w-0 break-all">
				{withParts("You ran {{command}}", {
					command: <span className="font-mono text-foreground">{item.command}</span>,
				})}
			</span>
			<span className="text-subtle-foreground">
				{t("· moved to the terminal, still running")}
			</span>
			<button
				type="button"
				className={cn(BOXED, "ml-auto")}
				onClick={() => actions.showInDrawer(agent, item.terminal)}
			>
				{t("Show in terminal")}
			</button>
		</div>
	);
};

const RunCard = ({
	item,
	terminal,
	status,
}: {
	item: UserRunItem;
	terminal: TerminalEntity | undefined;
	status: TerminalStatus;
}) => {
	useI18n();
	const row = useRow();
	const services = useServices();
	const actions = useTerminalActions();
	const view = useView((state) => state.view);
	const { busySend } = useTypingPrefs();
	const card = useRef<HTMLElement>(null);
	const [stopped, setStopped] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [open, setOpen] = useFold(`run-open:${item.id}`, item.id);
	const [all, setAll] = useFold(`run-all:${item.id}`, item.id);
	const held = useOutputHeld(row.agent, item.terminal);
	const device = deviceName(view, item.device);
	const running = status.state === "running";
	const asking = running && terminal?.password_prompt === true;
	const shown = !item.private || open;
	const acts = !row.readOnly;
	// When the card first drew, for a command that hasn't said when it started.
	const [drawn] = useState<Timestamp>(() => Date.now());
	const started: Timestamp = terminal?.started_at ?? item.started_at ?? drawn;
	// The field takes focus as it shows, and only for the command's first ask.
	const takeFocus = asking && firstAskHere(item.terminal);
	useEffect(() => {
		if (asking) askedHere(item.terminal);
	}, [asking, item.terminal]);

	/** Runs an action, and says why when it doesn't go. */
	const act = async (action: () => Promise<ActionResult>, failed: Failed) => {
		setError(null);
		const reason = failureOf(await attempt(action));
		if (reason !== null) setError(failedText(failed, reason));
	};

	const stop = () => {
		if (stopped) {
			void act(() => services.source.closeTerminal(item.terminal, true), "kill");
			return;
		}
		setStopped(true);
		void act(() => services.source.interruptTerminal(item.terminal), "stop");
	};

	const move = () => void act(() => services.source.moveTerminal(item.terminal), "move");

	const pill = () =>
		outputOf(
			item,
			terminal,
			device,
			running ? actions.text(item.terminal, OUTPUT_LINES) : null
		);

	const ask = async () => {
		const agent = view.tables.agent.get(row.agent);
		if (!agent || busy) return;
		setBusy(true);
		setError(null);
		try {
			const { sent, error } = await sendAlone(
				services,
				agent,
				busySend,
				() => [
					{ type: "text", text: `${t("Look at this output.")} ` },
					{ type: "pill", pill: pill() },
				],
				(result) => failureOf(result, "send")
			);
			setError(error);
			// It went with this message, so the next one doesn't take it again.
			if (sent) takeOutputBack(row.agent, item.terminal);
		} finally {
			setBusy(false);
		}
	};

	const runAgain = async () => {
		if (busy) return;
		setBusy(true);
		setError(null);
		const result = await startRun(services.source, () => services.view.getState().view, {
			agent: row.agent,
			command: item.command,
			private: item.private,
			size: runSize(card.current?.clientWidth ?? 0, actions.cellSize().width),
		});
		setBusy(false);
		if (result.error !== undefined) setError(result.error);
	};

	const attach = () => quoteInbox.send(row.agent, pill(), true);

	const state: ReactNode = asking ? (
		<span className="inline-flex items-center gap-[5px] text-xs whitespace-nowrap text-status-busy-text">
			<LockKeyhole aria-hidden className="size-3" strokeWidth={2.2} />
			{t("Waiting for your password")}
		</span>
	) : running ? (
		<span className="inline-flex items-center gap-[5px] text-xs whitespace-nowrap text-status-busy-text">
			<Spinner size={11} />
			<span>
				{withParts("Running {{time}}", {
					time: <Elapsed since={started} format={runClock} />,
				})}
			</span>
		</span>
	) : status.state === "exited" ? (
		<span
			className={cn(
				"inline-flex items-center gap-[5px] text-xs whitespace-nowrap",
				status.code === 0 ? "text-status-ready-text" : "text-status-error-text"
			)}
		>
			{status.code === 0 ? (
				<CircleCheck aria-hidden className="size-3" strokeWidth={2.2} />
			) : (
				<CircleX aria-hidden className="size-3" strokeWidth={2.2} />
			)}
			{endedText(status, item.duration_ms)}
		</span>
	) : null;

	const stopButton = (
		<button type="button" className={BOXED} onClick={stop}>
			<Square aria-hidden className="size-2.5 fill-current" strokeWidth={0} />
			{stopped ? t("Kill") : t("Stop")}
			{!stopped && !asking ? (
				<kbd aria-hidden className={KBD}>
					⌃C
				</kbd>
			) : null}
		</button>
	);

	const buttons: ReactNode = !acts ? null : asking ? (
		stopButton
	) : running ? (
		<>
			{stopButton}
			<button type="button" className={BOXED} onClick={move}>
				<PanelBottom aria-hidden className="size-3" strokeWidth={2} />
				{t("Move to terminal")}
			</button>
		</>
	) : item.private ? (
		<>
			<button type="button" className={BUTTON} onClick={() => setOpen(!open)}>
				{open ? t("Hide output") : t("Show output")}
			</button>
			{held ? null : (
				<button type="button" className={BOXED} onClick={attach}>
					{t("Attach to next message")}
				</button>
			)}
		</>
	) : (
		<>
			<button type="button" className={BOXED} disabled={busy} onClick={() => void ask()}>
				<MessageSquare aria-hidden className="size-3" strokeWidth={2} />
				{t("Ask about this")}
			</button>
			<button
				type="button"
				className={BUTTON}
				disabled={busy}
				onClick={() => void runAgain()}
			>
				{t("Run again")}
			</button>
			<CopyButton text={stripAnsi(item.output)} icon={false} className={BUTTON} />
		</>
	);

	const menu = acts && !asking && (
		<Menu.Root>
			<Menu.Trigger
				aria-label={t("More for this command")}
				className={cn(BUTTON, "px-1.5 data-popup-open:bg-accent")}
			>
				<Ellipsis aria-hidden className="size-3.5" strokeWidth={2.2} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="end" sideOffset={6} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[220px] text-[12.5px]")}>
						<Menu.Item
							className={MENU_ITEM}
							onClick={() => void navigator.clipboard?.writeText(item.command)}
						>
							{t("Copy command")}
						</Menu.Item>
						{item.private && !running ? (
							<Menu.Item className={MENU_ITEM} onClick={() => void runAgain()}>
								{t("Run again")}
							</Menu.Item>
						) : null}
						{!running && !item.private && !held ? (
							<Menu.Item className={MENU_ITEM} onClick={attach}>
								{t("Attach to next message")}
							</Menu.Item>
						) : null}
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);

	return (
		<section
			ref={card}
			aria-label={t("You ran {{command}}", { command: item.command })}
			data-user-run={asking ? "password" : running ? "running" : "finished"}
			className="my-2 overflow-hidden rounded-[10px] border border-border bg-background"
		>
			<div className="flex flex-wrap items-center gap-x-2 gap-y-1 py-[7px] pr-2 pl-3">
				{item.private ? (
					<LockKeyhole
						aria-hidden
						className="size-3.5 shrink-0 text-muted-foreground"
						strokeWidth={1.9}
					/>
				) : (
					<SquareTerminal
						aria-hidden
						className="size-3.5 shrink-0 text-muted-foreground"
						strokeWidth={1.9}
					/>
				)}
				<span className="min-w-0 text-xs break-all text-muted-foreground">
					{withParts(
						item.private ? "You ran privately {{command}}" : "You ran {{command}}",
						{
							command: (
								<span className="font-mono text-[12.5px] text-foreground">
									{item.command}
								</span>
							),
						}
					)}
				</span>
				{state}
				<span className="ml-auto flex flex-wrap items-center gap-1">
					{buttons}
					{menu}
				</span>
			</div>
			<div className="-mt-1 pr-3 pb-[7px] pl-[34px] font-mono text-[11.5px] text-subtle-foreground">
				{homeShort(item.cwd)} · {device}
			</div>
			<FreshLine item={item} device={device} />
			{shown && running ? (
				<div className="border-t border-border bg-subpanel px-3 py-2 empty:hidden">
					<LiveTerminal
						device={item.device}
						terminal={item.terminal}
						maxRows={LIVE_ROWS}
						interactive={false}
					/>
				</div>
			) : null}
			{shown && !running ? (
				<FinishedOutput
					output={item.output}
					truncated={item.output_truncated}
					all={all}
					onAll={() => setAll(true)}
				/>
			) : null}
			{acts && running && !asking ? (
				<InputRow
					onSend={(data) => actions.sendInput(item.device, item.terminal, data)}
					onInterrupt={stop}
				/>
			) : null}
			{acts && asking ? (
				<PasswordField
					label={t("Password for {{program}} on {{device}}", {
						program: programOf(item.command),
						device,
					})}
					onSend={(data) => actions.sendInput(item.device, item.terminal, data)}
					takeFocus={takeFocus}
				/>
			) : null}
			{acts && !running && held ? (
				<div className="flex flex-wrap items-center gap-2 border-t border-border py-1.5 pr-2 pl-3 text-xs text-muted-foreground">
					<ArrowRight aria-hidden className="size-3" strokeWidth={2} />
					{t("Goes with your next message")}
					<button
						type="button"
						className={cn(BUTTON, "ml-auto")}
						onClick={() => takeOutputBack(row.agent, item.terminal)}
					>
						{t("Keep private")}
					</button>
				</div>
			) : null}
			{error !== null ? (
				<p
					role="alert"
					className="border-t border-border px-3 py-1.5 text-[11.5px] text-status-error-text"
				>
					{error}
				</p>
			) : null}
		</section>
	);
};

/**
 * Why a command ran in a shell of its own rather than the agent's session,
 * in one quiet line. Shell integration being off links to its setting.
 */
const FreshLine = ({ item, device }: { item: UserRunItem; device: string }) => {
	useI18n();
	const services = useServices();
	const view = useView((state) => state.view);
	const note = freshNote(item.fresh);
	if (!note) return null;
	const link = linkForDevice(view, item.device);
	const setting =
		link === undefined ? (
			t("Shell integration")
		) : (
			<button
				type="button"
				className="cursor-pointer text-muted-foreground underline underline-offset-2 hover:text-foreground"
				onClick={() => showDeviceCard(services, link.id, "terminals")}
			>
				{t("Shell integration")}
			</button>
		);
	return (
		<p
			data-fresh={item.fresh}
			className="-mt-1 pr-3 pb-[7px] pl-[34px] text-[11.5px] text-subtle-foreground"
		>
			{note.setting ? withParts(note.key, { setting, device }) : t(note.key)}
		</p>
	);
};

/** A finished command's output, its last lines until you ask for all. */
export const FinishedOutput = ({
	output,
	truncated,
	all,
	onAll,
}: {
	output: string;
	truncated: boolean;
	all: boolean;
	onAll: () => void;
}) => {
	useI18n();
	const lines = useMemo(() => ansiLines(output), [output]);
	const hidden = all ? 0 : Math.max(0, lines.length - FOLDED_LINES);
	return (
		<div className="border-t border-border bg-subpanel">
			{hidden > 0 ? (
				<button
					type="button"
					onClick={onAll}
					className="flex h-6 w-full cursor-pointer items-center gap-1.5 px-3 pt-1 text-[11.5px] text-muted-foreground hover:text-foreground"
				>
					<ChevronUp size={10} strokeWidth={2.4} aria-hidden />
					{t("Show all {{count, number}} lines", { count: lines.length })}
				</button>
			) : null}
			{lines.length === 0 ? (
				<p className="px-3 py-2 text-[11.5px] text-muted-foreground">{t("No output")}</p>
			) : (
				<pre
					data-find="plain"
					className="max-h-[60vh] overflow-auto px-3 py-2 font-mono text-xs leading-[1.55] text-terminal"
				>
					<AnsiLines lines={lines.slice(hidden)} />
				</pre>
			)}
			{truncated ? (
				<p className="border-t border-border px-3 py-1 text-[11px] text-subtle-foreground">
					{t("MaxHQ kept only part of the output.")}
				</p>
			) : null}
		</div>
	);
};
