// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Run ▾ on a shell code block in an agent's reply. Nothing runs from the
// menu: Run with ! puts the command in the agent's composer in shell mode,
// ready to read and edit, and Paste into terminal types it into the drawer
// without Enter. Only a reply whose agent takes messages has one, and only
// while its device has terminals, so user messages, quotes and notes don't.

import { Menu } from "@base-ui/react/menu";
import { ChevronDown, Play } from "lucide-react";

import { t, useI18n } from "@maxhq/shared/i18n";
import { supportsTerminals } from "@maxhq/shared/terminal/select";
import { commandOfBlock, isShellLanguage, runPlace } from "@maxhq/shared/terminal/shell";

import { useView } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { putCommand } from "@/composer/shell-mode";
import { cn } from "@/lib/utils";
import { useTerminalActions } from "@/terminal";
import { useVisualOrigin } from "@/visuals/origin";

const ENTRY = cn(MENU_ITEM, "justify-between gap-[18px]");

/** Run ▾ for a code block, when its language holds commands. */
export const RunMenu = ({ code, lang }: { code: string; lang: string | null }) =>
	isShellLanguage(lang) ? <ShellRunMenu code={code} lang={lang} /> : null;

const ShellRunMenu = ({ code, lang }: { code: string; lang: string | null }) => {
	useI18n();
	// The reply around the block, when its agent takes messages.
	const agent = useVisualOrigin()?.agent.id ?? null;
	const actions = useTerminalActions();
	const runs = useView((state) => {
		if (agent === null) return false;
		const place = runPlace(state.view, agent);
		return place !== null && supportsTerminals(state.view, place.device);
	});
	if (agent === null || !runs) return null;
	const command = () => commandOfBlock(code, lang);
	return (
		<Menu.Root>
			<Menu.Trigger
				className="inline-flex h-[22px] items-center gap-1 rounded-[5px] bg-accent px-[7px] font-sans text-[11.5px] text-foreground outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-ring"
				aria-label={t("Run this code")}
			>
				<Play aria-hidden className="size-2.5 fill-current" strokeWidth={0} />
				{t("Run")}
				<ChevronDown aria-hidden className="size-3" strokeWidth={2.2} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="end" sideOffset={6} className="z-[130]">
					<Menu.Popup
						className={cn(MENU_POPUP, "w-auto min-w-[250px] font-sans text-[12.5px]")}
					>
						<Menu.Item
							className={ENTRY}
							onClick={() => putCommand(agent, command(), false)}
						>
							<span>{t("Run with !")}</span>
							<span className="text-[11.5px] text-subtle-foreground">
								{t("output goes to the agent")}
							</span>
						</Menu.Item>
						<Menu.Item
							className={ENTRY}
							onClick={() => putCommand(agent, command(), true)}
						>
							{t("Run privately with !!")}
						</Menu.Item>
						<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
						<Menu.Item
							className={ENTRY}
							onClick={() => actions.pasteIntoDrawer(agent, command())}
						>
							{t("Paste into terminal")}
						</Menu.Item>
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};
