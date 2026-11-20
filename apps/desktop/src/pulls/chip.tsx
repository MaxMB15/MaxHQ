// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The PR chips on an agent: one per PR linked to it, its number colored by
// the PR's state with a dot for its checks. In the agent's header a chip
// opens the PR beside the conversation, and its menu opens the PR on
// GitHub, sends the agent a message about it, unlinks it or links another.
// A narrow header keeps the first chip and folds the rest into "+N". The
// sidebar's row shows one chip small, with no menu, or one "2 PRs" chip for
// several. Both sit in a strip that scrolls sideways when they don't fit.
// Linking takes the PR's number, in a field that says why the device
// refused one.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import { ChevronDown, ExternalLink, GitPullRequest, MessageSquare, Unlink } from "lucide-react";
import { type RefObject, useState } from "react";

import { useServices, useView } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { ChipStrip } from "@/components/chip-strip";
import { Popover, PopoverPopup, PopoverTitle } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { agreedSend } from "@/settings/auto-consent";
import type { Agent, PullLink } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import {
	type ChipChecks,
	chipChecks,
	chipLabel,
	type ChipState,
	chipState,
	foldChips,
	linkPullNumber,
	parsePullNumber,
	pullPrompts,
} from "@maxhq/shared/pulls/chip";
import { shownPulls } from "@maxhq/shared/pulls/links";
import { pullUrl } from "@maxhq/shared/pulls/urls";

/** Runs an action from the header, which says below itself if it fails. */
export type RunAction = (what: string, action: () => Promise<ActionResult>) => void;

/** What a message you chose not to send comes back with: nothing to say. */
const LEFT: ActionResult = { status: "ok", data: { status: "done" } };

// The hues the composer's PR pills use, so a PR looks the same everywhere.
const STATE_LOOK: Record<ChipState, string> = {
	open: "border-(--pill-git-open-border) bg-(--pill-git-open-bg) text-(--pill-git-open-text)",
	draft: "border-(--pill-git-draft-border) bg-(--pill-git-draft-bg) text-(--pill-git-draft-text)",
	merged: "border-(--pill-git-merged-border) bg-(--pill-git-merged-bg) text-(--pill-git-merged-text)",
	closed: "border-(--pill-git-closed-border) bg-(--pill-git-closed-bg) text-(--pill-git-closed-text)",
	unread: "border-track bg-secondary text-secondary-foreground",
};

const DOT_LOOK: Record<ChipChecks, string> = {
	passing: "bg-status-ready",
	failing: "bg-status-error",
	pending: "bg-status-busy",
	unknown: "bg-status-off",
};

const Dot = ({ checks, size }: { checks: ChipChecks | null; size: number }) => {
	useI18n();
	return checks ? (
		<span
			aria-hidden
			data-checks={checks}
			className={cn("shrink-0 rounded-full", DOT_LOOK[checks])}
			style={{ width: size, height: size }}
		/>
	) : null;
};

const ChecksDot = ({ link, size }: { link: PullLink; size: number }) => {
	useI18n();
	return <Dot checks={chipChecks(link)} size={size} />;
};

/** The device an agent runs on, which reads its PRs. */
const useAgentDevice = (agent: Agent) =>
	useView((s) => s.view.tables.workspace.get(agent.workspace)?.device);

const LABEL = "text-[12.5px] font-[550]";
const ICON = "shrink-0 text-secondary-foreground";

