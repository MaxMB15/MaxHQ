// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent panes in projects mode. Each open agent, workflow run, project
// note, file or visual is a tab, and tabs dock, split and float with dockview.
// Drags run on pointer events, not HTML5 drag and drop, so they never fight
// the window's native file drops. The buttons and the tab menu show the
// pane shortcuts they share.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import "dockview-react/dist/styles/dockview.css";
import "./panes.css";

import {
	type BuiltInContextMenuItem,
	type DockviewApi,
	type DockviewGroupPanel,
	DockviewReact,
	type DockviewTheme,
	type GetTabContextMenuItemsParams,
	type IContextMenuItemComponentProps,
	type IDockviewHeaderActionsProps,
	type IDockviewPanelHeaderProps,
	type IDockviewPanelProps,
	type DockviewPanelApi,
	type ReactContextMenuItemConfig,
} from "dockview-react";
import {
	type ReactNode,
	type SVGProps,
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";

import { AgentView } from "@/agent-view";
import { FinishedView } from "@/agent-view/finished-view";
import { SessionMoved } from "@/agent-view/outside-banner";
import { useServices, useView } from "@/app/services";
import { useTheme } from "@/app/theme";
import { ErrorBoundary } from "@/components/error-boundary";
import { ActivityDot } from "@/components/status/dots";
import { canSplit, splitGroup, toggleMaximized } from "@/layout/commands";
import { paneAgent } from "@/layout/pane-id";
import {
	AGENT_PANE,
	FILE_PANE,
	NOTE_PANE,
	type PaneParams,
	type Panes,
	panesOf,
	VISUAL_PANE,
	WORKFLOW_PANE,
} from "@/layout/panes";
import { ariaKeys, keyHint, SHORTCUTS, type Shortcut, watchShortcuts } from "@/layout/shortcuts";
import { windowLabel } from "@/layout/storage";
import { syncPanes } from "@/layout/sync";
import { revealTab } from "@/layout/tab-strip";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { useOpenNote } from "@/notes/open";
import { NotePaneFor } from "@/notes/note-pane";
import { FilePaneFor } from "@/viewer/file-pane";
import { VisualPaneFor } from "@/visuals/visual-view";
import { CanvasPaneFor } from "@/workflow/canvas-pane";
import type { AgentId } from "@maxhq/shared/bindings";
import { movedTo } from "@maxhq/shared/agent-view/outside";
import { finishedLead, finishedNote } from "@maxhq/shared/agent-view/finished";
import { agentLook } from "@maxhq/shared/status/activity";
import { noteSummary } from "@maxhq/shared/view/notes";
import { runAt } from "@maxhq/shared/workflow/select";
import { FileText, NotebookText, Workflow } from "lucide-react";

import { Empty } from "./empty";
import { NoPane } from "./no-pane";

const THEME: DockviewTheme = {
	name: "maxhq",
	className: "dockview-theme-maxhq",
	gap: 4,
	dndOverlayMounting: "absolute",
	dndPanelOverlay: "group",
};

type Disposable = { dispose: () => void };

// A dockview value as React state, read again whenever `on` fires.
const useDockValue = <T,>(on: (notify: () => void) => Disposable, read: () => T): T => {
	const subscribe = useCallback(
		(notify: () => void) => {
			const listening = on(notify);
			return () => listening.dispose();
		},
		[on]
	);
	return useSyncExternalStore(subscribe, read);
};

const usePanelVisible = (api: DockviewPanelApi): boolean =>
	useDockValue(
		useCallback((notify) => api.onDidVisibilityChange(notify), [api]),
		() => api.isVisible
	);

const useFloating = (group: DockviewGroupPanel): boolean =>
	useDockValue(
		useCallback((notify) => group.api.onDidLocationChange(notify), [group]),
		() => group.api.location.type === "floating"
	);

const useMaximized = (containerApi: DockviewApi, group: DockviewGroupPanel): boolean =>
	useDockValue(
		useCallback((notify) => containerApi.onDidMaximizedGroupChange(notify), [containerApi]),
		() => group.api.isMaximized()
	);

const Icon = ({ size = 13, children, ...props }: SVGProps<SVGSVGElement> & { size?: number }) => {
	useI18n();
	return (
		<svg
			aria-hidden="true"
			width={size}
			height={size}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2}
			strokeLinecap="round"
			{...props}
		>
			{children}
		</svg>
	);
};

