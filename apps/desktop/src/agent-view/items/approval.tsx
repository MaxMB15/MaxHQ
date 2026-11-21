// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An approval the agent waits on, answered right in the timeline: the main
// option on the blue button, the others beside it, and a box to say why
// not next to the button that turns it down, which takes pills as the
// composer does. Once answered it shrinks to one line that says what won
// and where it came from. The folder is the one the approval names, or
// else the one its command runs in.

import {
	Ban,
	CircleCheck,
	CircleX,
	FilePen,
	FileText,
	Globe,
	Hand,
	ListChecks,
	Terminal,
	Wrench,
} from "lucide-react";
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from "react";
import { useStore } from "zustand";

import { useView } from "@/app/services";
import { AnswerField, useAnswerPlace } from "@/composer/answer-field";
import type { ApprovalKind, AttentionAnswer, Decided } from "@maxhq/shared/bindings";
import {
	NOTHING_TYPED,
	shownAnswer,
	takesAnswerPills,
	type Typed,
	typedFor,
} from "@maxhq/shared/pills/answer";
import { askTitle, showHidden } from "@maxhq/shared/status/format";
import { kindName } from "@maxhq/shared/status/agent-kind";
import { deviceName } from "@maxhq/shared/view/select";

import {
	approvalChoices,
	decidedLine,
	decidedNo,
	denyAnswer,
	useSend,
} from "@maxhq/shared/agent-view/answer";
import { rowKind, useRow } from "@maxhq/shared/agent-view/context";
import { SKIP_WORDS, wasDismissed } from "@maxhq/shared/agent-view/dismiss";
import { baseName, isOpen } from "@maxhq/shared/agent-view/format";
import { useKept } from "@maxhq/shared/agent-view/kept";
import {
	CodeTitle,
	EndedCard,
	PrimaryButton,
	SecondaryButton,
	SendNote,
	WaitingCard,
} from "./card";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const KIND_ICON: Record<ApprovalKind, (size: number) => ReactNode> = {
	command: (size) => <Terminal size={size} strokeWidth={1.9} aria-hidden />,
	file_change: (size) => <FilePen size={size} strokeWidth={1.9} aria-hidden />,
	file_read: (size) => <FileText size={size} strokeWidth={1.9} aria-hidden />,
	network: (size) => <Globe size={size} strokeWidth={1.9} aria-hidden />,
	tool: (size) => <Wrench size={size} strokeWidth={1.9} aria-hidden />,
	plan: (size) => <ListChecks size={size} strokeWidth={1.9} aria-hidden />,
	other: (size) => <Hand size={size} strokeWidth={1.9} aria-hidden />,
};

/** The folder the approval is for, or else the one its command runs in. */
const useWorkingFolder = (item: ItemOf<"approval">): string | null => {
	const { timeline } = useRow();
	const commandFolder = useStore(timeline, (state) => {
		const parent = item.parent !== null ? state.items.get(item.parent) : undefined;
		return parent?.kind === "command" ? parent.cwd : null;
	});
	return item.cwd ?? commandFolder;
};

/** The name of the device the agent runs on. */
const useDeviceName = (): string | null => {
	const { agent } = useRow();
	return useView((s) => {
		const record = s.view.tables.agent.get(agent);
		const workspace = record ? s.view.tables.workspace.get(record.workspace) : undefined;
		return workspace ? deviceName(s.view, workspace.device) : null;
	});
};

// A box and whether more of its text sits below what it shows, kept up as it
// scrolls and resizes.
const useMoreBelow = (text: string | null) => {
	const ref = useRef<HTMLPreElement>(null);
	const [more, setMore] = useState(false);
	const measure = useCallback(() => {
		const box = ref.current;
		setMore(box !== null && box.scrollHeight - box.clientHeight - box.scrollTop > 1);
	}, []);
	useLayoutEffect(() => {
		measure();
		const box = ref.current;
		if (box === null) return;
		const observer = new ResizeObserver(measure);
		observer.observe(box);
		return () => observer.disconnect();
	}, [measure, text]);
	return { ref, more, measure };
};