/** One PR chip in the agent's header, with its menu. */
const PullChip = ({
	agent,
	link,
	onRun,
	onLink,
	className,
}: {
	agent: Agent;
	link: PullLink;
	onRun: RunAction;
	onLink: () => void;
	className?: string;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source, ui } = services;
	const device = useAgentDevice(agent);
	// An Auto you didn't pick asks first. An agent that couldn't leave it says why.
	const agreed = async () => {
		const { go, failure } = await agreedSend(services, agent);
		if (failure && !go) throw new Error(failure);
		if (failure) console.warn(failure);
		return go;
	};
	const state = chipState(link);
	const label = chipLabel(link);
	const { repo, number } = link.key;
	// Only an agent MaxHQ holds takes messages.
	const prompts = agent.control === "observed" ? [] : pullPrompts(link);
	const look = cn(
		"inline-flex h-[22px] shrink-0 items-center border text-[11.5px] font-[600] tabular-nums",
		STATE_LOOK[state]
	);
	return (
		<span className={cn("inline-flex shrink-0 items-center", className)}>
			<button
				type="button"
				aria-label={label}
				title={link.snapshot?.title ?? `${repo}#${number}`}
				disabled={!device}
				onClick={() => device && ui.getState().openPull({ key: link.key, device })}
				className={cn(
					look,
					"gap-[5px] rounded-l-md border-r-0 pr-1 pl-[7px] hover:brightness-125"
				)}
			>
				<GitPullRequest aria-hidden size={11} strokeWidth={2.2} />#
				{formatNumber(number, { useGrouping: false })}
				<ChecksDot link={link} size={6} />
			</button>
			<Menu.Root>
				<Menu.Trigger
					aria-label={t("More for pull request {{number}}", {
						number: formatNumber(number, { useGrouping: false }),
					})}
					className={cn(
						look,
						"rounded-r-md border-l-0 pr-[5px] pl-[3px] outline-none hover:brightness-125 focus-visible:ring-2 focus-visible:ring-ring"
					)}
				>
					<ChevronDown aria-hidden size={11} strokeWidth={2.4} />
				</Menu.Trigger>
				<Menu.Portal>
					<Menu.Positioner side="bottom" align="start" sideOffset={6} className="z-[130]">
						<Menu.Popup className={cn(MENU_POPUP, "w-[240px]")}>
							<Menu.Item
								className={MENU_ITEM}
								render={
									<a href={pullUrl(link.key)} target="_blank" rel="noreferrer" />
								}
							>
								<ExternalLink
									aria-hidden
									size={13}
									strokeWidth={2}
									className={ICON}
								/>
								<span className={LABEL}>{t("Open on GitHub")}</span>
							</Menu.Item>
							{prompts.length > 0 ? (
								<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
							) : null}
							{prompts.map((prompt) => (
								<Menu.Item
									key={prompt.task}
									className={MENU_ITEM}
									onClick={() =>
										onRun(t("send the message"), async () => {
											if (!(await agreed())) return LEFT;
											return source.sendInput(
												agent.id,
												{ text: prompt.text, attachments: [] },
												"queue"
											);
										})
									}
								>
									<MessageSquare
										aria-hidden
										size={13}
										strokeWidth={2}
										className={ICON}
									/>
									<span className={LABEL}>{prompt.label}</span>
								</Menu.Item>
							))}
							<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
							<Menu.Item
								className={MENU_ITEM}
								onClick={() =>
									onRun(t("unlink the PR"), () =>
										source.unlinkPull(agent.id, repo, number)
									)
								}
							>
								<Unlink aria-hidden size={13} strokeWidth={2} className={ICON} />
								<span className={LABEL}>{t("Unlink")}</span>
							</Menu.Item>
							<Menu.Item className={MENU_ITEM} onClick={onLink}>
								<GitPullRequest
									aria-hidden
									size={13}
									strokeWidth={2}
									className={ICON}
								/>
								<span className={LABEL}>{t("Link another PR")}</span>
							</Menu.Item>
						</Menu.Popup>
					</Menu.Positioner>
				</Menu.Portal>
			</Menu.Root>
		</span>
	);
};

