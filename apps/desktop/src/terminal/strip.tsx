// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A slim line under the composer while the agent's terminal panel is hidden
// and one of its tabs still runs something, such as a dev server, so that
// doesn't go on out of sight. It names the command and how many tabs there
// are, and a click opens the panel on that tab. With nothing running
// there's no line.

import { t, useI18n } from "@maxhq/shared/i18n";
import type { AgentId, DeviceId } from "@maxhq/shared/bindings";
import { plural } from "@maxhq/shared/status/format";
import { runningCommand, runsCommand, tabTitle } from "@maxhq/shared/terminal/drawer";
import { supportsTerminals, terminalsOf } from "@maxhq/shared/terminal/select";

import { useServices, useView } from "@/app/services";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";

import { showInDrawer } from "./actions";
import { drawerOf, useDrawers } from "./drawer-prefs";

/** The line under the composer while a hidden tab runs something. */
export const TerminalStrip = ({ agent, device }: { agent: AgentId; device: DeviceId }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const open = useDrawers((s) => drawerOf(s, agent).open);
	if (open || !supportsTerminals(view, device)) return null;
	const tabs = terminalsOf(view, agent);
	const busy = tabs.find(runsCommand);
	if (!busy) return null;
	const command = runningCommand(busy) ?? tabTitle(busy);
	return (
		<div className="shrink-0 px-7 pb-2">
			<button
				type="button"
				data-terminal-strip=""
				onClick={() => showInDrawer(services, agent, busy.id)}
				className="mx-auto flex w-full max-w-[744px] min-w-0 cursor-pointer items-center gap-2 rounded-lg border border-border bg-subpanel px-3 py-1 text-left text-[12px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
			>
				<span className="shrink-0 font-[550] text-foreground">{t("Terminal")}</span>
				<span aria-hidden className="shrink-0 text-subtle-foreground">
					·
				</span>
				<span aria-hidden className="size-1.5 shrink-0 rounded-full bg-status-ready" />
				<span className="min-w-0 truncate">{t("{{command}} running", { command })}</span>
				<span aria-hidden className="shrink-0 text-subtle-foreground">
					·
				</span>
				<span className="shrink-0">{plural(tabs.length, "tab")}</span>
				<kbd
					aria-hidden
					className="ml-auto shrink-0 rounded-[4px] border border-border px-1 font-mono text-[10.5px] text-subtle-foreground"
				>
					{keyHint(SHORTCUTS.terminal)}
				</kbd>
			</button>
		</div>
	);
};