// What the detail box shows: a command after a prompt sign, anything else
// as it came, and the folder it runs in. A long one says there's more, so what
// sits past the fold can't go unread.
const Detail = ({ item }: { item: ItemOf<"approval"> }) => {
	useI18n();
	const working = useWorkingFolder(item);
	const device = useDeviceName();
	const { ref, more, measure } = useMoreBelow(item.detail);
	const folder = working === null ? null : showHidden(working);
	// The rules "Always allow" would save, while it's still on offer.
	const saves =
		item.saves?.length && item.options.some((option) => option.kind === "allow_always")
			? item.saves.map(showHidden)
			: null;
	if (!item.detail && folder === null && saves === null) return null;
	const command = item.approval_kind === "command";
	return (
		<div className="flex flex-col gap-[3px] rounded-md border border-border bg-background px-2.5 py-2">
			{item.detail ? (
				<pre
					ref={ref}
					onScroll={measure}
					className="max-h-[220px] overflow-auto font-mono text-[12px] leading-[1.5] break-words whitespace-pre-wrap text-foreground select-text"
				>
					{command ? <span className="text-muted-foreground select-none">$ </span> : null}
					{showHidden(item.detail)}
				</pre>
			) : null}
			{more ? (
				<span className="text-[11.5px] text-muted-foreground">
					{translate("More below. Scroll to read it all.")}
				</span>
			) : null}
			{folder !== null ? (
				<span title={folder} className="truncate text-[11.5px] text-muted-foreground">
					{device !== null
						? translate("In {{baseName}} on {{device}}.", {
								baseName: baseName(folder),
								device: device,
							})
						: translate("In {{baseName}}.", { baseName: baseName(folder) })}
				</span>
			) : null}
			{saves !== null ? (
				<span
					title={saves.join("\n")}
					className="truncate text-[11.5px] text-muted-foreground"
				>
					{translate("Always allow saves:")}
					{saves.map((rule) => (
						<code key={rule} className="ml-1.5 font-mono text-foreground">
							{rule}
						</code>
					))}
				</span>
			) : null}
		</div>
	);
};

