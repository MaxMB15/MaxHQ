// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The banner under a session someone started in a terminal. While the
// terminal runs it, MaxHQ only follows along, and Take over stops it
// there after asking. Once it ends, the session can continue here. While
// the device can't be reached, it says so. A sub-agent's banner takes over
// the session it runs in, and points there once it continues in MaxHQ.

import { SquareTerminal } from "lucide-react";
import { type ReactNode, useId } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { requestAnswer } from "@/components/confirm";
import { CopyButton } from "@/components/copy-button";
import type { Agent, Vendor } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { childrenOf, deviceName, linkForDevice } from "@maxhq/shared/view/select";

import {
	CLI,
	inTurn,
	MANUAL_NOTE,
	movedNote,
	type NotePart,
	outsideNote,
	outsideOffer,
	rootLead,
	takeOverQuestion,
	takingOverNote,
	useContinueHere,
	useFollowLead,
	useTakeOver,
} from "@maxhq/shared/agent-view/outside";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Parts = ({ parts, code }: { parts: readonly NotePart[]; code: string }) => {
	useI18n();
	return parts.map((part, i) =>
		part.code ? (
			<code key={i} className={cn("font-mono text-[11.5px]", code)}>
				{part.text}
			</code>
		) : (
			part.text
		)
	);
};

const button =
	"inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-[12px] whitespace-nowrap";
const PRIMARY = cn(
	button,
	"cursor-pointer bg-primary font-semibold text-primary-foreground shadow-lit hover:bg-primary/90 disabled:cursor-default disabled:opacity-60"
);
const SECONDARY = cn(
	button,
	"cursor-pointer border border-input bg-secondary text-foreground hover:bg-accent disabled:cursor-default disabled:opacity-60"
);

const RESUME: Record<Vendor, string> = {
	claude: "--resume",
	codex: "resume",
	cursor: "--resume",
	acp: "",
};

/** The command that resumes a session in its CLI. Another ACP agent has none MaxHQ knows. */
export const resumeCommand = (agent: Agent): string | null => {
	if (!agent.vendor_session_id || agent.vendor === "acp") return null;
	return `${CLI[agent.vendor]} ${RESUME[agent.vendor]} ${agent.vendor_session_id}`;
};

/** The banner's box: an icon, a title, what's below it, and buttons on the right. */
const Frame = ({
	label,
	title,
	children,
	actions,
}: {
	label: string;
	title: string;
	children: ReactNode;
	actions: ReactNode;
}) => (
	<section
		aria-label={label}
		className="relative mx-auto flex max-w-[744px] items-start gap-[11px] rounded-[10px] border border-input bg-card py-3 pr-3 pl-3.5"
	>
		<span
			aria-hidden
			className="mt-px grid size-[26px] shrink-0 place-items-center rounded-md border border-input bg-hover-card text-secondary-foreground"
		>
			<SquareTerminal size={14} strokeWidth={2} />
		</span>
		<div className="flex min-w-0 flex-1 flex-col gap-[3px]">
			<h3 className="text-[13px] font-semibold text-foreground-strong">{title}</h3>
			{children}
		</div>
		<div className="flex min-w-0 flex-wrap items-center justify-end gap-1.5 self-center">
			{actions}
		</div>
	</section>
);

/**
 * Where a sub-agent's session went once it continues in MaxHQ as `lead`, in
 * its banner or, once the sub-agent is gone, in its pane.
 */
export const SessionMoved = ({ lead }: { lead: Agent }) => {
	useI18n();
	const services = useServices();
	const { title, body } = movedNote(lead);
	return (
		<Frame
			label={translate("Session continues in MaxHQ")}
			title={title}
			actions={
				<button
					type="button"
					onClick={() => openAgent(services, lead.id)}
					className={PRIMARY}
				>
					{translate("Open session")}
				</button>
			}
		>
			<p className="text-[12px] leading-[1.45] text-muted-foreground">{body}</p>
		</Frame>
	);
};

