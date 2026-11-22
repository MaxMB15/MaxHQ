// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A session card: the commands you ran with `!` in the agent's session
// between two of your messages, each as a block with its output, exit
// code and whether the agent sees it. Until a message closes the card,
// each block switches between Shared and Private, Close shell in its ⋯
// menu closes the shell and Send now sends the shared blocks on their own,
// before your next message. Open in terminal shows the shell in the
// terminal panel. A closed card says whether its commands went with that
// message. A block that still runs shows the live terminal, a line to type
// into it and, when it asks for a password, a secure field. It has Stop,
// Force stop once the command ignored Stop, and Open in terminal. Force
// stop ends only the command, so the session keeps its folder and
// variables, and needs a device that says so.
//
// What you typed by hand in the session's terminal shows as compact
// Private rows, "typed in the terminal", that open to their output, and a
// run of more than a few folds into one row. A command whose program takes
// the whole screen is in the terminal, and the card says so in one line.
// One that reads keys one at a time takes your keys in the card once you
// click its output.

import { Menu } from "@base-ui/react/menu";
import {
	ArrowRight,
	ChevronRight,
	Ellipsis,
	Eye,
	LockKeyhole,
	PanelBottom,
	SquareTerminal,
} from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { useRow } from "@maxhq/shared/agent-view/context";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import { formatTook } from "@maxhq/shared/agent-view/format";
import type { SessionBlock } from "@maxhq/shared/bindings";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import { t, useI18n } from "@maxhq/shared/i18n";
import { endedText } from "@maxhq/shared/terminal/ended";
import { handoffOf, takesKeys } from "@maxhq/shared/terminal/handoff";
import { type TerminalEntity, terminalById } from "@maxhq/shared/terminal/select";
import {
	blockRunning,
	blockTyped,
	cardRows,
	cardSummary,
	fillSession,
	sessionPill,
	type UserSessionItem,
} from "@maxhq/shared/terminal/session";
import { homeShort, programOf } from "@maxhq/shared/terminal/shell";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { takeSessionBack, useSessionHeld } from "@/composer/held-pills";
import { MENU_ITEM, MENU_POPUP } from "@/composer/pickers";
import { askedHere, firstAskHere } from "@/composer/run-command";
import { draftsOf } from "@/composer/saved-drafts";
import { addSessionPill, markPlaced, shareBlock } from "@/composer/session-pill";
import { useTypingPrefs } from "@/composer/typing-prefs";
import { withParts } from "@/lib/parts";
import { cn } from "@/lib/utils";
import { LiveTerminal, useTerminalActions } from "@/terminal";

import { Elapsed } from "../elapsed";
import { Spinner } from "../icons";
import {
	heldLook,
	IgnoredStop,
	InputRow,
	KeysHint,
	PasswordField,
	useHeldKeys,
	useStop,
} from "./run-input";
import {
	BOXED,
	BUTTON,
	EndedChip,
	FinishedOutput,
	ForceStopButton,
	HandoffChip,
	InTerminalNote,
	LIVE_ROWS,
	runClock,
	sendAlone,
	StopButton,
	useOpensPanel,
	WaitingChip,
} from "./user-run";

/** Why a card's button didn't go, with the reason when there is one. */
const failed = (what: "end" | "send" | "share" | "stop" | "kill", reason: string): string => {
	switch (what) {
		case "stop":
			return reason
				? t("Couldn't stop the command. {{reason}}.", { reason })
				: t("Couldn't stop the command.");
		case "kill":
			return reason
				? t("Couldn't kill the command. {{reason}}.", { reason })
				: t("Couldn't kill the command.");
		case "end":
			return reason
				? t("Couldn't close the shell. {{reason}}.", { reason })
				: t("Couldn't close the shell.");
		case "send":
			return reason
				? t("Couldn't send the commands. {{reason}}.", { reason })
				: t("Couldn't send the commands.");
		case "share":
			return reason;
	}
};

