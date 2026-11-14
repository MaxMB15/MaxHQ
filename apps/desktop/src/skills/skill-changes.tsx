// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's skill changes: one row for each proposal waiting for you,
// newest first, which opens the proposal viewer. Hidden when none wait.

import { Sparkles } from "lucide-react";
import { useMemo, useState } from "react";

import { useView } from "@/app/services";
import type { SkillProposalId } from "@maxhq/shared/bindings";
import { plural } from "@maxhq/shared/status/format";
import { skillProposals } from "@maxhq/shared/skills/library";
import { proposalTitle } from "@maxhq/shared/skills/project";

import { ProposalViewer } from "./proposal-viewer";

/** The proposals waiting for you, each opening its viewer. */
export const SkillChanges = () => {
	const view = useView((s) => s.view);
	const proposals = useMemo(() => skillProposals(view), [view]);
	const [viewing, setViewing] = useState<SkillProposalId | null>(null);
	const open = proposals.find((p) => p.id === viewing) ?? null;
	if (proposals.length === 0) return null;
	return (
		<section
			aria-label={plural(proposals.length, "skill change")}
			className="mx-1 mb-2.5 flex shrink-0 flex-col gap-0.5"
		>
			{proposals.map((proposal) => (
				<button
					key={proposal.id}
					type="button"
					onClick={() => setViewing(proposal.id)}
					className="flex h-7 items-center gap-2 rounded-md px-1.5 text-left text-[12.5px] text-secondary-foreground hover:bg-accent hover:text-foreground"
				>
					<Sparkles aria-hidden size={13} strokeWidth={2} className="shrink-0" />
					<span className="min-w-0 flex-1 truncate">{proposalTitle(view, proposal)}</span>
					<span className="shrink-0 font-mono text-[11px] text-muted-foreground">
						{proposal.name}
					</span>
				</button>
			))}
			<ProposalViewer
				proposal={open}
				onOpenChange={(shown) => {
					if (!shown) setViewing(null);
				}}
				onDone={() => setViewing(null)}
			/>
		</section>
	);
};
