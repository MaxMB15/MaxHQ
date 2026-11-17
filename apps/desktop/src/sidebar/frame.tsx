// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's place in the window: its width, the edge you drag to
// change it, and the header button that hides and shows it. Dragging the
// edge or pressing an arrow key on it changes the width, and a
// double-click puts the default back. While you drag, only this frame and
// the header's start draw again, and the width is saved once you let go.

import { useI18n } from "@maxhq/shared/i18n";

import { PanelLeft, PanelLeftClose } from "lucide-react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";

import { useServices } from "@/app/services";
import { ErrorBoundary } from "@/components/error-boundary";
import { ariaKeys, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { followPointer } from "@/lib/drag";

import {
	clampWidth,
	DEFAULT_WIDTH,
	MAX_WIDTH,
	MIN_WIDTH,
	prefsOf,
	setPrefs,
	usePrefs,
} from "./prefs";
import { TOGGLE_ID, toggleSidebar } from "./toggle";

// How far an arrow key moves the edge.
const STEP = 16;

const ResizeEdge = () => {
	const { t } = useI18n();

	const services = useServices();
	const width = usePrefs((p) => p.width);
	const show = (next: number) => prefsOf(services).setState({ width: clampWidth(next) });
	const save = (next: number) => setPrefs(services, { width: clampWidth(next) });

	const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
		if (e.button !== 0) return;
		e.preventDefault();
		const from = e.clientX - width;
		followPointer(
			e,
			"col-resize",
			(m) => show(m.clientX - from),
			() => save(prefsOf(services).getState().width)
		);
	};

	const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
		const by = { ArrowLeft: -STEP, ArrowRight: STEP }[e.key];
		if (by === undefined) return;
		e.preventDefault();
		save(width + by);
	};

	return (
		<div
			role="separator"
			aria-orientation="vertical"
			aria-label={t("Resize the sidebar")}
			aria-valuenow={width}
			aria-valuemin={MIN_WIDTH}
			aria-valuemax={MAX_WIDTH}
			tabIndex={0}
			title={t("Drag to resize, double-click for the default width")}
			onPointerDown={onPointerDown}
			onDoubleClick={() => save(DEFAULT_WIDTH)}
			onKeyDown={onKeyDown}
			className="absolute inset-y-0 -right-[3px] z-10 w-1.5 cursor-col-resize outline-none hover:bg-primary/40 focus-visible:bg-primary/60"
		/>
	);
};

/** The sidebar's width, or null while it's hidden. */
export const useSidebarWidth = (): number | null => {
	const open = usePrefs((p) => p.open);
	const width = usePrefs((p) => p.width);
	return open ? width : null;
};

/** The sidebar at its width with its edge, or nothing while it's hidden. */
export const SidebarFrame = ({ children }: { children: ReactNode }) => {
	useI18n();

	const width = useSidebarWidth();
	if (width === null) return null;
	return (
		<div className="relative flex shrink-0" style={{ width }}>
			<ErrorBoundary
				part="sidebar"
				className="h-auto w-full border-r border-sidebar-border bg-sidebar px-4"
			>
				{children}
			</ErrorBoundary>
			<ResizeEdge />
		</div>
	);
};

/** The header's button that hides the sidebar and brings it back. */
export const SidebarToggle = () => {
	const { t } = useI18n();

	const services = useServices();
	const open = usePrefs((p) => p.open);
	const label = open ? t("Hide the sidebar") : t("Show the sidebar");
	const Icon = open ? PanelLeftClose : PanelLeft;
	return (
		<button
			id={TOGGLE_ID}
			type="button"
			aria-label={label}
			aria-keyshortcuts={ariaKeys(SHORTCUTS.sidebar)}
			title={`${label} ${keyHint(SHORTCUTS.sidebar)}`}
			onClick={() => toggleSidebar(services)}
			className="grid size-[30px] shrink-0 place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
		>
			<Icon aria-hidden size={16} strokeWidth={1.9} />
		</button>
	);
};
