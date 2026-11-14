// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The viewer's Summary tab: the PR's description, each reviewer's latest
// verdict, whether it merges cleanly, and Merge and Approve as you. Each
// action is disabled with its reason on a line under the buttons. With no
// sign-in, or with the device holding it offline, the two become one Open on
// GitHub button.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown } from "lucide-react";
import { useState } from "react";

import { useView } from "@/app/services";
import { Markdown } from "@/agent-view/markdown";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { cn } from "@/lib/utils";
import { PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import type { Note } from "@/settings/result";
import type { MergeMethod, PullDetail, PullReview, ReviewVerdict } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { approveUnavailableReason, type PullActed } from "@maxhq/shared/pulls/act";
import { githubSignIn } from "@maxhq/shared/pulls/devices";
import {
	allowedMergeMethods,
	MERGE_CONFLICT_REASON,
	MERGE_METHODS,
	mergeUnavailableReason,
} from "@maxhq/shared/pulls/merge";
import { pullUrl } from "@maxhq/shared/pulls/urls";

import { useApprovePull, useMergePull } from "./queries";

const VERDICT: Record<ReviewVerdict, string> = {
	get approved() {
		return t("Approved");
	},
	get changes_requested() {
		return t("Asked for changes");
	},
	get commented() {
		return t("Commented");
	},
	get dismissed() {
		return t("Review dismissed");
	},
};

/**
 * Each reviewer's latest verdict, in the order they first reviewed. A
 * comment after a verdict leaves the verdict standing.
 */
export const latestVerdicts = (reviews: readonly PullReview[]): PullReview[] => {
	const latest = new Map<string, PullReview>();
	for (const review of reviews) {
		const who = review.author.toLowerCase();
		const before = latest.get(who);
		if (review.verdict === "commented" && before && before.verdict !== "commented") continue;
		latest.set(who, review);
	}
	return [...latest.values()];
};

const Heading = ({ children }: { children: string }) => {
	useI18n();
	return <h3 className="text-[11.5px] font-[650] text-secondary-foreground">{children}</h3>;
};

const Reviews = ({ reviews }: { reviews: readonly PullReview[] }) => {
	const { t } = useI18n();

	const shown = latestVerdicts(reviews);
	if (shown.length === 0)
		return <p className="text-[12px] text-muted-foreground">{t("No reviews yet")}</p>;
	return (
		<ul aria-label={t("Reviews")} className="flex flex-col gap-1">
			{shown.map((review) => (
				<li key={review.author} className="flex items-center gap-2 text-[12px]">
					<span className="text-foreground">{review.author}</span>
					<span
						className={cn(
							review.verdict === "approved" && "text-status-ready-text",
							review.verdict === "changes_requested" && "text-status-busy-text",
							(review.verdict === "commented" || review.verdict === "dismissed") &&
								"text-muted-foreground"
						)}
					>
						{VERDICT[review.verdict]}
					</span>
				</li>
			))}
		</ul>
	);
};

/** Whether the PR merges cleanly, in words. */
export const mergeability = (pr: PullDetail): string | null => {
	const pull = pr.summary.pull;
	if (pull.state !== "open") return null;
	if (pull.mergeable === "conflicting") return MERGE_CONFLICT_REASON;
	if (pull.mergeable === "unknown") return t("GitHub is still checking for conflicts.");
	return t("No conflicts with {{base}}.", { base: pull.base });
};

const ACTED: Record<"merge" | "approve", Record<"done" | "queued", string>> = {
	merge: {
		get done() {
			return t("Merged.");
		},
		get queued() {
			return t("Queued to merge.");
		},
	},
	approve: {
		get done() {
			return t("Approved.");
		},
		get queued() {
			return t("Approved.");
		},
	},
};

/** What a merge or an approval came to, as a note. */
const actedNote = (act: "merge" | "approve", acted: PullActed): Note =>
	acted.state === "refused"
		? { text: acted.reason, error: true }
		: { text: ACTED[act][acted.state], error: false };

const Actions = ({ pr }: { pr: PullDetail }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(5000);
	const signIn = githubSignIn(view, now);
	const merge = useMergePull();
	const approve = useApprovePull();
	const [picked, setPicked] = useState<MergeMethod | null>(null);
	const [note, setNote] = useState<Note | null>(null);

	const methods = allowedMergeMethods(pr);
	const method = picked && methods.includes(picked) ? picked : methods[0];
	const mergeWhy = mergeUnavailableReason(pr, signIn);
	const approveWhy = approveUnavailableReason(pr, signIn);
	const busy = merge.isPending || approve.isPending;
	const at = { key: pr.summary.key, headSha: pr.summary.pull.head_sha };

	const run = (act: "merge" | "approve", acting: Promise<PullActed>) => {
		setNote(null);
		acting.then(
			(acted) => setNote(actedNote(act, acted)),
			(error: unknown) =>
				setNote({
					text: error instanceof Error ? error.message : String(error),
					error: true,
				})
		);
	};

	if (signIn.state !== "ready") {
		return (
			<div className="flex flex-col gap-1.5">
				<div>
					<a
						href={pullUrl(pr.summary.key)}
						target="_blank"
						rel="noreferrer"
						className={SECONDARY}
					>
						{t("Open on GitHub")}
					</a>
				</div>
				{mergeWhy ? (
					<p className="text-[11.5px] text-muted-foreground">{mergeWhy}</p>
				) : null}
			</div>
		);
	}

	const reasons = [...new Set([mergeWhy, approveWhy].filter((why) => why !== null))];
	return (
		<div className="flex flex-col gap-1.5">
			<div className="flex flex-wrap items-center gap-2">
				<span className="inline-flex">
					<button
						type="button"
						className={cn(PRIMARY, "rounded-r-none")}
						disabled={mergeWhy !== null || busy || !method}
						onClick={() => {
							if (!method) return;
							run("merge", merge.mutateAsync({ pull: at, method }));
						}}
					>
						{merge.isPending
							? t("Merging")
							: method
								? MERGE_METHODS[method].title
								: t("Merge")}
					</button>
					<Menu.Root>
						<Menu.Trigger
							aria-label={t("Merge method")}
							disabled={mergeWhy !== null || busy}
							className={cn(
								PRIMARY,
								"rounded-l-none border-l border-primary-foreground/20 px-2"
							)}
						>
							<ChevronDown aria-hidden size={13} />
						</Menu.Trigger>
						<Menu.Portal>
							<Menu.Positioner
								side="bottom"
								align="start"
								sideOffset={4}
								className="z-[130]"
							>
								<Menu.Popup className={MENU_POPUP}>
									<Menu.RadioGroup
										value={method}
										onValueChange={(value: MergeMethod) => setPicked(value)}
									>
										{methods.map((one) => (
											<Menu.RadioItem
												key={one}
												value={one}
												className={MENU_ITEM}
											>
												<span className="flex min-w-0 flex-1 flex-col">
													<span className="text-[12.5px] font-[550]">
														{MERGE_METHODS[one].title}
													</span>
													<span className="text-[11px] text-muted-foreground">
														{MERGE_METHODS[one].description}
													</span>
												</span>
												<span className="flex size-3.5 items-center justify-center">
													<Menu.RadioItemIndicator>
														<Check
															aria-hidden
															className="size-3.5 text-primary"
															strokeWidth={2.6}
														/>
													</Menu.RadioItemIndicator>
												</span>
											</Menu.RadioItem>
										))}
									</Menu.RadioGroup>
								</Menu.Popup>
							</Menu.Positioner>
						</Menu.Portal>
					</Menu.Root>
				</span>
				<button
					type="button"
					className={SECONDARY}
					disabled={approveWhy !== null || busy}
					onClick={() => run("approve", approve.mutateAsync({ pull: at }))}
				>
					{approve.isPending ? t("Approving") : t("Approve")}
				</button>
			</div>
			{reasons.map((why) => (
				<p key={why} className="text-[11.5px] text-muted-foreground">
					{why}
				</p>
			))}
			<StatusNote note={note} />
		</div>
	);
};

/** The Summary tab of `pr`. */
export const PullSummaryTab = ({ pr }: { pr: PullDetail }) => {
	const { t } = useI18n();

	const mergeWords = mergeability(pr);
	return (
		<div className="flex flex-col gap-4">
			{pr.body?.trim() ? (
				<Markdown text={pr.body} />
			) : (
				<p className="text-[12.5px] text-muted-foreground">{t("No description.")}</p>
			)}
			<section aria-label={t("Reviews")} className="flex flex-col gap-1.5">
				<Heading>{t("Reviews")}</Heading>
				<Reviews reviews={pr.reviews} />
			</section>
			{mergeWords ? (
				<section aria-label={t("Merging")} className="flex flex-col gap-1.5">
					<Heading>{t("Merging")}</Heading>
					<p
						className={cn(
							"text-[12px]",
							pr.summary.pull.mergeable === "conflicting"
								? "text-status-error-text"
								: "text-muted-foreground"
						)}
					>
						{mergeWords}
					</p>
				</section>
			) : null}
			<Actions pr={pr} />
		</div>
	);
};