/** The header's chip for the PRs past the first, with a menu that opens each beside the conversation. */
const MorePulls = ({
	agent,
	links,
	className,
}: {
	agent: Agent;
	links: readonly PullLink[];
	className?: string;
}) => {
	const { t } = useI18n();

	const { ui } = useServices();
	const device = useAgentDevice(agent);
	return (
		<Menu.Root>
			<Menu.Trigger
				aria-label={t("{{countText}} more pull requests", {
					count: links.length,
					countText: formatNumber(links.length),
				})}
				title={foldChips(links).title}
				className={cn(
					"h-[22px] shrink-0 items-center rounded-md border px-[7px] text-[11.5px] font-[600] tabular-nums outline-none hover:brightness-125 focus-visible:ring-2 focus-visible:ring-ring",
					STATE_LOOK.unread,
					className
				)}
			>
				+{formatNumber(links.length)}
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="start" sideOffset={6} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[260px]")}>
						{links.map((link) => (
							<Menu.Item
								key={`${link.key.repo}#${link.key.number}`}
								className={MENU_ITEM}
								disabled={!device}
								onClick={() =>
									device && ui.getState().openPull({ key: link.key, device })
								}
							>
								<GitPullRequest
									aria-hidden
									size={13}
									strokeWidth={2}
									className={ICON}
								/>
								<span className={cn(LABEL, "shrink-0 tabular-nums")}>
									#{formatNumber(link.key.number, { useGrouping: false })}
								</span>
								<span className="min-w-0 flex-1 truncate text-[12.5px] text-muted-foreground">
									{link.snapshot?.title}
								</span>
								<ChecksDot link={link} size={6} />
							</Menu.Item>
						))}
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};

// The agent header names its strip `header` as a container. Where it's
// narrow, the strip keeps the first PR and folds the rest into "+N".
const PAST_FIRST = "@max-[52rem]/header:hidden";
const FOLDED = "hidden @max-[52rem]/header:inline-flex";

/**
 * The agent header's PR chips, or nothing when it has no PR linked. They
 * take the room the header has left, never past their own width, and
 * scroll sideways in it. A narrow header shows the first and "+N".
 */
export const PullChips = ({
	agent,
	onRun,
	onLink,
}: {
	agent: Agent;
	onRun: RunAction;
	/** Opens the field that links a PR by its number. */
	onLink: () => void;
}) => {
	const { t } = useI18n();

	const links = shownPulls(agent);
	if (links.length === 0) return null;
	return (
		<ChipStrip rings className="max-w-max grow basis-28">
			<span
				role="group"
				aria-label={t("Pull requests")}
				className="flex items-center gap-1.5"
			>
				{links.map((link, i) => (
					<PullChip
						key={`${link.key.repo}#${link.key.number}`}
						agent={agent}
						link={link}
						onRun={onRun}
						onLink={onLink}
						className={i > 0 ? PAST_FIRST : undefined}
					/>
				))}
				{links.length > 1 ? (
					<MorePulls agent={agent} links={links.slice(1)} className={FOLDED} />
				) : null}
			</span>
		</ChipStrip>
	);
};

const SMALL_CHIP =
	"inline-flex h-4 items-center gap-[3px] rounded-[4px] border px-1 text-[10.5px] font-[600] tabular-nums";

/**
 * The sidebar row's PR chip, a number and its checks dot, or nothing. Two
 * or more fold into one chip, such as "2 PRs", that lists them on hover.
 * The chip grows with the title, never past its own width, and scrolls
 * sideways in the room it gets.
 */
export const SmallPullChips = ({ agent }: { agent: Agent }) => {
	useI18n();

	const links = shownPulls(agent);
	if (links.length === 0) return null;
	const [link] = links;
	if (links.length === 1 && link) {
		return (
			<ChipStrip className="flex-1 max-w-max">
				<span
					title={link.snapshot?.title}
					className={cn(SMALL_CHIP, STATE_LOOK[chipState(link)])}
				>
					<span aria-hidden>
						#{formatNumber(link.key.number, { useGrouping: false })}
					</span>
					<span className="sr-only">, {chipLabel(link)}</span>
					<ChecksDot link={link} size={5} />
				</span>
			</ChipStrip>
		);
	}
	const folded = foldChips(links);
	return (
		<span
			title={folded.title}
			className={cn(SMALL_CHIP, "shrink-0", STATE_LOOK[folded.state ?? "unread"])}
		>
			<span aria-hidden>{folded.text}</span>
			<span className="sr-only">, {folded.label}</span>
			<Dot checks={folded.checks} size={5} />
		</span>
	);
};

/** The field that links a PR to the agent by its number. */
const LinkPullForm = ({ agent, onDone }: { agent: Agent; onDone: () => void }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [typed, setTyped] = useState("");
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);
	const number = parsePullNumber(typed);

	const link = async () => {
		if (number === null || busy) return;
		setBusy(true);
		setFailure(null);
		try {
			const why = await linkPullNumber(source, agent.id, number);
			if (why) setFailure(why);
			else onDone();
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-2"
			onSubmit={(e) => {
				e.preventDefault();
				void link();
			}}
		>
			<div className="flex items-center gap-2">
				<input
					aria-label={t("Pull request number")}
					value={typed}
					onChange={(e) => {
						setTyped(e.target.value);
						setFailure(null);
					}}
					placeholder={t("Such as 142")}
					inputMode="numeric"
					autoComplete="off"
					spellCheck={false}
					className="h-7 min-w-0 flex-1 rounded-[7px] border border-border bg-subpanel px-2 text-[12.5px] text-foreground tabular-nums outline-none placeholder:text-subtle-foreground focus:border-primary"
				/>
				<button
					type="submit"
					disabled={number === null || busy}
					className="inline-flex h-7 shrink-0 items-center rounded-[7px] bg-primary px-2.5 text-[12px] font-[650] text-primary-foreground disabled:opacity-50"
				>
					{busy ? t("Linking") : t("Link")}
				</button>
			</div>
			{failure ? (
				<p role="alert" className="text-[11.5px] text-status-error-text">
					{failure}
				</p>
			) : (
				<p className="text-[11.5px] text-muted-foreground">
					{t("A PR in the repository the agent works in.")}
				</p>
			)}
		</form>
	);
};

/** The popover with the link field, placed under the end of `anchor`. */
export const LinkPullPopover = ({
	agent,
	open,
	onOpenChange,
	anchor,
}: {
	agent: Agent;
	open: boolean;
	onOpenChange: (open: boolean) => void;
	anchor: RefObject<HTMLElement | null>;
}) => {
	const { t } = useI18n();
	return (
		<Popover open={open} onOpenChange={onOpenChange}>
			<PopoverPopup
				anchor={anchor}
				side="bottom"
				align="end"
				sideOffset={2}
				className="w-[260px] p-3"
			>
				<PopoverTitle className="mb-2.5 text-[12.5px] font-[650]">
					{t("Link a pull request")}
				</PopoverTitle>
				{open ? <LinkPullForm agent={agent} onDone={() => onOpenChange(false)} /> : null}
			</PopoverPopup>
		</Popover>
	);
};
