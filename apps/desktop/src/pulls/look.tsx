// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a PR looks wherever it shows: its state as a badge, its checks and
// review as marks with their words, its size, a link out to GitHub, and
// the banner for checks the GitHub App can't read yet.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import {
	CircleCheck,
	CircleDashed,
	CircleDot,
	CircleQuestionMark,
	CircleX,
	ExternalLink,
	type LucideIcon,
	TriangleAlert,
	UserCheck,
	UserRound,
	UserRoundX,
} from "lucide-react";
import { type ReactNode, useMemo } from "react";

import { useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { Badge } from "@/tasks/activity";
import type { ChecksState, DeviceId, PullSnapshot, ReviewState } from "@maxhq/shared/bindings";
import { appInstallUrl, githubAppOn } from "@maxhq/shared/pulls/app";
import { checksLabel, PULL_STATE, pullStateKey, reviewLabel } from "@maxhq/shared/pulls/look";

/** A link that opens in the browser, as the Markdown renderer's do. */
export const OutLink = ({
	href,
	children,
	className,
}: {
	href: string;
	children: ReactNode;
	className?: string;
}) => {
	useI18n();
	return (
		<a
			href={href}
			target="_blank"
			rel="noreferrer"
			className={cn(
				"inline-flex items-center gap-1 text-[12px] text-primary hover:underline",
				className
			)}
		>
			{children}
			<ExternalLink aria-hidden size={11} strokeWidth={2.2} />
		</a>
	);
};

/** Open, draft, merged or closed. */
export const PullStateBadge = ({ pull }: { pull: PullSnapshot }) => {
	useI18n();

	const look = PULL_STATE[pullStateKey(pull)];
	return <Badge tone={look.tone}>{look.label}</Badge>;
};

type Mark = { icon: LucideIcon; color: string };

const CHECKS: Record<ChecksState, Mark> = {
	passing: { icon: CircleCheck, color: "text-status-ready-text" },
	failing: { icon: CircleX, color: "text-status-error-text" },
	pending: { icon: CircleDot, color: "text-status-busy-text" },
	none: { icon: CircleDashed, color: "text-subtle-foreground" },
	unknown: { icon: CircleQuestionMark, color: "text-muted-foreground" },
};

const REVIEW: Record<ReviewState, Mark> = {
	approved: { icon: UserCheck, color: "text-status-ready-text" },
	changes_requested: { icon: UserRoundX, color: "text-status-busy-text" },
	none: { icon: UserRound, color: "text-subtle-foreground" },
};

const MarkIcon = ({ mark, label }: { mark: Mark; label: string }) => {
	useI18n();
	return (
		<span title={label} className="inline-flex shrink-0">
			<mark.icon
				role="img"
				aria-label={label}
				size={14}
				strokeWidth={2.1}
				className={mark.color}
			/>
		</span>
	);
};

/** How the PR's checks stand, as an icon. */
export const ChecksMark = ({ state }: { state: ChecksState }) => {
	useI18n();
	return <MarkIcon mark={CHECKS[state]} label={checksLabel(state)} />;
};

/** Where the PR's reviews leave it, as an icon. */
export const ReviewMark = ({ state }: { state: ReviewState }) => {
	useI18n();
	return <MarkIcon mark={REVIEW[state]} label={reviewLabel(state)} />;
};

/** Lines added and deleted, or nothing while they aren't measured yet. */
export const PullSize = ({ pull }: { pull: Pick<PullSnapshot, "additions" | "deletions"> }) => {
	useI18n();
	return pull.additions + pull.deletions > 0 ? (
		<span className="inline-flex shrink-0 gap-1 font-mono text-[11px] tabular-nums">
			<span className="text-status-ready-text">+{formatNumber(pull.additions)}</span>
			<span className="text-status-error-text">-{formatNumber(pull.deletions)}</span>
		</span>
	) : null;
};

/**
 * Says the GitHub App on `device` needs its new permissions accepted on
 * the repository's owner before checks show, with a link to accept them
 * once GitHub has said the App's slug.
 */
export const UnknownChecksBanner = ({
	repo,
	device,
}: {
	repo: string;
	device: DeviceId | undefined;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const url = useMemo(
		() => appInstallUrl(device ? githubAppOn(view, device) : undefined),
		[view, device]
	);
	const owner = repo.split("/")[0] ?? repo;
	return (
		<div
			role="note"
			className="flex items-start gap-2 rounded-[9px] border border-status-busy/30 bg-status-busy/8 px-3 py-2 text-[12px] text-foreground"
		>
			<TriangleAlert
				aria-hidden
				size={14}
				strokeWidth={2.1}
				className="mt-px shrink-0 text-status-busy-text"
			/>
			<span className="min-w-0 flex-1">
				{t(
					"Accept the new permissions for your GitHub App on {{owner}} to see checks here",
					{ owner: owner }
				)}
			</span>
			{url ? <OutLink href={url}>{t("Accept")}</OutLink> : null}
		</div>
	);
};
