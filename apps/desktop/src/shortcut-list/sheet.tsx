// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The list of keyboard shortcuts, by group, with the keys that work only in
// projects mode marked. Cmd+/ shows it over the window, and the settings
// show it as a page.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { X } from "lucide-react";
import { type KeyboardEvent } from "react";

import { useServices } from "@/app/services";
import { holdsCommand, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { BACKDROP, POPUP } from "@/workspaces/new-workspace";

import { SHORTCUT_GROUPS, type ShortcutRow, shortcutRows } from "./rows";
import { closeShortcutList, useShortcutList } from "./store";

const ProjectsOnly = () => {
	const { t } = useI18n();
	return (
		<span
			title={t("Works only in projects mode")}
			className="shrink-0 rounded-[4px] border border-input px-1.5 py-px text-[10.5px] font-normal tracking-normal text-muted-foreground normal-case"
		>
			{t("Projects only")}
		</span>
	);
};

const Keys = ({ keys }: { keys: readonly string[] }) => {
	const { t } = useI18n();
	return (
		<span className="flex shrink-0 items-center gap-1.5 text-[11px] text-muted-foreground">
			{keys.map((key, at) => (
				<span key={key} className="flex items-center gap-1.5">
					{at > 0 ? <span>{t("or")}</span> : null}
					<kbd className="min-w-[22px] rounded-[5px] border border-input bg-secondary px-1.5 py-px text-center font-sans text-[11.5px] text-foreground">
						{key}
					</kbd>
				</span>
			))}
		</span>
	);
};

const Group = ({ title, rows }: { title: string; rows: readonly ShortcutRow[] }) => {
	useI18n();

	const projectsOnly = rows.every((row) => row.projectsOnly);
	return (
		<section aria-label={title} className="flex flex-col gap-1">
			<h3 className="flex items-center gap-2 text-[11px] font-[650] tracking-[0.04em] text-muted-foreground uppercase">
				{title}
				{projectsOnly ? <ProjectsOnly /> : null}
			</h3>
			<ul className="flex flex-col">
				{rows.map((row) => (
					<li
						key={`${row.what}:${row.keys.join(" ")}`}
						className="flex min-h-[30px] items-center justify-between gap-4 border-b border-border py-1 last:border-b-0"
					>
						<span className="flex min-w-0 items-center gap-2 text-[12.5px] text-foreground">
							{row.what}
							{row.projectsOnly && !projectsOnly ? <ProjectsOnly /> : null}
						</span>
						<Keys keys={row.keys} />
					</li>
				))}
			</ul>
		</section>
	);
};

/** Every shortcut, by group. A group whose keys all work only in projects mode says so once. */
export const ShortcutList = () => {
	const { t } = useI18n();

	const groups = (() => {
		const rows = shortcutRows();
		return SHORTCUT_GROUPS.flatMap((group) => {
			const inGroup = rows.filter((row) => row.group === group);
			return inGroup.length > 0 ? [[group, inGroup] as const] : [];
		});
	})();
	return (
		<div className="flex flex-col gap-5">
			{groups.map(([group, inGroup]) => (
				<Group key={group} title={t(group)} rows={inGroup} />
			))}
		</div>
	);
};

/** The list over the window, which Cmd+/ opens and closes. */
export const ShortcutSheet = () => {
	const { t } = useI18n();

	const services = useServices();
	const open = useShortcutList((s) => s.open);
	// The window's shortcuts leave a dialog alone, so the sheet closes on its own key.
	const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
		const key = SHORTCUTS.shortcuts;
		if (!holdsCommand(e) || e.shiftKey || e.altKey || e.code !== key.code) return;
		e.preventDefault();
		closeShortcutList(services);
	};
	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next) closeShortcutList(services);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup
					onKeyDown={onKeyDown}
					className={cn(
						POPUP,
						"flex max-h-[76vh] w-[min(560px,calc(100vw-32px))] flex-col gap-4 p-0"
					)}
				>
					<div className="flex items-start justify-between gap-4 px-5 pt-5">
						<div className="flex flex-col gap-1">
							<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
								{t("Keyboard shortcuts")}
							</Dialog.Title>
							<Dialog.Description className="text-[12.5px] text-muted-foreground">
								{t(
									"{{value1}} opens and closes this list. Projects only marks a key that works only in projects mode.",
									{ value1: keyHint(SHORTCUTS.shortcuts) }
								)}
							</Dialog.Description>
						</div>
						<Dialog.Close
							aria-label={t("Close")}
							className="grid size-[28px] shrink-0 place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
						>
							<X aria-hidden size={15} strokeWidth={2} />
						</Dialog.Close>
					</div>
					<div className="min-h-0 overflow-y-auto px-5 pb-5">
						<ShortcutList />
					</div>
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
