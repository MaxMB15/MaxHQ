// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The More menu at the end of the agent's header. It opens the agent's
// shell in the terminal panel, ends the agent's process, which keeps the
// conversation, copies the ID the CLI resumes the session by, saves the
// conversation as Markdown in a file you pick or copies it, opens the
// sheet that lets it run agents, links a PR when it has none, since the
// PR chips' menu links the rest, moves it to another device, and deletes
// an agent MaxHQ holds.

import { Menu } from "@base-ui/react/menu";
import {
	Bot,
	Check,
	CircleAlert,
	Copy,
	Ellipsis,
	FileDown,
	GitPullRequest,
	MonitorUp,
	Power,
	SquareTerminal,
	Trash2,
} from "lucide-react";

import { useEffect, useState } from "react";

import { useServices } from "@/app/services";
import { copyLabel, CopyStatus, useCopy } from "@/components/copy-button";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { inApp } from "@/native/platform";
import { canEnd, sessionIdName, STOP_AGENT } from "@maxhq/shared/agent-view/controls";
import { viewNames } from "@maxhq/shared/agent-view/agent-tools";
import type { Agent } from "@maxhq/shared/bindings";
import { agentKind } from "@maxhq/shared/status/agent-kind";
import { cn } from "@/lib/utils";

import { conversationText, copyWhenReady, saveWhenReady } from "./export-conversation";
import { Spinner } from "./icons";
import { MENU_KEY } from "./run-menu";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

export { sessionIdName } from "@maxhq/shared/agent-view/controls";

const LABEL = "text-[12.5px] font-[550]";
const DETAIL = "text-[11px] text-muted-foreground";

/**
 * A menu item that copies `text`, shown under its label. It stays open
 * long enough to say whether the text went to the clipboard.
 */
export const CopyItem = ({ label, text }: { label: string; text: string }) => {
	useI18n();
	const [copied, copy] = useCopy();
	return (
		<Menu.Item closeOnClick={false} onClick={() => copy(text)} className={MENU_ITEM}>
			{copied === "copied" ? (
				<Check aria-hidden size={13} strokeWidth={2.4} className="shrink-0 text-primary" />
			) : copied === "failed" ? (
				<CircleAlert
					aria-hidden
					size={13}
					strokeWidth={2}
					className="shrink-0 text-status-error-text"
				/>
			) : (
				<Copy
					aria-hidden
					size={13}
					strokeWidth={2}
					className="shrink-0 text-secondary-foreground"
				/>
			)}
			<span className="flex min-w-0 flex-1 flex-col gap-px">
				<span className={LABEL}>{copyLabel(copied, label)}</span>
				<span title={text} className={cn(DETAIL, "truncate font-mono")}>
					{text}
				</span>
			</span>
			<CopyStatus state={copied} />
		</Menu.Item>
	);
};

// Where an export is, and what to say once it's done or why it failed.
type Exporting =
	| { readonly step: "idle" | "loading" | "saving" }
	| { readonly step: "done"; readonly detail: string }
	| { readonly step: "failed"; readonly reason: string };

// How long the outcome shows before the item reads as it did.
const EXPORT_SHOWN_MS = { done: 2000, failed: 5000 } as const;

// What each export says before it starts and once it's done.
const EXPORT_WORDS = {
	save: {
		get label() {
			return translate("Export as Markdown…");
		},
		get detail() {
			return translate("Saves the whole conversation in a file.");
		},
		get done() {
			return translate("Saved as Markdown");
		},
	},
	copy: {
		get label() {
			return translate("Copy as Markdown");
		},
		get detail() {
			return translate("Copies the whole conversation to the clipboard.");
		},
		get done() {
			return translate("Copied as Markdown");
		},
	},
} as const;

