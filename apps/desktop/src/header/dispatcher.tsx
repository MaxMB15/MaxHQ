// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatch box in the header and the panel that drops from it. You ask
// in plain words, and the dispatcher, an agent on this Mac that can only
// read, answers in the panel, asks what it needs to know, and hands back a
// plan. Nothing in a plan runs until you confirm it, with Enter or the
// Confirm button, and Esc dismisses it. Once a plan runs, each item says how
// it went and links to the agent or task it made, and a failed item stops
// the rest. Each conversation is a chat, and Chats lists them to go back to.

import { useI18n } from "@maxhq/shared/i18n";

import { ChevronDown, ChevronRight } from "lucide-react";
import {
	Fragment,
	type KeyboardEvent,
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { useStore } from "zustand";

import { ItemBodyView } from "@/agent-view/item-row";
import { AnswerField, type AnswerFieldHandle } from "@/composer/answer-field";
import { useTimeline } from "@/agent-view/use-timeline";
import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { PRIMARY, SECONDARY } from "@/settings/controls";
import { openSettings } from "@/settings/store";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import type {
	Agent,
	AttentionAnswer,
	AttentionId,
	InlineRef,
	Plan,
	PlanOutcome,
} from "@maxhq/shared/bindings";
import {
	CONFIRM_HINT,
	conversationItems,
	isWorking,
	noConversationHint,
	WORKING_HINT,
} from "@maxhq/shared/dispatch/conversation";
import {
	ask,
	confirm,
	createDispatchFlow,
	dismiss,
	hasPanel,
	setDraft,
	shownPlan,
	startOver,
	talkingTo,
} from "@maxhq/shared/dispatch/flow";
import {
	dispatcherTitle,
	itemLines,
	itemMode,
	itemPhrase,
	itemPreview,
	planSummary,
	resultRows,
	resultsTitle,
} from "@maxhq/shared/dispatch/plan";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";
import { enterAction } from "@maxhq/shared/dispatch/field";
import { ANY_PLACE, takesAnswerPills } from "@maxhq/shared/pills/answer";
import { MODE_LABEL } from "@maxhq/shared/settings/permission";
import { sentence } from "@maxhq/shared/status/format";

import { useDispatcherAccount } from "./dispatcher-prefs";
import { ChatButtons, ChatsList, inDialog, MovedBar, useChatPanel } from "./dispatcher/chats";

/** A quiet button in the panel's top bar. */
const QUIET =
	"h-[26px] rounded-[6px] px-2 text-[12px] text-secondary-foreground enabled:hover:bg-secondary disabled:opacity-50";

/** A key hint inside a button, left out of its name. */
const Key = ({ children }: { children: string }) => {
	useI18n();
	return (
		<kbd aria-hidden className="font-sans text-[10.5px] opacity-70">
			{children}
		</kbd>
	);
};

const noSubAgents = () => {};

/**
 * The conversation so far, scrolled to its newest item: what was said, what
 * it asks, and what went wrong. Its reading and tool calls stay in the full
 * pane.
 */
const Conversation = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const timeline = useTimeline(agent.id);
	const rows = useStore(timeline.store, (s) => s.rows);
	const items = useStore(timeline.store, (s) => s.items);
	const shown = useMemo(() => conversationItems(rows, items), [rows, items]);
	const answer = useCallback(
		(attention: AttentionId, value: AttentionAnswer, refs?: readonly InlineRef[]) =>
			source.answer(attention, value, refs),
		[source]
	);
	const row = useMemo<RowContext>(
		() => ({
			agent: agent.id,
			vendor: agent.vendor,
			timeline: timeline.store,
			readOnly: false,
			remote: false,
			answer,
			subAgentsShown: false,
			toggleSubAgents: noSubAgents,
		}),
		[agent.id, agent.vendor, timeline.store, answer]
	);
	const scroller = useRef<HTMLDivElement>(null);
	const last = shown.at(-1);
	useLayoutEffect(() => {
		const el = scroller.current;
		if (el) el.scrollTop = el.scrollHeight;
	}, [last]);
	return (
		<div
			ref={scroller}
			role="log"
			aria-label={t("Conversation")}
			className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3 select-text"
		>
			<RowProvider value={row}>
				{shown.map((item) => (
					<div key={item.id} className="min-w-0">
						<ItemBodyView item={item} />
					</div>
				))}
			</RowProvider>
			{isWorking(agent) ? (
				<p className="text-[12px] text-muted-foreground">{WORKING_HINT}</p>
			) : null}
		</div>
	);
};

