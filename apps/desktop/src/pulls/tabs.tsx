// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The viewer's Files, Checks and Conversation tabs, read only. Files come
// a page at a time, each with its diff. Checks group as failing, pending,
// passing and skipped. Replying and line reviews go through GitHub.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";

import { DiffView } from "@/agent-view/diff-view";
import { Markdown } from "@/agent-view/markdown";
import { cn } from "@/lib/utils";
import { SECONDARY } from "@/settings/controls";
import type { PullCheck, PullFile } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { CHECK_GROUPS, CHECK_STATE, checksLabel, FILE_STATUS } from "@maxhq/shared/pulls/look";
import { pullUrl } from "@maxhq/shared/pulls/urls";
import { formatWhen } from "@maxhq/shared/status/format";
import type { ShownPull } from "@maxhq/shared/ui/store";

import { ChecksMark, OutLink, PullSize, UnknownChecksBanner } from "./look";
import { usePullChecks, usePullComments, usePullFiles } from "./queries";

/** A tab's loading line, or its failure in words. */
const Status = ({ error, what }: { error: Error | null; what: string }) => {
	const { t } = useI18n();
	return error ? (
		<p role="alert" className="text-[12.5px] text-status-error-text">
			{t("Couldn't read the {{what}}. {{message}}", { what: what, message: error.message })}
		</p>
	) : (
		<p className="text-[12.5px] text-muted-foreground">
			{t("Reading the {{what}}", { what: what })}
		</p>
	);
};

const Quiet = ({ children }: { children: ReactNode }) => {
	useI18n();
	return <p className="text-[12.5px] text-muted-foreground">{children}</p>;
};

/**
 * GitHub's patch for a file with the file headers a diff view reads. GitHub
 * leaves them out, since the file is named beside the patch.
 */
export const withHeaders = (file: PullFile & { patch: string }): string => {
	const before = file.status === "added" ? "/dev/null" : `a/${file.path}`;
	const after = file.status === "removed" ? "/dev/null" : `b/${file.path}`;
	return `--- ${before}\n+++ ${after}\n${file.patch}`;
};

/** One changed file with its diff, or `noPatch` when there's none to show. */
export const FileRow = ({
	file,
	noPatch = t("GitHub shows no diff for this file."),
}: {
	file: PullFile;
	noPatch?: string;
}) => {
	useI18n();
	return (
		<li className="flex flex-col gap-1.5">
			<div className="flex min-w-0 items-center gap-2 text-[12px]">
				<span className="shrink-0 text-muted-foreground">{FILE_STATUS[file.status]}</span>
				<span
					className="min-w-0 flex-1 truncate font-mono text-foreground"
					title={file.path}
				>
					{file.path}
				</span>
				<PullSize pull={file} />
			</div>
			{file.patch ? (
				<div className="overflow-hidden rounded-[7px] border border-sidebar-border">
					<DiffView patch={withHeaders({ ...file, patch: file.patch })} />
				</div>
			) : (
				<Quiet>{noPatch}</Quiet>
			)}
		</li>
	);
};

/** The PR's changed files, a page at a time. */
export const PullFilesTab = ({ shown }: { shown: ShownPull }) => {
	const { t } = useI18n();

	const files = usePullFiles(shown.device, shown.key);
	const listed = files.data?.pages.flatMap((page) => page.files);
	return (
		<div className="flex flex-col gap-3">
			<OutLink href={`${pullUrl(shown.key)}/files`}>{t("Open on GitHub")}</OutLink>
			{!listed ? (
				<Status error={files.error} what="files" />
			) : listed.length === 0 ? (
				<Quiet>{t("No files changed")}</Quiet>
			) : (
				<ul aria-label={t("Files")} className="flex flex-col gap-4">
					{listed.map((file) => (
						<FileRow key={file.path} file={file} />
					))}
				</ul>
			)}
			{files.hasNextPage ? (
				<div>
					<button
						type="button"
						className={SECONDARY}
						disabled={files.isFetchingNextPage}
						onClick={() => void files.fetchNextPage()}
					>
						{files.isFetchingNextPage ? t("Reading more files") : t("Show more files")}
					</button>
				</div>
			) : null}
			{listed && files.isFetchNextPageError && files.error ? (
				<Status error={files.error} what="next files" />
			) : null}
		</div>
	);
};

