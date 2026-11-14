// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR viewer, a panel on the right of the main area, laid out as a
// detail panel: the PR's number, title, state, author and branches, a link to
// it on GitHub, and four read-only tabs. The close button and Escape close it.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Tabs } from "@base-ui/react/tabs";
import { X } from "lucide-react";
import { useEffect } from "react";

import { useServices } from "@/app/services";
import type { ShownPull } from "@maxhq/shared/ui/store";
import { pullUrl } from "@maxhq/shared/pulls/urls";

import { PullStateBadge, OutLink } from "./look";
import { usePull } from "./queries";
import { PullSummaryTab } from "./summary";
import { PullChecksTab, PullConversationTab, PullFilesTab } from "./tabs";

const TAB =
	"h-[26px] rounded-[6px] px-2 text-[12px] text-secondary-foreground hover:bg-secondary data-active:bg-selected data-active:text-foreground";

/** The PR `shown` names, beside the main area. */
export const PullViewer = ({ shown }: { shown: ShownPull }) => {
	const { t } = useI18n();

	const services = useServices();
	const { key, device } = shown;
	const pr = usePull(device, key);
	const pull = pr.data?.summary.pull;

	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape" || e.defaultPrevented) return;
			services.ui.getState().closePull();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [services]);

	return (
		<aside
			aria-label={`Pull request #${key.number}`}
			className="flex w-[min(520px,46%)] min-w-[340px] shrink-0 animate-slide-in flex-col border-l border-sidebar-border bg-subpanel"
		>
			<header className="flex flex-col gap-1.5 border-b border-sidebar-border px-4 pt-3 pb-2.5">
				<div className="flex items-start gap-2">
					<h2 className="min-w-0 flex-1 text-[14px] font-[650] text-foreground-strong">
						<span className="mr-1.5 font-mono font-normal text-muted-foreground">
							#{formatNumber(key.number, { useGrouping: false })}
						</span>
						{pull?.title ?? ""}
					</h2>
					<button
						type="button"
						aria-label={t("Close the pull request")}
						onClick={() => services.ui.getState().closePull()}
						className="grid size-[22px] shrink-0 place-items-center rounded-[5px] text-muted-foreground hover:bg-accent hover:text-foreground"
					>
						<X aria-hidden size={13} strokeWidth={2.4} />
					</button>
				</div>
				<div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-muted-foreground">
					{pull ? (
						<>
							<PullStateBadge pull={pull} />
							<span>{pull.author}</span>
							<span className="min-w-0 truncate font-mono">
								{t("{{head}} into {{base}}", { head: pull.head, base: pull.base })}
							</span>
						</>
					) : null}
					<OutLink href={pullUrl(key)} className="ml-auto">
						{t("Open on GitHub")}
					</OutLink>
				</div>
			</header>
			<Tabs.Root defaultValue="summary" className="flex min-h-0 flex-1 flex-col">
				<Tabs.List className="flex gap-1 border-b border-sidebar-border px-3 py-1.5">
					<Tabs.Tab value="summary" className={TAB}>
						{t("Summary")}
					</Tabs.Tab>
					<Tabs.Tab value="files" className={TAB}>
						{t("Files")}
					</Tabs.Tab>
					<Tabs.Tab value="checks" className={TAB}>
						{t("Checks")}
					</Tabs.Tab>
					<Tabs.Tab value="conversation" className={TAB}>
						{t("Conversation")}
					</Tabs.Tab>
				</Tabs.List>
				<div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
					<Tabs.Panel value="summary">
						{pr.error ? (
							<p role="alert" className="text-[12.5px] text-status-error-text">
								{t("Couldn't read the pull request. {{message}}", {
									message: pr.error.message,
								})}
							</p>
						) : pr.data ? (
							<PullSummaryTab key={`${key.repo}#${key.number}`} pr={pr.data} />
						) : (
							<p className="text-[12.5px] text-muted-foreground">
								{t("Reading the pull request")}
							</p>
						)}
					</Tabs.Panel>
					<Tabs.Panel value="files">
						<PullFilesTab shown={shown} />
					</Tabs.Panel>
					<Tabs.Panel value="checks">
						<PullChecksTab shown={shown} />
					</Tabs.Panel>
					<Tabs.Panel value="conversation">
						<PullConversationTab shown={shown} />
					</Tabs.Panel>
				</div>
			</Tabs.Root>
		</aside>
	);
};
