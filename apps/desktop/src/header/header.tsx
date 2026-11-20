// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The title bar. The window draws its own, so the header leaves room for the
// traffic lights and lets you drag the window by its empty parts. The
// button beside them hides and shows the sidebar, the sun after the title
// keeps the screen on, the box in the middle asks the dispatcher, and the
// bell opens the notification inbox.

import { useI18n } from "@maxhq/shared/i18n";

import {} from "react";

import { SettingsButton } from "@/settings/settings";
import { SidebarToggle, useSidebarWidth } from "@/sidebar/frame";

import { DispatchBox } from "./dispatcher";
import { NotificationBell } from "./notifications";
import { ScreenToggle } from "./screen-toggle";
import { UsageMeter } from "./usage";

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
				<NotificationBell />
				<SettingsButton />
			</div>
		</header>
	);
};
