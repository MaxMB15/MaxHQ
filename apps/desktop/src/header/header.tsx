// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The title bar. The window draws its own, so the header leaves room for the
// traffic lights and lets you drag the window by its empty parts. The
// button beside them hides and shows the sidebar, the sun after the title
// keeps the screen on, and the box in the middle asks the dispatcher.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import {} from "react";

import { openNextWaiting } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { ariaKeys, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { SettingsButton } from "@/settings/settings";
import { SidebarToggle, useSidebarWidth } from "@/sidebar/frame";

import { needsYou } from "@maxhq/shared/view/select";

import { DispatchBox } from "./dispatcher";
import { ScreenToggle } from "./screen-toggle";
import { UsageMeter } from "./usage";

/** What the bell's screen reader label says. */
export const bellLabel = (count: number): string =>
	count === 0
		? t("No agent needs you")
		: t("{{countText}} agents need you", { count, countText: formatNumber(count) });

/** What the bell's tooltip says, with the key that does the same. */
export const bellTitle = (): string =>
	t("Open the next agent waiting {{value1}}", { value1: keyHint(SHORTCUTS.nextWaiting) });

// Each press opens the next agent waiting, going round as ⌘J does.
const Bell = () => {
	useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const items = needsYou(view);
	return (
		<button
			type="button"
			aria-label={bellLabel(items.length)}
			aria-keyshortcuts={ariaKeys(SHORTCUTS.nextWaiting)}
			title={items.length > 0 ? bellTitle() : undefined}
			disabled={items.length === 0}
			onClick={() => openNextWaiting(services)}
			className="relative grid size-[30px] place-items-center rounded-[7px] text-secondary-foreground enabled:hover:bg-secondary"
		>
			<svg
				aria-hidden
				width={17}
				height={17}
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				strokeWidth={1.8}
				strokeLinecap="round"
				strokeLinejoin="round"
			>
				<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0" />
			</svg>
			{items.length > 0 ? (
				<span
					aria-hidden
					className="absolute top-[3px] right-0.5 h-[15px] min-w-[15px] rounded-lg bg-status-ready px-1 text-center text-[10px] leading-[15px] font-bold text-status-ready-foreground"
				>
					{formatNumber(items.length)}
				</span>
			) : null}
		</button>
	);
};

// The header's left padding, which the start takes off the sidebar's width.
const INSET = 14;

// As wide as the sidebar, so the box starts where the main area does, or
// only as wide as it needs while the sidebar is hidden. It draws on its
// own as you drag the sidebar's edge.
const Start = () => {
	useI18n();

	const sidebar = useSidebarWidth();
	return (
		<div
			data-tauri-drag-region
			className="flex shrink-0 items-center"
			style={{ width: sidebar === null ? undefined : sidebar - INSET }}
		>
			{/* Room for the window's traffic lights, which tauri.conf.json centres on the header, and a gap before the toggle. */}
			<span data-tauri-drag-region aria-hidden className="w-[74px] shrink-0" />
			<SidebarToggle />
			<span
				data-tauri-drag-region
				className="ml-1.5 text-sm font-[650] tracking-[-0.01em] text-foreground"
			>
				MaxHQ
			</span>
			<span data-tauri-drag-region aria-hidden className="w-1 shrink-0" />
			<ScreenToggle />
		</div>
	);
};

/** The header. */
export const Header = () => {
	useI18n();
	return (
		<header
			data-tauri-drag-region
			className="flex h-[52px] shrink-0 items-center gap-3 border-b border-sidebar-border bg-sidebar pr-3 pl-3.5"
		>
			<Start />
			<DispatchBox />
			<div className="flex shrink-0 items-center gap-3.5">
				<UsageMeter />
				<Bell />
				<SettingsButton />
			</div>
		</header>
	);
};