/** A plan waiting on you: the summary, its items, and Confirm and Dismiss. */
const PlanView = ({
	plan,
	busy,
	onConfirm,
	onDismiss,
}: {
	plan: Plan;
	busy: boolean;
	onConfirm: () => void;
	onDismiss: () => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const [opened, setOpened] = useState<ReadonlySet<number>>(() => new Set());
	const toggle = (index: number) => {
		const next = new Set(opened);
		if (!next.delete(index)) next.add(index);
		setOpened(next);
	};
	return (
		<section
			aria-label={t("Plan")}
			className="flex shrink-0 flex-col gap-2 border-t border-sidebar-border px-4 py-3"
		>
			<p className="text-[12.5px] font-[600] text-foreground">
				{sentence(planSummary(view, plan))}
			</p>
			<ol className="flex max-h-[260px] flex-col gap-1.5 overflow-y-auto">
				{plan.items.map((item, index) => {
					const open = opened.has(index);
					const mode = itemMode(view, item);
					const preview = itemPreview(item);
					return (
						<li key={index} className="rounded-[8px] border border-track bg-subpanel">
							<button
								type="button"
								aria-expanded={open}
								onClick={() => toggle(index)}
								className="flex w-full items-start gap-2 px-2.5 py-2 text-left"
							>
								{open ? (
									<ChevronDown
										size={12}
										aria-hidden
										className="mt-[3px] shrink-0"
									/>
								) : (
									<ChevronRight
										size={12}
										aria-hidden
										className="mt-[3px] shrink-0"
									/>
								)}
								<span className="flex min-w-0 flex-col gap-0.5">
									<span className="text-[12.5px] text-foreground">
										{sentence(itemPhrase(view, item))}
									</span>
									<span className="text-[11.5px] text-muted-foreground">
										{item.reason}
									</span>
									{mode ? (
										<span
											className={
												mode === "full_access"
													? "text-[11.5px] font-[600] text-status-error"
													: "text-[11.5px] text-muted-foreground"
											}
										>
											{t("Runs in {{value2}}", { value2: MODE_LABEL[mode] })}
										</span>
									) : null}
									{preview ? (
										<span className="text-[11.5px] break-words whitespace-pre-wrap text-muted-foreground">
											{preview}
										</span>
									) : null}
								</span>
							</button>
							{open ? (
								<dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 border-t border-sidebar-border px-2.5 py-2 text-[12px] select-text">
									{itemLines(view, plan, index).map((line, i) => (
										<Fragment key={`${i}:${line.label}`}>
											<dt className="text-muted-foreground">{line.label}</dt>
											<dd className="min-w-0 break-words whitespace-pre-wrap text-foreground">
												{line.value}
											</dd>
										</Fragment>
									))}
								</dl>
							) : null}
						</li>
					);
				})}
			</ol>
			<div className="flex items-center gap-2">
				<span className="mr-auto text-[11.5px] text-muted-foreground">{CONFIRM_HINT}</span>
				<button
					type="button"
					className={SECONDARY}
					disabled={busy}
					aria-keyshortcuts="Escape"
					onClick={onDismiss}
				>
					{t("Dismiss")}
					<Key>Esc</Key>
				</button>
				<button
					type="button"
					className={PRIMARY}
					disabled={busy}
					aria-keyshortcuts="Enter"
					onClick={onConfirm}
				>
					{t("Confirm")}
					<Key>Enter</Key>
				</button>
			</div>
		</section>
	);
};

/** A plan that ran: how each item went, with links to what it made. */
const Results = ({
	plan,
	results,
	onOpen,
}: {
	plan: Plan;
	results: readonly PlanOutcome[];
	onOpen: () => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const rows = resultRows(view, plan, results);
	return (
		<section
			aria-label={t("Plan results")}
			className="flex shrink-0 flex-col gap-2 border-t border-sidebar-border px-4 py-3"
		>
			<p className="text-[12.5px] font-[600] text-foreground">{resultsTitle(results)}</p>
			<ol className="flex flex-col gap-1.5">
				{rows.map((row, index) => {
					const { agent } = row;
					return (
						<li
							key={index}
							className="flex items-center gap-3 rounded-[8px] border border-track bg-subpanel px-2.5 py-2"
						>
							<span className="flex min-w-0 flex-1 flex-col gap-0.5">
								<span className="text-[12.5px] text-foreground">{row.phrase}</span>
								<span
									className={cn(
										"text-[11.5px]",
										row.failed
											? "text-status-error-text"
											: "text-muted-foreground"
									)}
								>
									{row.line}
								</span>
							</span>
							{agent ? (
								<button
									type="button"
									className={SECONDARY}
									onClick={() => {
										onOpen();
										openAgent(services, agent);
									}}
								>
									{t("Open agent")}
								</button>
							) : null}
							{row.task !== null ? (
								<button
									type="button"
									className={SECONDARY}
									onClick={() => {
										onOpen();
										openSettings(services, { kind: "scheduled-tasks" });
									}}
								>
									{t("Open task")}
								</button>
							) : null}
						</li>
					);
				})}
			</ol>
		</section>
	);
};

/** The box in the header, and the panel under it while you use it. */
export const DispatchBox = () => {
	const { t } = useI18n();

	const services = useServices();
	const [flow] = useState(() => createDispatchFlow(services.source));
	const state = useStore(flow.store);
	const { text, refs, busy, note, ran } = state;
	const mac = useView((s) => dispatcherDevice(s.view));
	const pills = useView((s) => (mac ? takesAnswerPills(s.view, mac) : false));
	const account = useDispatcherAccount();
	const dispatcher = useView((s) => (mac ? s.view.tables.dispatcher.get(mac) : undefined));
	const talking = useView((s) => talkingTo(s.view, state, dispatcher));
	const plan = shownPlan(state, dispatcher);
	const [open, setOpen] = useState(false);
	const [focused, setFocused] = useState(false);
	const box = useRef<HTMLDivElement>(null);
	const input = useRef<AnswerFieldHandle>(null);
	const chats = useChatPanel(flow, mac, setOpen);

	const send = () => {
		if (!text.trim() || busy) return;
		setOpen(true);
		chats.setListing(false);
		void ask(flow, { device: mac, account: account?.id ?? null, pills });
	};

	const confirmPlan = () => void confirm(flow, mac, plan);

	const dismissPlan = () => void dismiss(flow, mac, plan);

	const newConversation = () => {
		startOver(flow);
		chats.setListing(false);
		input.current?.focus();
	};

	// A click anywhere else closes the panel. The conversation carries on.
	useEffect(() => {
		if (!open) return;
		const onDown = (e: PointerEvent) => {
			if (!box.current?.contains(e.target as Node) && !inDialog(e.target)) setOpen(false);
		};
		document.addEventListener("pointerdown", onDown);
		return () => document.removeEventListener("pointerdown", onDown);
	}, [open]);

	// Esc dismisses the plan, or with none closes the panel.
	const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
		// The field's menu takes its own Escape.
		if (e.key !== "Escape" || !open || e.defaultPrevented) return;
		e.preventDefault();
		if (plan) dismissPlan();
		else {
			setOpen(false);
			input.current?.blur();
		}
	};

	const shown = open && (hasPanel(state, talking) || chats.listing);

	const onEnter = () => {
		setOpen(true);
		const action = enterAction(text, plan !== null);
		if (action === "send") send();
		else if (action === "confirm") confirmPlan();
	};

	// The field stays open while the panel is, so the panel holds still
	// under the pointer when a click in it takes the focus from the field.
	const expanded = focused || shown;

	// The box keeps one line's room in the header. The field and the panel
	// lie over what's below, so the field grows without moving the header.
	return (
		<div
			ref={box}
			onKeyDown={onKeyDown}
			className="relative mx-auto h-[34px] max-w-[780px] min-w-0 flex-[1_1_520px]"
		>
			<div className="absolute top-0 right-0 left-0 z-[110] flex flex-col gap-[6px]">
				<AnswerField
					ref={input}
					value={{ text, refs }}
					label={t("Ask the dispatcher")}
					placeholder={t("Ask anything, or describe a task for any project")}
					agent={null}
					place={ANY_PLACE}
					device={mac}
					look="bar"
					busy={busy}
					shut={!expanded}
					onChange={(typed) => setDraft(flow, typed.text, typed.refs)}
					onEnter={onEnter}
					onFocus={() => {
						setFocused(true);
						setOpen(true);
					}}
					onBlur={() => setFocused(false)}
				/>
				{shown ? (
					<section
						aria-label={t("Dispatcher")}
						tabIndex={-1}
						onKeyDown={(e) => {
							// Enter on the panel itself, not on a field or button in it.
							if (e.key === "Enter" && e.target === e.currentTarget && plan) {
								e.preventDefault();
								confirmPlan();
							}
						}}
						className="flex max-h-[min(560px,calc(100vh-96px))] flex-col overflow-hidden rounded-xl border border-input bg-card shadow-popup outline-none"
					>
						<div className="flex shrink-0 items-center gap-1 border-b border-sidebar-border py-1.5 pr-2 pl-4">
							<span className="mr-auto truncate text-[12px] text-muted-foreground">
								{dispatcherTitle(account)}
							</span>
							<ChatButtons
								panel={chats}
								device={mac}
								current={talking?.id}
								className={QUIET}
							/>
							<button
								type="button"
								className={QUIET}
								disabled={busy || talking === undefined}
								onClick={newConversation}
							>
								{t("New conversation")}
							</button>
							<button
								type="button"
								className={QUIET}
								disabled={talking === undefined}
								onClick={() => {
									if (!talking) return;
									setOpen(false);
									openAgent(services, talking.id);
								}}
							>
								{t("Open as pane")}
							</button>
						</div>
						{chats.listing && mac ? (
							<ChatsList
								flow={flow}
								device={mac}
								current={talking?.id}
								panel={chats}
							/>
						) : talking ? (
							<>
								<MovedBar flow={flow} device={mac} current={talking.id} />
								<Conversation agent={talking} />
							</>
						) : (
							<p className="px-4 py-3 text-[12px] text-muted-foreground">
								{noConversationHint(busy)}
							</p>
						)}
						{ran ? (
							<Results
								plan={ran.plan}
								results={ran.results}
								onOpen={() => setOpen(false)}
							/>
						) : plan ? (
							<PlanView
								key={plan.id}
								plan={plan}
								busy={busy}
								onConfirm={confirmPlan}
								onDismiss={dismissPlan}
							/>
						) : null}
						{note ? (
							<p
								role="alert"
								className="shrink-0 border-t border-sidebar-border px-4 py-2 text-[12px] text-status-error-text"
							>
								{note}
							</p>
						) : null}
					</section>
				) : null}
			</div>
		</div>
	);
};
