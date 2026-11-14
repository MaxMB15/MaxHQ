// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A native context menu built from the page, such as the one on a
// selection in the timeline. Tauri's menu API builds it, so it needs no
// command of its own.

import { t } from "@maxhq/shared/i18n";

import { LogicalPosition } from "@tauri-apps/api/dpi";
import {
	Menu,
	type MenuItemOptions,
	type PredefinedMenuItemOptions,
	type SubmenuOptions,
} from "@tauri-apps/api/menu";

import { inApp, onMac } from "./platform";

/** One line of a context menu. */
export type MenuEntry =
	| {
			readonly kind: "item";
			readonly text: string;
			readonly enabled?: boolean;
			readonly action: () => void;
	  }
	/** A line that opens more lines. With none, it shows dimmed. */
	| { readonly kind: "submenu"; readonly text: string; readonly entries: readonly MenuEntry[] }
	/** Copy, which on a Mac is the system's own, for the page's selection. */
	| { readonly kind: "copy"; readonly text: string }
	| { readonly kind: "separator" };

type Options = MenuItemOptions | SubmenuOptions | PredefinedMenuItemOptions;

// Each menu's items get their own IDs, so a pick never reaches an older menu's.
let menus = 0;

const optionsOf = (entry: MenuEntry, index: string): Options => {
	switch (entry.kind) {
		case "item":
			return {
				id: `page-menu-${index}`,
				text: entry.text,
				enabled: entry.enabled ?? true,
				action: entry.action,
			};
		case "submenu":
			return {
				id: `page-menu-${index}`,
				text: entry.text,
				enabled: entry.entries.length > 0,
				items: entry.entries.map((child, at) => optionsOf(child, `${index}-${at}`)),
			};
		case "copy":
			return onMac()
				? { item: "Copy", text: t("Copy") }
				: {
						id: `page-menu-${index}`,
						text: t("Copy"),
						action: () =>
							void navigator.clipboard.writeText(entry.text).catch(console.error),
					};
		case "separator":
			return { item: "Separator" };
	}
};

// The menu on show now. Its items have to live until you pick one, which
// can come after `popup` resolves, so each menu goes when the next opens.
let shown: Menu | null = null;

/**
 * Shows a native context menu at a point in the webview, such as a
 * `contextmenu` event's `clientX` and `clientY`. Returns false outside the
 * app, where nothing shows, so the caller leaves the page's own menu be.
 */
export const contextMenu = (
	entries: readonly MenuEntry[],
	at: { readonly x: number; readonly y: number }
): boolean => {
	if (!inApp()) return false;
	const previous = shown;
	shown = null;
	void previous?.close().catch(console.error);
	const mine = ++menus;
	const items = entries.map((entry, index) => optionsOf(entry, `${mine}-${index}`));
	Menu.new({ items })
		.then(async (menu) => {
			// A newer menu was asked for while this one was being built.
			if (mine !== menus) return menu.close();
			shown = menu;
			await menu.popup(new LogicalPosition(at.x, at.y));
		})
		.catch(console.error);
	return true;
};
