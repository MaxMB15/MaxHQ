// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The `@`, `#` and `$` menus above the text box, or below it in a field
// at the top of the window. What each offers, and the lookups behind it,
// are the shared ones in @maxhq/shared/composer/menu. This draws them as a
// list box the text box points into.

import {
	FINDS_EVERYTHING,
	groupName,
	type MenuContents,
	type MenuFinds,
	menuGroups,
	menuLabel,
	type MenuOption,
	menuStatus,
	optionText,
} from "@maxhq/shared/composer/menu";
import type { Trigger } from "@maxhq/shared/pills/trigger";
import { File, Folder, Sparkles, StickyNote, Type } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";

import { AgentIcon } from "@/agent-view/icons";
import { cn } from "@/lib/utils";
import { GitIcon } from "@/pills/pill";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

export type { MenuOption } from "@maxhq/shared/composer/menu";
export { useMenuContents } from "@maxhq/shared/composer/menu";

export const optionId = (menu: string, index: number) =>
	translate("{{menu}}-option-{{index}}", { menu: menu, index: index });

const ROW =
	"flex h-7 w-full cursor-default items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px] text-foreground";

const ICON = "size-3.5 shrink-0 text-muted-foreground";

const optionIcon = (option: MenuOption): ReactNode => {
	switch (option.kind) {
		case "path":
			return option.folder ? (
				<Folder aria-hidden className={ICON} />
			) : (
				<File aria-hidden className={ICON} />
			);
		case "agent":
			return <AgentIcon agent={option.agent} className={ICON} />;
		case "note":
			return <StickyNote aria-hidden className={ICON} />;
		case "git":
			return (
				<GitIcon
					kind={option.item.kind}
					state={option.item.state}
					aria-hidden
					className={ICON}
				/>
			);
		case "keep":
			return <Type aria-hidden className={ICON} />;
		case "skill":
			return <Sparkles aria-hidden className={ICON} />;
	}
};

/**
 * The open menu, as a list box the text box points into. It opens above
 * the box unless `below` is set, and `finds` says what its `@` menu finds.
 */
export const ComposerMenu = ({
	id,
	kind,
	contents,
	active,
	onActive,
	onPick,
	below = false,
	finds = FINDS_EVERYTHING,
}: {
	id: string;
	kind: Trigger["kind"];
	contents: MenuContents;
	active: number;
	onActive: (index: number) => void;
	onPick: (option: MenuOption) => void;
	below?: boolean;
	finds?: MenuFinds;
}) => {
	useI18n();
	const { options } = contents;
	const list = useRef<HTMLDivElement>(null);
	useEffect(() => {
		list.current?.ownerDocument
			.getElementById(optionId(id, active))
			?.scrollIntoView({ block: "nearest" });
	}, [active, id]);

	const groups = menuGroups(options);
	const status = menuStatus(kind, contents, finds);
	const label = menuLabel(kind, finds);

	return (
		<div
			ref={list}
			id={id}
			role="listbox"
			aria-label={label}
			className={cn(
				"absolute right-0 left-0 z-20 max-h-64 overflow-y-auto rounded-lg border border-input bg-popover p-1 shadow-lg",
				below ? "top-full mt-1.5" : "bottom-full mb-1.5"
			)}
			onMouseDown={(event) => event.preventDefault()}
		>
			{status !== null && (
				<div role="status" className="px-2 py-1.5 text-[12px] text-muted-foreground">
					{status}
				</div>
			)}
			{groups.map((group) => (
				<div key={group.kind} role="group" aria-labelledby={`${id}-${group.kind}`}>
					<div
						id={`${id}-${group.kind}`}
						className="px-2 pt-1.5 pb-1 text-[11px] font-medium text-muted-foreground"
					>
						{groupName(group.kind)}
					</div>
					{group.options.map((option, offset) => {
						const index = group.start + offset;
						const look = optionText(option);
						return (
							<div
								key={index}
								id={optionId(id, index)}
								role="option"
								aria-selected={index === active}
								className={cn(ROW, index === active && "bg-accent")}
								onMouseMove={() => {
									if (index !== active) onActive(index);
								}}
								onClick={() => onPick(option)}
							>
								{optionIcon(option)}
								<span className="min-w-0 flex-1 truncate">{look.label}</span>
								{look.detail && (
									<span className="max-w-[55%] shrink-0 truncate text-[11.5px] text-muted-foreground">
										{look.detail}
									</span>
								)}
							</div>
						);
					})}
				</div>
			))}
		</div>
	);
};
