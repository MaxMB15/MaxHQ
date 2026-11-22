// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminal drawer under an agent's composer. Its bar has a tab for each
// shell and each command you moved there, a + for another shell, the folder
// and device they run in, a split that shows two tabs side by side, a way
// out to a pane of its own and a hide button. You drag its top edge to make
// it taller. The first time it opens on an agent with no terminals, it
// opens a shell. A tab out in a pane leaves the bar until the pane closes.

import { Columns2, Lock, Plus, SquareArrowOutUpRight, SquareTerminal, X } from "lucide-react";
import {
	type KeyboardEvent,
	type PointerEvent,
	type RefObject,
	useEffect,
	useRef,
	useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { ICON_BUTTON } from "@/agent-view/find/bar";
import { useServices, useView } from "@/app/services";
import { ErrorBoundary } from "@/components/error-boundary";
import { ariaKeys, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { followPointer } from "@/lib/drag";
import { cn } from "@/lib/utils";
import type { AgentId, DeviceId } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { runsCommand, tabTitle } from "@maxhq/shared/terminal/drawer";
import { supportsTerminals, type TerminalEntity, terminalsOf } from "@maxhq/shared/terminal/select";
import { deviceName } from "@maxhq/shared/view/select";

import {
	activeOf,
	closeTab,
	openFirstTab,
	openTab,
	popOut,
	renameTab,
	toggleSplit,
} from "./actions";
import {
	clampHeight,
	DEFAULT_HEIGHT,
	drawerOf,
	drawersOf,
	MAX_HEIGHT,
	MIN_HEIGHT,
	setActive,
	setDrawer,
	useDrawers,
} from "./drawer-prefs";
import { registryOf } from "./live";
import { usePopped } from "./shown";
import { TerminalBody } from "./terminal-body";

// How far an arrow key moves the edge.
const STEP = 16;

// What the drawer leaves above it for the timeline and the composer.
const ROOM = 240;

/** The line that names where a terminal runs, such as `~/code/example-app · Mac`. */
export const whereLine = (cwd: string, device: string): string =>
	cwd ? `${cwd} · ${device}` : device;

/** The drawer's top edge, which you drag or move with the arrow keys. */
const Edge = ({
	agent,
	frame,
	onDrag,
}: {
	agent: AgentId;
	frame: RefObject<HTMLDivElement | null>;
	/** Hears the height while you drag, and null once you let go. */
	onDrag: (height: number | null) => void;
}) => {
	useI18n();
	const services = useServices();
	const height = useDrawers((s) => drawerOf(s, agent).height);
	const save = (next: number) => setDrawer(services, agent, { height: next });

	const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
		if (e.button !== 0) return;
		e.preventDefault();
		const shown = frame.current?.getBoundingClientRect().height ?? height;
		const room = (frame.current?.parentElement?.clientHeight ?? Infinity) - ROOM;
		const from = e.clientY + shown;
		let last = clampHeight(shown);
		followPointer(
			e,
			"row-resize",
			(m) => {
				last = clampHeight(Math.min(room, from - m.clientY));
				onDrag(last);
			},
			() => {
				save(last);
				onDrag(null);
			}
		);
	};

	const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
		const by = { ArrowUp: STEP, ArrowDown: -STEP }[e.key];
		if (by === undefined) return;
		e.preventDefault();
		save(height + by);
	};

	return (
		<div
			role="separator"
			aria-orientation="horizontal"
			aria-label={translate("Resize the terminal")}
			aria-valuenow={height}
			aria-valuemin={MIN_HEIGHT}
			aria-valuemax={MAX_HEIGHT}
			tabIndex={0}
			title={translate("Drag to resize, double-click for the default height")}
			onPointerDown={onPointerDown}
			onDoubleClick={() => save(DEFAULT_HEIGHT)}
			onKeyDown={onKeyDown}
			className="grid h-[7px] shrink-0 cursor-row-resize place-items-center border-t border-border bg-card outline-none hover:bg-accent focus-visible:bg-primary/30"
		>
			<span aria-hidden className="h-[3px] w-9 rounded-full bg-border" />
		</div>
	);
};

