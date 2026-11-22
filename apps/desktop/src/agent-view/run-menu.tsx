// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Run ▾ on a shell code block in an agent's reply, and on a shell command
// the agent ran. Nothing runs from the menu: Run puts the command in the
// agent's composer in shell mode, ready to read and edit, and Paste into
// terminal types it into the terminal panel without Enter. Only an agent
// that takes messages has one, and only while its device has terminals,
// so user messages, quotes and notes don't.

import { Menu } from "@base-ui/react/menu";
import {
	ChevronDown,
	Eye,
	LockKeyhole,
	type LucideIcon,
	PanelBottom,
	Play,
	SquareTerminal,
} from "lucide-react";
import type { ReactNode } from "react";

import type { AgentId } from "@maxhq/shared/bindings";
import { t, useI18n } from "@maxhq/shared/i18n";
import { supportsTerminals } from "@maxhq/shared/terminal/select";
import { commandOfBlock, isShellLanguage, runPlace } from "@maxhq/shared/terminal/shell";

import { useView } from "@/app/services";
// From the file itself, not the composer's index: the index reaches back
// here through the timeline, and ENTRY reads MENU_ITEM at load.
import { MENU_ITEM, MENU_POPUP } from "@/composer/pickers";
import { putCommand } from "@/composer/shell-mode";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { withParts } from "@/lib/parts";
import { cn } from "@/lib/utils";
import { useTerminalActions } from "@/terminal";
import { useVisualOrigin } from "@maxhq/shared/visuals/origin";

const ENTRY = cn(MENU_ITEM, "pr-2.5");
const LABEL = "text-[12.5px] font-[550]";
const DETAIL = "text-[11px] text-muted-foreground";
/** A shortcut at the right of a menu entry. */
export const MENU_KEY = "ml-auto shrink-0 pl-4 font-mono text-[11px] text-subtle-foreground";

/**
 * Whether the agent can run commands from here: it takes messages, and its
 * device has terminals.
 */
const useRuns = (agent: AgentId | null): boolean =>
	useView((state) => {
		if (agent === null) return false;
		if (state.view.tables.agent.get(agent)?.control === "observed") return false;
		const place = runPlace(state.view, agent);
		return place !== null && supportsTerminals(state.view, place.device);
	});

/** One entry: an icon, its words with what it does under them, and its key. */
const Entry = ({
	icon: Icon,
	label,
	detail,
	keys,
	onClick,
}: {
	icon: LucideIcon;
	label: string;
	detail?: ReactNode;
	keys?: string;
	onClick: () => void;
}) => (
	<Menu.Item className={ENTRY} label={label} onClick={onClick}>
		<Icon
			aria-hidden
			size={13}
			strokeWidth={2}
			className="shrink-0 text-secondary-foreground"
		/>
		<span className="flex min-w-0 flex-col gap-px">
			<span className={LABEL}>{label}</span>
			{detail ? <span className={DETAIL}>{detail}</span> : null}
		</span>
		{keys ? (
			<kbd aria-hidden className={MENU_KEY}>
				{keys}
			</kbd>
		) : null}
	</Menu.Item>
);

/** Run ▾ and its menu. */
const RunPopup = ({ label, children }: { label: string; children: ReactNode }) => (
	<Menu.Root>
		<Menu.Trigger
			className="inline-flex h-[22px] shrink-0 items-center gap-1 rounded-[5px] bg-accent px-[7px] font-sans text-[11.5px] text-foreground outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-ring"
			aria-label={label}
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
					{children}
				</Menu.Popup>
			</Menu.Positioner>
		</Menu.Portal>
	</Menu.Root>
);

/** Paste into terminal, which types the command into the panel without Enter. */
const PasteEntry = ({ agent, command }: { agent: AgentId; command: () => string }) => {
	const actions = useTerminalActions();
	return (
		<Entry
			icon={PanelBottom}
			label={t("Paste into terminal")}
			keys={keyHint(SHORTCUTS.terminal)}
			onClick={() => actions.pasteIntoDrawer(agent, command())}
		/>
	);
};

/** Run ▾ for a code block, when its language holds commands. */
export const RunMenu = ({ code, lang }: { code: string; lang: string | null }) =>
	isShellLanguage(lang) ? <ShellRunMenu code={code} lang={lang} /> : null;

const ShellRunMenu = ({ code, lang }: { code: string; lang: string | null }) => {
	useI18n();
	// The reply around the block, when its agent takes messages.
	const agent = useVisualOrigin()?.agent.id ?? null;
	const runs = useRuns(agent);
	if (agent === null || !runs) return null;
	const command = () => commandOfBlock(code, lang);
	return (
		<RunPopup label={t("Run this code")}>
			<Entry
				icon={Eye}
				label={t("Run")}
				detail={t("The agent sees the output")}
				keys="!"
				onClick={() => putCommand(agent, command(), false)}
			/>
			<Entry
				icon={LockKeyhole}
				label={t("Run privately")}
				detail={t("The agent doesn't see it")}
				keys="!!"
				onClick={() => putCommand(agent, command(), true)}
			/>
			<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
			<PasteEntry agent={agent} command={command} />
		</RunPopup>
	);
};

/**
 * Run ▾ for a shell command the agent ran, so you can run it yourself.
 * `agent` is null for an agent that doesn't take messages.
 */
export const CommandRunMenu = ({ agent, command }: { agent: AgentId | null; command: string }) => {
	useI18n();
	const runs = useRuns(agent);
	if (agent === null || !runs || command.trim() === "") return null;
	return (
		<RunPopup label={t("Run this command")}>
			<Entry
				icon={SquareTerminal}
				label={t("Run it yourself")}
				detail={withParts("In the composer with {{bang}}, ready to edit", {
					bang: <span className="font-mono">!</span>,
				})}
				onClick={() => putCommand(agent, command, false)}
			/>
			<PasteEntry agent={agent} command={() => command} />
		</RunPopup>
	);
};
