// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's "Pull requests" entry in the sidebar, with how many of its
// open PRs are ready to merge. It shows only when the project has a PR,
// open, merged or closed, on a device whose GitHub App reaches its
// repository. It stays while its page is open, and after a failed read,
// since the page is where that read's reason shows. The project's menu
// opens the page when the entry isn't there.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { GitPullRequest } from "lucide-react";
import { useMemo } from "react";

import { useUi, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { usePulls } from "@/pulls/queries";
import type { Project, Timestamp } from "@maxhq/shared/bindings";
import { appReaches } from "@maxhq/shared/pulls/app";
import { readyCount } from "@maxhq/shared/pulls/list";

/**
 * How often the entry reads the lists again, so it comes and goes on its own.
 * The device asks GitHub only for a change since its last read.
 */
const READ_EVERY_MS = 60_000;

/** The project's Pulls entry, or nothing when no App reaches it or it has no PRs. */
export const PullsEntry = ({ project, now }: { project: Project; now: Timestamp }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const reaches = useMemo(() => appReaches(view, project, now), [view, project, now]);
	const list = usePulls(reaches ? project.id : undefined, "open", READ_EVERY_MS);
	// Merged and closed PRs only matter once none is open, so most projects
	// read one list.
	const noneOpen = list.data?.length === 0;
	const closed = usePulls(reaches && noneOpen ? project.id : undefined, "closed", READ_EVERY_MS);
	const openPulls = useUi((s) => s.openPulls);
	const showing = useUi((s) => s.mode === "projects" && s.pullsOf === project.id);
	const ready = useMemo(() => (list.data ? readyCount(list.data) : 0), [list.data]);
	const anyOpen = (list.data?.length ?? 0) > 0;
	const anyClosed = noneOpen && (closed.data?.length ?? 0) > 0;
	const failed = list.isError || (noneOpen && closed.isError);
	if (!reaches || !(anyOpen || anyClosed || showing || failed)) return null;
	return (
		<button
			type="button"
			aria-current={showing ? "page" : undefined}
			title={
				ready > 0
					? t("{{countText}} pull requests ready to merge", {
							count: ready,
							countText: formatNumber(ready),
						})
					: undefined
			}
			onClick={() => openPulls(project.id)}
			className={cn(
				"flex h-[26px] shrink-0 items-center gap-2 rounded-md pr-2 pl-[22px] text-left text-[12.5px] text-secondary-foreground hover:bg-accent hover:text-foreground",
				showing && "bg-selected text-foreground"
			)}
		>
			<GitPullRequest aria-hidden size={13} strokeWidth={2} className="shrink-0" />
			<span className="min-w-0 flex-1 truncate">{t("Pull requests")}</span>
			{ready > 0 ? (
				<span
					aria-label={t("{{ready}} ready to merge", { ready: formatNumber(ready) })}
					className="shrink-0 rounded-[5px] bg-status-ready/15 px-1.5 text-[11px] font-[600] text-status-ready-text tabular-nums"
				>
					{formatNumber(ready)}
				</span>
			) : null}
		</button>
	);
};
