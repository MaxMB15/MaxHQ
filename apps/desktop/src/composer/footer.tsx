// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The composer's bottom row. It stays on one line at any width: as its
// pane narrows, its controls give up words a step at a time, in the order
// `footerSteps` gives, and a control that gave some up keeps its full text
// in its tooltip and its accessible name. This file draws the row, the
// paperclip with the attachments' count, and the send buttons. The
// pickers fold themselves.

import {
	type FooterShows,
	type FooterStep,
	footerSteps,
} from "@maxhq/shared/agent-view/footer-fit";
import { SKIP_WORDS } from "@maxhq/shared/agent-view/dismiss";
import type { SendMode } from "@maxhq/shared/bindings";
import { BUSY_SEND_WORDS, type BusySend, otherBusySend } from "@maxhq/shared/composer/send";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { foldsAt } from "@maxhq/shared/ui/fold";
import {
	ArrowUp,
	CornerUpRight,
	type LucideIcon,
	Paperclip,
	SkipForward,
	Square,
	TextAlignStart,
} from "lucide-react";
import { type ReactNode, useRef } from "react";

import { Tooltip, TooltipGroup } from "@/components/ui/tooltip";
import { ariaKeys, keyHint, SHORTCUTS, type Shortcut } from "@/layout/shortcuts";
import { useFold } from "@/lib/use-fold";
import { cn } from "@/lib/utils";
import { onMac } from "@/native/platform";

const KEY = "text-[10.5px]";

// Both keys take Command, or Control off a Mac, as the editor does.
const ATTACH: Shortcut = { label: "U", code: "KeyU" };
/** Command-Enter as the key hints show it, such as ⌘⏎. */
export const commandEnter = (): string => keyHint({ label: onMac() ? "⏎" : "Enter", code: "" });

const BUTTON =
	"inline-flex h-7 shrink-0 items-center gap-[7px] rounded-md px-2.5 text-xs whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed";

// A send button folded to its icon, as square as the paperclip.
const ICON_ONLY = "w-7 justify-center px-0";

const PRIMARY =
	"bg-primary font-semibold text-primary-foreground shadow-lit hover:brightness-110 disabled:border disabled:border-input disabled:bg-secondary disabled:font-normal disabled:text-muted-foreground disabled:shadow-none disabled:brightness-100";

const SECONDARY =
	"border border-input bg-secondary text-foreground hover:bg-accent disabled:text-muted-foreground";

const ICON_LOOK =
	"shrink-0 rounded-md text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

const ICON_BUTTON = `grid size-7 place-items-center ${ICON_LOOK}`;

/**
 * The footer's row, drawing `children` with the steps folded at its width.
 * `content` names what the row shows, such as its labels joined, so the
 * row measures itself again when that changes.
 */
export const FooterRow = ({
	shows,
	content,
	children,
}: {
	shows: FooterShows;
	content: string;
	children: (folded: ReadonlySet<FooterStep>) => ReactNode;
}) => {
	const row = useRef<HTMLDivElement>(null);
	const steps = footerSteps(shows);
	const level = useFold(row, { steps: steps.length, content: `${steps.join()} ${content}` });
	return (
		<TooltipGroup>
			<div
				ref={row}
				data-footer-row
				className="flex h-[38px] min-w-0 flex-nowrap items-center gap-0.5 px-1.5"
			>
				{children(foldsAt(steps, level))}
			</div>
		</TooltipGroup>
	);
};

/** The attachments the footer counts. */
export type Attached = {
	readonly length: number;
	/** Their count, such as "2 attachments". */
	readonly count: string;
	/** Their count and size, such as "2 attachments, 121 KB". */
	readonly summary: string;
};

/**
 * The paperclip, with a summary of what's `attached` beside it. Folded, the
 * summary drops the size, then moves onto the paperclip as a number.
 */
export const AttachButton = ({
	attached,
	folded,
	onPick,
}: {
	attached: Attached | null;
	folded: ReadonlySet<FooterStep>;
	onPick: () => void;
}) => {
	useI18n();
	const onClip = attached !== null && folded.has("count");
	const short = attached !== null && folded.has("size");
	return (
		<>
			<Tooltip
				content={
					<>
						{(onClip || short) && <span className="block">{attached.summary}</span>}
						{translate("Attach files or folders ({{keyHint}})", {
							keyHint: keyHint(ATTACH),
						})}
					</>
				}
			>
				<button
					type="button"
					aria-label={translate("Attach files")}
					className={
						onClip
							? `inline-flex h-7 min-w-7 items-center gap-1 px-1.5 ${ICON_LOOK}`
							: ICON_BUTTON
					}
					onClick={onPick}
				>
					<Paperclip aria-hidden className="size-[15px]" strokeWidth={1.9} />
					{onClip && (
						<span aria-hidden className="text-[11.5px] tabular-nums">
							{attached.length}
						</span>
					)}
				</button>
			</Tooltip>
			{attached &&
				(onClip ? (
					<span className="sr-only">{attached.summary}</span>
				) : (
					<span className="shrink-0 pr-1 pl-1 text-[11.5px] whitespace-nowrap text-muted-foreground">
						{short ? (
							<>
								<span aria-hidden>{attached.count}</span>
								<span className="sr-only">{attached.summary}</span>
							</>
						) : (
							attached.summary
						)}
					</span>
				))}
		</>
	);
};