/** The field a tab's title turns into while you rename it. */
const RenameField = ({
	initial,
	done,
}: {
	initial: string;
	/** Hears the new name, or null when you let it be. */
	done: (name: string | null) => void;
}) => {
	useI18n();
	const [value, setValue] = useState(initial);
	const input = useRef<HTMLInputElement>(null);
	const finished = useRef(false);
	const finish = (name: string | null) => {
		if (finished.current) return;
		finished.current = true;
		done(name);
	};
	useEffect(() => {
		input.current?.focus();
		input.current?.select();
	}, []);
	return (
		<input
			ref={input}
			value={value}
			aria-label={translate("Rename")}
			spellCheck={false}
			onChange={(e) => setValue(e.target.value)}
			onKeyDown={(e) => {
				e.stopPropagation();
				if (e.key === "Enter") finish(value);
				else if (e.key === "Escape") finish(null);
			}}
			onBlur={() => finish(value)}
			onClick={(e) => e.stopPropagation()}
			onDoubleClick={(e) => e.stopPropagation()}
			className="w-[16ch] min-w-0 bg-transparent text-xs text-foreground outline-none"
		/>
	);
};

/** One tab in the drawer's bar. */
const Tab = ({
	terminal,
	agent,
	active,
}: {
	terminal: TerminalEntity;
	agent: AgentId;
	active: boolean;
}) => {
	useI18n();
	const services = useServices();
	const [renaming, setRenaming] = useState(false);
	const title = tabTitle(terminal);
	const running = runsCommand(terminal);
	const Icon = terminal.password_prompt ? Lock : SquareTerminal;

	const pick = () => {
		setActive(services, agent, terminal.id);
		drawersOf(services).setState({ focusing: agent });
	};
	const rename = (name: string | null) => {
		setRenaming(false);
		if (name !== null && name.trim() !== title)
			void renameTab(services, terminal.id, name.trim());
	};

	return (
		<div
			role="tab"
			aria-selected={active}
			tabIndex={active ? 0 : -1}
			title={title}
			data-terminal-tab={terminal.id}
			onClick={pick}
			onDoubleClick={() => setRenaming(true)}
			onKeyDown={(e) => {
				if (e.target !== e.currentTarget) return;
				if (e.key === "Enter" || e.key === " ") {
					e.preventDefault();
					pick();
				} else if (e.key === "F2") setRenaming(true);
			}}
			className={cn(
				"group/ttab flex h-6 max-w-[220px] shrink-0 cursor-pointer items-center gap-1.5 rounded-[6px] pr-1 pl-[9px] text-xs outline-none select-none focus-visible:ring-2 focus-visible:ring-ring",
				active
					? "bg-accent text-foreground"
					: "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
			)}
		>
			<Icon
				aria-hidden
				size={12}
				strokeWidth={2}
				className={cn("shrink-0", terminal.password_prompt && "text-status-busy-text")}
			/>
			{renaming ? (
				<RenameField initial={title} done={rename} />
			) : (
				<span
					className={cn("truncate", terminal.status.state === "exited" && "opacity-60")}
				>
					{title}
				</span>
			)}
			{running ? (
				<span
					role="img"
					aria-label={translate("Running")}
					className="size-1.5 shrink-0 rounded-full bg-status-ready"
				/>
			) : null}
			<button
				type="button"
				aria-label={translate("Close {{title}}", { title })}
				onClick={(e) => {
					e.stopPropagation();
					void closeTab(services, terminal);
				}}
				onDoubleClick={(e) => e.stopPropagation()}
				className={cn(
					"grid size-4 shrink-0 place-items-center rounded-[4px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring",
					!active && "opacity-0 group-hover/ttab:opacity-100"
				)}
			>
				<X aria-hidden size={10} strokeWidth={2.4} />
			</button>
		</div>
	);
};

