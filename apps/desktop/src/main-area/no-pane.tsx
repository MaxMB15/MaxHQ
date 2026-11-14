// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the main area shows with no pane open. It lists the agents waiting on
// you, the last few you closed, and the keys that open one, each a button that
// does the same. While this Mac's maxd can't be reached, it says so first.

import { useI18n } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";

import { openAgent, openNextWaiting } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { panesOf } from "@/layout/panes";
import { ariaKeys, keyHint, SHORTCUTS, type Shortcut } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { openNewAgent } from "@/new-agent/store";
import { needsYou } from "@maxhq/shared/view/select";

import { LocalMaxdNotice } from "./local-maxd";

// How many agents each list shows.
const SHOWN = 5;

const ROW =
	"flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[12.5px] hover:bg-accent";

const List = ({ title, children }: { title: string; children: ReactNode }) => {
	useI18n();
	return (
		<section aria-label={title} className="flex flex-col gap-0.5">
			<h3 className="px-2 pb-1 text-[11px] font-[650] tracking-[0.04em] text-muted-foreground uppercase">
				{title}
			</h3>
			{children}
		</section>
	);
};

const Key = ({
	label,
	shortcut,
	disabled,
	run,
}: {
	label: string;
	shortcut: Shortcut;
	disabled?: boolean;
	run: () => void;
}) => {
	useI18n();
	return (
		<button
			type="button"
			disabled={disabled}
			aria-keyshortcuts={ariaKeys(shortcut)}
			onClick={run}
			className={cn(ROW, "disabled:opacity-50 disabled:hover:bg-transparent")}
		>
			<span className="flex-1 text-secondary-foreground">{label}</span>
			<kbd className="font-sans text-[11px] text-muted-foreground">{keyHint(shortcut)}</kbd>
		</button>
	);
};

/** The main area with no pane open. */
export const NoPane = () => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const panes = panesOf(services);
	const waiting = needsYou(view).slice(0, SHOWN);
	const closed = panes
		.recentlyClosed()
		.flatMap((params) => {
			const agent = params.kind === "agent" ? view.tables.agent.get(params.agent) : undefined;
			return agent ? [agent] : [];
		})
		.slice(0, SHOWN);

	return (
		<div className="h-full overflow-y-auto">
			<div className="mx-auto flex w-full max-w-[380px] flex-col gap-5 px-6 py-[12vh]">
				<header className="flex flex-col gap-1 px-2">
					<h2 className="text-[14px] font-semibold text-foreground-strong">
						{t("Pick an agent to continue")}
					</h2>
					<p className="text-[12.5px] text-muted-foreground">
						{t("Open one from the sidebar or from the lists here, or start a new one.")}
					</p>
				</header>
				<LocalMaxdNotice />
				{waiting.length > 0 ? (
					<List title={t("Needs you")}>
						{waiting.map((item) => (
							<button
								key={item.agent.id}
								type="button"
								onClick={() => openAgent(services, item.agent.id)}
								className={ROW}
							>
								<span
									aria-hidden
									className="size-[7px] shrink-0 rounded-full bg-status-ready"
								/>
								<span className="min-w-0 flex-1 truncate text-foreground-strong">
									{item.agent.title}
								</span>
								<span className="shrink-0 text-[11px] text-status-ready-text">
									{item.ask}
								</span>
							</button>
						))}
					</List>
				) : null}
				{closed.length > 0 ? (
					<List title={t("Recently closed")}>
						{closed.map((agent) => (
							<button
								key={agent.id}
								type="button"
								onClick={() => openAgent(services, agent.id)}
								className={ROW}
							>
								<span className="min-w-0 flex-1 truncate text-secondary-foreground">
									{agent.title}
								</span>
							</button>
						))}
					</List>
				) : null}
				<List title={t("Keys")}>
					<Key
						label={t("New agent")}
						shortcut={SHORTCUTS.newAgent}
						run={() => openNewAgent(services)}
					/>
					<Key
						label={t("Next agent waiting")}
						shortcut={SHORTCUTS.nextWaiting}
						disabled={waiting.length === 0}
						run={() => openNextWaiting(services)}
					/>
					<Key
						label={t("Reopen the last closed pane")}
						shortcut={SHORTCUTS.reopen}
						disabled={!panes.canReopen()}
						run={() => panes.reopen()}
					/>
				</List>
			</div>
		</div>
	);
};