const HeaderButton = ({
	label,
	shortcut,
	small,
	disabled,
	onClick,
	children,
}: {
	label: string;
	shortcut?: Shortcut;
	small?: boolean;
	disabled?: boolean;
	onClick: () => void;
	children: ReactNode;
}) => {
	useI18n();
	return (
		<button
			type="button"
			aria-label={label}
			aria-keyshortcuts={shortcut ? ariaKeys(shortcut) : undefined}
			title={shortcut ? `${label} ${keyHint(shortcut)}` : label}
			disabled={disabled}
			// Keep the press from starting a drag of the group.
			onPointerDown={(e) => e.stopPropagation()}
			onClick={onClick}
			className={cn(
				"grid place-items-center rounded-[5px] text-muted-foreground hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40",
				small ? "size-[22px]" : "size-6"
			)}
		>
			{children}
		</button>
	);
};

// A tab's frame: what it starts with, its title, what follows, and a close
// button. A middle click closes it too.
const PaneTab = ({
	api,
	title,
	visible,
	lead,
	strong,
	waiting,
	children,
}: {
	api: DockviewPanelApi;
	title: string;
	visible: boolean;
	lead: ReactNode;
	strong?: boolean;
	waiting?: boolean;
	children?: ReactNode;
}) => {
	const { t } = useI18n();

	const middle = useRef(false);
	const frame = useRef<HTMLDivElement>(null);
	// dockview scrolls the strip to the shown tab before its title draws, so
	// the strip can stop short. Finish the scroll once the tab has its size.
	useLayoutEffect(() => {
		if (visible && frame.current) revealTab(frame.current);
	}, [visible, title]);
	return (
		<div
			ref={frame}
			data-needs-input={waiting ? "" : undefined}
			className="group/tab flex h-full items-center gap-[7px] pr-2 pl-3 select-none"
			onPointerDown={(e) => {
				middle.current = e.button === 1;
			}}
			onPointerUp={(e) => {
				if (middle.current && e.button === 1) api.close();
				middle.current = false;
			}}
			onPointerLeave={() => {
				middle.current = false;
			}}
		>
			{lead}
			<span className={cn("max-w-[28ch] truncate", strong && "font-[650] text-foreground")}>
				{title}
			</span>
			{children}
			<button
				type="button"
				aria-label={t("Close {{title}}", { title: title })}
				title={t("Close {{value1}}", { value1: keyHint(SHORTCUTS.close) })}
				className={cn(
					"grid size-4 place-items-center rounded-[4px] text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:opacity-100",
					!visible && "opacity-0 group-hover/tab:opacity-100"
				)}
				onPointerDown={(e) => e.preventDefault()}
				onClick={(e) => {
					e.preventDefault();
					e.stopPropagation();
					api.close();
				}}
			>
				<Icon size={10} strokeWidth={2.4}>
					<path d="M6 6l12 12M18 6L6 18" />
				</Icon>
			</button>
		</div>
	);
};

// What an agent's pane is given. A layout saved before panes had kinds
// gives only this, so its panes read it alone.
type AgentParams = { readonly agent: AgentId };

/** An agent pane's tab: the agent's dot and title, and an Input badge when it waits on you. */
const AgentTab = ({ api, params }: IDockviewPanelHeaderProps<AgentParams>) => {
	const { t } = useI18n();

	const agent = useView((s) => s.view.tables.agent.get(params.agent));
	const now = useNow(5000);
	const visible = usePanelVisible(api);
	const title = agent?.title ?? api.title ?? t("Agent");

	// The tab's accessible name and the saved layout use dockview's title.
	useEffect(() => {
		if (agent && api.title !== agent.title) api.setTitle(agent.title);
	}, [api, agent]);

	const look = agent ? agentLook(agent, now) : null;
	const needsInput = look === "needs_input";
	return (
		<PaneTab
			api={api}
			title={title}
			visible={visible}
			lead={look ? <ActivityDot look={look} /> : null}
			strong={look === "done_unread" && !visible}
			waiting={needsInput}
		>
			{needsInput ? (
				<span className="inline-flex h-4 items-center rounded-[4px] bg-status-ready/16 px-[5px] text-[10.5px] font-semibold text-status-ready-text">
					{t("Input")}
				</span>
			) : null}
		</PaneTab>
	);
};

/** A run canvas's tab: the workflow icon and the run's name. */
const RunTab = ({
	api,
	params,
}: IDockviewPanelHeaderProps<Extract<PaneParams, { kind: "workflow" }>>) => {
	const { t } = useI18n();

	const run = useView((s) => runAt(s.view, params.run));
	const visible = usePanelVisible(api);
	const title = run?.label ?? api.title ?? t("Workflow");
	useEffect(() => {
		if (run && api.title !== run.label) api.setTitle(run.label);
	}, [api, run]);
	return (
		<PaneTab
			api={api}
			title={title}
			visible={visible}
			lead={
				<Workflow
					aria-hidden
					size={12}
					strokeWidth={2}
					className="shrink-0 text-muted-foreground"
				/>
			}
			waiting={run?.needs_you}
		/>
	);
};

