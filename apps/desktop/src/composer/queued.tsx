// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The messages waiting for the turn to end, as dashed bubbles above the
// composer. Each can still go into the turn now, or past the prompts that
// wait on you, come back for editing, or be taken back.

import { Clock, Paperclip } from "lucide-react";

import { plural } from "@maxhq/shared/status/format";

import { Tooltip } from "@/components/ui/tooltip";
import { SKIP_WORDS } from "@maxhq/shared/agent-view/dismiss";
import { type QueuedMessage, nowMode, queuedLook } from "@maxhq/shared/composer/send";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** How a waiting message can go at once, if it can. */
type Now = ReturnType<typeof nowMode>;

const ACTION =
	"h-[22px] rounded-[5px] px-[7px] text-[11.5px] text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

/** One waiting message and what you can do with it. */
const QueuedBubble = ({
	message,
	now,
	busy,
	onSteer,
	onEdit,
	onRemove,
}: {
	message: QueuedMessage;
	now: Now;
	busy: boolean;
	onSteer: () => void;
	onEdit: () => void;
	onRemove: () => void;
}) => {
	useI18n();
	const { text, files } = queuedLook(message);
	return (
		<li className="flex flex-col items-end gap-[5px]" aria-label={translate("Queued message")}>
			<div className="max-w-[80%] rounded-xl border border-dashed border-scrollbar-thumb-hover px-3 py-[9px] text-[13px] leading-normal whitespace-pre-wrap text-secondary-foreground">
				{text}
				{files > 0 && (
					<span className="mt-1 flex items-center gap-1 text-[11.5px] text-muted-foreground">
						<Paperclip aria-hidden className="size-[11px]" />
						{plural(files, "attachment")}
					</span>
				)}
			</div>
			<div className="flex items-center gap-1 text-[11.5px] text-muted-foreground">
				<Clock aria-hidden className="size-[11px]" strokeWidth={2.2} />
				<span className="mr-1.5">{translate("Queued, sends when this turn ends")}</span>
				{now !== null && (
					<Tooltip content={now === "interrupt" ? SKIP_WORDS.sendNowTip : null}>
						<button
							type="button"
							disabled={busy}
							onClick={onSteer}
							className="h-[22px] rounded-[5px] border border-input bg-secondary px-2 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
						>
							{now === "interrupt" ? SKIP_WORDS.sendNow : translate("Steer now")}
						</button>
					</Tooltip>
				)}
				<button type="button" disabled={busy} onClick={onEdit} className={ACTION}>
					{translate("Edit")}
				</button>
				<button type="button" disabled={busy} onClick={onRemove} className={ACTION}>
					{translate("Remove")}
				</button>
			</div>
		</li>
	);
};

/** The waiting messages, oldest first, or nothing when none wait. */
export const QueuedList = ({
	messages,
	now,
	pending,
	onSteer,
	onEdit,
	onRemove,
}: {
	messages: readonly QueuedMessage[];
	/** How a message can go at once: Steer, Send now, or not at all. */
	now: Now;
	/** The messages an action is under way for. */
	pending: ReadonlySet<string>;
	onSteer: (message: QueuedMessage) => void;
	onEdit: (message: QueuedMessage) => void;
	onRemove: (message: QueuedMessage) => void;
}) => {
	useI18n();
	return messages.length === 0 ? null : (
		<ul aria-label={translate("Queued messages")} className="mb-2.5 flex flex-col gap-2.5">
			{messages.map((message) => (
				<QueuedBubble
					key={message.id}
					message={message}
					now={now}
					busy={pending.has(message.id)}
					onSteer={() => onSteer(message)}
					onEdit={() => onEdit(message)}
					onRemove={() => onRemove(message)}
				/>
			))}
		</ul>
	);
};