const LiveApproval = ({ item }: { item: ItemOf<"approval"> }) => {
	useI18n();
	const { agent, vendor, readOnly, answer } = useRow();
	const kind = rowKind(useRow());
	const [reason, setReason] = useKept<Typed>(`deny:${item.attention}`, NOTHING_TYPED);
	const place = useAnswerPlace(agent);
	// A maxd too old for pills gets each one as its words.
	const pills = useView(
		(state) => place.device !== null && takesAnswerPills(state.view, place.device)
	);
	const send = useCallback(
		(a: AttentionAnswer) => {
			if (a.type !== "text") return answer(item.attention, a);
			const typed = typedFor({ text: a.text, refs: reason.refs }, pills);
			return answer(item.attention, { type: "text", text: typed.text }, typed.refs);
		},
		[answer, item.attention, pills, reason.refs]
	);
	const [state, submit] = useSend(send);
	const { primary, others, deny } = approvalChoices(item.options);
	const busy = state.state === "sending";
	const turnDown = () => {
		if (deny && !busy) submit(denyAnswer(deny, reason.text));
	};
	const label =
		vendor === "codex" ? translate("Approval request") : translate("Permission request");

	if (readOnly) {
		return (
			<WaitingCard
				label={label}
				icon={KIND_ICON[item.approval_kind](15)}
				title={<CodeTitle text={askTitle(item.title)} />}
				kind={kind}
				since={item.started_at}
			>
				<Detail item={item} />
				<ul className="flex flex-wrap gap-1.5" aria-label={translate("Options")}>
					{item.options.map((o) => (
						<li
							key={o.id}
							className="inline-flex h-6 items-center rounded-md border border-hover-card-border px-2 text-[12px] text-secondary-foreground"
						>
							{showHidden(o.label)}
						</li>
					))}
				</ul>
				<p className="text-[11.5px] text-muted-foreground">
					{translate(
						"Answer it in the terminal. MaxHQ can't answer a session it doesn't hold."
					)}
				</p>
			</WaitingCard>
		);
	}

	return (
		<WaitingCard
			label={label}
			icon={KIND_ICON[item.approval_kind](15)}
			title={<CodeTitle text={askTitle(item.title)} />}
			kind={kind}
			since={item.started_at}
			onKeyDown={(event) => {
				// Enter on the card itself picks the main option, as the hint on its button says.
				if (
					event.key === "Enter" &&
					event.target === event.currentTarget &&
					primary &&
					!busy
				) {
					event.preventDefault();
					submit({ type: "option", id: primary.id });
				}
			}}
		>
			<Detail item={item} />
			<div className="flex flex-wrap items-start gap-1.5">
				{primary ? (
					<PrimaryButton
						busy={busy}
						disabled={busy}
						onClick={() => submit({ type: "option", id: primary.id })}
					>
						{primary.label}
					</PrimaryButton>
				) : null}
				{others.map((o) => (
					<SecondaryButton
						key={o.id}
						disabled={busy}
						onClick={() => submit({ type: "option", id: o.id })}
					>
						{o.label}
					</SecondaryButton>
				))}
				{deny ? (
					<>
						<span className="flex-[1_1_24px]" />
						<form
							className="flex min-w-[220px] flex-[0_1_300px]"
							onSubmit={(event) => {
								event.preventDefault();
								turnDown();
							}}
						>
							<AnswerField
								className="flex-1"
								value={reason}
								onChange={setReason}
								onEnter={turnDown}
								label={
									vendor === "codex"
										? translate("Reason for declining")
										: translate("Reason for denying")
								}
								placeholder={translate("Why not, or what to do instead")}
								agent={agent}
								place={place}
								device={place.device}
								look="card"
								busy={busy}
								trailing={
									<button
										type="submit"
										disabled={busy}
										className="min-h-[26px] shrink-0 cursor-pointer self-stretch rounded-r-[5px] border-l border-input bg-secondary px-2.5 text-[12px] text-foreground hover:bg-accent disabled:cursor-default disabled:opacity-60"
									>
										{deny.label}
									</button>
								}
							/>
						</form>
					</>
				) : null}
			</div>
			<SendNote state={state} kind={kind} />
			{deny ? (
				<p className="text-[11.5px] text-muted-foreground">
					{vendor === "codex"
						? translate("{{label}} sends your reason to Codex as its next message.", {
								label: deny.label,
							})
						: translate(
								"{{label}} sends your reason to {{value2}} and the turn continues.",
								{ label: deny.label, value2: kindName(kind) }
							)}
				</p>
			) : null}
		</WaitingCard>
	);
};

const DecidedApproval = ({ item, decided }: { item: ItemOf<"approval">; decided: Decided }) => {
	useI18n();
	const no = decidedNo(decided, item.options);
	const { answer } = decided;
	return (
		<EndedCard
			label={no ? translate("Request turned down") : translate("Request allowed")}
			icon={
				no ? (
					<CircleX size={14} strokeWidth={1.9} aria-hidden className="text-destructive" />
				) : (
					<CircleCheck
						size={14}
						strokeWidth={1.9}
						aria-hidden
						className="text-status-ready"
					/>
				)
			}
			title={<CodeTitle text={askTitle(item.title)} />}
			meta={decidedLine(decided, item.options)}
		>
			{answer.type === "text" ? (
				<p className="ml-[22px] border-l-2 border-track pl-2.5 text-[12px] break-words text-secondary-foreground select-text">
					{shownAnswer(answer.text)}
				</p>
			) : null}
		</EndedCard>
	);
};

/** An approval, waiting or answered. */
export const ApprovalCard = ({ item }: { item: ItemOf<"approval"> }) => {
	useI18n();
	if (item.decided) return <DecidedApproval item={item} decided={item.decided} />;
	if (isOpen(item.status)) return <LiveApproval item={item} />;
	// Send now in the composer dismisses an approval that waits.
	const dismissed = wasDismissed(item);
	return (
		<EndedCard
			label={dismissed ? SKIP_WORDS.dismissed : translate("Request closed")}
			icon={
				<Ban size={14} strokeWidth={1.9} aria-hidden className="text-subtle-foreground" />
			}
			title={<CodeTitle text={askTitle(item.title)} />}
			meta={dismissed ? SKIP_WORDS.dismissed : "Closed without an answer"}
		/>
	);
};