type NoteParams = Extract<PaneParams, { kind: "note" }>;

/** A note's tab: the note icon, its title as typed, and a dot while a change waits to be saved. */
const NoteTab = ({ api, params }: IDockviewPanelHeaderProps<NoteParams>) => {
	const { t } = useI18n();

	const summary = useView((s) => noteSummary(s.view, params.project, params.note));
	const open = useOpenNote(params.project, params.note);
	const visible = usePanelVisible(api);
	const title = (open?.title ?? summary?.title ?? api.title) || t("Untitled");
	useEffect(() => {
		if (api.title !== title) api.setTitle(title);
	}, [api, title]);
	return (
		<PaneTab
			api={api}
			title={title}
			visible={visible}
			lead={
				<NotebookText
					aria-hidden
					size={12}
					strokeWidth={2}
					className="shrink-0 text-muted-foreground"
				/>
			}
		>
			{open?.unsaved ? (
				<span
					role="img"
					aria-label={t("Not saved yet")}
					title={t("Not saved yet")}
					className="size-1.5 shrink-0 rounded-full bg-muted-foreground"
				/>
			) : null}
		</PaneTab>
	);
};

/** What a note's pane shows: the note, mounted while the pane is open. */
const NotePane = ({ api, params }: IDockviewPanelProps<NoteParams>) => {
	useI18n();
	return (
		<ErrorBoundary part="note pane" className="bg-background">
			<NotePaneFor project={params.project} note={params.note} close={() => api.close()} />
		</ErrorBoundary>
	);
};

type FileParams = Extract<PaneParams, { kind: "file" }>;

/** A file's tab: the file icon and the file's name. */
const FileTab = ({ api }: IDockviewPanelHeaderProps<FileParams>) => {
	const { t } = useI18n();

	const visible = usePanelVisible(api);
	return (
		<PaneTab
			api={api}
			title={api.title ?? t("File")}
			visible={visible}
			lead={
				<FileText
					aria-hidden
					size={12}
					strokeWidth={2}
					className="shrink-0 text-muted-foreground"
				/>
			}
		/>
	);
};

/** What a file's pane shows: the file, read from its agent's device. */
const FilePane = ({ params }: IDockviewPanelProps<FileParams>) => {
	useI18n();
	return (
		<ErrorBoundary part="file pane" className="bg-background">
			<FilePaneFor agent={params.agent} path={params.path} />
		</ErrorBoundary>
	);
};

type VisualParams = Extract<PaneParams, { kind: "visual" }>;

/** A visual's tab: the pink diamond and the visual's title. */
const VisualTab = ({ api, params }: IDockviewPanelHeaderProps<VisualParams>) => {
	const { t } = useI18n();
	const visible = usePanelVisible(api);
	return (
		<PaneTab
			api={api}
			title={params.title || t("Visual")}
			visible={visible}
			lead={
				<span aria-hidden className="shrink-0 text-[11px] text-(--pill-visual-text)">
					◆
				</span>
			}
		/>
	);
};

/** What a visual's pane shows: its page, while this window has it. */
const VisualPane = ({ api, params }: IDockviewPanelProps<VisualParams>) => {
	const close = useCallback(() => api.close(), [api]);
	return (
		<ErrorBoundary part="visual pane" className="bg-background">
			<VisualPaneFor pageKey={params.key} close={close} />
		</ErrorBoundary>
	);
};

/**
 * What a pane shows: its agent's view, mounted while the pane is open. A
 * sub-agent gone with its session points to where the session went, and
 * one that finished shows its history.
 */
const AgentPane = ({ params }: IDockviewPanelProps<AgentParams>) => {
	const { t } = useI18n();

	const agent = useView((s) => s.view.tables.agent.get(params.agent));
	const moved = useView((s) => (agent ? null : movedTo(s.view.tables.agent, params.agent)));
	if (moved)
		return (
			<div className="grid h-full place-items-center px-8">
				<SessionMoved lead={moved} />
			</div>
		);
	if (!agent)
		return (
			<FinishedOr agent={params.agent}>
				{t("This agent isn't here. It may still be starting.")}
			</FinishedOr>
		);
	return (
		<ErrorBoundary part="agent pane" className="bg-background">
			<AgentView agent={agent} />
		</ErrorBoundary>
	);
};