/** The card for the commands you ran in the agent's session. */
export const UserSessionCard = ({ item }: { item: UserSessionItem }) => {
	useI18n();
	const row = useRow();
	const services = useServices();
	const actions = useTerminalActions();
	const view = useView((state) => state.view);
	const { busySend } = useTypingPrefs();
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const terminal = terminalById(view, item.terminal);
	const device = deviceName(view, item.device);
	const held = useSessionHeld(row.agent, item.id);
	const { total, shared } = cardSummary(item);
	const alive = terminal !== undefined && terminal.status.state === "running";
	const acts = !row.readOnly;
	// A running block has its own Open in terminal beside Stop, so the
	// header leaves it out then rather than showing the button twice. A
	// line you typed has none, since you typed it there.
	const blockOpens =
		acts && item.blocks.some((block) => blockRunning(block) && !blockTyped(block));
	const open = !item.sealed;

	const end = async () => {
		setError(null);
		const result = await attempt(() => services.source.sessionEnd(row.agent));
		let reason = failureOf(result);
		// The device refuses while a command runs, so you're asked first. Any
		// other failure, such as the device being away, only says so.
		const refused = result.status === "ok" && result.data.status === "refused";
		const runs = item.blocks.some(blockRunning) || terminal?.busy === true;
		if (reason !== null && refused && runs) {
			const sure = await requestConfirm(services, {
				title: t("Close the shell?"),
				detail: reason,
				action: t("Close shell"),
			});
			if (!sure) return;
			reason = failureOf(await attempt(() => services.source.sessionEnd(row.agent, true)));
		}
		if (reason !== null) setError(failed("end", reason));
	};

	/** Sends the shared blocks to the agent on their own, as a message holding only the pill. */
	const send = async () => {
		const agent = view.tables.agent.get(row.agent);
		if (!agent || busy) return;
		setBusy(true);
		setError(null);
		try {
			const pill = () =>
				fillSession(sessionPill(item, device), item, {
					now: Date.now(),
					live: (block) => actions.runningText(item.terminal, block.start),
				});
			const { sent, error } = await sendAlone(
				services,
				agent,
				busySend,
				() => [{ type: "pill", pill: pill() }],
				(result) => {
					const reason = failureOf(result);
					return reason === null ? null : failed("send", reason);
				}
			);
			setError(error);
			if (sent) {
				// It went, so the next message doesn't take these commands again,
				// even before the device closes the card.
				markPlaced(draftsOf(services), row.agent, item.id);
				takeSessionBack(row.agent, item.id);
			}
		} finally {
			setBusy(false);
		}
	};

	const footer: ReactNode = open ? (
		<div className="flex flex-wrap items-center gap-2 border-t border-border bg-subpanel py-[7px] pr-2 pl-3 text-xs text-muted-foreground">
			<span>
				{held || !acts
					? t("{{shared}} of {{total}} go with your next message", {
							shared,
							total,
						})
					: t("Not in your next message")}
			</span>
			{acts ? (
				<span className="ml-auto flex flex-wrap items-center gap-1">
					{!held && shared > 0 ? (
						<button
							type="button"
							className={BUTTON}
							onClick={() =>
								addSessionPill(draftsOf(services), row.agent, item, device)
							}
						>
							{t("Add to next message")}
						</button>
					) : null}
					<button
						type="button"
						className={cn(
							BOXED,
							"border-primary bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground"
						)}
						disabled={busy || shared === 0}
						onClick={() => void send()}
					>
						<ArrowRight aria-hidden className="size-3" strokeWidth={2} />
						{t("Send now")}
					</button>
					{alive ? (
						<Menu.Root>
							<Menu.Trigger
								aria-label={t("More for this shell")}
								className={cn(BUTTON, "px-1.5 data-popup-open:bg-accent")}
							>
								<Ellipsis aria-hidden className="size-3.5" strokeWidth={2.2} />
							</Menu.Trigger>
							<Menu.Portal>
								<Menu.Positioner
									side="bottom"
									align="end"
									sideOffset={6}
									className="z-[130]"
								>
									<Menu.Popup
										className={cn(MENU_POPUP, "w-[200px] text-[12.5px]")}
									>
										<Menu.Item className={MENU_ITEM} onClick={() => void end()}>
											{t("Close shell")}
										</Menu.Item>
									</Menu.Popup>
								</Menu.Positioner>
							</Menu.Portal>
						</Menu.Root>
					) : null}
				</span>
			) : null}
		</div>
	) : (
		<div
			data-session-sealed={item.sent ? "sent" : "not-sent"}
			className="flex items-center gap-1.5 border-t border-border bg-subpanel py-[7px] pr-2 pl-3 text-xs text-muted-foreground"
		>
			{item.sent ? <ArrowRight aria-hidden className="size-3" strokeWidth={2} /> : null}
			{item.sent ? t("Sent with your message") : t("Not sent")}
		</div>
	);

	return (
		<section
			aria-label={t("Commands you ran on {{device}}", { device })}
			data-user-session={open ? "open" : "sealed"}
			className="my-2 overflow-hidden rounded-[10px] border border-border bg-background"
		>
			<div className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border py-[7px] pr-2 pl-3">
				<SquareTerminal
					aria-hidden
					className="size-3.5 shrink-0 text-shell"
					strokeWidth={1.9}
				/>
				<span className="min-w-0 text-xs text-muted-foreground">
					{withParts("You ran {{commands}}", {
						commands: (
							<b className="text-[12.5px] font-semibold text-foreground">
								{t("{{count, number}} commands", { count: total })}
							</b>
						),
					})}
				</span>
				<span className="text-[11.5px] text-subtle-foreground">
					{t("on {{device}}", { device })}
				</span>
				{alive && !blockOpens ? (
					<button
						type="button"
						className={cn(BUTTON, "ml-auto")}
						onClick={() => actions.showInDrawer(row.agent, item.terminal)}
					>
						<PanelBottom aria-hidden className="size-3" strokeWidth={2} />
						{t("Open in terminal")}
					</button>
				) : null}
			</div>
			{cardRows(item.blocks).map((shown) =>
				shown.kind === "typed" ? (
					<TypedRun
						key={`typed-${shown.blocks[0]!.id}`}
						item={item}
						blocks={shown.blocks}
						terminal={terminal}
						first={shown.index === 0}
						acts={acts}
						onError={setError}
					/>
				) : blockTyped(shown.block) ? (
					<TypedBlock
						key={shown.block.id}
						item={item}
						block={shown.block}
						terminal={terminal}
						first={shown.index === 0}
						acts={acts}
						onError={setError}
					/>
				) : (
					<Block
						key={shown.block.id}
						item={item}
						block={shown.block}
						terminal={terminal}
						device={device}
						first={shown.index === 0}
						acts={acts}
						onError={setError}
					/>
				)
			)}
			{footer}
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
 * Whether a block goes to the agent: a toggle until a message closes the
 * card, and a quiet Private once it has.
 */
const ShareToggle = ({
	item,
	block,
	acts,
	onError,
}: {
	item: UserSessionItem;
	block: SessionBlock;
	acts: boolean;
	onError: (error: string | null) => void;
}) => {
	useI18n();
	const row = useRow();
	const services = useServices();
	const toggle = async () => {
		onError(null);
		const reason = await shareBlock(services.source, row.agent, item, block);
		if (reason !== null) onError(failed("share", reason));
	};
	if (item.sealed || !acts)
		return block.private ? (
			<span className="ml-auto inline-flex items-center gap-1 text-[11.5px] text-subtle-foreground">
				<LockKeyhole aria-hidden className="size-3" strokeWidth={2} />
				{t("Private")}
			</span>
		) : null;
	return (
		<button
			type="button"
			aria-pressed={!block.private}
			aria-label={
				block.private
					? t("Share {{command}} with the agent", { command: block.command })
					: t("Keep {{command}} private", { command: block.command })
			}
			onClick={() => void toggle()}
			className={cn(
				"ml-auto inline-flex h-[22px] cursor-pointer items-center gap-1 rounded-md border px-[7px] text-[11.5px] whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring",
				block.private
					? "border-status-busy/45 bg-status-busy/10 text-status-busy-text"
					: "border-[var(--pill-output-border)] bg-[var(--pill-output-bg)] text-[var(--pill-output-text)]"
			)}
		>
			{block.private ? (
				<LockKeyhole aria-hidden className="size-3" strokeWidth={2} />
			) : (
				<Eye aria-hidden className="size-3" strokeWidth={2} />
			)}
			{block.private ? t("Private") : t("Shared")}
		</button>
	);
};

/** The chip that says you typed a command in the terminal by hand. */
const TypedChip = () => {
	useI18n();
	return (
		<span
			data-typed-chip
			className="inline-flex items-center rounded-full border border-border px-2 py-px text-[11.5px] whitespace-nowrap text-muted-foreground"
		>
			{t("typed in the terminal")}
		</span>
	);
};

/**
 * A command you typed in the session's terminal, as one compact row. Once
 * it ends, a click opens it to its output.
 */
const TypedBlock = ({
	item,
	block,
	terminal,
	first,
	acts,
	onError,
}: {
	item: UserSessionItem;
	block: SessionBlock;
	terminal: TerminalEntity | undefined;
	first: boolean;
	acts: boolean;
	onError: (error: string | null) => void;
}) => {
	useI18n();
	const [open, setOpen] = useFold(`session-typed:${item.id}:${block.id}`, item.id);
	const [all, setAll] = useFold(`session-all:${item.id}:${block.id}`, item.id);
	const running = blockRunning(block);
	const handoff = handoffOf(running, terminal);
	return (
		<div
			data-session-block={block.id}
			data-typed
			data-private={block.private || undefined}
			className={cn(!first && "border-t border-dashed border-border")}
		>
			<div className="flex flex-wrap items-center gap-x-2 gap-y-1 py-1.5 pr-2 pl-3">
				<button
					type="button"
					disabled={running}
					aria-expanded={running ? undefined : open}
					title={running ? undefined : open ? t("Hide output") : t("Show output")}
					onClick={() => setOpen(!open)}
					className={cn(
						"min-w-0 rounded-sm text-left font-mono text-[12.5px] break-all text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default",
						!running &&
							"cursor-pointer hover:underline hover:decoration-border hover:underline-offset-2",
						block.private && "opacity-50"
					)}
				>
					<span aria-hidden className="text-subtle-foreground">
						${" "}
					</span>
					{block.command}
				</button>
				<TypedChip />
				{running ? (
					handoff !== null ? (
						<HandoffChip handoff={handoff} />
					) : (
						<Spinner size={11} />
					)
				) : null}
				<ShareToggle item={item} block={block} acts={acts} onError={onError} />
			</div>
			{open && !running ? (
				<div className={cn(block.private && "opacity-50")}>
					<FinishedOutput
						output={block.output}
						truncated={block.output_truncated}
						all={all}
						onAll={() => setAll(true)}
					/>
				</div>
			) : null}
		</div>
	);
};

/** A run of commands you typed in the terminal, folded into one row that opens to them. */
const TypedRun = ({
	item,
	blocks,
	terminal,
	first,
	acts,
	onError,
}: {
	item: UserSessionItem;
	blocks: readonly SessionBlock[];
	terminal: TerminalEntity | undefined;
	first: boolean;
	acts: boolean;
	onError: (error: string | null) => void;
}) => {
	useI18n();
	const [open, setOpen] = useFold(`session-typed-run:${item.id}:${blocks[0]!.id}`, item.id);
	const shared = blocks.filter((block) => !block.private).length;
	return (
		<div
			data-typed-run={blocks.length}
			className={cn(!first && "border-t border-dashed border-border")}
		>
			<button
				type="button"
				aria-expanded={open}
				onClick={() => setOpen(!open)}
				className="flex w-full cursor-pointer flex-wrap items-center gap-x-2 gap-y-1 py-1.5 pr-2 pl-3 text-left text-xs text-muted-foreground outline-none hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
			>
				<ChevronRight
					aria-hidden
					className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
					strokeWidth={2.2}
				/>
				<span className="min-w-0">
					{t("{{count, number}} commands typed in the terminal", {
						count: blocks.length,
					})}
				</span>
				{blocks.some(blockRunning) ? <Spinner size={11} /> : null}
				<span className="ml-auto inline-flex items-center gap-1 text-[11.5px] whitespace-nowrap text-subtle-foreground">
					{shared > 0 ? (
						<>
							<Eye aria-hidden className="size-3" strokeWidth={2} />
							{t("{{shared}} of {{total}} shared", {
								shared,
								total: blocks.length,
							})}
						</>
					) : (
						<>
							<LockKeyhole aria-hidden className="size-3" strokeWidth={2} />
							{t("Private")}
						</>
					)}
				</span>
			</button>
			{open
				? blocks.map((block) => (
						<TypedBlock
							key={block.id}
							item={item}
							block={block}
							terminal={terminal}
							first={false}
							acts={acts}
							onError={onError}
						/>
					))
				: null}
		</div>
	);
};

/** One command in a session card. */
const Block = ({
	item,
	block,
	terminal,
	device,
	first,
	acts,
	onError,
}: {
	item: UserSessionItem;
	block: SessionBlock;
	terminal: TerminalEntity | undefined;
	device: string;
	first: boolean;
	acts: boolean;
	onError: (error: string | null) => void;
}) => {
	useI18n();
	const row = useRow();
	const services = useServices();
	const actions = useTerminalActions();
	const [all, setAll] = useFold(`session-all:${item.id}:${block.id}`, item.id);
	const running = blockRunning(block);
	// The session's terminal asks for the command that runs in it.
	const asking = running && terminal?.password_prompt === true;
	const waiting = running && !asking && terminal?.waiting === true;
	// A program that takes the whole screen is in the terminal, and one that
	// reads keys one at a time waits for keys rather than a line.
	const handoff = handoffOf(running, terminal);
	const full = handoff === "in_terminal";
	// Running a command here marks the session again, so each command's
	// first ask takes focus.
	const takeFocus = asking && firstAskHere(item.terminal);
	useEffect(() => {
		if (asking) askedHere(item.terminal);
	}, [asking, item.terminal]);
	const moved = block.cwd_after !== null && block.cwd_after !== block.cwd;
	const status = block.process;

	/** Interrupts the command, or with `force` ends it, and says why when that doesn't go. */
	const interrupt = async (force: boolean) => {
		onError(null);
		const reason = failureOf(
			await attempt(() => services.source.interruptTerminal(item.terminal, force))
		);
		if (reason !== null) onError(failed(force ? "kill" : "stop", reason));
	};
	const { stop, ignored } = useStop(running, () => void interrupt(false));
	// Only a device that says it answers for the terminal ends the command alone.
	const forces = terminal?.answers === true;
	const open = () => actions.showInDrawer(row.agent, item.terminal);

	// Just run from here, a program that takes the whole screen opens the
	// Agent shell's tab.
	useOpensPanel(acts && full, row.agent, { terminal: item.terminal, block: block.id }, open);

	const keys = useHeldKeys({
		agent: row.agent,
		takes: acts && takesKeys(running, terminal),
		appCursor: terminal?.app_cursor === true,
		onSend: (data) => actions.sendInput(item.device, item.terminal, data),
		onStop: stop,
	});
	// While you hold its keys it reads them, also just after one, before it waits again.
	const chip = handoff ?? (keys.held ? "keys" : null);

	const state: ReactNode = asking ? (
		<span className="inline-flex items-center gap-[5px] text-xs whitespace-nowrap text-status-busy-text">
			<LockKeyhole aria-hidden className="size-3" strokeWidth={2.2} />
			{t("Waiting for your password")}
		</span>
	) : chip !== null ? (
		<HandoffChip handoff={chip} />
	) : waiting ? (
		<WaitingChip />
	) : running ? (
		<span className="inline-flex items-center gap-[5px] text-xs whitespace-nowrap text-status-busy-text">
			<Spinner size={11} />
			<span>
				{withParts("Running {{time}}", {
					time: <Elapsed since={block.started_at} format={runClock} />,
				})}
			</span>
		</span>
	) : status.state === "exited" ? (
		<EndedChip
			status={status}
			took={block.duration_ms}
			text={
				status.code === 0 && status.stopped !== true && block.duration_ms !== null
					? formatTook(block.duration_ms)
					: endedText(status, block.duration_ms)
			}
		/>
	) : null;

	// It shows only while the card is open or the block is private.
	const share =
		(item.sealed || !acts) && !block.private ? null : (
			<ShareToggle item={item} block={block} acts={acts} onError={onError} />
		);

	const stopButtons = (
		<>
			{ignored ? <IgnoredStop program={programOf(block.command)} /> : null}
			<StopButton onStop={stop} hint={!asking && !ignored} />
			{ignored && forces ? <ForceStopButton onForce={() => void interrupt(true)} /> : null}
		</>
	);
	const openButton = (
		<button type="button" className={BUTTON} onClick={open}>
			<PanelBottom aria-hidden className="size-3" strokeWidth={2} />
			{t("Open in terminal")}
		</button>
	);

	return (
		<div
			data-session-block={block.id}
			data-private={block.private || undefined}
			className={cn(!first && "border-t border-dashed border-border")}
		>
			<div className="flex flex-wrap items-center gap-x-2 gap-y-1 py-1.5 pr-2 pl-3">
				<span
					className={cn(
						"min-w-0 font-mono text-[12.5px] break-all text-foreground",
						block.private && "opacity-50"
					)}
				>
					<span aria-hidden className="text-subtle-foreground">
						${" "}
					</span>
					{block.command}
				</span>
				{state}
				{share}
				{acts && running && !keys.held ? (
					<span
						className={cn(
							"flex flex-wrap items-center gap-1",
							share === null && "ml-auto"
						)}
					>
						{stopButtons}
						{/* While the program is in the terminal, the note has the one Open in terminal. */}
						{full ? null : openButton}
					</span>
				) : null}
			</div>
			{moved ? (
				<div className="-mt-1 pr-3 pb-1.5 pl-6 text-[11.5px] text-subtle-foreground">
					{t("now in {{folder}}", { folder: homeShort(block.cwd_after!) })}
				</div>
			) : null}
			{full ? (
				<InTerminalNote
					agent={row.agent}
					terminal={item.terminal}
					command={block.command}
					onOpen={acts ? open : null}
				/>
			) : running ? (
				<div {...keys.box}>
					<div
						className={cn(
							"border-t border-border bg-subpanel px-3 py-2 empty:hidden",
							heldLook(keys)
						)}
					>
						<LiveTerminal
							device={item.device}
							terminal={item.terminal}
							maxRows={LIVE_ROWS}
							interactive={false}
						/>
					</div>
					{keys.held ? (
						<KeysHint>
							{openButton}
							{stopButtons}
						</KeysHint>
					) : null}
				</div>
			) : block.output !== "" ? (
				<div className={cn(block.private && "opacity-50")}>
					<FinishedOutput
						output={block.output}
						truncated={block.output_truncated}
						all={all}
						onAll={() => setAll(true)}
					/>
				</div>
			) : null}
			{acts && running && !asking && !full && !keys.held ? (
				<InputRow
					onSend={(data, options) =>
						actions.sendInput(item.device, item.terminal, data, options)
					}
					onInterrupt={stop}
					choices={waiting ? terminal?.choices : undefined}
					appCursor={terminal?.app_cursor === true}
					waiting={waiting}
				/>
			) : null}
			{acts && asking ? (
				<PasswordField
					label={t("Password for {{program}} on {{device}}", {
						program: programOf(block.command),
						device,
					})}
					onSend={(data, options) =>
						actions.sendInput(item.device, item.terminal, data, options)
					}
					takeFocus={takeFocus}
				/>
			) : null}
		</div>
	);
};
