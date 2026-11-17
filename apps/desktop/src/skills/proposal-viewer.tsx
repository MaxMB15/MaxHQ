// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A skill change an agent proposes: who wrote it, where and when, what the
// changed skill does beyond its instructions, its changed files against
// the skill as it is in the library, then Approve, Approve and push, or
// Reject. A third party's skill asks for a fork's name before it can be
// approved. The diff comes when the viewer opens, worked out by the device
// that keeps the library when it can. Approve waits for it and carries what
// it showed, so an approve can't land on anything else.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { FileRow } from "@/pulls/tabs";
import { DANGER, FIELD, Label, PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { ProposalAction, ProposalDiff, SkillProposal } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { readOf } from "@maxhq/shared/pulls/read";
import { skillBadges, skillLibrary } from "@maxhq/shared/skills/library";
import { answerDone, proposalReview, proposalTitle } from "@maxhq/shared/skills/project";
import { formatAgo } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { SkillBadges, useSkillAction } from "./look";

/** The proposal's changed files, read from the device it's on. */
const useProposalDiff = (proposal: SkillProposal, given: ProposalDiff | null) => {
	const { source } = useServices();
	return useQuery({
		queryKey: ["skills", "proposal-diff", proposal.id, proposal.tree],
		queryFn: async () =>
			readOf(await source.skillProposalDiff(proposal.id), "skill_proposal_diff").diff,
		enabled: given === null,
	});
};

const Changes = ({ diff, error }: { diff: ProposalDiff | undefined; error: Error | null }) => {
	const { t } = useI18n();

	if (!diff) {
		return (
			<p
				role={error ? "alert" : undefined}
				className={cn(
					"px-3 py-6 text-center text-[12.5px]",
					error ? "text-status-error-text" : "text-muted-foreground"
				)}
			>
				{error
					? t("Couldn't read the change. {{message}}", { message: error.message })
					: t("Reading the change")}
			</p>
		);
	}
	if (diff.files.length === 0) {
		return (
			<p className="px-3 py-6 text-center text-[12.5px] text-muted-foreground">
				{t("No files changed")}
			</p>
		);
	}
	return (
		<ul aria-label={t("Files")} className="flex flex-col gap-4 p-3">
			{diff.files.map((file) => (
				<FileRow
					key={file.path}
					file={file}
					noPatch={t("There's no text diff for this file.")}
				/>
			))}
		</ul>
	);
};

const Body = ({
	proposal,
	diff: given,
	onDone,
}: {
	proposal: SkillProposal;
	diff: ProposalDiff | null;
	onDone: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const read = useProposalDiff(proposal, given);
	const diff = given ?? read.data;
	const [opened] = useState(proposal.tree);
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const { note, busy, run } = useSkillAction();
	const [fork, setFork] = useState("");
	const library = skillLibrary(view);
	const { from, forking, refusal, base, seen, standing } = proposalReview(
		library,
		proposal,
		diff,
		fork
	);
	const cantYet = busy || refusal !== null || seen === undefined;
	const answer = async (action: ProposalAction, push: boolean) => {
		const done = answerDone(action, push);
		const named = action === "approve" && forking ? fork.trim() : undefined;
		const told = action === "approve" ? seen : undefined;
		const send = () => source.answerSkillProposal(proposal.id, action, push, named, told);
		if (await run(t("Sending"), done, send)) onDone();
	};
	return (
		<div className="flex flex-col gap-4">
			<Title>{proposalTitle(view, proposal)}</Title>
			<div className="flex flex-col gap-1">
				<span className="flex items-center gap-2">
					<span className="font-mono text-[13px] text-foreground">{proposal.name}</span>
					{standing ? (
						<span
							className={cn(
								"text-xs",
								base === null ? "text-muted-foreground" : "text-status-error-text"
							)}
						>
							{standing}
						</span>
					) : null}
					<SkillBadges badges={skillBadges(diff ?? proposal, from)} />
				</span>
				<span className="text-xs text-muted-foreground">
					{t("In {{checkout}} on {{value2}}, {{value3}}", {
						checkout: proposal.checkout,
						value2: deviceName(view, proposal.device),
						value3: formatAgo(proposal.created_at, now),
					})}
				</span>
				{from ? (
					<span className="text-xs text-muted-foreground">
						{t("From {{name}}", { name: from.name })}
					</span>
				) : null}
			</div>
			<div className="max-h-[48vh] overflow-auto rounded-[7px] border border-border bg-subpanel">
				<Changes diff={diff} error={read.error} />
			</div>
			{proposal.tree !== opened ? (
				<p role="alert" className="text-xs text-status-error-text">
					{t(
						"This proposal changed since you opened it. What's above is the new version."
					)}
				</p>
			) : null}
			{forking ? (
				<label className="flex flex-col gap-1">
					<Label>{t("Fork name in your personal source")}</Label>
					<input
						className={FIELD}
						value={fork}
						placeholder={`my-${proposal.name}`}
						spellCheck={false}
						onChange={(e) => setFork(e.target.value)}
					/>
				</label>
			) : null}
			{refusal ? <p className="text-xs text-muted-foreground">{refusal}</p> : null}
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<button
					type="button"
					className={DANGER}
					disabled={busy}
					onClick={() => void answer("reject", false)}
				>
					{t("Reject")}
				</button>
				<button
					type="button"
					className={SECONDARY}
					disabled={cantYet}
					onClick={() => void answer("approve", true)}
				>
					{t("Approve and push")}
				</button>
				<button
					type="button"
					className={PRIMARY}
					disabled={cantYet}
					onClick={() => void answer("approve", false)}
				>
					{t("Approve")}
				</button>
			</div>
		</div>
	);
};

/** The viewer for one proposal. `onDone` runs once the device took the answer. */
export const ProposalViewer = ({
	proposal,
	diff = null,
	onOpenChange,
	onDone,
}: {
	/** The proposal to show, or null while the viewer is shut. */
	proposal: SkillProposal | null;
	/** The changed files to show, read from the proposal's device when not given. */
	diff?: ProposalDiff | null;
	onOpenChange: (open: boolean) => void;
	onDone: () => void;
}) => {
	useI18n();
	return (
		<Dialog.Root open={proposal !== null} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={cn(POPUP, "w-[min(760px,calc(100vw-32px))]")}>
					{proposal ? (
						<Body key={proposal.id} proposal={proposal} diff={diff} onDone={onDone} />
					) : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