/** A finished sub-agent's history, or why it can't show, or `children`. */
const FinishedOr = ({ agent, children }: { agent: AgentId; children: string }) => {
	const lead = useView((s) => finishedLead(s.view, agent));
	const note = useView((s) => finishedNote(s.view, agent));
	if (!lead) return <Empty>{note ?? children}</Empty>;
	return (
		<ErrorBoundary part="agent pane" className="bg-background">
			<FinishedView agent={agent} lead={lead} />
		</ErrorBoundary>
	);
};

/** What a run's pane shows: its canvas, mounted while the pane is open. */
const RunPane = ({ params }: IDockviewPanelProps<Extract<PaneParams, { kind: "workflow" }>>) => {
	useI18n();
	return (
		<ErrorBoundary part="workflow pane" className="bg-background">
			<CanvasPaneFor run={params.run} />
		</ErrorBoundary>
	);
};

/**
 * How many of a group's tabs behind the visible one wait on you. Clicking
 * the count shows the first of them.
 */
const HiddenWaiting = ({ activePanel, panels }: IDockviewHeaderActionsProps) => {
	const { t } = useI18n();

	const agents = useView((s) => s.view.tables.agent);
	const waiting = panels.filter(
		(p) =>
			p !== activePanel && agents.get(paneAgent(p.id) ?? "")?.activity.state === "needs_input"
	);
	if (waiting.length === 0) return null;
	const label = t("{{countText}} hidden tabs need you", {
		count: waiting.length,
		countText: formatNumber(waiting.length),
	});
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			onPointerDown={(e) => e.stopPropagation()}
			onClick={() => waiting[0]!.api.setActive()}
			className="mr-1 inline-flex h-4 items-center gap-1 rounded-[4px] bg-status-ready/16 px-[5px] text-[10.5px] font-semibold text-status-ready-text tabular-nums hover:bg-status-ready/24"
		>
			<span aria-hidden className="size-1.5 rounded-full bg-status-ready" />
			{formatNumber(waiting.length)}
		</button>
	);
};

/** The buttons at the right of a group's tabs. */
const GroupActions = (props: IDockviewHeaderActionsProps) => {
	const { t } = useI18n();

	const { containerApi, group, panels } = props;
	const floating = useFloating(group);
	const maximized = useMaximized(containerApi, group);

	if (floating) {
		return (
			<div className="flex items-center gap-[7px] pr-1.5 pl-2">
				<HiddenWaiting {...props} />
				<span className="text-[11px] text-muted-foreground">{t("Floating")}</span>
				<HeaderButton
					label={t("Dock this pane")}
					small
					onClick={() => group.api.moveTo({ position: "right" })}
				>
					<Icon size={12}>
						<rect x="3" y="4" width="18" height="16" rx="2" />
						<path d="M3 15h18" />
					</Icon>
				</HeaderButton>
			</div>
		);
	}

	return (
		<div className="flex items-center gap-0.5 px-1.5">
			<HiddenWaiting {...props} />
			<HeaderButton
				label={t("Split pane")}
				shortcut={SHORTCUTS.splitRight}
				disabled={panels.length < 2}
				onClick={() => splitGroup(containerApi, group, "right")}
			>
				<Icon>
					<rect x="3" y="4" width="18" height="16" rx="2" />
					<path d="M12 4v16" />
				</Icon>
			</HeaderButton>
			<HeaderButton
				label={maximized ? t("Restore group") : t("Maximize group")}
				shortcut={SHORTCUTS.maximize}
				onClick={() => toggleMaximized(group)}
			>
				<Icon>
					{maximized ? (
						<path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7" />
					) : (
						<path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7" />
					)}
				</Icon>
			</HeaderButton>
		</div>
	);
};

/** The grip at the left of a floating group's tabs, which you drag it by. */
const GroupGrip = ({ group }: IDockviewHeaderActionsProps) => {
	useI18n();

	const floating = useFloating(group);
	if (!floating) return null;
	return (
		<span className="grid h-full place-items-center pr-0.5 pl-2.5 text-subtle-foreground">
			<svg aria-hidden="true" width="10" height="12" viewBox="0 0 24 24" fill="currentColor">
				<circle cx="9" cy="6" r="1.6" />
				<circle cx="15" cy="6" r="1.6" />
				<circle cx="9" cy="12" r="1.6" />
				<circle cx="15" cy="12" r="1.6" />
				<circle cx="9" cy="18" r="1.6" />
				<circle cx="15" cy="18" r="1.6" />
			</svg>
		</span>
	);
};

type MenuItem = BuiltInContextMenuItem | ReactContextMenuItemConfig;

type RowProps = {
	readonly label: string;
	readonly shortcut?: Shortcut;
	readonly disabled?: boolean;
	readonly run: () => void;
};

