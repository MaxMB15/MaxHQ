// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a visual's card offers, in one list: the card's More menu shows it,
// and so does the menu a right click opens. Copy source puts the page on
// the clipboard as it is, Save as .html writes it to a file you pick,
// Reload runs it again in a new frame, and Open in a tab shows it in a pane
// of its own. Add to chat puts it in a composer as a pill, and an action
// with entries opens them as a submenu, as Add to note does with the
// project's notes.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import {
	AppWindow,
	ChevronRight,
	Code,
	Ellipsis,
	FileDown,
	type LucideIcon,
	RotateCw,
} from "lucide-react";
import { Fragment, type MouseEvent } from "react";

import { type MenuEntry, useComposerNative } from "@/composer/native";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { cn } from "@/lib/utils";
import { saveExport } from "@/native/files";
import { inApp } from "@/native/platform";
import type { Visual } from "@maxhq/shared/visuals/fence";
import { visualFileName } from "@maxhq/shared/visuals/visual";

/** One thing a visual's card can do. */
export type VisualAction = {
	readonly label: string;
	readonly icon: LucideIcon;
	readonly run: () => void;
	/** Set, the action opens these as a submenu, read as it opens, and `run` goes unused. */
	readonly entries?: () => readonly VisualAction[];
	/** Set, a line across goes before it. */
	readonly separated?: boolean;
};

/** Asks where to save a visual's page, and saves it there. */
export const saveVisual = ({ html, title }: Visual): void =>
	void saveExport(html, visualFileName(title), null).catch(() => {});

/** What a card can do with its visual, as this build can. */
export const visualActions = (
	{ html, title }: Visual,
	{
		reload,
		open,
		inApp: app = inApp(),
		carry = [],
	}: {
		reload: () => void;
		/** Opens the visual in a tab, where the card offers that. */
		open?: () => void;
		inApp?: boolean;
		/** Where the visual can go, such as Add to chat and Add to note, after Copy source. */
		carry?: readonly VisualAction[];
	}
): VisualAction[] => [
	...(open
		? [
				{
					get label() {
						return t("Open in a tab");
					},
					icon: AppWindow,
					run: open,
				},
			]
		: []),
	{
		get label() {
			return t("Copy source");
		},
		icon: Code,
		run: () => void navigator.clipboard.writeText(html).catch(() => {}),
	},
	...carry,
	...(app
		? [
				{
					get label() {
						return t("Save as .html…");
					},
					icon: FileDown,
					run: () => saveVisual({ html, title }),
				},
			]
		: []),
	{
		get label() {
			return t("Reload");
		},
		icon: RotateCw,
		run: reload,
	},
];

const ActionIcon = ({ icon: Icon }: { icon: LucideIcon }) => (
	<Icon aria-hidden size={13} strokeWidth={2} className="shrink-0 text-secondary-foreground" />
);

/**
 * The actions as menu lines, an action with entries as a submenu, and an
 * empty submenu as a dimmed line saying so.
 */
const MenuItems = ({ actions }: { actions: readonly VisualAction[] }) => {
	const { t } = useI18n();
	if (actions.length === 0)
		return (
			<Menu.Item disabled className={MENU_ITEM}>
				<span className="text-[12.5px] text-muted-foreground">{t("Nothing here")}</span>
			</Menu.Item>
		);
	// Two notes can share a title, so the key holds the place too.
	return actions.map(({ label, icon, run, entries, separated }, at) => (
		<Fragment key={`${at}:${label}`}>
			{separated ? <Menu.Separator className="my-1 h-px bg-border" /> : null}
			{entries ? (
				<Menu.SubmenuRoot>
					<Menu.SubmenuTrigger className={MENU_ITEM}>
						<ActionIcon icon={icon} />
						<span className="flex-1 text-[12.5px] font-[550]">{label}</span>
						<ChevronRight
							aria-hidden
							size={12}
							strokeWidth={2.4}
							className="shrink-0"
						/>
					</Menu.SubmenuTrigger>
					<Menu.Portal>
						<Menu.Positioner
							side="right"
							align="start"
							sideOffset={4}
							className="z-[130]"
						>
							<Menu.Popup
								className={cn(
									MENU_POPUP,
									"max-h-[320px] max-w-[260px] min-w-[180px] overflow-y-auto"
								)}
							>
								<MenuItems actions={entries()} />
							</Menu.Popup>
						</Menu.Positioner>
					</Menu.Portal>
				</Menu.SubmenuRoot>
			) : (
				<Menu.Item onClick={run} className={MENU_ITEM}>
					<ActionIcon icon={icon} />
					<span className="min-w-0 truncate text-[12.5px] font-[550]">{label}</span>
				</Menu.Item>
			)}
		</Fragment>
	));
};

/** The card's More menu. */
export const VisualMenu = ({
	actions,
	className,
}: {
	actions: readonly VisualAction[];
	className?: string;
}) => {
	const { t } = useI18n();
	return (
		<Menu.Root>
			<Menu.Trigger
				aria-label={t("More")}
				className={cn(
					"grid size-[22px] shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-accent data-popup-open:text-foreground",
					className
				)}
			>
				<Ellipsis aria-hidden size={14} strokeWidth={2.2} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="end" sideOffset={6} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[200px]")}>
						<MenuItems actions={actions} />
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};

/** The actions as the system menu's entries; an empty submenu shows dimmed. */
export const menuEntries = (actions: readonly VisualAction[]): MenuEntry[] =>
	actions.flatMap(({ label, run, entries, separated }): MenuEntry[] => [
		...(separated ? [{ kind: "separator" } as const] : []),
		entries
			? { kind: "submenu", text: label, entries: menuEntries(entries()) }
			: { kind: "item", text: label, action: run },
	]);

/** Opens the same list as the system's menu on a right click, where there is one. */
export const useVisualContextMenu = (actions: readonly VisualAction[]) => {
	const native = useComposerNative();
	return (event: MouseEvent) => {
		if (native.contextMenu?.(menuEntries(actions), { x: event.clientX, y: event.clientY }))
			event.preventDefault();
	};
};
