// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The conversation, newest at the bottom, with only the rows in view in
// the page. It follows new output until you scroll up, keeps its place
// while older pages load above, and finds its place again when a pane is
// hidden and shown.

import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import { ArrowDown, CircleAlert } from "lucide-react";
import {
	type KeyboardEvent,
	memo,
	type ReactNode,
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { useStore } from "zustand";

import type { AgentId, ItemId, Timestamp } from "@maxhq/shared/bindings";
import { formatClock, plural } from "@maxhq/shared/status/format";
import type { Timeline, TimelineStore } from "@maxhq/shared/timeline/store";

import { onReattach } from "@/layout/reattach";
import { visualItemHeight, visualsHeight } from "@/visuals/heights";
import { useVisualPages } from "@/visuals/pages";
import { visualVersions } from "@maxhq/shared/visuals/versions";
import { Spinner } from "./icons";
import { gapAbove } from "@maxhq/shared/agent-view/format";
import { estimateHeight, ItemBodyView } from "./item-row";
import { Divider } from "./items/notices";
import { type JumpTarget, registerJumpTarget } from "./jump";
import { PillSteps, PromptRail, usePromptSteps } from "./prompt-rail";
import { goToRequest, takeRequest, watchRequests } from "./request";
import { cn } from "@/lib/utils";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// Within this many pixels of the end, scrolling down follows again.
const FOLLOW_WITHIN = 40;
// Within this many pixels of the top, the page before loads.
const LOAD_WITHIN = 600;
// A scroll this soon after a wheel, key or touch is yours.
const INTENT_MS = 250;
const PADDING_START = 18;
const PADDING_END = 12;

const UP_KEYS = new Set(["PageUp", "ArrowUp", "Home"]);
const SCROLL_KEYS = new Set(["PageUp", "ArrowUp", "Home", "PageDown", "ArrowDown", "End", " "]);

// What sits above the first item.
type Top = "more" | "error" | "start" | "none";

// Where a list was when it unmounted, by agent.
type Snapshot = {
	readonly timeline: Timeline;
	readonly offset: number;
	readonly measurements: VirtualItem[];
	readonly following: boolean;
	readonly seen: ItemId | null;
};

const snapshots = new Map<AgentId, Snapshot>();

/** Forgets where every list was, for tests. */
export const clearSnapshots = () => snapshots.clear();

const isTyping = (target: EventTarget) =>
	target instanceof HTMLElement &&
	(target.isContentEditable || target.tagName === "INPUT" || target.tagName === "TEXTAREA");

// Space presses a button in a row instead of scrolling the list.
const pressesButton = (event: KeyboardEvent) =>
	event.key === " " &&
	event.target instanceof Element &&
	event.target.closest("button, summary, [role='button']") !== null;

// Rows after `seen`, counted from the end.
const countAfter = (rows: readonly ItemId[], seen: ItemId | null): number => {
	let count = 0;
	for (let i = rows.length - 1; i >= 0 && rows[i] !== seen; i--) count++;
	return count;
};

const RowView = ({
	store,
	id,
	before,
}: {
	store: TimelineStore;
	id: ItemId;
	before: ItemId | null;
}) => {
	useI18n();
	const item = useStore(store, (s) => s.items.get(id));
	const previous = useStore(store, (s) => (before ? (s.items.get(before)?.kind ?? null) : null));
	if (!item) return null;
	return (
		<div
			data-quote-item={id}
			className="mx-auto max-w-[800px] px-7"
			style={{ paddingTop: gapAbove(item.kind, previous) }}
		>
			<ItemBodyView item={item} />
		</div>
	);
};

const Row = memo(RowView);

const TopRow = ({
	top,
	timeline,
	startedAt,
}: {
	top: Top;
	timeline: Timeline;
	startedAt: Timestamp;
}) => {
	useI18n();
	const loading = useStore(timeline.store, (s) => s.loadingOlder);
	switch (top) {
		case "more":
			return (
				<div className="flex h-8 items-center justify-center gap-2 text-[11.5px] text-muted-foreground">
					{loading ? (
						<>
							<Spinner size={10} />
							{translate("Loading earlier items")}
						</>
					) : null}
				</div>
			);
		case "error":
			return (
				<div
					role="alert"
					className="flex h-8 items-center justify-center gap-2 text-[11.5px] text-status-error-text"
				>
					<CircleAlert size={12} strokeWidth={1.9} aria-hidden />
					{translate("Couldn't load earlier items.")}
					<button
						type="button"
						onClick={timeline.loadOlder}
						className="cursor-pointer rounded-sm px-1 text-foreground underline-offset-2 hover:underline"
					>
						{translate("Try again")}
					</button>
				</div>
			);
		case "start":
			return (
				<div className="mx-auto max-w-[800px] px-7 pb-1">
					<Divider icon={null}>
						{translate("Started in a terminal at ")}
						{formatClock(startedAt)}
						{translate(". History comes from the session file.")}
					</Divider>
				</div>
			);
		case "none":
			return <div />;
	}
};

const EndRow = ({ endedAt }: { endedAt: Timestamp | null }) => {
	useI18n();
	return (
		<div className="mx-auto max-w-[800px] px-7 pt-3">
			<Divider icon={null}>
				{endedAt !== null
					? translate("Session in the terminal ended at {{clock}}", {
							clock: formatClock(endedAt),
						})
					: translate("Session in the terminal ended")}
			</Divider>
		</div>
	);
};

const Status = ({ children, alert = false }: { children: ReactNode; alert?: boolean }) => {
	useI18n();
	return (
		<div
			role={alert ? "alert" : "status"}
			className="absolute inset-0 grid place-items-center px-7 text-center text-[12.5px] text-muted-foreground"
		>
			<div className="flex max-w-[420px] flex-col items-center gap-2">{children}</div>
		</div>
	);
};

type Props = {
	readonly agent: AgentId;
	readonly timeline: Timeline;
	/** Set for a session in a terminal, which MaxHQ only watches. */
	readonly observed: boolean;
	readonly startedAt: Timestamp;
	/** Set once the terminal session MaxHQ watched has ended. */
	readonly ended: boolean;
	/** When it ended, if maxd knows. */
	readonly endedAt: Timestamp | null;
	/** Set for a sub-agent that finished, whose history is all there is. */
	readonly finished?: boolean;
	/** Hears the element that scrolls, once it mounts, for what works on the rows. */
	readonly onScroller?: (scroller: HTMLElement | null) => void;
	/** Hears what lets a jump or a find go to an item, once the list mounts. */
	readonly onTarget?: (target: JumpTarget | null) => void;
};

const TimelineListView = ({
	agent,
	timeline,
	observed,
	startedAt,
	ended: down,
	endedAt,
	finished = false,
	onScroller,
	onTarget,
}: Props) => {
	useI18n();
	const { store } = timeline;
	const status = useStore(store, (s) => s.status);
	const error = useStore(store, (s) => s.error);
	const rows = useStore(store, (s) => s.rows);
	const hasOlder = useStore(store, (s) => s.hasOlder);
	const loadingOlder = useStore(store, (s) => s.loadingOlder);

	const [saved] = useState(() => {
		const snapshot = snapshots.get(agent);
		return snapshot?.timeline === timeline ? snapshot : undefined;
	});
	const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
	const [following, setFollowing] = useState(saved?.following ?? true);
	const [seen, setSeen] = useState<ItemId | null>(saved?.seen ?? null);
	const intentAt = useRef(0);
	const holding = useRef(false);
	// Where the last scroll left the list, to tell a step up from one down.
	const lastTop = useRef(0);
	// How far the list could scroll then, to tell when it got shorter.
	const lastMax = useRef(0);
	const latest = useRef({ following, seen });

	const ready = status === "ready";
	const top: Top = hasOlder
		? error && !loadingOlder
			? "error"
			: "more"
		: observed
			? "start"
			: "none";
	const ended = observed && down;
	const keys = useMemo(
		() => [`:top:${top}`, ...rows, ...(ended ? [":end"] : [])],
		[top, rows, ended]
	);

	const pages = useVisualPages();
	const getItemKey = useCallback((index: number) => keys[index] ?? index, [keys]);
	const estimateSize = useCallback(
		(index: number) => {
			const key = keys[index];
			if (key === undefined || key.startsWith(":")) return 32;
			const item = store.getState().items.get(key);
			if (!item) return 90;
			if (item.kind === "agent_visual") {
				const items = store.getState().items;
				const page = pages.get(item.attachment.id);
				const folded = visualVersions(items).replacedBy(item.id) !== null;
				return (
					visualItemHeight(page?.status === "ready" ? page.html : undefined, folded) + 10
				);
			}
			const visuals = item.kind === "assistant_message" ? visualsHeight(item.text) : 0;
			return estimateHeight(item.kind) + visuals + 10;
		},
		[keys, pages, store]
	);

	// The app doesn't run the React compiler, and the virtualizer re-renders this itself.
	// eslint-disable-next-line react-hooks/incompatible-library
	const virtualizer = useVirtualizer({
		count: ready ? keys.length : 0,
		getScrollElement: () => scroller,
		estimateSize,
		getItemKey,
		overscan: 6,
		paddingStart: PADDING_START,
		paddingEnd: PADDING_END,
		anchorTo: "end",
		// While following, any growth keeps the end in view. Otherwise the rows stay put.
		followOnAppend: following ? "auto" : false,
		scrollEndThreshold: following ? Number.POSITIVE_INFINITY : 1,
		initialOffset: saved?.offset,
		initialMeasurementsCache: saved?.measurements,
		initialRect: { width: 800, height: 600 },
		directDomUpdates: true,
	});

	// A message you send brings the end back into view.
	const last = rows.at(-1) ?? null;
	const [lastRow, setLastRow] = useState(last);
	if (last !== lastRow) {
		setLastRow(last);
		if (!following && last && store.getState().items.get(last)?.kind === "user_message") {
			setFollowing(true);
		}
	}

	useEffect(() => {
		latest.current = { following, seen };
	});

	// Keep the place for the next mount of this agent.
	useEffect(
		() => () => {
			snapshots.set(agent, {
				timeline,
				offset: virtualizer.scrollOffset ?? 0,
				measurements: virtualizer.takeSnapshot(),
				...latest.current,
			});
		},
		[agent, timeline, virtualizer]
	);

	// Also once the scroller mounts: a timeline that was already loaded is ready
	// on the first render, before there is anything to scroll.
	useLayoutEffect(() => {
		if (scroller && ready && following) virtualizer.scrollToEnd();
	}, [scroller, ready, following, virtualizer]);

	const loadIfNearTop = useCallback(() => {
		const s = store.getState();
		if (!scroller || s.status !== "ready" || !s.hasOlder || s.loadingOlder || s.error) return;
		// A hidden list reads as scrolled to the top.
		if (scroller.clientHeight === 0) return;
		if (scroller.scrollTop < LOAD_WITHIN) timeline.loadOlder();
	}, [scroller, store, timeline]);

	useEffect(() => {
		loadIfNearTop();
	}, [loadIfNearTop, rows, hasOlder]);

	// A pane in a background tab leaves the page, and the browser forgets how far
	// its list was scrolled. The virtualizer doesn't see that and keeps the rows it
	// had, so when the list shows again, scroll it back to them. dockview also
	// takes a pane out and puts it straight back when its tab is clicked again,
	// which no resize shows, so that brings it back too. A list that only gets
	// shorter or taller, say when a strip opens above it, keeps its end in view
	// while it follows.
	useEffect(() => {
		if (!scroller) return;
		const restore = () => {
			if (scroller.clientHeight === 0) return;
			if (latest.current.following) virtualizer.scrollToEnd();
			else virtualizer.scrollToOffset(virtualizer.scrollOffset ?? 0);
			loadIfNearTop();
		};
		let hidden = false;
		let last = scroller.clientHeight;
		const observer = new ResizeObserver(() => {
			const height = scroller.clientHeight;
			if (height === 0) {
				hidden = true;
				return;
			}
			const changed = height !== last;
			last = height;
			if (hidden) {
				hidden = false;
				restore();
				return;
			}
			if (changed && latest.current.following) virtualizer.scrollToEnd();
		});
		observer.observe(scroller);
		const stop = onReattach(scroller, restore);
		return () => {
			observer.disconnect();
			stop();
		};
	}, [scroller, virtualizer, loadIfNearTop]);

	const stopFollowing = useCallback(() => {
		if (!following || !scroller) return;
		if (scroller.scrollHeight <= scroller.clientHeight + 1) return;
		// Before the next render, so output arriving now doesn't pull the view back down.
		virtualizer.setOptions({
			...virtualizer.options,
			followOnAppend: false,
			scrollEndThreshold: 1,
		});
		setFollowing(false);
		setSeen(store.getState().rows.at(-1) ?? null);
	}, [following, scroller, store, virtualizer]);

	// A jump to a quote scrolls to its row. The latest keys and stop, since it
	// asks again over several frames.
	const keysRef = useRef(keys);
	const stopFollowingRef = useRef(stopFollowing);
	useEffect(() => {
		keysRef.current = keys;
		stopFollowingRef.current = stopFollowing;
	});
	useEffect(() => {
		if (!scroller || !onScroller) return;
		onScroller(scroller);
		return () => onScroller(null);
	}, [onScroller, scroller]);
	const target = useMemo<JumpTarget | null>(
		() =>
			scroller && {
				timeline,
				scroller,
				reveal: (item, align = "center") => {
					const index = keysRef.current.indexOf(item);
					if (index < 0 || store.getState().status !== "ready") return false;
					stopFollowingRef.current();
					virtualizer.scrollToIndex(index, { align });
					return true;
				},
				hold: () => stopFollowingRef.current(),
			},
		[timeline, scroller, store, virtualizer]
	);
	useEffect(() => {
		if (target) return registerJumpTarget(agent, target);
	}, [agent, target]);
	// Go to the card waiting on you when the composer, ⌘J or the bell asks.
	useEffect(() => {
		if (!target) return;
		let stop: (() => void) | null = null;
		const go = () => {
			const ask = takeRequest(agent);
			if (!ask) return;
			stop?.();
			stop = goToRequest(target, ask.attention);
		};
		const unwatch = watchRequests(agent, go);
		go();
		return () => {
			unwatch();
			stop?.();
		};
	}, [agent, target]);
	useEffect(() => {
		if (!target || !onTarget) return;
		onTarget(target);
		return () => onTarget(null);
	}, [onTarget, target]);

	const jumpToLatest = () => {
		setFollowing(true);
		virtualizer.scrollToEnd();
	};

	const prompts = usePromptSteps({
		store,
		rows,
		virtualizer,
		target,
		hasOlder,
		atEnd: () =>
			following ||
			(scroller !== null &&
				scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <=
					FOLLOW_WITHIN),
		hold: stopFollowing,
		toLatest: jumpToLatest,
	});
	const stepsShown = ready && prompts.shown;

	const onScroll = () => {
		if (!scroller) return;
		const max = scroller.scrollHeight - scroller.clientHeight;
		// Past the end only while the end bounces, which isn't a step up.
		const top = Math.min(scroller.scrollTop, max);
		// A list that got shorter takes the view with it to its new end, which
		// isn't a step up either.
		const pulledBack = max < lastMax.current && max - top <= 1;
		const up = top < lastTop.current && !pulledBack;
		lastTop.current = top;
		lastMax.current = max;
		const yours = holding.current || performance.now() - intentAt.current < INTENT_MS;
		// A trackpad starts a scroll up with small steps, each still near the
		// end. Following again on those would pull the view back down each time.
		if (yours && up) stopFollowing();
		else if (max - top <= FOLLOW_WITHIN) {
			if (!following) setFollowing(true);
		} else if (yours) {
			stopFollowing();
		}
		loadIfNearTop();
		prompts.refresh();
	};

	const onKeyDown = (event: KeyboardEvent) => {
		// Option+Up and Option+Down step from prompt to prompt.
		if (
			event.altKey &&
			!event.metaKey &&
			!event.ctrlKey &&
			(event.key === "ArrowUp" || event.key === "ArrowDown") &&
			!event.defaultPrevented &&
			!isTyping(event.target)
		) {
			event.preventDefault();
			prompts.step(event.key === "ArrowUp" ? -1 : 1);
			return;
		}
		if (
			event.defaultPrevented ||
			isTyping(event.target) ||
			pressesButton(event) ||
			!SCROLL_KEYS.has(event.key)
		)
			return;
		intentAt.current = performance.now();
		if (UP_KEYS.has(event.key)) stopFollowing();
	};

	// A scroll bar can be let go of anywhere, so this listens past the list. A click
	// on a row isn't a scroll, and the card it answers gets shorter right after.
	useEffect(() => {
		const letGo = () => {
			if (!holding.current) return;
			holding.current = false;
			intentAt.current = performance.now();
		};
		window.addEventListener("pointerup", letGo);
		window.addEventListener("pointercancel", letGo);
		return () => {
			window.removeEventListener("pointerup", letGo);
			window.removeEventListener("pointercancel", letGo);
		};
	}, []);

	const fresh = following ? 0 : countAfter(rows, seen);
	const items = virtualizer.getVirtualItems();

	return (
		<div className="relative min-h-0 flex-1">
			<div
				ref={setScroller}
				tabIndex={0}
				role="region"
				aria-label={translate("Conversation")}
				onScroll={onScroll}
				onWheel={(event) => {
					intentAt.current = performance.now();
					if (event.deltaY < 0) stopFollowing();
				}}
				onTouchMove={() => {
					intentAt.current = performance.now();
				}}
				onKeyDown={onKeyDown}
				onPointerDown={(event) => {
					// The scroller itself, not a row, means its scroll bar.
					if (event.target === event.currentTarget) holding.current = true;
				}}
				className="absolute inset-0 overflow-x-hidden overflow-y-auto outline-none [overflow-anchor:none]"
			>
				{ready ? (
					<div ref={virtualizer.containerRef} className="relative w-full">
						{items.map((row) => {
							const key = String(row.key);
							return (
								<div
									key={key}
									data-index={row.index}
									ref={virtualizer.measureElement}
									className="absolute top-0 left-0 w-full"
								>
									{key.startsWith(":top:") ? (
										<TopRow
											top={top}
											timeline={timeline}
											startedAt={startedAt}
										/>
									) : key === ":end" ? (
										<EndRow endedAt={endedAt} />
									) : (
										<Row
											store={store}
											id={key}
											before={
												row.index > 1 ? (keys[row.index - 1] ?? null) : null
											}
										/>
									)}
								</div>
							);
						})}
					</div>
				) : null}
			</div>
			{status === "loading" ? (
				<Status>
					<Spinner size={14} />
					{translate("Loading the conversation")}
				</Status>
			) : status === "failed" ? (
				<Status alert>
					<CircleAlert
						size={16}
						strokeWidth={1.9}
						aria-hidden
						className="text-destructive"
					/>
					<span className="text-foreground">
						{translate("Couldn't open the conversation.")}
					</span>
					{error ? <span className="select-text">{error}</span> : null}
				</Status>
			) : rows.length === 0 && !hasOlder ? (
				<Status>
					{finished
						? translate("MaxHQ found no history for this sub-agent.")
						: observed
							? translate("Nothing in the session file yet.")
							: translate("No messages yet. Send one below to start.")}
				</Status>
			) : null}
			{stepsShown ? <PromptRail steps={prompts} /> : null}
			{!following && ready ? (
				<div
					className={cn(
						"absolute bottom-3 left-1/2 inline-flex h-7 -translate-x-1/2 items-center rounded-full border border-input bg-popover text-[12px] whitespace-nowrap text-foreground shadow-md",
						stepsShown && "pl-0.5"
					)}
				>
					{stepsShown ? <PillSteps steps={prompts} /> : null}
					<button
						type="button"
						onClick={jumpToLatest}
						className="inline-flex h-full cursor-pointer items-center gap-1.5 rounded-full px-3 hover:bg-accent"
					>
						<ArrowDown size={13} strokeWidth={1.9} aria-hidden />
						{fresh > 0 ? plural(fresh, "new item") : translate("Jump to latest")}
					</button>
				</div>
			) : null}
		</div>
	);
};

/** The conversation, virtualized. Only the rows in view are in the page. */
export const TimelineList = memo(TimelineListView);