// A tab menu row with its shortcut at the right. dockview's own rows only
// take a label.
const MenuRow = ({ close, componentProps }: IContextMenuItemComponentProps) => {
	useI18n();

	const { label, shortcut, disabled, run } = componentProps as RowProps;
	return (
		<div
			role="menuitem"
			aria-disabled={disabled || undefined}
			aria-keyshortcuts={shortcut ? ariaKeys(shortcut) : undefined}
			className={cn(
				"dv-context-menu-item justify-between gap-6",
				disabled && "dv-context-menu-item--disabled"
			)}
			onClick={() => {
				if (disabled) return;
				run();
				close();
			}}
		>
			<span>{label}</span>
			{shortcut ? <span className="text-muted-foreground">{keyHint(shortcut)}</span> : null}
		</div>
	);
};

const row = (props: RowProps): MenuItem => ({ component: MenuRow, componentProps: props });

/** The menu a tab opens on a right click, for the panes of one window. */
export const tabMenuFor =
	(panes: Panes) =>
	({ panel, group, api }: GetTabContextMenuItemsParams): MenuItem[] => {
		const floating = group.api.location.type === "floating";
		const others = group.panels.filter((p) => p !== panel);
		const layout: MenuItem[] = floating
			? [row({ label: t("Dock"), run: () => group.api.moveTo({ position: "right" }) })]
			: [
					row({ label: t("Float"), run: () => api.addFloatingGroup(panel) }),
					row({
						label: group.api.isMaximized() ? t("Restore group") : t("Maximize group"),
						shortcut: SHORTCUTS.maximize,
						run: () => toggleMaximized(group),
					}),
					row({
						label: t("Split right"),
						shortcut: SHORTCUTS.splitRight,
						disabled: !canSplit(group),
						run: () => splitGroup(api, group, "right", panel),
					}),
					row({
						label: t("Split down"),
						shortcut: SHORTCUTS.splitDown,
						disabled: !canSplit(group),
						run: () => splitGroup(api, group, "below", panel),
					}),
				];
		return [
			...layout,
			"separator",
			row({ label: t("Close"), shortcut: SHORTCUTS.close, run: () => panel.api.close() }),
			row({
				label: t("Close others"),
				disabled: others.length === 0,
				run: () => {
					for (const other of others) other.api.close();
				},
			}),
			row({
				label: t("Reopen closed pane"),
				shortcut: SHORTCUTS.reopen,
				disabled: !panes.canReopen(),
				run: () => panes.reopen(),
			}),
		];
	};

/**
 * The panes, kept mounted in devices mode so their views and sizes survive a
 * trip to the devices list. Hidden, they leave the accessibility tree too.
 * dockview marks its views with a `visible` class, which is also the utility
 * that undoes `invisible`, and lifts its empty-pane note to z-index 1. The
 * panes' own stacking context keeps that note under the page drawn on top.
 */
export const AgentPanes = ({ hidden }: { hidden: boolean }) => {
	useI18n();

	const services = useServices();
	const [api, setApi] = useState<DockviewApi | null>(null);
	const [label] = useState(windowLabel);
	const tabMenu = useMemo(() => tabMenuFor(panesOf(services)), [services]);
	const scheme = useTheme();
	const theme = useMemo(() => ({ ...THEME, colorScheme: scheme }), [scheme]);

	useEffect(() => (api ? syncPanes(services, api, label) : undefined), [api, services, label]);
	useEffect(() => watchShortcuts(services), [services]);

	return (
		<div
			className={cn("absolute inset-0 isolate", hidden && "invisible")}
			aria-hidden={hidden || undefined}
			inert={hidden}
		>
			<DockviewReact
				theme={theme}
				dndStrategy="pointer"
				floatingGroupDragHandle="tabbar"
				floatingGroupBounds="boundedWithinViewport"
				components={{
					[AGENT_PANE]: AgentPane,
					[WORKFLOW_PANE]: RunPane,
					[NOTE_PANE]: NotePane,
					[FILE_PANE]: FilePane,
					[VISUAL_PANE]: VisualPane,
				}}
				tabComponents={{
					[AGENT_PANE]: AgentTab,
					[WORKFLOW_PANE]: RunTab,
					[NOTE_PANE]: NoteTab,
					[FILE_PANE]: FileTab,
					[VISUAL_PANE]: VisualTab,
				}}
				watermarkComponent={NoPane}
				rightHeaderActionsComponent={GroupActions}
				prefixHeaderActionsComponent={GroupGrip}
				getTabContextMenuItems={tabMenu}
				onReady={(event) => setApi(event.api)}
			/>
		</div>
	);
};