/** One send button, which folds to its icon. */
const SendButton = ({
	label,
	keys,
	keyLook,
	look,
	icon: Icon,
	iconShown = false,
	tip,
	waiting,
	folded,
	disabled,
	onClick,
}: {
	label: string;
	/** Its key, such as ⏎. */
	keys: string;
	/** How the button shows its key until the key hints fold, or null to leave it to the row's hint. */
	keyLook: string | null;
	look: string;
	icon: LucideIcon;
	/** Whether it shows its icon before it folds. */
	iconShown?: boolean;
	/** What it does, when its label doesn't say it all, which its tooltip always tells. */
	tip?: string;
	waiting: string | null;
	folded: ReadonlySet<FooterStep>;
	disabled: boolean;
	onClick: () => void;
}) => {
	const icon = folded.has("send");
	const hinted = !folded.has("keys");
	return (
		<Tooltip
			content={
				hinted && !icon
					? (tip ?? null)
					: (waiting ?? (
							<>
								{label}{" "}
								<span className={cn(KEY, "text-muted-foreground")}>{keys}</span>
								{tip !== undefined && <span className="block">{tip}</span>}
							</>
						))
			}
		>
			{/* A disabled button takes no pointer, so the tooltip hangs on this
			    wrapper, which the pointer reaches through the button. */}
			<span className={cn("inline-flex shrink-0", disabled && "cursor-not-allowed")}>
				<button
					type="button"
					disabled={disabled}
					className={cn(
						BUTTON,
						look,
						"disabled:pointer-events-none",
						iconShown && "gap-1.5",
						icon && ICON_ONLY
					)}
					onClick={onClick}
				>
					{(icon || iconShown) && (
						<Icon aria-hidden className="size-3" strokeWidth={2.2} />
					)}
					<span className={cn(icon && "sr-only")}>{label}</span>
					{hinted && !icon && keyLook !== null && (
						<span className={cn(KEY, keyLook)}>{keys}</span>
					)}
				</button>
			</span>
		</Tooltip>
	);
};

/**
 * What the send buttons offer: `send` while the agent idles, steer and
 * queue while a turn is `busy`, `now` and queue while a prompt waits, and
 * `queue` alone while the agent waits for its limit to reset.
 */
export type Sends = "send" | "busy" | "now" | "queue";

const BUSY_ICON: Record<BusySend, LucideIcon> = { queue: TextAlignStart, steer: CornerUpRight };

/**
 * The right end of the row: a note while the message waits for its
 * files, the stop button while a turn runs, and the send buttons. While a
 * turn runs, the one Enter does, as `pick` says, sits last with ⏎, and the
 * other before it with ⌘⏎.
 */
export const SendButtons = ({
	sends,
	pick,
	ready,
	waiting,
	stops,
	folded,
	onSend,
	onStop,
}: {
	sends: Sends;
	/** What Enter does while a turn runs. */
	pick: BusySend;
	ready: boolean;
	/** Why the message waits, while it waits for its files. */
	waiting: string | null;
	stops: boolean;
	folded: ReadonlySet<FooterStep>;
	onSend: (mode: SendMode) => void;
	onStop: () => void;
}) => {
	useI18n();
	const hint =
		waiting ??
		(sends === "queue"
			? translate("⏎ to queue")
			: sends === "now"
				? translate("⏎ to queue, {{keyHint}} to send now", { keyHint: commandEnter() })
				: null);
	const enter = "font-medium opacity-70";
	const shared = { waiting, folded, disabled: !ready };
	const busy = (mode: BusySend, keys: string, keyLook: string, look: string) => (
		<SendButton
			label={BUSY_SEND_WORDS[mode].label}
			keys={keys}
			keyLook={keyLook}
			look={look}
			icon={BUSY_ICON[mode]}
			onClick={() => onSend(mode)}
			{...shared}
		/>
	);
	return (
		<div className="ml-auto flex shrink-0 items-center gap-1">
			{hint !== null && (
				<span
					className={cn(
						"pr-1.5 text-[11px] whitespace-nowrap text-muted-foreground",
						folded.has("keys") && "sr-only"
					)}
				>
					{hint}
				</span>
			)}
			{stops && (
				<Tooltip
					content={translate("Stop this turn {{keyHint}}", {
						keyHint: keyHint(SHORTCUTS.stop),
					})}
				>
					<button
						type="button"
						aria-label={translate("Stop")}
						aria-keyshortcuts={ariaKeys(SHORTCUTS.stop)}
						className={ICON_BUTTON}
						onClick={onStop}
					>
						<Square aria-hidden className="size-3 fill-current" />
					</button>
				</Tooltip>
			)}
			{sends === "busy" &&
				busy(otherBusySend(pick), commandEnter(), "text-muted-foreground", SECONDARY)}
			{sends === "now" && (
				<SendButton
					label={SKIP_WORDS.sendNow}
					keys={commandEnter()}
					keyLook={null}
					look={SECONDARY}
					icon={SkipForward}
					iconShown
					tip={SKIP_WORDS.sendNowTip}
					onClick={() => onSend("interrupt")}
					{...shared}
				/>
			)}
			{sends === "send" ? (
				<SendButton
					label={translate("Send")}
					keys="⏎"
					keyLook={enter}
					look={PRIMARY}
					icon={ArrowUp}
					onClick={() => onSend("queue")}
					{...shared}
				/>
			) : sends === "busy" ? (
				busy(pick, "⏎", enter, PRIMARY)
			) : (
				// The row's hint names the keys while a prompt waits, so the button doesn't.
				<SendButton
					label={BUSY_SEND_WORDS.queue.label}
					keys="⏎"
					keyLook={null}
					look={PRIMARY}
					icon={BUSY_ICON.queue}
					iconShown
					onClick={() => onSend("queue")}
					{...shared}
				/>
			)}
		</div>
	);
};
