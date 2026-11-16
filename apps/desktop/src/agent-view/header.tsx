// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The strip above a conversation: what the agent is doing, its title,
// account and model, its PR chips, then how full its context is, where it
// runs, which opens the workspace's ports, its git branch, which a click
// copies, Open in for an editor, and a
// More menu, with who started it below for one the agent tools started,
// the device it moved from with a way back to the old agent, the
// scheduled tasks that reach it, a chip when it may run agents
// itself, one for each test machine it leases, one for its latest
// workflow run, and one when it restarts for changed skills. Linking a PR
// opens a field under the strip, and a double-click on the title, as in
// the sidebar, turns it into one. The model picker and the menu act only on
// agents MaxHQ holds, and the strip says below itself when an action fails.
// In a narrow pane the PR chips fold to the first and "+N", and the labels
// on the right fold to their icons in the order `header-fit` sets, so they
// never run into each other. The title gives up room last.

import { Bot, Check, CircleAlert, Folder, GitBranch, X } from "lucide-react";
import { type RefObject, useRef, useState } from "react";

import { AllowSheet, canAllow, useHeld } from "@/agent-tools/allow-sheet";
import { confirmAndDelete } from "@/app/delete-agents";
import { renamesOf, useShownTitle } from "@/app/rename";
import { useServices, useView } from "@/app/services";
import { copyLabel, CopyStatus, useCopy } from "@/components/copy-button";
import { RenameInput } from "@/components/rename-input";
import { ModelPicker } from "@/composer";
import type { ActionResult } from "@maxhq/shared/data/source";
import type { Agent, AgentId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { useFold } from "@/lib/use-fold";
import { cn } from "@/lib/utils";
import { WorkspacePorts } from "@/ports";

import { HeaderActivityDot } from "./header-dot";
import { LinkPullPopover, PullChips } from "@/pulls/chip";
import { Automations } from "@/tasks/automations";
import { SkillsRestartChip } from "@/skills/restart-chip";
import { LeaseChips } from "@/test-hosts/lease-chips";
import { HeaderRunChipFor } from "@/workflow/containers";
import { limitsLine } from "@maxhq/shared/agent-tools/scope";
import { shownPulls } from "@maxhq/shared/pulls/links";
import { formatContext } from "@maxhq/shared/status/format";
import { deletableAgents } from "@maxhq/shared/ui/delete-agents";
import { renameAgent } from "@maxhq/shared/ui/rename-agent";
import { modelsOf, movesOf, startedByLine } from "@maxhq/shared/view/select";
import { accountName, VENDOR_NAME } from "@maxhq/shared/status/usage";

import { failureOf } from "@maxhq/shared/agent-view/answer";
import { picksModel } from "@maxhq/shared/agent-view/controls";
import { baseName } from "@maxhq/shared/agent-view/format";
import { foldedAt, headerNeeds } from "@maxhq/shared/agent-view/header-fit";
import { canMove, moveBlocked } from "@maxhq/shared/agent-view/move-agent";
import { AgentMenu } from "./header-menu";
import { VendorIcon } from "./icons";
import { MoveDialog } from "./move-dialog";
import { MovedFrom } from "./moved-banner";
import { OpenInButton } from "./open-in";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/**
 * Which labels the strip folds, from its width and how many chips it shows
 * after the model. It renders again only when that changes.
 */
const useFolded = (strip: RefObject<HTMLElement | null>, chips: number) =>
	foldedAt(useFold(strip, { needs: headerNeeds(chips) }));

const ContextMeter = ({
	used,
	window,
	folded,
}: {
	used: number;
	window: number;
	folded: boolean;
}) => {
	useI18n();
	const percent = window > 0 ? Math.min(100, (used / window) * 100) : 0;
	const text = formatContext(used, window);
	const label = translate("Context window, {{text}} used", { text: text });
	return (
		<span
			role="img"
			aria-label={label}
			title={folded ? label : undefined}
			className={cn(
				"inline-flex h-6 shrink-0 items-center gap-1.5 text-secondary-foreground tabular-nums",
				folded ? null : "px-1.5"
			)}
		>
			<span aria-hidden className="h-1 w-9 overflow-hidden rounded-[2px] bg-track">
				<span
					className={cn("block h-1", percent >= 85 ? "bg-status-busy" : "bg-primary")}
					style={{ width: `${percent}%` }}
				/>
			</span>
			{folded ? null : text}
		</span>
	);
};

/** The agent's git branch, which a click copies. */
const BranchChip = ({ branch, folded }: { branch: string; folded: boolean }) => {
	useI18n();
	const [state, copy] = useCopy();
	const Icon = state === "copied" ? Check : state === "failed" ? CircleAlert : GitBranch;
	return (
		<>
			<button
				type="button"
				title={translate("Copy branch {{branch}}", { branch: branch })}
				onClick={() => copy(branch)}
				className={cn(
					"inline-flex min-w-0 items-center gap-[5px] hover:text-foreground",
					folded ? "shrink-0" : "max-w-40"
				)}
			>
				<Icon size={12} strokeWidth={2} aria-hidden className="shrink-0" />
				<span className={folded ? "sr-only" : "truncate"}>{copyLabel(state, branch)}</span>
			</button>
			<CopyStatus state={state} />
		</>
	);
};

/** Why the last action from the header failed, for the agent it was on. */
type Failure = { readonly agent: AgentId; readonly text: string };

/** The agent's header: status, title, account and model, context and place. */
export const AgentHeader = ({ agent }: { agent: Agent }) => {
	useI18n();
	const services = useServices();
	const { source } = services;
	const view = useView((s) => s.view);
	const now = useNow(1000);
	const [failure, setFailure] = useState<Failure | null>(null);
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const workspace = view.tables.workspace.get(agent.workspace);
	const observed = agent.control === "observed";
	const location = workspace?.location;
	const models = modelsOf(view, account);
	const moves = movesOf(view, agent);
	const picks = picksModel(agent, models, moves);

	const fail = (text: string) => setFailure({ agent: agent.id, text });
	const run = (what: string, action: () => Promise<ActionResult>) => {
		setFailure(null);
		action().then(
			(result) => {
				const why = failureOf(result);
				if (why) fail(translate("Couldn't {{what}}. {{why}}", { what: what, why: why }));
			},
			(error: unknown) =>
				fail(
					error instanceof Error && error.message
						? translate("Couldn't {{what}}. {{message}}", {
								what: what,
								message: error.message,
							})
						: translate("Couldn't {{what}}.", { what: what })
				)
		);
	};
	const shown = failure?.agent === agent.id ? failure.text : null;
	const startedBy = startedByLine(view, agent);
	const allows = canAllow(agent);
	const held = useHeld(agent);
	const deletable = deletableAgents(view, [agent.id]).length > 0;
	const [allowing, setAllowing] = useState(false);
	const [linking, setLinking] = useState(false);
	const [moving, setMoving] = useState(false);
	const movable = canMove(view, agent);
	const [portsOpen, setPortsOpen] = useState(false);
	const strip = useRef<HTMLDivElement>(null);
	const folded = useFolded(
		strip,
		[observed, allows && held !== undefined, shownPulls(agent).length > 0].filter(Boolean)
			.length
	);
	const accountLabel = account ? accountName(account) : VENDOR_NAME[agent.vendor];
	const terminalLine =
		agent.link.state === "down"
			? translate("Ended in a terminal")
			: translate("In a terminal, read only");
	const title = useShownTitle(agent);
	// The agent being renamed, so another agent's header opens on its title.
	const [renaming, setRenaming] = useState<AgentId | null>(null);
	const rename = (draft: string) => {
		setRenaming(null);
		setFailure(null);
		void renameAgent(renamesOf(services), agent, draft).then((why) => {
			if (why !== null) fail(why);
		});
	};

	return (
		<div className="shrink-0 border-b border-secondary">
			{/* A container, so the PR chips can fold where it's narrow; its width folds the labels. */}
			<div
				ref={strip}
				className={cn(
					"@container/header flex h-10 items-center pr-3 pl-3.5",
					folded.has("model") ? "gap-2" : "gap-2.5"
				)}
			>
				<HeaderActivityDot
					agent={agent}
					view={view}
					now={now}
					onShowRunning={workspace ? () => setPortsOpen(true) : undefined}
				/>
				{renaming === agent.id ? (
					<RenameInput
						title={title}
						label={translate("Agent name")}
						onCommit={rename}
						onCancel={() => setRenaming(null)}
						className="h-6 max-w-[420px] text-[13.5px] font-[650]"
					/>
				) : (
					<h1
						title={title}
						onDoubleClick={(e) => {
							if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
							setRenaming(agent.id);
						}}
						// It gives up a fifth as much room as the chips and the
						// folder, and never less than a few words, so the title
						// stays readable in a narrow pane.
						className="min-w-16 shrink-[0.2] truncate text-[13.5px] font-[650] tracking-[-0.005em] text-foreground-strong"
					>
						{title}
					</h1>
				)}
				<span
					title={folded.has("account") ? accountLabel : undefined}
					className="inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-md border border-track bg-secondary px-[7px] text-[11.5px] whitespace-nowrap text-terminal"
				>
					<VendorIcon vendor={agent.vendor} />
					<span className={folded.has("account") ? "sr-only" : undefined}>
						{accountLabel}
					</span>
				</span>
				{(picks || agent.model) && !folded.has("model") ? (
					<ModelPicker
						place="header"
						vendor={agent.vendor}
						model={agent.model}
						account={account}
						choices={models}
						moves={moves}
						onPick={
							picks
								? (model, to) =>
										run(translate("change the model"), () =>
											source.setModel(agent.id, model, to)
										)
								: undefined
						}
					/>
				) : null}
				{/* The chips give up room before the labels on the right and the menu. */}
				<div className="flex min-w-0 shrink items-center gap-2.5 overflow-hidden">
					{allows && held ? (
						<button
							type="button"
							title={limitsLine(held.limits, now)}
							onClick={() => setAllowing(true)}
							className="inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-md border border-track bg-secondary px-[7px] text-[11.5px] whitespace-nowrap text-secondary-foreground hover:bg-accent"
						>
							<Bot aria-hidden size={12} strokeWidth={2} />
							<span className={folded.has("account") ? "sr-only" : undefined}>
								{translate("Can run agents")}
							</span>
						</button>
					) : null}
					<LeaseChips agent={agent.id} />
					<HeaderRunChipFor agent={agent.id} />
					<SkillsRestartChip agent={agent.id} />
					{observed ? (
						<span
							title={terminalLine}
							className="inline-flex h-[22px] min-w-12 shrink items-center rounded-md border border-dashed border-input px-[7px] text-[11.5px] whitespace-nowrap text-muted-foreground"
						>
							<span className="truncate">{terminalLine}</span>
						</span>
					) : null}
					<PullChips agent={agent} onRun={run} onLink={() => setLinking(true)} />
				</div>
				<LinkPullPopover
					agent={agent}
					open={linking}
					onOpenChange={setLinking}
					anchor={strip}
				/>

				<div
					className={cn(
						"ml-auto flex shrink-0 items-center text-[11.5px] whitespace-nowrap text-muted-foreground",
						folded.has("folder") ? "gap-2" : "gap-3"
					)}
				>
					{agent.context ? (
						<>
							<ContextMeter
								used={agent.context.used_tokens}
								window={agent.context.window_tokens}
								folded={folded.has("context")}
							/>
							{folded.has("model") ? null : (
								<span aria-hidden className="h-4 w-px shrink-0 bg-border" />
							)}
						</>
					) : null}
					{workspace ? (
						<WorkspacePorts
							agent={agent}
							workspace={workspace}
							open={portsOpen}
							onOpenChange={setPortsOpen}
							folded={folded.has("device")}
						/>
					) : null}
					{location ? (
						<span
							className={cn(
								"inline-flex min-w-0 items-center gap-[5px]",
								folded.has("folder") ? "shrink-0" : "max-w-40"
							)}
							title={location.path}
						>
							<Folder size={12} strokeWidth={2} aria-hidden className="shrink-0" />
							<span className={folded.has("folder") ? "sr-only" : "truncate"}>
								{baseName(location.path)}
							</span>
						</span>
					) : null}
					{agent.branch ? (
						<BranchChip
							key={agent.id}
							branch={agent.branch}
							folded={folded.has("branch")}
						/>
					) : null}
					{workspace ? (
						<OpenInButton
							workspace={workspace.id}
							onFail={fail}
							labelClassName={folded.has("openIn") ? "sr-only" : undefined}
						/>
					) : null}
				</div>
				<AgentMenu
					agent={agent}
					onEnd={() => run(translate("stop the agent"), () => source.stopAgent(agent.id))}
					onAllow={allows ? () => setAllowing(true) : undefined}
					allowed={held !== undefined}
					onLink={shownPulls(agent).length === 0 ? () => setLinking(true) : undefined}
					onMove={movable ? () => setMoving(true) : undefined}
					moveOff={movable ? moveBlocked(view, agent) : null}
					onDelete={
						deletable
							? () => {
									setFailure(null);
									void confirmAndDelete(services, [agent]).then((failures) => {
										if (failures.length > 0) fail(failures.join(" "));
									});
								}
							: undefined
					}
				/>
				{allows ? (
					<AllowSheet agent={agent} open={allowing} onOpenChange={setAllowing} />
				) : null}
				{movable ? (
					<MoveDialog agent={agent} open={moving} onOpenChange={setMoving} />
				) : null}
			</div>
			{startedBy ? (
				<p className="truncate pr-3 pb-2 pl-3.5 text-[11.5px] text-muted-foreground">
					{startedBy}
				</p>
			) : null}
			<MovedFrom agent={agent} />
			<Automations agent={agent} />
			{shown ? (
				<div className="flex items-start gap-2 pr-3 pb-2 pl-3.5">
					<p role="alert" className="min-w-0 flex-1 text-[11.5px] text-status-error-text">
						{shown}
					</p>
					<button
						type="button"
						aria-label={translate("Dismiss")}
						onClick={() => setFailure(null)}
						className="grid size-[18px] shrink-0 cursor-pointer place-items-center rounded-[5px] text-muted-foreground hover:bg-border hover:text-foreground"
					>
						<X aria-hidden size={11} strokeWidth={2.4} />
					</button>
				</div>
			) : null}
		</div>
	);
};