const CheckRow = ({ check }: { check: PullCheck }) => {
	const { t } = useI18n();
	return (
		<li className="flex items-center gap-2 py-1 text-[12px]">
			<span className="min-w-0 flex-1 truncate text-foreground">{check.name}</span>
			<span
				className={cn(
					"shrink-0",
					check.state === "failing" ? "text-status-error-text" : "text-muted-foreground"
				)}
			>
				{CHECK_STATE[check.state]}
			</span>
			{check.url ? (
				<OutLink href={check.url} className="shrink-0">
					{t("Details")}
				</OutLink>
			) : null}
		</li>
	);
};

/** The PR's checks on its head, grouped by how each stands. */
export const PullChecksTab = ({ shown }: { shown: ShownPull }) => {
	const { t } = useI18n();

	const checks = usePullChecks(shown.device, shown.key);
	if (!checks.data) return <Status error={checks.error} what="checks" />;
	const { state, checks: all } = checks.data;
	if (state === "unknown")
		return <UnknownChecksBanner repo={shown.key.repo} device={shown.device} />;
	if (all.length === 0) return <Quiet>{t("No checks on this pull request")}</Quiet>;
	return (
		<div className="flex flex-col gap-3">
			<p className="flex items-center gap-1.5 text-[12px] text-secondary-foreground">
				<ChecksMark state={state} />
				{checksLabel(state)}
			</p>
			{CHECK_GROUPS.map(({ state: group, title }) => {
				const inGroup = all.filter((check) => check.state === group);
				if (inGroup.length === 0) return null;
				return (
					<section key={group} aria-label={title} className="flex flex-col">
						<h3 className="text-[11.5px] font-[650] text-secondary-foreground">
							{title} {formatNumber(inGroup.length)}
						</h3>
						<ul>
							{inGroup.map((check) => (
								<CheckRow key={`${check.name}:${check.url ?? ""}`} check={check} />
							))}
						</ul>
					</section>
				);
			})}
		</div>
	);
};

/** The PR's conversation, oldest first, with a link to reply on GitHub. */
export const PullConversationTab = ({ shown }: { shown: ShownPull }) => {
	const { t } = useI18n();

	const comments = usePullComments(shown.device, shown.key);
	const now = useNow(60_000);
	return (
		<div className="flex flex-col gap-3">
			<OutLink href={pullUrl(shown.key)}>{t("Reply on GitHub")}</OutLink>
			{!comments.data ? (
				<Status error={comments.error} what="conversation" />
			) : comments.data.length === 0 ? (
				<Quiet>{t("No comments yet")}</Quiet>
			) : (
				<ul aria-label={t("Comments")} className="flex flex-col gap-3">
					{comments.data.map((comment) => (
						<li
							key={comment.id}
							className="flex flex-col gap-1 rounded-[9px] border border-sidebar-border bg-card px-3 py-2"
						>
							<div className="flex min-w-0 items-baseline gap-2 text-[11.5px]">
								<span className="font-[600] text-foreground">{comment.author}</span>
								<span className="text-muted-foreground">
									{formatWhen(comment.at, now)}
								</span>
								{comment.path ? (
									<span className="min-w-0 truncate font-mono text-muted-foreground">
										{comment.line === null
											? comment.path
											: `${comment.path}:${comment.line}`}
									</span>
								) : null}
							</div>
							<Markdown text={comment.body} />
						</li>
					))}
				</ul>
			)}
		</div>
	);
};