/** In place of the composer for a session started in a terminal, or a sub-agent in one. */
export const OutsideBanner = ({ agent }: { agent: Agent }) => {
	useI18n();
	const services = useServices();
	const { source } = services;
	const device = useView((s) => {
		const workspace = s.view.tables.workspace.get(agent.workspace);
		if (!workspace) return null;
		const local = linkForDevice(s.view, workspace.device)?.route.type === "local";
		return local ? translate("this Mac") : deviceName(s.view, workspace.device);
	});
	const link = useView((s) => {
		const workspace = s.view.tables.workspace.get(agent.workspace);
		return (workspace && linkForDevice(s.view, workspace.device)?.state) ?? null;
	});
	const protocol = useView((s) => {
		const workspace = s.view.tables.workspace.get(agent.workspace);
		return (workspace && linkForDevice(s.view, workspace.device)?.protocol) ?? null;
	});
	// A sub-agent's session is its lead's, at the top of its tree.
	const lead = useView((s) => rootLead(s.view.tables.agent, agent));
	useFollowLead(agent.id, lead?.id ?? null);
	// For "12 min ago" while the device can't be reached.
	const now = useNow(30_000);
	const [continued, continueHere] = useContinueHere(source, agent.id);
	const [taken, takeOver] = useTakeOver(source, lead?.id ?? agent.id);
	const tip = useId();
	const note = outsideNote(agent, device, { link, now });
	const offer = outsideOffer(agent, lead, note, protocol, link, device);
	if (offer.kind === "moved")
		return (
			<div className="shrink-0 px-7 pb-3.5">
				<SessionMoved lead={offer.lead} />
			</div>
		);
	const resume = offer.kind === "continue" ? resumeCommand(agent) : null;
	const own = agent.parent === null;
	const failed = [continued, taken].find((state) => state.state === "failed");
	const ask = () => {
		if (!lead) return;
		const subAgents = childrenOf(services.view.getState().view, lead.id).length > 0;
		const place = device ?? translate("the device");
		const question = takeOverQuestion(agent, place, lead, subAgents);
		void requestAnswer(services, question).then((choice) => {
			if (choice === "action") takeOver(inTurn(lead) ? "after_turn" : "now");
			if (choice === "alternative") takeOver("now");
		});
	};
	const sending = (state: string) =>
		state === "sending" ? (
			<Spinner
				size={11}
				className="border-primary-foreground/30 border-t-primary-foreground"
			/>
		) : null;

	return (
		<div className="shrink-0 px-7 pb-3.5">
			<Frame
				label={
					note.ended
						? translate("Terminal session ended")
						: translate("Running in a terminal")
				}
				title={note.title}
				actions={
					<>
						{resume ? (
							<CopyButton
								text={resume}
								label={translate("Copy resume command")}
								icon={false}
								title={resume}
								className={SECONDARY}
							/>
						) : null}
						{offer.kind === "taking" ? (
							<button
								type="button"
								disabled={taken.state === "sending"}
								onClick={() => takeOver("cancel")}
								className={SECONDARY}
							>
								{translate("Cancel")}
							</button>
						) : offer.kind === "take_over" ? (
							<button
								type="button"
								disabled={taken.state === "sending"}
								onClick={ask}
								className={PRIMARY}
							>
								{sending(taken.state)}
								{translate("Take over")}
							</button>
						) : offer.kind === "continue" ? (
							<button
								type="button"
								disabled={
									continued.state === "sending" || continued.state === "sent"
								}
								onClick={continueHere}
								className={PRIMARY}
							>
								{sending(continued.state)}
								{translate("Continue in MaxHQ")}
							</button>
						) : (
							<span className="group relative">
								<button
									type="button"
									aria-disabled="true"
									aria-describedby={tip}
									className={cn(
										button,
										"cursor-not-allowed border border-track bg-secondary font-semibold text-subtle-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
									)}
								>
									{offer.button === "take_over"
										? translate("Take over")
										: translate("Continue in MaxHQ")}
								</button>
								<span
									id={tip}
									role="tooltip"
									className="pointer-events-none absolute right-0 bottom-[calc(100%+8px)] z-10 hidden w-[286px] rounded-lg border border-input bg-popover px-[11px] py-[9px] text-[12px] leading-[1.45] text-terminal shadow-menu group-focus-within:block group-hover:block"
								>
									<Parts parts={offer.why} code="text-foreground" />
								</span>
							</span>
						)}
					</>
				}
			>
				<p className="text-[12px] leading-[1.45] text-muted-foreground">
					<Parts parts={note.body} code="text-terminal" />
				</p>
				{offer.kind === "taking" && lead ? (
					<p role="status" className="text-[12px] leading-[1.45] text-foreground">
						{takingOverNote(agent, device, lead)}
					</p>
				) : null}
				{offer.kind === "take_over" && own ? (
					<p className="text-[11.5px] leading-[1.45] text-subtle-foreground">
						<Parts parts={MANUAL_NOTE} code="text-terminal" />
					</p>
				) : null}
				{failed?.state === "failed" ? (
					<p role="alert" className="text-[11.5px] text-status-error-text">
						{failed.message}
					</p>
				) : null}
			</Frame>
		</div>
	);
};