// Exports the conversation to a file or the clipboard. It stays open while
// older pages load, and says how it went.
const ExportItem = ({ agent, to }: { agent: Agent; to: "save" | "copy" }) => {
	useI18n();
	const services = useServices();
	const [state, setState] = useState<Exporting>({ step: "idle" });
	const words = EXPORT_WORDS[to];

	useEffect(() => {
		if (state.step !== "done" && state.step !== "failed") return;
		const timer = setTimeout(() => setState({ step: "idle" }), EXPORT_SHOWN_MS[state.step]);
		return () => clearTimeout(timer);
	}, [state]);

	const run = () => {
		if (state.step === "loading" || state.step === "saving") return;
		setState({ step: "loading" });
		const view = services.view.getState().view;
		const names = viewNames(view);
		const text = conversationText(
			services.source,
			agent,
			names,
			agentKind(agent, view.tables.account)
		);
		if (to === "copy") {
			void copyWhenReady(text).then((result) =>
				setState(
					result.ok
						? {
								step: "done",
								detail: translate("The whole conversation is on the clipboard."),
							}
						: { step: "failed", reason: result.reason }
				)
			);
			return;
		}
		void saveWhenReady(text, agent.title, () => setState({ step: "saving" })).then((result) =>
			setState(
				result.step === "saved"
					? { step: "done", detail: result.name }
					: result.step === "failed"
						? { step: "failed", reason: result.reason }
						: { step: "idle" }
			)
		);
	};

	const label =
		state.step === "loading"
			? translate("Loading earlier messages")
			: state.step === "saving"
				? translate("Saving as Markdown")
				: state.step === "done"
					? words.done
					: state.step === "failed"
						? translate("Couldn't export")
						: words.label;
	const detail =
		state.step === "failed"
			? state.reason
			: state.step === "done"
				? state.detail
				: state.step === "saving"
					? translate("Choose where the file goes.")
					: words.detail;
	const Idle = to === "save" ? FileDown : Copy;
	return (
		<Menu.Item closeOnClick={false} onClick={run} className={MENU_ITEM}>
			{state.step === "loading" || state.step === "saving" ? (
				<Spinner size={13} />
			) : state.step === "done" ? (
				<Check aria-hidden size={13} strokeWidth={2.4} className="shrink-0 text-primary" />
			) : state.step === "failed" ? (
				<CircleAlert
					aria-hidden
					size={13}
					strokeWidth={2}
					className="shrink-0 text-status-error-text"
				/>
			) : (
				<Idle
					aria-hidden
					size={13}
					strokeWidth={2}
					className="shrink-0 text-secondary-foreground"
				/>
			)}
			<span className="flex min-w-0 flex-1 flex-col gap-px">
				<span className={LABEL}>{label}</span>
				<span className={DETAIL}>{detail}</span>
			</span>
			<span role="status" className="sr-only">
				{state.step === "idle" ? "" : label}
			</span>
		</Menu.Item>
	);
};

