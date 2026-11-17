// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A terminal taken out of its agent's drawer into a pane of its own, with
// a button that puts it back. Closing the pane puts it back too, still
// running.

import { PanelBottom, SquareTerminal } from "lucide-react";

import { useServices, useView } from "@/app/services";
import type { AgentId, TerminalId } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { runningCommand, tabTitle } from "@maxhq/shared/terminal/drawer";
import { terminalById } from "@maxhq/shared/terminal/select";
import { deviceName } from "@maxhq/shared/view/select";

import { popBack } from "./actions";
import { whereLine } from "./drawer";
import { useDrawers } from "./drawer-prefs";
import { TerminalBody } from "./terminal-body";

/** What a terminal's pane tab shows: its title, and whether a command runs in it. */
export const useTerminalTab = (
	terminal: TerminalId
): { readonly title: string | null; readonly running: boolean } => {
	const entity = useView((s) => terminalById(s.view, terminal));
	const named = useDrawers((s) => s.named[terminal]);
	return {
		title: entity ? tabTitle(entity) : null,
		running: entity ? runningCommand(entity, named) !== null : false,
	};
};

/** The icon a terminal's pane tab starts with. */
export const TerminalIcon = () => (
	<SquareTerminal
		aria-hidden
		size={12}
		strokeWidth={2}
		className="shrink-0 text-muted-foreground"
	/>
);

/** What a terminal's pane shows: where it runs, the way back, and the terminal. */
export const TerminalPaneFor = ({ agent, terminal }: { agent: AgentId; terminal: TerminalId }) => {
	useI18n();
	const services = useServices();
	const entity = useView((s) => terminalById(s.view, terminal));
	const name = useView((s) => (entity ? deviceName(s.view, entity.device) : ""));
	if (!entity)
		return (
			<p className="grid h-full place-items-center text-sm text-muted-foreground">
				{translate("This terminal closed.")}
			</p>
		);
	return (
		<div className="flex h-full min-h-0 flex-col bg-(--terminal-background)">
			<div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-card pr-1.5 pl-3">
				<span className="min-w-0 flex-1 truncate font-mono text-[11px] text-subtle-foreground">
					{whereLine(entity.cwd, name)}
				</span>
				<button
					type="button"
					onClick={() => popBack(services, agent, terminal)}
					className="inline-flex h-6 shrink-0 cursor-pointer items-center gap-1.5 rounded-[5px] px-2 text-xs text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
				>
					<PanelBottom aria-hidden size={13} strokeWidth={1.9} />
					{translate("Back to the drawer")}
				</button>
			</div>
			<TerminalBody terminal={entity} className="flex-1" />
		</div>
	);
};