const Drawer = ({ agent, device }: { agent: AgentId; device: DeviceId }) => {
	useI18n();
	const services = useServices();
	const frame = useRef<HTMLDivElement>(null);
	const [dragged, setDragged] = useState<number | null>(null);
	const height = useDrawers((s) => drawerOf(s, agent).height);
	const picked = useDrawers((s) => s.active[agent]);
	const besideId = useDrawers((s) => s.split[agent]);
	const focusing = useDrawers((s) => s.focusing === agent);
	const all = useView(useShallow((s) => terminalsOf(s.view, agent)));
	const name = useView((s) => deviceName(s.view, device));
	const popped = usePopped(services);
	const tabs = all.filter((tab) => !popped.has(tab.id));
	const active = activeOf(tabs, picked);
	const beside = besideId === active?.id ? undefined : tabs.find((tab) => tab.id === besideId);
	// Side by side in the order of their tabs, so they stay put as you click between them.
	const shown = tabs.filter((tab) => tab === active || tab === beside);

	// A drawer that opens on an agent with no terminals opens a shell.
	useEffect(() => {
		if (terminalsOf(services.view.getState().view, agent).length === 0)
			openFirstTab(services, agent);
	}, [services, agent]);

	// The tab placed itself in an effect of its own, which runs before this one.
	useEffect(() => {
		if (!focusing || !active) return;
		const view = registryOf(services)?.view(active.id);
		if (!view) return;
		view.focus();
		drawersOf(services).setState({ focusing: null });
	}, [services, focusing, active]);

	const iconButton = cn(
		ICON_BUTTON,
		"aria-pressed:bg-accent aria-pressed:text-foreground hover:bg-accent"
	);
	const hide = translate("Hide");
	return (
		<div
			ref={frame}
			role="region"
			aria-label={translate("Terminal")}
			className="flex shrink-0 flex-col"
			style={{
				height: `min(${dragged ?? height}px, max(${MIN_HEIGHT}px, 100% - ${ROOM}px))`,
			}}
		>
			<Edge agent={agent} frame={frame} onDrag={setDragged} />
			<div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-border bg-card pr-1.5 pl-2">
				<div
					role="tablist"
					aria-label={translate("Terminals")}
					className="flex min-w-0 items-center gap-0.5 overflow-x-auto [scrollbar-width:none]"
				>
					{tabs.map((tab) => (
						<Tab key={tab.id} terminal={tab} agent={agent} active={tab === active} />
					))}
				</div>
				<button
					type="button"
					aria-label={translate("New terminal")}
					title={translate("New terminal")}
					onClick={() => void openTab(services, agent)}
					className={iconButton}
				>
					<Plus aria-hidden size={14} strokeWidth={2} />
				</button>
				<span className="mr-1.5 ml-auto min-w-0 truncate pl-3 font-mono text-[11px] text-subtle-foreground">
					{whereLine(active?.cwd ?? "", name)}
				</span>
				<button
					type="button"
					aria-label={translate("Split")}
					title={translate("Split")}
					aria-pressed={beside !== undefined}
					disabled={!active}
					onClick={() => void toggleSplit(services, agent, tabs)}
					className={iconButton}
				>
					<Columns2 aria-hidden size={14} strokeWidth={1.9} />
				</button>
				<button
					type="button"
					aria-label={translate("Open as pane")}
					title={translate("Open as pane")}
					disabled={!active}
					onClick={() => active && popOut(services, agent, active.id)}
					className={iconButton}
				>
					<SquareArrowOutUpRight aria-hidden size={13} strokeWidth={1.9} />
				</button>
				<button
					type="button"
					aria-label={hide}
					aria-keyshortcuts={ariaKeys(SHORTCUTS.terminal)}
					title={`${hide} ${keyHint(SHORTCUTS.terminal)}`}
					onClick={() => setDrawer(services, agent, { open: false })}
					className={iconButton}
				>
					<X aria-hidden size={14} strokeWidth={2} />
				</button>
			</div>
			<div
				className={cn(
					"grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)] bg-(--terminal-background)",
					shown.length > 1 ? "grid-cols-2" : "grid-cols-1"
				)}
			>
				{shown.map((tab, at) => (
					<div
						key={tab.id}
						role="tabpanel"
						className={cn("flex min-h-0 min-w-0", at > 0 && "border-l border-border")}
						onPointerDownCapture={() => setActive(services, agent, tab.id)}
					>
						<TerminalBody terminal={tab} className="flex-1" />
					</div>
				))}
				{shown.length === 0 && all.length > 0 ? (
					<p className="m-auto text-xs text-muted-foreground">
						{translate("Your terminals are open as panes.")}
					</p>
				) : null}
			</div>
		</div>
	);
};

/**
 * The agent's terminal drawer while it's open, on a device whose `maxd`
 * has terminals.
 */
export const TerminalDrawer = ({ agent, device }: { agent: AgentId; device: DeviceId }) => {
	const supported = useView((s) => supportsTerminals(s.view, device));
	const open = useDrawers((s) => drawerOf(s, agent).open);
	if (!supported || !open) return null;
	return (
		<ErrorBoundary key={`${agent}:terminal`} part="terminal drawer" look="strip">
			<Drawer agent={agent} device={device} />
		</ErrorBoundary>
	);
};