/** The header's More menu. */
export const AgentMenu = ({
	agent,
	onTerminal,
	onEnd,
	onAllow,
	allowed,
	onLink,
	onMove,
	moveOff,
	onDelete,
}: {
	agent: Agent;
	/** Opens the agent's shell in the terminal panel, while its device has terminals. */
	onTerminal?: () => void;
	onEnd: () => void;
	/** Opens the allow sheet, when the agent can hold a permission. */
	onAllow?: () => void;
	/** Whether it holds one. */
	allowed?: boolean;
	/** Opens the field that links a PR, when the agent has none. */
	onLink?: () => void;
	/** Opens the Move dialog, when the agent can move. */
	onMove?: () => void;
	/** Why it can't move now, which turns the entry off. */
	moveOff?: string | null;
	/** Asks, then deletes the agent, when MaxHQ can. */
	onDelete?: () => void;
}) => {
	useI18n();
	const id = agent.vendor_session_id;
	const end = canEnd(agent);

	return (
		<Menu.Root>
			<Menu.Trigger
				aria-label={translate("More")}
				className="grid size-6 shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-border data-popup-open:text-foreground"
			>
				<Ellipsis aria-hidden size={14} strokeWidth={2.2} />
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="end" sideOffset={6} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[260px]")}>
						{onTerminal ? (
							<>
								<Menu.Item
									label={translate("Open terminal")}
									onClick={onTerminal}
									className={MENU_ITEM}
								>
									<SquareTerminal
										aria-hidden
										size={13}
										strokeWidth={2}
										className="shrink-0 text-secondary-foreground"
									/>
									<span className="flex min-w-0 flex-1 flex-col gap-px">
										<span className={LABEL}>{translate("Open terminal")}</span>
										<span className={DETAIL}>
											{translate("Work by hand where the agent works.")}
										</span>
									</span>
									<kbd aria-hidden className={MENU_KEY}>
										{keyHint(SHORTCUTS.terminal)}
									</kbd>
								</Menu.Item>
								<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
							</>
						) : null}
						{id ? (
							<CopyItem
								label={translate("Copy {{sessionIdName}}", {
									sessionIdName: sessionIdName(agent),
								})}
								text={id}
							/>
						) : null}
						{inApp() ? <ExportItem agent={agent} to="save" /> : null}
						<ExportItem agent={agent} to="copy" />
						{onAllow ? (
							<Menu.Item onClick={onAllow} className={MENU_ITEM}>
								<Bot
									aria-hidden
									size={13}
									strokeWidth={2}
									className="shrink-0 text-secondary-foreground"
								/>
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className={LABEL}>
										{allowed
											? translate("Change what it can run")
											: translate("Allow to run agents")}
									</span>
									<span className={DETAIL}>
										{allowed
											? translate("Change or revoke its permission.")
											: translate("Let it start and manage other agents.")}
									</span>
								</span>
							</Menu.Item>
						) : null}
						{onLink ? (
							<Menu.Item onClick={onLink} className={MENU_ITEM}>
								<GitPullRequest
									aria-hidden
									size={13}
									strokeWidth={2}
									className="shrink-0 text-secondary-foreground"
								/>
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className={LABEL}>
										{translate("Link a pull request")}
									</span>
									<span className={DETAIL}>
										{translate("Show its state and checks here.")}
									</span>
								</span>
							</Menu.Item>
						) : null}
						{onMove ? (
							<Menu.Item
								onClick={onMove}
								disabled={moveOff != null}
								className={MENU_ITEM}
							>
								<MonitorUp
									aria-hidden
									size={13}
									strokeWidth={2}
									className="shrink-0 text-secondary-foreground"
								/>
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className={LABEL}>{translate("Move to…")}</span>
									<span className={DETAIL}>
										{moveOff ??
											translate(
												"Go on in a new agent on another device, from a summary."
											)}
									</span>
								</span>
							</Menu.Item>
						) : null}
						{end ? (
							<Menu.Item onClick={onEnd} className={MENU_ITEM}>
								<Power
									aria-hidden
									size={13}
									strokeWidth={2}
									className="shrink-0 text-secondary-foreground"
								/>
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className={LABEL}>{STOP_AGENT.label}</span>
									<span className={DETAIL}>{STOP_AGENT.detail}</span>
								</span>
							</Menu.Item>
						) : null}
						{onDelete ? (
							<Menu.Item onClick={onDelete} className={MENU_ITEM}>
								<Trash2
									aria-hidden
									size={13}
									strokeWidth={2}
									className="shrink-0 text-status-error-text"
								/>
								<span className="flex min-w-0 flex-1 flex-col gap-px">
									<span className={cn(LABEL, "text-status-error-text")}>
										{translate("Delete agent")}
									</span>
									<span className={DETAIL}>
										{translate(
											"Stops it and removes it from MaxHQ. Its folder stays."
										)}
									</span>
								</span>
							</Menu.Item>
						) : null}
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};
